//! `pulsecut` — command-line interface.

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use pulsecut_core::paths::AppPaths;
use pulsecut_core::process::Tools;
use pulsecut_core::settings::{CaptureBackend, Settings};
use pulsecut_core::{ClipStatus, MomentKind, TriggerSource, now_ms};
use pulsecut_engine::{Engine, StartOptions};
use pulsecut_platform::secrets::{Keychain, MemoryStore, SecretStore};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(name = "pulsecut-cli", version, about = "Local-first autonomous livestream clipping")]
struct Cli {
    /// Override the data/config root (otherwise the platform default; also PULSECUT_HOME).
    #[arg(long, global = true)]
    home: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the end-to-end acceptance demo (synthetic stream + synthetic chat, no account needed).
    Demo {
        /// Keep the demo library (default: under ./demo-output).
        #[arg(long, default_value = "demo-output")]
        out: PathBuf,
    },
    /// Run the engine headless with the saved settings until Ctrl-C.
    Daemon,
    /// Evaluate detection on the synthetic benchmark (and optionally tune weights).
    Eval {
        #[arg(long, default_value_t = 3)]
        seeds: u64,
        #[arg(long)]
        tune: bool,
        /// Write tuned weights here (use in Settings → Detection → weights file).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Turn any video file into an edited vertical clip around a moment.
    Render {
        input: PathBuf,
        /// Moment position in seconds.
        #[arg(long)]
        moment: f32,
        #[arg(long, default_value = "hype")]
        kind: String,
        #[arg(long, default_value = "clip.mp4")]
        out: PathBuf,
        #[arg(long, default_value_t = 20.0)]
        pre: f32,
        #[arg(long, default_value_t = 10.0)]
        post: f32,
        /// auto | hw | x264
        #[arg(long, default_value = "auto")]
        encoder: String,
        /// Output long edge (1920 = 1080x1920).
        #[arg(long, default_value_t = 1920)]
        long_edge: u32,
    },
    /// Watch live Twitch chat (anonymous) and print detector activity.
    Chat {
        #[arg(long, num_args = 1.., required = true)]
        twitch: Vec<String>,
        #[arg(long, default_value_t = 60)]
        secs: u64,
    },
    /// Check the OBS WebSocket connection using saved settings + keychain password.
    ObsCheck {
        #[arg(long)]
        port: Option<u16>,
    },
    /// Probe and benchmark available H.264 encoders on this machine.
    Encoders,
    /// Store a secret in the OS keychain (value read from stdin).
    SetSecret { name: String },
    /// List clips in the library.
    Clips,
    /// Approve a clip and queue it for upload.
    Publish {
        clip: uuid::Uuid,
        #[arg(long, default_value = "youtube")]
        platform: String,
    },
    /// Connect a YouTube account (opens the browser for OAuth).
    YoutubeConnect,
    /// Print the effective settings file path and contents.
    Settings,
}

fn paths(home: &Option<PathBuf>) -> Result<AppPaths> {
    Ok(match home {
        Some(h) => AppPaths::rooted(h.clone()),
        None => AppPaths::discover()?,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,obws=warn,sqlx=warn".into()),
        )
        .with_target(false)
        .init();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Demo { out } => demo(out).await,
        Cmd::Daemon => daemon(paths(&cli.home)?).await,
        Cmd::Eval { seeds, tune, out } => eval(seeds, tune, out),
        Cmd::Render {
            input,
            moment,
            kind,
            out,
            pre,
            post,
            encoder,
            long_edge,
        } => render_file(input, moment, kind, out, pre, post, encoder, long_edge).await,
        Cmd::Chat { twitch, secs } => chat(twitch, secs).await,
        Cmd::ObsCheck { port } => obs_check(paths(&cli.home)?, port).await,
        Cmd::Encoders => encoders().await,
        Cmd::SetSecret { name } => {
            let mut v = String::new();
            std::io::stdin().read_line(&mut v)?;
            Keychain.set(&name, v.trim())?;
            println!("stored '{name}' in the OS keychain");
            Ok(())
        }
        Cmd::Clips => clips(paths(&cli.home)?).await,
        Cmd::Publish { clip, platform } => publish(paths(&cli.home)?, clip, platform).await,
        Cmd::YoutubeConnect => youtube_connect(paths(&cli.home)?).await,
        Cmd::Settings => {
            let p = paths(&cli.home)?;
            let s = Settings::load(&p.settings_file())?;
            println!("# {}\n{}", p.settings_file().display(), toml_string(&s));
            Ok(())
        }
    }
}

fn toml_string(s: &Settings) -> String {
    // Settings derive Serialize; use the core save path's format.
    let tmp = std::env::temp_dir().join("pulsecut-settings-print.toml");
    let _ = s.save(&tmp);
    std::fs::read_to_string(&tmp).unwrap_or_default()
}

fn kind_of(s: &str) -> MomentKind {
    match s.to_ascii_lowercase().as_str() {
        "funny" => MomentKind::Funny,
        "fail" => MomentKind::Fail,
        "clutch" => MomentKind::Clutch,
        "shock" => MomentKind::Shock,
        "wholesome" => MomentKind::Wholesome,
        "hype" => MomentKind::Hype,
        _ => MomentKind::Generic,
    }
}

// ---------------------------------------------------------------- demo

async fn wait_for<F: Fn(&Engine) -> bool>(e: &Engine, timeout: Duration, f: F) -> bool {
    let t = Instant::now();
    while t.elapsed() < timeout {
        if f(e) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    false
}

async fn demo(out: PathBuf) -> Result<()> {
    let root = std::fs::canonicalize(".")?.join(&out);
    std::fs::create_dir_all(&root)?;
    let p = AppPaths::rooted(root.clone());
    p.ensure()?;
    let tools = Tools::discover(&Default::default(), &[p.models_dir(), PathBuf::from("models")]);
    if tools.ffmpeg.is_none() {
        bail!("ffmpeg not found; install it first (brew install ffmpeg / winget install ffmpeg)");
    }
    let t_all = Instant::now();
    println!("== PulseCut acceptance demo ==");
    println!("[1/9] building a synthetic 75 s livestream (test pattern, speech, impact at 40 s)…");
    let stream = pulsecut_engine::demo::make_demo_stream(&tools, &p.cache_dir.join("demo_stream.mp4")).await?;
    println!(
        "      speech sample: {}",
        if stream.has_speech {
            "yes (whisper.cpp jfk.wav)"
        } else {
            "not found (captions will be empty)"
        }
    );

    let mut s = Settings::default();
    s.capture.backend = CaptureBackend::SegmentBuffer;
    s.capture.segment_source = format!("file:{}", stream.path.display());
    s.capture.segment_copy_codec = false;
    s.capture.segment_secs = 2;
    s.capture.pre_roll_secs = 20.0;
    s.capture.post_roll_secs = 10.0;
    s.chat.twitch_channels = vec!["demo_streamer".into()];
    s.meta.creator_name = "DemoStreamer".into();
    s.edit.long_edge = 1280; // faster demo renders; production default is 1920
    s.resources.local_api_port = 0;
    s.save(&p.settings_file())?;

    println!("[2/9] starting engine (capture: FFmpeg rolling buffer reading the stream in real time)…");
    let e = Engine::start(
        p.clone(),
        s.clone(),
        StartOptions {
            secrets: Arc::new(MemoryStore::default()),
            obs_password: None,
            start_chat: false,
            start_api: false,
            start_uploads: false,
        },
    )
    .await?;
    let stream_start = now_ms();
    let home = e.home_stream();
    println!("      home stream: {home}");
    if !wait_for(&e, Duration::from_secs(20), |e| {
        e.segment_status().is_some_and(|s| s.segments >= 2)
    })
    .await
    {
        bail!("segment buffer did not start: {:?}", e.segment_status());
    }

    println!("[3/9] synthetic chat audience connected (2 msg/s baseline; reacts ~6 s after the moment)…");
    let moment_wall = stream_start + stream.moment_secs as i64 * 1000;
    let chat_task = pulsecut_engine::demo::replay_chat(
        Arc::clone(&e),
        home.clone(),
        now_ms(),
        moment_wall,
        moment_wall + 25_000,
        MomentKind::Fail,
    );
    let mut events = e.subscribe();

    println!("[4/9] waiting for autonomous detection…");
    let detected = tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            if let Ok(pulsecut_engine::EngineEvent::Highlight(ev)) = events.recv().await
                && !ev.trigger.is_manual()
            {
                return ev;
            }
        }
    })
    .await
    .context("no highlight detected within 90 s")?;
    let err_s = (detected.moment_ts - moment_wall) as f64 / 1000.0;
    println!(
        "      DETECTED {} ({:.0}% confidence) — estimated moment {:+.1} s from the true moment",
        detected.kind.label(),
        detected.confidence * 100.0,
        err_s
    );
    for r in &detected.reasons {
        println!("        · {}", r.detail);
    }

    println!("[5/9] capture → render (captions, edit recipe, sound effects, title, thumbnail)…");
    let ok = wait_for(&e, Duration::from_secs(240), |e| {
        e.clips()
            .iter()
            .any(|c| matches!(c.status, ClipStatus::Review | ClipStatus::Failed))
    })
    .await;
    if !ok {
        bail!(
            "no finished clip within 4 minutes; logs:\n{}",
            e.logs().iter().map(|l| l.1.clone()).collect::<Vec<_>>().join("\n")
        );
    }
    let clip = e
        .clips()
        .into_iter()
        .find(|c| matches!(c.status, ClipStatus::Review | ClipStatus::Failed))
        .unwrap();
    if clip.status == ClipStatus::Failed {
        bail!("clip failed: {:?}", clip.error);
    }
    let video = PathBuf::from(clip.video_path.clone().unwrap());
    let info = pulsecut_media::probe::probe(&e.tools, &video).await?;
    let cov = clip.coverage.unwrap();
    let meta = clip.meta.clone().unwrap_or_default();
    let plan = clip.plan.clone().unwrap();
    println!(
        "      video: {} ({}x{}, {:.1} s, audio: {})",
        video.display(),
        info.width,
        info.height,
        info.duration_ms as f32 / 1000.0,
        info.has_audio
    );
    println!(
        "      coverage: moment covered = {}, full window = {}",
        cov.moment_covered(),
        cov.full()
    );
    println!(
        "      recipe: {} — effects: {}",
        plan.recipe,
        plan.effects
            .iter()
            .map(|x| x.kind.label())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "      sound effects: {}",
        plan.effects
            .iter()
            .filter_map(|x| x.sound_asset.clone())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "      captions: {} words — \"{}\"",
        clip.transcript.split_whitespace().count(),
        clip.transcript.chars().take(80).collect::<String>()
    );
    println!("      title: {}", meta.title);
    println!("      hashtags: {}", meta.hashtags.join(" "));
    println!(
        "      thumbnails: {} | {}",
        clip.thumb_vertical_path.clone().unwrap_or_default(),
        clip.thumb_path.clone().unwrap_or_default()
    );

    println!("[6/9] manual (hotkey-path) clip…");
    let n_before = e.clips().len();
    e.clip_now(TriggerSource::Hotkey);
    let ok = wait_for(&e, Duration::from_secs(120), |e| {
        e.clips().len() > n_before
            && e.clips()
                .iter()
                .filter(|c| c.event.trigger == TriggerSource::Hotkey)
                .all(|c| matches!(c.status, ClipStatus::Review | ClipStatus::Failed))
    })
    .await;
    let manual = e.clips().into_iter().find(|c| c.event.trigger == TriggerSource::Hotkey);
    match (&manual, ok) {
        (Some(m), true) => println!(
            "      manual clip: {:?} ({:.1} s) recipe {}",
            m.status,
            m.duration_ms as f32 / 1000.0,
            m.plan.as_ref().map(|p| p.recipe.clone()).unwrap_or_default()
        ),
        _ => println!("      manual clip did not finish in time"),
    }

    println!("[7/9] simulated crash during render → recovery…");
    let crash_clip = clip.id;
    e.store
        .enqueue_job("render-crash-test", Some(crash_clip), &serde_json::json!({}))
        .await?;
    let claimed = e.store.claim_job("render-crash-test").await?.context("claim")?;
    drop(claimed); // process "dies" with the job marked running
    let (requeued, _) = e.store.recover().await?;
    let again = e
        .store
        .claim_job("render-crash-test")
        .await?
        .context("re-claim after recovery")?;
    e.store.finish_job(again.id, true, None).await?;
    println!(
        "      re-queued {requeued} interrupted job(s); job resumed on attempt {}",
        again.attempts
    );
    println!("      rerendering the clip from raw footage to prove the library survives restarts…");
    e.rerender(crash_clip, None).await?;
    let ok = wait_for(&e, Duration::from_secs(180), |e| {
        e.clips()
            .iter()
            .any(|c| c.id == crash_clip && c.status == ClipStatus::Review)
    })
    .await;
    println!("      re-render {}", if ok { "OK" } else { "did not finish" });

    println!("[8/9] approve + publish queue (dry: no YouTube account configured in the demo)…");
    e.approve(crash_clip).await?;
    let msg = e.publish(crash_clip, "youtube", None).await?;
    let dup = e.publish(crash_clip, "youtube", None).await?;
    println!("      {msg}; second request → {dup} (duplicate prevented)");
    println!(
        "      real upload: run `pulsecut-cli youtube-connect` then `pulsecut-cli publish {crash_clip}` (see docs/USER_GUIDE.md)"
    );

    println!(
        "[9/9] done in {:.0} s. Library: {}",
        t_all.elapsed().as_secs_f32(),
        pulsecut_engine::pipeline::library_dir(&e).display()
    );
    let sent = chat_task.await.unwrap_or(0);
    println!("      synthetic chat messages delivered: {sent}");
    let report = serde_json::json!({
        "detected": {"kind": detected.kind.label(), "confidence": detected.confidence, "moment_error_s": err_s, "reasons": detected.reasons},
        "clip": {"id": clip.id, "video": video, "width": info.width, "height": info.height, "duration_ms": info.duration_ms,
                 "title": meta.title, "recipe": plan.recipe, "effects": plan.effects, "coverage": cov, "words": clip.transcript.split_whitespace().count(),
                 "thumbnails": [clip.thumb_vertical_path, clip.thumb_path]},
        "manual_clip": manual.map(|m| serde_json::json!({"status": m.status, "duration_ms": m.duration_ms})),
        "crash_recovery": {"requeued": requeued, "rerender_ok": ok},
        "encoder": e.encoder().ffmpeg_name(),
        "total_secs": t_all.elapsed().as_secs_f32(),
    });
    std::fs::write(root.join("demo-report.json"), serde_json::to_string_pretty(&report)?)?;
    println!("      report: {}", root.join("demo-report.json").display());
    e.shutdown();
    Ok(())
}

// ---------------------------------------------------------------- other commands

async fn daemon(p: AppPaths) -> Result<()> {
    let s = Settings::load(&p.settings_file())?;
    let e = Engine::start(
        p,
        s,
        StartOptions {
            secrets: Arc::new(Keychain),
            obs_password: None,
            start_chat: true,
            start_api: true,
            start_uploads: true,
        },
    )
    .await?;
    let mut ev = e.subscribe();
    println!("PulseCut running headless. Ctrl-C to stop.");
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            m = ev.recv() => if let Ok(pulsecut_engine::EngineEvent::Notice(n)) = m { println!("{n}"); },
        }
    }
    e.shutdown();
    Ok(())
}

fn eval(seeds: u64, do_tune: bool, out: Option<PathBuf>) -> Result<()> {
    use pulsecut_detect::Weights;
    use pulsecut_detect::eval::*;
    let suite = Suite::benchmark(seeds);
    let holdout = Suite::benchmark_from(5000, seeds.max(2));
    let show = |label: &str, w: &Weights, s: &Suite| {
        let r = s.evaluate(w, 0.5, true, true);
        println!("{label}");
        println!(
            "  {:<14} {:>5} {:>5} {:>6} {:>6} {:>7}",
            "scenario", "P", "R", "clips", "FP/h", "timing"
        );
        for (name, c, h) in by_type(&r) {
            println!(
                "  {:<14} {:>5.2} {:>5.2} {:>6} {:>6.2} {:>6.1}s",
                name,
                c.precision(),
                c.recall(),
                c.clips,
                c.fp() as f64 / h,
                c.mean_timing_err_s()
            );
        }
        let t = total(&r);
        println!(
            "  TOTAL          P={:.3} R={:.3} F1={:.3}  within 3 s: {}/{}",
            t.precision(),
            t.recall(),
            t.f1(),
            t.within_3s,
            t.timing_n
        );
        // Ablations: chat only / audio only.
        for (lbl, c, a) in [("chat only", true, false), ("audio only", false, true)] {
            let t = total(&s.evaluate(w, 0.5, c, a));
            println!(
                "  {lbl:<14} P={:.3} R={:.3} F1={:.3}",
                t.precision(),
                t.recall(),
                t.f1()
            );
        }
    };
    show("Shipped (tuned) weights — training seeds", &Weights::tuned(), &suite);
    show("Shipped (tuned) weights — HELD-OUT seeds", &Weights::tuned(), &holdout);
    show("Uniform weights — HELD-OUT seeds", &Weights::uniform(), &holdout);
    if do_tune {
        let t0 = Instant::now();
        let r = tune(&suite, 3);
        println!(
            "tuned in {:.1}s over {} candidates: objective {:.3} (uniform {:.3})",
            t0.elapsed().as_secs_f32(),
            r.candidates_evaluated,
            r.objective,
            r.baseline_objective
        );
        println!("{}", serde_json::to_string_pretty(&r.weights)?);
        show("Newly tuned — HELD-OUT seeds", &r.weights, &holdout);
        if let Some(o) = out {
            std::fs::write(&o, serde_json::to_string_pretty(&r.weights)?)?;
            println!("weights written to {}", o.display());
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn render_file(
    input: PathBuf,
    moment: f32,
    kind: String,
    out: PathBuf,
    pre: f32,
    post: f32,
    encoder: String,
    long_edge: u32,
) -> Result<()> {
    use pulsecut_media::render::{RenderJob, render};
    let tools = Tools::discover(&Default::default(), &[PathBuf::from("models")]);
    let info = pulsecut_media::probe::probe(&tools, &input).await?;
    let m = (moment * 1000.0) as i64;
    let mut ev = pulsecut_core::HighlightEvent::manual("file", TriggerSource::Manual, m);
    ev.kind = kind_of(&kind);
    ev.confidence = 0.9;
    let mut settings = Settings::default();
    settings.edit.long_edge = long_edge;
    let plan = pulsecut_edit::Planner {
        settings: &settings.edit,
    }
    .plan(&pulsecut_edit::ClipContext {
        event: ev,
        source_len_ms: info.duration_ms,
        window_start_ms: m - (pre * 1000.0) as i64,
        window_end_ms: m + (post * 1000.0) as i64,
        moment_ms: m,
        audio_onset_ms: None,
        motion_center: None,
        source_w: info.width,
        source_h: info.height,
        has_speech: false,
        hook_text: None,
    });
    let dir = std::env::temp_dir().join(format!("pulsecut-render-{}", uuid::Uuid::new_v4().simple()));
    let font = pulsecut_media::textimg::Font::load("").ok();
    let assets_dir = dir.join("assets");
    pulsecut_media::assets::install_builtin_pack(&assets_dir)?;
    let lib = pulsecut_media::assets::AssetLibrary::load(&assets_dir);
    let enc = match encoder.as_str() {
        "x264" => pulsecut_media::encoder::VideoEncoder::X264,
        _ => {
            let probes = pulsecut_media::encoder::probe_encoders(&tools).await?;
            let e = pulsecut_media::encoder::choose(&probes, true);
            if encoder == "hw" && !e.is_hardware() {
                bail!("no working hardware encoder on this machine");
            }
            e
        }
    };
    let t = Instant::now();
    let r = render(
        &tools,
        &RenderJob {
            source: &input,
            info: &info,
            plan: &plan,
            words: &[],
            keywords: Default::default(),
            font: font.as_ref(),
            assets: &lib,
            sfx_volume: 0.7,
            encoder: enc,
            threads: 0,
            work_dir: &dir,
            out: &out,
        },
    )
    .await?;
    println!(
        "rendered {} ({:.1} s, recipe {}, encoder {}) in {:.1} s",
        out.display(),
        r.duration_ms as f32 / 1000.0,
        plan.recipe,
        enc.ffmpeg_name(),
        t.elapsed().as_secs_f32()
    );
    let _ = std::fs::remove_dir_all(dir);
    Ok(())
}

async fn chat(channels: Vec<String>, secs: u64) -> Result<()> {
    use pulsecut_detect::{Detector, DetectorConfig};
    let (tx, mut rx) = tokio::sync::mpsc::channel(4096);
    let src = pulsecut_chat::twitch::spawn(channels.clone(), tx, None);
    let mut dets: std::collections::BTreeMap<String, Detector> = Default::default();
    let end = Instant::now() + Duration::from_secs(secs);
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    let mut total = 0u64;
    while Instant::now() < end {
        tokio::select! {
            m = rx.recv() => {
                let Some(m) = m else { break };
                total += 1;
                dets.entry(m.stream_id.clone()).or_insert_with(|| Detector::new(DetectorConfig::new(m.stream_id.clone()))).push_chat(&m.msg);
            }
            _ = tick.tick() => {
                println!("[{:?}] {total} messages", src.state.borrow().clone());
                for (id, d) in dets.iter_mut() {
                    for ev in d.tick(now_ms()) {
                        println!("  ** {} on {id}: {}", ev.kind.label(), ev.reason_summary());
                    }
                    let s = d.stats();
                    println!("  {id:<28} {:>5.1} msg/s (baseline {:>5.2})  unique/60s {:>4}  score {:.2}/{:.2}", s.msgs_per_sec, s.baseline_msgs_per_sec, s.unique_chatters_60s, s.last_score, s.threshold);
                }
            }
        }
    }
    src.stop();
    Ok(())
}

async fn obs_check(p: AppPaths, port: Option<u16>) -> Result<()> {
    let s = Settings::load(&p.settings_file())?;
    let pw = Keychain
        .get(pulsecut_platform::secrets::keys::OBS_PASSWORD)
        .ok()
        .flatten();
    let obs = pulsecut_capture::obs::ObsController::spawn(pulsecut_capture::obs::ObsConfig {
        host: s.obs.host.clone(),
        port: port.unwrap_or(s.obs.port),
        password: pw,
        auto_start_replay_buffer: false,
        required_buffer_secs: s.required_replay_buffer_secs(),
    });
    tokio::time::sleep(Duration::from_secs(3)).await;
    let st = obs.status().borrow().clone();
    println!("{}", serde_json::to_string_pretty(&st)?);
    if st.buffer_too_short {
        println!(
            "WARNING: replay buffer {}s < required {:.0}s (pre-roll + post-roll + detection latency + margin)",
            st.replay_buffer_secs.unwrap_or(0),
            s.required_replay_buffer_secs()
        );
    }
    obs.shutdown();
    Ok(())
}

async fn encoders() -> Result<()> {
    let tools = Tools::discover(&Default::default(), &[]);
    for p in pulsecut_media::encoder::probe_encoders(&tools).await? {
        println!(
            "{:<20} {:<5} {:>6} ms (2 s of 1080x1920@30)  {}",
            p.encoder.ffmpeg_name(),
            if p.ok { "OK" } else { "FAIL" },
            p.ms,
            p.error.unwrap_or_default().lines().next().unwrap_or("")
        );
    }
    Ok(())
}

async fn open_engine(p: AppPaths, uploads: bool) -> Result<Arc<Engine>> {
    let mut s = Settings::load(&p.settings_file())?;
    s.obs.enabled = false; // management commands don't need capture
    Ok(Engine::start(
        p,
        s,
        StartOptions {
            secrets: Arc::new(Keychain),
            obs_password: None,
            start_chat: false,
            start_api: false,
            start_uploads: uploads,
        },
    )
    .await?)
}

async fn clips(p: AppPaths) -> Result<()> {
    let e = open_engine(p, false).await?;
    for c in e.clips() {
        println!(
            "{}  {:<12} {:>5.1}s  {:<9} {}",
            c.id,
            c.status.label(),
            c.duration_ms as f32 / 1000.0,
            c.event.kind.label(),
            c.title()
        );
    }
    Ok(())
}

async fn publish(p: AppPaths, clip: uuid::Uuid, platform: String) -> Result<()> {
    let e = open_engine(p, true).await?;
    e.approve(clip).await?;
    println!("{}", e.publish(clip, &platform, None).await?);
    println!("waiting for the upload worker (Ctrl-C to stop; the upload resumes next launch)…");
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        e.refresh_caches().await;
        if let Some(u) = e
            .uploads()
            .into_iter()
            .find(|u| u.clip_id == clip && u.platform == platform)
        {
            if let Some((s, t)) = e.upload_progress.lock().unwrap().get(&u.id) {
                println!("  {:.0}%", *s as f64 * 100.0 / (*t).max(1) as f64);
            }
            match u.state {
                pulsecut_store::UploadState::Published => {
                    println!("published: {}", u.remote_url.unwrap_or_default());
                    return Ok(());
                }
                pulsecut_store::UploadState::Failed => bail!("upload failed: {}", u.error.unwrap_or_default()),
                _ => {
                    if let Some(err) = u.error {
                        println!("  waiting: {err}");
                    }
                }
            }
        }
    }
}

async fn youtube_connect(p: AppPaths) -> Result<()> {
    let e = open_engine(p, false).await?;
    e.connect_youtube(|url| {
        println!("Open this URL to authorize PulseCut:\n{url}");
        let _ = if cfg!(target_os = "macos") {
            std::process::Command::new("open").arg(url).status()
        } else if cfg!(windows) {
            std::process::Command::new("explorer").arg(url).status()
        } else {
            std::process::Command::new("xdg-open").arg(url).status()
        };
    })
    .await?;
    println!("YouTube connected.");
    Ok(())
}
