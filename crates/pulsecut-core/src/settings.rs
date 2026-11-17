//! User settings, persisted as TOML in the app config directory. Secrets (OBS password,
//! OAuth tokens) are NOT stored here; they live in the OS keychain (`pulsecut-platform`).

use crate::domain::{AspectFormat, CaptionStyle, LayoutMode, NormRect};
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Settings {
    pub obs: ObsSettings,
    pub capture: CaptureSettings,
    pub detection: DetectionSettings,
    pub chat: ChatSettings,
    pub audio: AudioSettings,
    pub voice: VoiceSettings,
    pub hotkeys: HotkeySettings,
    pub edit: EditSettings,
    pub meta: MetaSettings,
    pub publish: PublishSettings,
    pub storage: StorageSettings,
    pub tools: ToolSettings,
    pub privacy: PrivacySettings,
    pub resources: ResourceSettings,
    pub startup: StartupSettings,
    /// Twitch watchlist, trending discovery and stream-selection rules.
    pub watch: WatchSettings,
}

/// One streamer on the watchlist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WatchStreamer {
    /// Twitch login (lowercase channel name).
    pub login: String,
    pub enabled: bool,
    pub favorite: bool,
    /// Higher = preferred. Typical range -10..10.
    pub priority: i32,
}

impl Default for WatchStreamer {
    fn default() -> Self {
        WatchStreamer {
            login: String::new(),
            enabled: true,
            favorite: false,
            priority: 0,
        }
    }
}

/// Unified stream discovery and selection ("Hot Watch"). Watchlisted streamers come first;
/// trending live streams are used as a fallback when no watchlisted stream is live.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WatchSettings {
    /// Master switch for autonomous stream monitoring (buffers + selection).
    pub monitoring_enabled: bool,
    /// Pick streams automatically. When false, only `manual_override` is monitored.
    pub auto_select: bool,
    /// Login to force-monitor (empty = none). Takes the first slot while it is live.
    pub manual_override: String,
    /// Fall back to trending live streams when no watchlisted stream is live.
    pub trending_enabled: bool,
    /// Optional filters for trending discovery (Twitch Helix `language`, `game_id`).
    pub trending_language: String,
    pub trending_game_id: String,
    pub trending_min_viewers: u32,
    /// How many trending candidates to consider per poll.
    pub trending_candidates: u32,
    /// A newly selected stream is kept at least this long (unless it goes offline).
    pub min_watch_secs: u32,
    /// Minimum time between voluntary switches.
    pub switch_cooldown_secs: u32,
    /// A challenger must score this many times higher than an incumbent to replace it.
    pub hysteresis: f32,
    /// A live favorite replaces a non-favorite once `min_watch_secs` has passed,
    /// regardless of `hysteresis`.
    pub favorites_preempt: bool,
    /// Base poll interval for live status (seconds). Backs off on errors.
    pub poll_secs: u32,
    /// Twitch application client id (dev.twitch.tv). The client secret is stored in the
    /// keychain entry `twitch-client-secret`. Without both, live status and trending are unknown
    /// and every enabled watchlist stream is treated as possibly live.
    pub twitch_client_id: String,
    /// Watchlist. Entries in `chat.twitch_channels` are also watched (legacy list).
    pub streamers: Vec<WatchStreamer>,
}

impl Default for WatchSettings {
    fn default() -> Self {
        WatchSettings {
            monitoring_enabled: true,
            auto_select: true,
            manual_override: String::new(),
            trending_enabled: false,
            trending_language: String::new(),
            trending_game_id: String::new(),
            trending_min_viewers: 500,
            trending_candidates: 20,
            min_watch_secs: 300,
            switch_cooldown_secs: 120,
            hysteresis: 1.25,
            favorites_preempt: true,
            poll_secs: 60,
            twitch_client_id: String::new(),
            streamers: vec![],
        }
    }
}

/// Normalise a Twitch login: trim, drop a leading `#`, URL prefix, lowercase.
pub fn normalize_login(s: &str) -> String {
    let s = s.trim();
    let s = s.rsplit("twitch.tv/").next().unwrap_or(s);
    s.trim_start_matches('#').trim_matches('/').to_ascii_lowercase()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ObsSettings {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    /// Password is read from the keychain entry `obs-websocket`; this flag says whether to use it.
    pub use_password: bool,
    /// Start OBS's replay buffer automatically when connected.
    pub auto_start_replay_buffer: bool,
    /// Replay buffer length configured in OBS (seconds). PulseCut warns when it is too short
    /// for pre-roll + post-roll + detection latency.
    pub replay_buffer_seconds: u32,
}

impl Default for ObsSettings {
    fn default() -> Self {
        ObsSettings {
            enabled: false,
            host: "127.0.0.1".into(),
            port: 4455,
            use_password: true,
            auto_start_replay_buffer: true,
            replay_buffer_seconds: 120,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CaptureBackend {
    /// OBS replay buffer, saved after the post-roll has elapsed (deferred save).
    #[default]
    ObsReplayBuffer,
    /// PulseCut-managed FFmpeg rolling segment buffer reading a URL/device.
    SegmentBuffer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureSettings {
    pub backend: CaptureBackend,
    /// Seconds kept before the moment.
    pub pre_roll_secs: f32,
    /// Seconds kept after the moment.
    pub post_roll_secs: f32,
    /// Extra seconds waited after post-roll before saving, to absorb encoder/mux latency.
    pub save_margin_secs: f32,
    /// Source for the segment buffer (an FFmpeg input URL, e.g. rtmp://, srt://, file, or
    /// `lavfi:testsrc` for development).
    pub segment_source: String,
    pub segment_secs: u32,
    /// Ring length in seconds for the segment buffer.
    pub segment_ring_secs: u32,
    /// Copy the source codec when possible (no re-encode while buffering).
    pub segment_copy_codec: bool,
    /// Extra streams that can be captured in Autonomous multi-stream mode
    /// (`stream_id` such as `twitch:somechannel` → FFmpeg input URL).
    pub extra_sources: Vec<StreamSource>,
    /// Max simultaneous rolling buffers for extra streams (the hottest streams win).
    pub max_parallel_buffers: usize,
    /// Longest clip PulseCut will produce (seconds).
    pub max_clip_secs: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct StreamSource {
    pub stream_id: String,
    pub input: String,
}

impl Default for CaptureSettings {
    fn default() -> Self {
        CaptureSettings {
            backend: CaptureBackend::ObsReplayBuffer,
            pre_roll_secs: 20.0,
            post_roll_secs: 10.0,
            save_margin_secs: 2.0,
            segment_source: String::new(),
            segment_secs: 2,
            segment_ring_secs: 180,
            segment_copy_codec: true,
            extra_sources: vec![],
            max_parallel_buffers: 2,
            max_clip_secs: 60.0,
        }
    }
}

impl CaptureSettings {
    pub fn pre_roll_ms(&self) -> i64 {
        (self.pre_roll_secs * 1000.0) as i64
    }
    pub fn post_roll_ms(&self) -> i64 {
        (self.post_roll_secs * 1000.0) as i64
    }
    pub fn clip_secs(&self) -> f32 {
        self.pre_roll_secs + self.post_roll_secs
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DetectionSettings {
    pub autonomous_enabled: bool,
    /// 0 (only huge moments) .. 1 (clip lots). Maps onto the score threshold.
    pub sensitivity: f32,
    pub cooldown_secs: f32,
    /// Estimated delay between the on-stream moment and the chat reaction (stream latency +
    /// human reaction time). Subtracted from chat-burst onsets.
    pub chat_reaction_delay_secs: f32,
    pub use_chat: bool,
    pub use_audio: bool,
    /// Path to a weights file produced by `pulsecut-cli eval --tune`; empty = built-in tuned weights.
    pub weights_file: String,
    pub max_clips_per_hour: u32,
}

impl Default for DetectionSettings {
    fn default() -> Self {
        DetectionSettings {
            autonomous_enabled: true,
            sensitivity: 0.5,
            cooldown_secs: 45.0,
            chat_reaction_delay_secs: 6.0,
            use_chat: true,
            use_audio: true,
            weights_file: String::new(),
            max_clips_per_hour: 20,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatSettings {
    /// Twitch channels to watch (read-only, anonymous). The first is the "home" channel used
    /// for Streamer mode; others are watched in Autonomous multi-stream mode.
    pub twitch_channels: Vec<String>,
    /// YouTube live video IDs or channel handles to watch (needs an API key in the keychain).
    pub youtube_targets: Vec<String>,
    /// Kick channel slugs (webhook mode, requires a Kick app).
    pub kick_channels: Vec<String>,
    /// Extra reaction words, merged with the built-in vocabulary.
    pub custom_reactions: Vec<String>,
    /// Auto-focus capture on the hottest channel when watching several.
    pub follow_hottest: bool,
}

impl Default for ChatSettings {
    fn default() -> Self {
        ChatSettings {
            twitch_channels: vec![],
            youtube_targets: vec![],
            kick_channels: vec![],
            custom_reactions: vec![],
            follow_hottest: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    /// Analyse the microphone (Streamer mode) — requires explicit opt-in + OS permission.
    pub mic_enabled: bool,
    /// Device name; empty = system default input.
    pub mic_device: String,
    /// Analyse the captured stream's audio track (Autonomous mode, segment buffer).
    pub stream_audio_enabled: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        AudioSettings {
            mic_enabled: false,
            mic_device: String::new(),
            stream_audio_enabled: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceSettings {
    pub enabled: bool,
    /// Phrases that trigger a clip (case-insensitive, punctuation ignored).
    pub phrases: Vec<String>,
    /// Max seconds of speech sent to whisper per check.
    pub max_chunk_secs: f32,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        VoiceSettings {
            enabled: false,
            phrases: vec!["clip it".into(), "clip that".into()],
            max_chunk_secs: 3.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeySettings {
    pub enabled: bool,
    /// e.g. "CmdOrCtrl+Shift+K" (parsed by `global-hotkey`).
    pub clip: String,
    pub mark: String,
}

impl Default for HotkeySettings {
    fn default() -> Self {
        HotkeySettings {
            enabled: true,
            clip: "CmdOrCtrl+Shift+K".into(),
            mark: "CmdOrCtrl+Shift+M".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditSettings {
    pub format: AspectFormat,
    pub long_edge: u32,
    pub layout: LayoutMode,
    pub facecam: Option<NormRect>,
    pub captions: bool,
    pub caption_style: CaptionStyle,
    pub effects_enabled: bool,
    /// 0 = never add meme effects, 1 = add whenever a recipe matches.
    pub effects_intensity: f32,
    pub sound_effects: bool,
    pub sfx_volume: f32,
    pub normalize_audio: bool,
    pub watermark_text: String,
    pub intro_text: String,
    pub outro_text: String,
    pub fps: u32,
    /// Prefer the hardware encoder (VideoToolbox / Media Foundation / NVENC) when it is
    /// verified to work at startup.
    pub hardware_encode: bool,
    /// Also export a landscape version.
    pub also_landscape: bool,
    pub font_path: String,
}

impl Default for EditSettings {
    fn default() -> Self {
        EditSettings {
            format: AspectFormat::Vertical9x16,
            long_edge: 1920,
            layout: LayoutMode::BlurFill,
            facecam: None,
            captions: true,
            caption_style: CaptionStyle::default(),
            effects_enabled: true,
            effects_intensity: 0.6,
            sound_effects: true,
            sfx_volume: 0.7,
            normalize_audio: true,
            watermark_text: String::new(),
            intro_text: String::new(),
            outro_text: String::new(),
            fps: 30,
            hardware_encode: true,
            also_landscape: false,
            font_path: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MetaSettings {
    pub creator_name: String,
    pub default_hashtags: Vec<String>,
    /// Optional OpenAI-compatible local endpoint (e.g. llama.cpp `llama-server`). Empty = templates only.
    pub local_llm_url: String,
    pub local_llm_model: String,
    pub thumbnail_text: bool,
}

impl Default for MetaSettings {
    fn default() -> Self {
        MetaSettings {
            creator_name: String::new(),
            default_hashtags: vec!["#shorts".into(), "#gaming".into()],
            local_llm_url: String::new(),
            local_llm_model: String::new(),
            thumbnail_text: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PublishSettings {
    /// Never upload without approval unless this is explicitly enabled.
    pub auto_publish: bool,
    /// Minimum confidence for auto-publish.
    pub auto_publish_min_confidence: f32,
    pub auto_publish_max_per_day: u32,
    pub youtube_enabled: bool,
    /// OAuth client id of the user's own Google Cloud "Desktop app" client.
    pub youtube_client_id: String,
    pub youtube_privacy: String,
    pub youtube_category_id: String,
    pub instagram_enabled: bool,
    pub instagram_user_id: String,
    /// Base URL where `media_host_dir` is publicly served (Instagram needs a public video URL).
    pub instagram_public_base_url: String,
    pub media_host_dir: String,
    pub tiktok_enabled: bool,
    pub tiktok_client_key: String,
    pub max_attempts: u32,
}

impl Default for PublishSettings {
    fn default() -> Self {
        PublishSettings {
            auto_publish: false,
            auto_publish_min_confidence: 0.85,
            auto_publish_max_per_day: 3,
            youtube_enabled: false,
            youtube_client_id: String::new(),
            youtube_privacy: "unlisted".into(),
            youtube_category_id: "20".into(),
            instagram_enabled: false,
            instagram_user_id: String::new(),
            instagram_public_base_url: String::new(),
            media_host_dir: String::new(),
            tiktok_enabled: false,
            tiktok_client_key: String::new(),
            max_attempts: 5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageSettings {
    /// Library root; empty = default app data dir.
    pub library_dir: String,
    pub retention_days: u32,
    /// Stop capturing when free disk space drops below this many GB.
    pub min_free_gb: f32,
    /// Delete raw source captures after a successful render.
    pub delete_raw_after_render: bool,
}

impl Default for StorageSettings {
    fn default() -> Self {
        StorageSettings {
            library_dir: String::new(),
            retention_days: 30,
            min_free_gb: 5.0,
            delete_raw_after_render: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ToolSettings {
    pub ffmpeg: String,
    pub ffprobe: String,
    pub whisper_cli: String,
    pub whisper_model: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrivacySettings {
    /// Off by default. PulseCut has no telemetry backend; this flag only gates local crash logs.
    pub local_crash_logs: bool,
    pub telemetry: bool,
}

impl Default for PrivacySettings {
    fn default() -> Self {
        PrivacySettings {
            local_crash_logs: true,
            telemetry: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ResourceSettings {
    /// Local control API port on 127.0.0.1 (0 = disabled). Used by Stream Deck / scripts.
    pub local_api_port: u16,
    /// Concurrent FFmpeg render jobs.
    pub max_render_jobs: usize,
    /// Threads per FFmpeg job (0 = FFmpeg default).
    pub ffmpeg_threads: u32,
    /// Bounded queue size for pending renders; extra events are dropped lowest-score-first.
    pub render_queue: usize,
    /// Skip transcription when available memory is below this (MB).
    pub min_free_mem_mb_for_whisper: u64,
}

impl Default for ResourceSettings {
    fn default() -> Self {
        ResourceSettings {
            local_api_port: 47_120,
            max_render_jobs: 1,
            ffmpeg_threads: 0,
            render_queue: 8,
            min_free_mem_mb_for_whisper: 700,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct StartupSettings {
    pub launch_at_login: bool,
    pub start_minimized: bool,
    pub auto_connect_obs: bool,
}

impl Settings {
    pub fn load(path: &Path) -> Result<Settings> {
        match std::fs::read_to_string(path) {
            Ok(s) => {
                let mut st: Settings =
                    toml::from_str(&s).map_err(|e| Error::Settings(format!("{}: {e}", path.display())))?;
                st.sanitize();
                Ok(st)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Atomic write (temp file + rename) so a crash never leaves a truncated config.
    pub fn save(&self, path: &Path) -> Result<()> {
        let s = toml::to_string_pretty(self).map_err(|e| Error::Settings(e.to_string()))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp: PathBuf = path.with_extension("toml.tmp");
        std::fs::write(&tmp, s)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Clamp values to sane ranges (user-edited files can contain anything).
    pub fn sanitize(&mut self) {
        let w = &mut self.watch;
        w.hysteresis = if w.hysteresis.is_finite() {
            w.hysteresis.clamp(1.0, 5.0)
        } else {
            1.25
        };
        w.poll_secs = w.poll_secs.clamp(15, 900);
        w.trending_candidates = w.trending_candidates.clamp(1, 100);
        w.min_watch_secs = w.min_watch_secs.min(7200);
        w.switch_cooldown_secs = w.switch_cooldown_secs.min(7200);
        w.manual_override = normalize_login(&w.manual_override);
        let mut seen = std::collections::HashSet::new();
        w.streamers.retain_mut(|x| {
            x.login = normalize_login(&x.login);
            x.priority = x.priority.clamp(-100, 100);
            !x.login.is_empty() && seen.insert(x.login.clone())
        });
        let c = &mut self.capture;
        c.pre_roll_secs = c.pre_roll_secs.clamp(0.0, 600.0);
        c.post_roll_secs = c.post_roll_secs.clamp(0.0, 600.0);
        c.save_margin_secs = c.save_margin_secs.clamp(0.0, 30.0);
        c.segment_secs = c.segment_secs.clamp(1, 30);
        c.segment_ring_secs = c.segment_ring_secs.clamp(30, 3600);
        c.max_parallel_buffers = c.max_parallel_buffers.clamp(0, 8);
        c.max_clip_secs = c.max_clip_secs.clamp(5.0, 180.0);
        let d = &mut self.detection;
        d.sensitivity = d.sensitivity.clamp(0.0, 1.0);
        d.cooldown_secs = d.cooldown_secs.clamp(0.0, 3600.0);
        d.chat_reaction_delay_secs = d.chat_reaction_delay_secs.clamp(0.0, 60.0);
        let e = &mut self.edit;
        e.long_edge = e.long_edge.clamp(320, 3840);
        e.fps = e.fps.clamp(15, 60);
        e.effects_intensity = e.effects_intensity.clamp(0.0, 1.0);
        e.sfx_volume = e.sfx_volume.clamp(0.0, 2.0);
        e.facecam = e.facecam.map(|r| r.clamped());
        self.resources.max_render_jobs = self.resources.max_render_jobs.clamp(1, 8);
        self.resources.render_queue = self.resources.render_queue.clamp(1, 256);
        self.publish.auto_publish_min_confidence = self.publish.auto_publish_min_confidence.clamp(0.0, 1.0);
        self.publish.max_attempts = self.publish.max_attempts.clamp(1, 20);
        for ch in &mut self.chat.twitch_channels {
            *ch = ch.trim().trim_start_matches('#').to_ascii_lowercase();
        }
        self.chat.twitch_channels.retain(|c| !c.is_empty());
    }

    /// Minimum OBS replay-buffer length for the configured clip window under deferred save.
    pub fn required_replay_buffer_secs(&self) -> f32 {
        self.capture.pre_roll_secs
            + self.capture.post_roll_secs
            + self.capture.save_margin_secs
            + if self.detection.use_chat {
                self.detection.chat_reaction_delay_secs + 4.0
            } else {
                0.0
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("settings.toml");
        let missing = Settings::load(&p).unwrap();
        assert_eq!(missing, Settings::default());
        let mut s = Settings::default();
        s.chat.twitch_channels = vec!["#SomeChannel ".into()];
        s.save(&p).unwrap();
        let back = Settings::load(&p).unwrap();
        assert_eq!(back.chat.twitch_channels, vec!["somechannel".to_string()]);
    }

    #[test]
    fn partial_file_uses_defaults() {
        let s: Settings = toml::from_str("[capture]\npre_roll_secs = 15.0\n").unwrap();
        assert_eq!(s.capture.pre_roll_secs, 15.0);
        assert_eq!(s.capture.post_roll_secs, 10.0);
        assert!(!s.publish.auto_publish);
    }

    #[test]
    fn sanitize_clamps() {
        let mut s = Settings::default();
        s.detection.sensitivity = 7.0;
        s.edit.fps = 1000;
        s.sanitize();
        assert_eq!(s.detection.sensitivity, 1.0);
        assert_eq!(s.edit.fps, 60);
    }
}
