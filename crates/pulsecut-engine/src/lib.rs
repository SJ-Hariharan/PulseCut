//! The PulseCut engine: owns every long-running task and exposes a synchronous-friendly API to
//! the desktop UI and the CLI.
//!
//! ```text
//! chat sources ─┐                ┌─ tick (1 s) ─ HighlightEvent ─┐
//! mic / stream  ├─► detectors ───┤                               ├─► capture coordinator(s)
//! audio ────────┘                └─ Hype Map / stats              │        │ CaptureResult
//! hotkey / voice / API / UI ──── manual HighlightEvent ───────────┘        ▼
//!                                         SQLite job queue ◄── clip record (Rendering)
//!                                                │ render workers (bounded)
//!                                                ▼
//!   analysis → whisper → metadata → EditPlan → FFmpeg render → thumbnails → Review
//!                                                │ approve (or gated auto-publish)
//!                                                ▼
//!                                         upload worker → YouTube / Instagram / TikTok
//! ```

pub mod api;
pub mod demo;
pub mod multistream;
pub mod pipeline;
pub mod voice;

use pulsecut_capture::coordinator::{self, Backend, CaptureHandle, CaptureResult, CoordinatorConfig};
use pulsecut_capture::obs::{ObsConfig, ObsController, ObsStatus};
use pulsecut_capture::segment::{SegmentConfig, SegmentRecorder, SegmentStatus};
use pulsecut_chat::{SourceState, StreamChat};
use pulsecut_core::paths::AppPaths;
use pulsecut_core::process::Tools;
use pulsecut_core::settings::CaptureBackend;
use pulsecut_core::{
    CaptureRequest, ClipMetadata, ClipStatus, HighlightEvent, Result, Settings, TriggerSource, TsMs, now_ms,
};
use pulsecut_detect::merge::MergePolicy;
use pulsecut_detect::scorer::{HypePoint, LiveStats};
use pulsecut_detect::{AudioSecond, Detector, DetectorConfig, Weights};
use pulsecut_media::assets::{AssetLibrary, install_builtin_pack};
use pulsecut_media::encoder::{EncoderProbe, VideoEncoder};
use pulsecut_media::textimg::Font;
use pulsecut_platform::secrets::{SecretStore, keys};
use pulsecut_platform::sysstats::{Monitor, Snapshot};
use pulsecut_publish::oauth::OAuthClient;
use pulsecut_publish::worker::{ProgressMap, TokenStore, Worker, WorkerConfig};
use pulsecut_store::{ClipFilter, ClipRecord, Store, UploadRecord};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum EngineEvent {
    Highlight(HighlightEvent),
    CaptureFailed { stream_id: String, error: String },
    ClipChanged(Uuid),
    Notice(String),
    Error(String),
}

#[derive(Debug, Clone)]
pub struct StreamView {
    pub stream_id: String,
    pub stats: LiveStats,
    /// Recent mean score / threshold (1.0 = at threshold).
    pub hype_index: f32,
    pub capturing: bool,
    pub is_home: bool,
}

pub struct StartOptions {
    pub secrets: Arc<dyn SecretStore>,
    /// Override the OBS password (e.g. for the mock); otherwise read from the keychain.
    pub obs_password: Option<String>,
    pub start_chat: bool,
    pub start_api: bool,
    pub start_uploads: bool,
}

struct ChatHandles {
    twitch: Option<pulsecut_chat::twitch::TwitchChat>,
    youtube: Vec<pulsecut_chat::youtube::YouTubeChat>,
}

pub struct Engine {
    pub paths: AppPaths,
    settings: RwLock<Settings>,
    pub tools: Tools,
    pub store: Store,
    pub secrets: Arc<dyn SecretStore>,
    assets: RwLock<Arc<AssetLibrary>>,
    font: Option<Arc<Font>>,
    encoder: RwLock<VideoEncoder>,
    encoder_probes: RwLock<Vec<EncoderProbe>>,
    detectors: Mutex<BTreeMap<String, Detector>>,
    weights: RwLock<Weights>,
    captures: Mutex<HashMap<String, CaptureHandle>>,
    home_stream: RwLock<String>,
    obs: Mutex<Option<Arc<ObsController>>>,
    home_segments: Mutex<Option<Arc<SegmentRecorder>>>,
    pub(crate) extra_recorders: Mutex<HashMap<String, (Arc<SegmentRecorder>, TsMs)>>,
    chat_tx: mpsc::Sender<StreamChat>,
    pub(crate) audio_tx: mpsc::Sender<(String, AudioSecond)>,
    chat: Mutex<ChatHandles>,
    chat_states: Mutex<BTreeMap<String, SourceState>>,
    events: broadcast::Sender<EngineEvent>,
    clips_cache: RwLock<Vec<ClipRecord>>,
    uploads_cache: RwLock<Vec<UploadRecord>>,
    logs: Mutex<VecDeque<(TsMs, String)>>,
    pub upload_progress: ProgressMap,
    worker_cfg: Arc<Mutex<WorkerConfig>>,
    capture_paused: AtomicBool,
    sys: Mutex<(Monitor, Snapshot)>,
    pub(crate) results_tx: mpsc::Sender<CaptureResult>,
    pub(crate) voice: Mutex<Option<voice::VoiceHandle>>,
    pub rt: tokio::runtime::Handle,
}

struct KeychainTokens(Arc<dyn SecretStore>);
impl TokenStore for KeychainTokens {
    fn get(&self, name: &str) -> Option<String> {
        self.0.get(name).ok().flatten()
    }
    fn set(&self, name: &str, value: &str) {
        if let Err(e) = self.0.set(name, value) {
            tracing::error!("could not store {name}: {e}");
        }
    }
}

fn worker_config(s: &Settings, secrets: &Arc<dyn SecretStore>) -> WorkerConfig {
    let p = &s.publish;
    let yt_secret = secrets.get(keys::YOUTUBE_CLIENT_SECRET).ok().flatten();
    let tt_secret = secrets.get(keys::TIKTOK_CLIENT_SECRET).ok().flatten();
    let mut caps = HashMap::new();
    caps.insert("youtube".into(), 90); // stay under the 100/day videos.insert bucket
    caps.insert("instagram".into(), 45); // under the 50 posts / 24 h limit
    caps.insert("tiktok".into(), 15);
    WorkerConfig {
        youtube_client: (!p.youtube_client_id.is_empty()).then(|| OAuthClient::google(&p.youtube_client_id, yt_secret)),
        youtube_category: p.youtube_category_id.clone(),
        instagram_user_id: p.instagram_user_id.clone(),
        hosting: (!p.media_host_dir.is_empty()).then(|| pulsecut_publish::instagram::Hosting {
            dir: p.media_host_dir.clone().into(),
            public_base: p.instagram_public_base_url.clone(),
        }),
        tiktok_client: (!p.tiktok_client_key.is_empty()).then(|| OAuthClient::tiktok(&p.tiktok_client_key, tt_secret)),
        max_attempts: p.max_attempts as i64,
        daily_caps: caps,
    }
}

impl Engine {
    /// Start everything. Must be called from within a Tokio runtime.
    pub async fn start(paths: AppPaths, mut settings: Settings, opts: StartOptions) -> Result<Arc<Engine>> {
        paths.ensure()?;
        settings.sanitize();
        let store = Store::open(&paths.database_file())
            .await
            .map_err(|e| pulsecut_core::Error::other(format!("database: {e}")))?;
        let (jobs, uploads) = store.recover().await.unwrap_or((0, 0));
        if jobs + uploads > 0 {
            tracing::info!("crash recovery: re-queued {jobs} jobs and {uploads} uploads");
        }
        install_builtin_pack(&paths.assets_dir())?;
        let assets = Arc::new(AssetLibrary::load(&paths.assets_dir()));
        let mut model_dirs = vec![paths.models_dir(), std::path::PathBuf::from("models")];
        if let Ok(exe) = std::env::current_exe()
            && let Some(d) = exe.parent()
        {
            model_dirs.push(d.join("models"));
            model_dirs.push(d.join("../Resources/models"));
        }
        let tools = Tools::discover(&settings.tools, &model_dirs);
        let font = Font::load(&settings.edit.font_path).ok().map(Arc::new);
        if font.is_none() {
            tracing::warn!("no font available: captions and text overlays disabled");
        }
        let weights = if settings.detection.weights_file.is_empty() {
            Weights::default()
        } else {
            Weights::load(std::path::Path::new(&settings.detection.weights_file)).unwrap_or_default()
        };
        let (chat_tx, chat_rx) = mpsc::channel(8192);
        let (audio_tx, audio_rx) = mpsc::channel(512);
        let (results_tx, results_rx) = mpsc::channel(32);
        let (events, _) = broadcast::channel(256);
        let wcfg = Arc::new(Mutex::new(worker_config(&settings, &opts.secrets)));
        let engine = Arc::new(Engine {
            paths: paths.clone(),
            settings: RwLock::new(settings.clone()),
            tools,
            store: store.clone(),
            secrets: Arc::clone(&opts.secrets),
            assets: RwLock::new(assets),
            font,
            encoder: RwLock::new(VideoEncoder::X264),
            encoder_probes: RwLock::new(vec![]),
            detectors: Mutex::new(BTreeMap::new()),
            weights: RwLock::new(weights),
            captures: Mutex::new(HashMap::new()),
            home_stream: RwLock::new(String::new()),
            obs: Mutex::new(None),
            home_segments: Mutex::new(None),
            extra_recorders: Mutex::new(HashMap::new()),
            chat_tx,
            audio_tx,
            chat: Mutex::new(ChatHandles {
                twitch: None,
                youtube: vec![],
            }),
            chat_states: Mutex::new(BTreeMap::new()),
            events,
            clips_cache: RwLock::new(vec![]),
            uploads_cache: RwLock::new(vec![]),
            logs: Mutex::new(VecDeque::new()),
            upload_progress: Default::default(),
            worker_cfg: Arc::clone(&wcfg),
            capture_paused: AtomicBool::new(false),
            sys: Mutex::new((Monitor::new(), Snapshot::default())),
            results_tx,
            voice: Mutex::new(None),
            rt: tokio::runtime::Handle::current(),
        });

        // Encoder probing in the background (a real test encode per candidate).
        {
            let e = Arc::clone(&engine);
            tokio::spawn(async move {
                if let Ok(p) = pulsecut_media::encoder::probe_encoders(&e.tools).await {
                    let prefer = e.settings().edit.hardware_encode;
                    let chosen = pulsecut_media::encoder::choose(&p, prefer);
                    *e.encoder.write().unwrap() = chosen;
                    e.log(format!("video encoder: {} (tested)", chosen.ffmpeg_name()));
                    *e.encoder_probes.write().unwrap() = p;
                }
            });
        }

        engine.start_capture_backend(opts.obs_password.clone());
        if opts.start_chat {
            engine.start_chat_sources();
        }
        engine.spawn_ingest(chat_rx, audio_rx);
        engine.spawn_ticker();
        engine.spawn_capture_consumer(results_rx);
        for _ in 0..settings.resources.max_render_jobs {
            engine.spawn_render_worker();
        }
        if opts.start_uploads {
            let w = Arc::new(Worker {
                store: store.clone(),
                tokens: Arc::new(KeychainTokens(Arc::clone(&opts.secrets))),
                cfg: wcfg,
                youtube: Default::default(),
                instagram: Default::default(),
                tiktok: Default::default(),
                progress: Arc::clone(&engine.upload_progress),
            });
            w.spawn();
        }
        engine.spawn_maintenance();
        multistream::spawn(Arc::clone(&engine));
        if opts.start_api && settings.resources.local_api_port != 0 {
            api::spawn(Arc::clone(&engine), settings.resources.local_api_port);
        }
        if (settings.audio.mic_enabled || settings.voice.enabled)
            && let Err(e) = voice::start(&engine)
        {
            engine.log(format!("microphone not started: {e}"));
        }
        engine.refresh_caches().await;
        engine.log("PulseCut engine started".into());
        Ok(engine)
    }

    // ---------- accessors ----------

    pub fn settings(&self) -> Settings {
        self.settings.read().unwrap().clone()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.events.subscribe()
    }

    pub fn log(&self, msg: String) {
        tracing::info!("{msg}");
        let mut l = self.logs.lock().unwrap();
        if l.len() >= 400 {
            l.pop_front();
        }
        l.push_back((now_ms(), msg.clone()));
        let _ = self.events.send(EngineEvent::Notice(msg));
    }

    pub fn logs(&self) -> Vec<(TsMs, String)> {
        self.logs.lock().unwrap().iter().cloned().collect()
    }

    pub fn home_stream(&self) -> String {
        self.home_stream.read().unwrap().clone()
    }

    pub fn font(&self) -> Option<Arc<Font>> {
        self.font.clone()
    }

    pub fn assets(&self) -> Arc<AssetLibrary> {
        Arc::clone(&self.assets.read().unwrap())
    }

    pub fn reload_assets(&self) {
        *self.assets.write().unwrap() = Arc::new(AssetLibrary::load(&self.paths.assets_dir()));
    }

    pub fn encoder(&self) -> VideoEncoder {
        *self.encoder.read().unwrap()
    }

    pub fn encoder_probes(&self) -> Vec<EncoderProbe> {
        self.encoder_probes.read().unwrap().clone()
    }

    pub fn obs_status(&self) -> Option<ObsStatus> {
        self.obs.lock().unwrap().as_ref().map(|o| o.status().borrow().clone())
    }

    pub fn segment_status(&self) -> Option<SegmentStatus> {
        self.home_segments
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.status().borrow().clone())
    }

    pub fn chat_states(&self) -> BTreeMap<String, SourceState> {
        self.chat_states.lock().unwrap().clone()
    }

    pub fn system(&self) -> Snapshot {
        self.sys.lock().unwrap().1
    }

    pub fn capture_paused(&self) -> bool {
        self.capture_paused.load(Ordering::Relaxed)
    }

    pub fn clips(&self) -> Vec<ClipRecord> {
        self.clips_cache.read().unwrap().clone()
    }

    pub fn uploads(&self) -> Vec<UploadRecord> {
        self.uploads_cache.read().unwrap().clone()
    }

    pub fn streams(&self) -> Vec<StreamView> {
        let home = self.home_stream();
        let caps = self.captures.lock().unwrap();
        let dets = self.detectors.lock().unwrap();
        let mut v: Vec<StreamView> = dets
            .iter()
            .map(|(id, d)| StreamView {
                stream_id: id.clone(),
                stats: d.stats(),
                hype_index: multistream::hype_index(d),
                capturing: caps.contains_key(id) || (id == &home && caps.contains_key("home")),
                is_home: id == &home,
            })
            .collect();
        v.sort_by(|a, b| b.is_home.cmp(&a.is_home).then(b.hype_index.total_cmp(&a.hype_index)));
        v
    }

    /// Hype Map points for a stream (last `secs` seconds).
    pub fn hype_history(&self, stream_id: &str, secs: usize) -> Vec<HypePoint> {
        self.detectors
            .lock()
            .unwrap()
            .get(stream_id)
            .map(|d| {
                let h = d.history();
                h.iter().skip(h.len().saturating_sub(secs)).copied().collect()
            })
            .unwrap_or_default()
    }

    pub async fn refresh_caches(&self) {
        if let Ok(c) = self
            .store
            .list_clips(&ClipFilter {
                limit: 500,
                ..Default::default()
            })
            .await
        {
            *self.clips_cache.write().unwrap() = c;
        }
        if let Ok(u) = self.store.list_uploads(200).await {
            *self.uploads_cache.write().unwrap() = u;
        }
    }

    // ---------- triggers ----------

    /// Manual clip (hotkey, voice, UI button, local API). Returns the event id.
    pub fn clip_now(self: &Arc<Self>, trigger: TriggerSource) -> Uuid {
        let s = self.settings();
        let stream = self.home_stream();
        let ev = HighlightEvent::manual(stream.clone(), trigger, now_ms());
        let id = ev.id;
        let req = CaptureRequest::around(ev.clone(), s.capture.pre_roll_ms(), s.capture.post_roll_ms());
        let _ = self.events.send(EngineEvent::Highlight(ev.clone()));
        let st = self.store.clone();
        tokio::spawn(async move {
            let _ = st.log_event(&ev).await;
        });
        if !self.request_capture(&stream, req) {
            self.log(
                "manual clip requested but no capture source is running (connect OBS or set a stream source)".into(),
            );
        } else {
            self.log(format!("{} clip requested", trigger.label()));
        }
        id
    }

    /// Drop a marker on the home stream's timeline without capturing (review later).
    pub fn mark_moment(self: &Arc<Self>) {
        let mut ev = HighlightEvent::manual(self.home_stream(), TriggerSource::Manual, now_ms());
        ev.reasons[0].detail = "marker (no capture)".into();
        let _ = self.events.send(EngineEvent::Highlight(ev.clone()));
        let st = self.store.clone();
        tokio::spawn(async move {
            let _ = st.log_event(&ev).await;
        });
        self.log("marker added to the timeline".into());
    }

    /// Recent events from the database (detections, manual clips, markers).
    pub async fn recent_events(&self, n: i64) -> Vec<HighlightEvent> {
        self.store.recent_events(n).await.unwrap_or_default()
    }

    fn request_capture(&self, stream_id: &str, req: CaptureRequest) -> bool {
        if self.capture_paused() && !req.event.trigger.is_manual() {
            self.log("capture paused: low disk space".into());
            return false;
        }
        let caps = self.captures.lock().unwrap();
        let home = self.home_stream();
        let h = caps
            .get(stream_id)
            .or_else(|| (stream_id == home).then(|| caps.get("home")).flatten());
        h.map(|h| h.request(req)).unwrap_or(false)
    }

    /// Feed a chat message (used by tests, the demo and webhook receivers).
    pub fn inject_chat(&self, stream_id: &str, msg: pulsecut_detect::ChatMessage) {
        let _ = self.chat_tx.try_send(StreamChat {
            stream_id: stream_id.to_string(),
            msg,
        });
    }

    pub fn chat_sender(&self) -> mpsc::Sender<StreamChat> {
        self.chat_tx.clone()
    }

    // ---------- sources ----------

    fn start_capture_backend(self: &Arc<Self>, obs_password: Option<String>) {
        let s = self.settings();
        let home = if let Some(c) = s.chat.twitch_channels.first() {
            format!("twitch:{c}")
        } else if let Some(y) = s.chat.youtube_targets.first() {
            format!("youtube:{y}")
        } else {
            "local".to_string()
        };
        *self.home_stream.write().unwrap() = home.clone();
        let policy = self.merge_policy();
        let cfg = || CoordinatorConfig {
            policy,
            save_margin_ms: (s.capture.save_margin_secs * 1000.0) as i64,
            raw_dir: self.paths.raw_dir(),
        };
        match s.capture.backend {
            CaptureBackend::ObsReplayBuffer => {
                if !s.obs.enabled {
                    self.log("OBS capture disabled in settings; enable it to capture clips".into());
                    return;
                }
                let password = obs_password.or_else(|| {
                    s.obs
                        .use_password
                        .then(|| self.secrets.get(keys::OBS_PASSWORD).ok().flatten())
                        .flatten()
                });
                let obs = ObsController::spawn(ObsConfig {
                    host: s.obs.host.clone(),
                    port: s.obs.port,
                    password,
                    auto_start_replay_buffer: s.obs.auto_start_replay_buffer,
                    required_buffer_secs: s.required_replay_buffer_secs(),
                });
                let h = coordinator::spawn(
                    Backend::Obs(Arc::clone(&obs)),
                    self.tools.clone(),
                    cfg(),
                    self.results_tx.clone(),
                );
                *self.obs.lock().unwrap() = Some(obs);
                self.captures.lock().unwrap().insert("home".into(), h);
            }
            CaptureBackend::SegmentBuffer => {
                if s.capture.segment_source.trim().is_empty() {
                    self.log("segment buffer selected but no source configured".into());
                    return;
                }
                let (atx, mut arx) = mpsc::channel::<AudioSecond>(256);
                let rec = SegmentRecorder::spawn(
                    SegmentConfig {
                        input: s.capture.segment_source.clone(),
                        dir: self.paths.segments_dir().join("home"),
                        segment_secs: s.capture.segment_secs,
                        ring_secs: s
                            .capture
                            .segment_ring_secs
                            .max(s.required_replay_buffer_secs() as u32 + 20),
                        copy_codec: s.capture.segment_copy_codec,
                        audio_analysis: s.audio.stream_audio_enabled,
                    },
                    self.tools.clone(),
                    Some(atx),
                );
                match rec {
                    Ok(rec) => {
                        let tx = self.audio_tx.clone();
                        let hs = home.clone();
                        tokio::spawn(async move {
                            while let Some(a) = arx.recv().await {
                                let _ = tx.try_send((hs.clone(), a));
                            }
                        });
                        let h = coordinator::spawn(
                            Backend::Segments(Arc::clone(&rec)),
                            self.tools.clone(),
                            cfg(),
                            self.results_tx.clone(),
                        );
                        *self.home_segments.lock().unwrap() = Some(rec);
                        self.captures.lock().unwrap().insert("home".into(), h);
                    }
                    Err(e) => self.log(format!("segment buffer failed to start: {e}")),
                }
            }
        }
    }

    pub(crate) fn merge_policy(&self) -> MergePolicy {
        let s = self.settings();
        MergePolicy {
            pre_ms: s.capture.pre_roll_ms(),
            post_ms: s.capture.post_roll_ms(),
            cooldown_ms: (s.detection.cooldown_secs * 1000.0) as i64,
            max_len_ms: (s.capture.max_clip_secs * 1000.0) as i64,
        }
    }

    pub(crate) fn register_capture(&self, stream_id: &str, h: CaptureHandle) {
        self.captures.lock().unwrap().insert(stream_id.to_string(), h);
    }

    pub(crate) fn unregister_capture(&self, stream_id: &str) {
        self.captures.lock().unwrap().remove(stream_id);
    }

    fn start_chat_sources(self: &Arc<Self>) {
        let s = self.settings();
        let mut handles = self.chat.lock().unwrap();
        if !s.chat.twitch_channels.is_empty() {
            let t = pulsecut_chat::twitch::spawn(s.chat.twitch_channels.clone(), self.chat_tx.clone(), None);
            let mut st = t.state.clone();
            let e = Arc::clone(self);
            tokio::spawn(async move {
                loop {
                    let v = st.borrow().clone();
                    e.chat_states.lock().unwrap().insert("twitch".into(), v);
                    if st.changed().await.is_err() {
                        break;
                    }
                }
            });
            handles.twitch = Some(t);
        }
        if !s.chat.youtube_targets.is_empty() {
            match self.secrets.get(keys::YOUTUBE_API_KEY).ok().flatten() {
                Some(key) => {
                    for vid in &s.chat.youtube_targets {
                        let y = pulsecut_chat::youtube::spawn(
                            vid.clone(),
                            key.clone(),
                            self.chat_tx.clone(),
                            Duration::from_secs(10),
                            None,
                        );
                        let mut st = y.state.clone();
                        let e = Arc::clone(self);
                        let name = format!("youtube:{vid}");
                        tokio::spawn(async move {
                            loop {
                                let v = st.borrow().clone();
                                e.chat_states.lock().unwrap().insert(name.clone(), v);
                                if st.changed().await.is_err() {
                                    break;
                                }
                            }
                        });
                        handles.youtube.push(y);
                    }
                }
                None => self.log("YouTube chat targets set but no API key in the keychain".into()),
            }
        }
    }

    fn stop_sources(&self) {
        let mut h = self.chat.lock().unwrap();
        if let Some(t) = h.twitch.take() {
            t.stop();
        }
        for y in h.youtube.drain(..) {
            y.stop();
        }
        if let Some(o) = self.obs.lock().unwrap().take() {
            o.shutdown();
        }
        if let Some(r) = self.home_segments.lock().unwrap().take() {
            r.stop();
        }
        for (_, (r, _)) in self.extra_recorders.lock().unwrap().drain() {
            r.stop();
        }
        self.captures.lock().unwrap().clear();
        self.chat_states.lock().unwrap().clear();
    }

    /// Save new settings and apply them live (sources restart only when their config changed).
    pub fn apply_settings(self: &Arc<Self>, mut new: Settings) -> Result<()> {
        new.sanitize();
        new.save(&self.paths.settings_file())?;
        let old = std::mem::replace(&mut *self.settings.write().unwrap(), new.clone());
        *self.worker_cfg.lock().unwrap() = worker_config(&new, &self.secrets);
        let weights = self.weights.read().unwrap().clone();
        for (id, d) in self.detectors.lock().unwrap().iter_mut() {
            d.cfg = DetectorConfig::from_settings(id.clone(), &new, weights.clone());
        }
        let sources_changed = old.obs != new.obs
            || old.capture != new.capture
            || old.chat.twitch_channels != new.chat.twitch_channels
            || old.chat.youtube_targets != new.chat.youtube_targets;
        if sources_changed {
            self.stop_sources();
            self.start_capture_backend(None);
            self.start_chat_sources();
            self.log("sources restarted with new settings".into());
        }
        let mic_changed = old.audio != new.audio || old.voice != new.voice;
        if mic_changed {
            voice::stop(self);
            if (new.audio.mic_enabled || new.voice.enabled)
                && let Err(e) = voice::start(self)
            {
                self.log(format!("microphone: {e}"));
            }
        }
        Ok(())
    }

    // ---------- background tasks ----------

    fn detector_for<'a>(&self, dets: &'a mut BTreeMap<String, Detector>, stream_id: &str) -> &'a mut Detector {
        if !dets.contains_key(stream_id) {
            let cfg = DetectorConfig::from_settings(
                stream_id.to_string(),
                &self.settings(),
                self.weights.read().unwrap().clone(),
            );
            dets.insert(stream_id.to_string(), Detector::new(cfg));
        }
        dets.get_mut(stream_id).unwrap()
    }

    fn spawn_ingest(
        self: &Arc<Self>,
        mut chat_rx: mpsc::Receiver<StreamChat>,
        mut audio_rx: mpsc::Receiver<(String, AudioSecond)>,
    ) {
        let e = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    m = chat_rx.recv() => {
                        let Some(m) = m else { break };
                        let mut d = e.detectors.lock().unwrap();
                        e.detector_for(&mut d, &m.stream_id).push_chat(&m.msg);
                    }
                    a = audio_rx.recv() => {
                        let Some((sid, a)) = a else { break };
                        let mut d = e.detectors.lock().unwrap();
                        e.detector_for(&mut d, &sid).push_audio(a);
                    }
                }
            }
        });
    }

    fn spawn_ticker(self: &Arc<Self>) {
        let e = Arc::clone(self);
        tokio::spawn(async move {
            let mut iv = tokio::time::interval(Duration::from_secs(1));
            loop {
                iv.tick().await;
                let now = now_ms();
                let fired: Vec<HighlightEvent> = {
                    let mut d = e.detectors.lock().unwrap();
                    d.values_mut().flat_map(|det| det.tick(now)).collect()
                };
                for ev in fired {
                    let _ = e.store.log_event(&ev).await;
                    let _ = e.events.send(EngineEvent::Highlight(ev.clone()));
                    let s = e.settings();
                    if !s.detection.autonomous_enabled {
                        continue;
                    }
                    let req = CaptureRequest::around(ev.clone(), s.capture.pre_roll_ms(), s.capture.post_roll_ms());
                    let queued = e.request_capture(&ev.stream_id, req);
                    e.log(format!(
                        "{} moment on {} (confidence {:.0}%): {}{}",
                        ev.kind.label(),
                        ev.stream_id,
                        ev.confidence * 100.0,
                        ev.reason_summary(),
                        if queued {
                            ""
                        } else {
                            " — not captured (no buffer for this stream)"
                        }
                    ));
                }
            }
        });
    }

    fn spawn_capture_consumer(self: &Arc<Self>, mut rx: mpsc::Receiver<CaptureResult>) {
        let e = Arc::clone(self);
        tokio::spawn(async move {
            while let Some(r) = rx.recv().await {
                let mut clip = ClipRecord::new(r.primary.clone(), r.clip_start, r.clip_end);
                if let Some(f) = &r.file {
                    clip.raw_path = Some(f.path.display().to_string());
                    clip.coverage = Some(f.coverage);
                }
                match (&r.file, &r.error) {
                    (Some(f), None) => {
                        clip.status = ClipStatus::Rendering;
                        if e.store.upsert_clip(&clip).await.is_ok() {
                            let payload = serde_json::json!({"file": f, "events": r.events});
                            let _ = e.store.enqueue_job("render", Some(clip.id), &payload).await;
                        }
                    }
                    (_, Some(err)) => {
                        clip.status = ClipStatus::Failed;
                        clip.error = Some(err.clone());
                        let _ = e.store.upsert_clip(&clip).await;
                        e.log(format!("capture failed for {}: {err}", r.primary.stream_id));
                        let _ = e.events.send(EngineEvent::CaptureFailed {
                            stream_id: r.primary.stream_id.clone(),
                            error: err.clone(),
                        });
                    }
                    (None, None) => {}
                }
                e.refresh_caches().await;
                let _ = e.events.send(EngineEvent::ClipChanged(clip.id));
            }
        });
    }

    fn spawn_render_worker(self: &Arc<Self>) {
        let e = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                let job = match e.store.claim_job("render").await {
                    Ok(Some(j)) => j,
                    Ok(None) => {
                        tokio::time::sleep(Duration::from_millis(700)).await;
                        continue;
                    }
                    Err(err) => {
                        tracing::error!("job queue: {err}");
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                };
                let Some(clip_id) = job.clip_id else {
                    let _ = e.store.finish_job(job.id, false, Some("no clip id")).await;
                    continue;
                };
                let r = pipeline::render_clip(&e, clip_id, &job.payload).await;
                match r {
                    Ok(()) => {
                        let _ = e.store.finish_job(job.id, true, None).await;
                    }
                    Err(err) if job.attempts < 3 => {
                        e.log(format!("render attempt {} failed (will retry): {err}", job.attempts));
                        let _ = e
                            .store
                            .retry_job_later(job.id, 10_000 * job.attempts, &err.to_string())
                            .await;
                    }
                    Err(err) => {
                        let _ = e.store.finish_job(job.id, false, Some(&err.to_string())).await;
                        let _ = e
                            .store
                            .set_clip_status(clip_id, ClipStatus::Failed, Some(&err.to_string()))
                            .await;
                        e.log(format!("render failed: {err}"));
                    }
                }
                e.refresh_caches().await;
                let _ = e.events.send(EngineEvent::ClipChanged(clip_id));
            }
        });
    }

    fn spawn_maintenance(self: &Arc<Self>) {
        let e = Arc::clone(self);
        tokio::spawn(async move {
            let mut iv = tokio::time::interval(Duration::from_secs(2));
            let mut n: u64 = 0;
            loop {
                iv.tick().await;
                n += 1;
                let snap = {
                    let mut s = e.sys.lock().unwrap();
                    let snap = s.0.snapshot(&e.paths.data_dir);
                    s.1 = snap;
                    snap
                };
                let min_free = e.settings().storage.min_free_gb;
                let low = snap.disk_free_gb.is_finite() && snap.disk_free_gb < min_free;
                if low != e.capture_paused.swap(low, Ordering::Relaxed) {
                    e.log(if low {
                        format!(
                            "disk space low ({:.1} GB < {min_free} GB): autonomous capture paused",
                            snap.disk_free_gb
                        )
                    } else {
                        "disk space OK: capture resumed".into()
                    });
                }
                if n.is_multiple_of(5) {
                    e.refresh_caches().await;
                }
                if n % 1800 == 1 {
                    e.retention().await;
                }
            }
        });
    }

    /// Delete expired clips (published/rejected/failed older than retention).
    pub async fn retention(&self) {
        let days = self.settings().storage.retention_days as i64;
        if days == 0 {
            return;
        }
        let before = now_ms() - days * 86_400_000;
        if let Ok(old) = self.store.expired_clips(before).await {
            for c in old {
                self.remove_clip_files(&c);
                let _ = self.store.delete_clip(c.id).await;
            }
        }
    }

    fn remove_clip_files(&self, c: &ClipRecord) {
        for p in [
            &c.raw_path,
            &c.video_path,
            &c.landscape_path,
            &c.thumb_path,
            &c.thumb_vertical_path,
        ]
        .into_iter()
        .flatten()
        {
            // Only delete files inside PulseCut's own data directory.
            if std::path::Path::new(p).starts_with(&self.paths.data_dir) {
                let _ = std::fs::remove_file(p);
            }
        }
    }

    // ---------- library actions ----------

    pub async fn approve(self: &Arc<Self>, id: Uuid) -> Result<()> {
        self.store
            .set_clip_status(id, ClipStatus::Approved, None)
            .await
            .map_err(|e| pulsecut_core::Error::other(e.to_string()))?;
        self.refresh_caches().await;
        let _ = self.events.send(EngineEvent::ClipChanged(id));
        Ok(())
    }

    pub async fn reject(self: &Arc<Self>, id: Uuid) -> Result<()> {
        self.store
            .set_clip_status(id, ClipStatus::Rejected, None)
            .await
            .map_err(|e| pulsecut_core::Error::other(e.to_string()))?;
        self.refresh_caches().await;
        Ok(())
    }

    pub async fn delete_clip(self: &Arc<Self>, id: Uuid) -> Result<()> {
        if let Ok(Some(c)) = self.store.get_clip(id).await {
            self.remove_clip_files(&c);
        }
        self.store
            .delete_clip(id)
            .await
            .map_err(|e| pulsecut_core::Error::other(e.to_string()))?;
        self.refresh_caches().await;
        Ok(())
    }

    pub async fn update_meta(self: &Arc<Self>, id: Uuid, meta: ClipMetadata) -> Result<()> {
        self.store
            .update_meta(id, &meta)
            .await
            .map_err(|e| pulsecut_core::Error::other(e.to_string()))?;
        self.refresh_caches().await;
        Ok(())
    }

    /// Re-render with an edited plan (from the Editor page).
    pub async fn rerender(self: &Arc<Self>, id: Uuid, plan: Option<pulsecut_core::EditPlan>) -> Result<()> {
        let mut c = self
            .store
            .get_clip(id)
            .await
            .ok()
            .flatten()
            .ok_or_else(|| pulsecut_core::Error::other("clip not found"))?;
        if c.raw_path.is_none() {
            return Err(pulsecut_core::Error::other("raw footage was deleted; cannot re-render"));
        }
        if let Some(p) = plan {
            c.plan = Some(p);
        }
        c.status = ClipStatus::Rendering;
        self.store
            .upsert_clip(&c)
            .await
            .map_err(|e| pulsecut_core::Error::other(e.to_string()))?;
        let payload = serde_json::json!({"rerender": true});
        self.store
            .enqueue_job("render", Some(id), &payload)
            .await
            .map_err(|e| pulsecut_core::Error::other(e.to_string()))?;
        self.refresh_caches().await;
        Ok(())
    }

    /// Queue an upload (requires the clip to be approved). Duplicate-safe.
    pub async fn publish(self: &Arc<Self>, id: Uuid, platform: &str, scheduled_at: Option<TsMs>) -> Result<String> {
        let c = self
            .store
            .get_clip(id)
            .await
            .ok()
            .flatten()
            .ok_or_else(|| pulsecut_core::Error::other("clip not found"))?;
        if !matches!(
            c.status,
            ClipStatus::Approved | ClipStatus::Published | ClipStatus::Publishing
        ) {
            return Err(pulsecut_core::Error::other("approve the clip before publishing"));
        }
        let meta = c.meta.clone().unwrap_or_default();
        let s = self.settings();
        let (u, created) = self
            .store
            .create_upload(
                id,
                platform,
                "default",
                &meta.title,
                &meta.description,
                &s.publish.youtube_privacy,
                scheduled_at,
            )
            .await
            .map_err(|e| pulsecut_core::Error::other(e.to_string()))?;
        self.refresh_caches().await;
        Ok(if created {
            format!("queued for {platform}")
        } else {
            format!("already {} on {platform}", u.state.as_str())
        })
    }

    /// Retry a failed upload now.
    pub async fn retry_upload(self: &Arc<Self>, upload_id: Uuid) -> Result<()> {
        if let Ok(Some(mut u)) = self.store.get_upload(upload_id).await {
            u.state = pulsecut_store::UploadState::Queued;
            u.next_attempt_at = 0;
            u.attempts = 0;
            u.error = None;
            self.store
                .save_upload(&u)
                .await
                .map_err(|e| pulsecut_core::Error::other(e.to_string()))?;
        }
        self.refresh_caches().await;
        Ok(())
    }

    /// Connect a YouTube account via browser OAuth (loopback + PKCE).
    pub async fn connect_youtube(self: &Arc<Self>, open_browser: impl FnOnce(&str)) -> Result<()> {
        let client = self.worker_cfg.lock().unwrap().youtube_client.clone().ok_or_else(|| {
            pulsecut_core::Error::other("set your YouTube OAuth client id first (Settings → Platforms)")
        })?;
        let t = pulsecut_publish::oauth::authorize(&client, open_browser)
            .await
            .map_err(pulsecut_core::Error::other)?;
        self.secrets
            .set(keys::YOUTUBE_OAUTH, &serde_json::to_string(&t).unwrap_or_default())?;
        self.log("YouTube account connected".into());
        Ok(())
    }

    pub fn account_connected(&self, key: &str) -> bool {
        self.secrets.get(key).ok().flatten().is_some()
    }

    /// Graceful shutdown: stop sources (FFmpeg children are killed on drop).
    pub fn shutdown(&self) {
        voice::stop_inner(self);
        self.stop_sources();
        self.log("engine stopped".into());
    }
}
