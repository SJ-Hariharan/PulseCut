//! Capture integration tests with real FFmpeg and a protocol-accurate mock OBS.

use pulsecut_capture::coordinator::{self, Backend, CoordinatorConfig};
use pulsecut_capture::mock_obs::{self, MockObsConfig};
use pulsecut_capture::obs::{ObsConfig, ObsController};
use pulsecut_capture::segment::{SegmentConfig, SegmentRecorder};
use pulsecut_core::process::Tools;
use pulsecut_core::settings::ToolSettings;
use pulsecut_core::{CaptureRequest, HighlightEvent, TriggerSource, now_ms};
use pulsecut_detect::merge::MergePolicy;
use std::time::Duration;
use tokio::sync::mpsc;

fn tools() -> Option<Tools> {
    let t = Tools::discover(&ToolSettings::default(), &[]);
    (t.ffmpeg.is_some() && t.ffprobe.is_some()).then_some(t)
}

const POLICY: MergePolicy = MergePolicy {
    pre_ms: 4_000,
    post_ms: 2_000,
    cooldown_ms: 10_000,
    max_len_ms: 20_000,
};

#[tokio::test]
async fn segment_buffer_captures_anchored_window() {
    let Some(t) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let spawn_wall = now_ms();
    let (atx, mut arx) = mpsc::channel(256);
    let rec = SegmentRecorder::spawn(
        SegmentConfig {
            input: "lavfi:test".into(),
            dir: dir.path().join("seg"),
            segment_secs: 1,
            ring_secs: 30,
            copy_codec: false,
            audio_analysis: true,
        },
        t.clone(),
        Some(atx),
    )
    .unwrap();
    // Let ~8 s accumulate.
    tokio::time::sleep(Duration::from_secs(8)).await;
    let st = rec.status().borrow().clone();
    eprintln!("segment status: {st:?}");
    assert!(st.running, "{:?}", st.last_error);
    assert!(st.segments >= 5);
    let until = rec.buffered_until().unwrap();
    // Anchor sanity: buffered end is close to "now" (live), and stream start ≈ process start.
    assert!(
        (now_ms() - until).abs() < 2_500,
        "buffer lags by {} ms",
        now_ms() - until
    );

    // Coordinator with a request whose window is already in the buffer.
    let (rtx, mut rrx) = mpsc::channel(4);
    let h = coordinator::spawn(
        Backend::Segments(rec.clone()),
        t.clone(),
        CoordinatorConfig {
            policy: POLICY,
            save_margin_ms: 500,
            raw_dir: dir.path().join("raw"),
        },
        rtx,
    );
    let moment = until - 3_000;
    let ev = HighlightEvent::manual("local:test", TriggerSource::Hotkey, moment);
    assert!(h.request(CaptureRequest::around(ev, 4_000, 2_000)));
    let res = tokio::time::timeout(Duration::from_secs(30), rrx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(res.error.is_none(), "{:?}", res.error);
    let f = res.file.unwrap();
    eprintln!("captured {:?}", f);
    assert!(f.coverage.full(), "coverage {:?}", f.coverage);
    assert!(f.coverage.moment_covered());
    assert!(f.sync_uncertainty_ms <= 1_500);
    assert!(
        (f.file_start_ts - spawn_wall) > -1_500,
        "file starts before the recorder existed"
    );
    let info = pulsecut_media::probe::probe(&t, &f.path).await.unwrap();
    assert!(
        (info.duration_ms - f.duration_ms).abs() < 400,
        "probe {} vs {}",
        info.duration_ms,
        f.duration_ms
    );
    assert!(info.has_audio && info.width == 1280);

    // Live audio analysis produced wall-clock-aligned seconds.
    let mut secs = vec![];
    while let Ok(a) = arx.try_recv() {
        secs.push(a);
    }
    assert!(secs.len() >= 4, "audio seconds: {}", secs.len());
    let last = secs.last().unwrap();
    assert!(
        (last.sec * 1000 - now_ms()).abs() < 4_000,
        "audio second {} vs now {}",
        last.sec * 1000,
        now_ms()
    );
    assert!(
        last.loudness_db > -40.0,
        "test tone should be audible: {}",
        last.loudness_db
    );
    rec.stop();
}

/// Releasing a stream (dropping every capture handle) while a clip is still waiting for its
/// post-roll must finish that clip, not discard it.
#[tokio::test]
async fn released_source_finishes_pending_clips() {
    let Some(t) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let rec = SegmentRecorder::spawn(
        SegmentConfig {
            input: "lavfi:test".into(),
            dir: dir.path().join("seg"),
            segment_secs: 1,
            ring_secs: 30,
            copy_codec: false,
            audio_analysis: false,
        },
        t.clone(),
        None,
    )
    .unwrap();
    tokio::time::sleep(Duration::from_secs(6)).await;
    let (rtx, mut rrx) = mpsc::channel(4);
    let (h, done) = coordinator::spawn_tracked(
        Backend::Segments(rec.clone()),
        t.clone(),
        CoordinatorConfig {
            policy: POLICY,
            save_margin_ms: 500,
            raw_dir: dir.path().join("raw"),
        },
        rtx,
    );
    // Moment "now": the post-roll (2 s) is still in the future when the handle is dropped.
    let mut ev = HighlightEvent::manual("twitch:x", TriggerSource::Manual, now_ms());
    ev.trigger = TriggerSource::MultiSignal;
    assert!(h.request(CaptureRequest::around(ev, 3_000, 2_000)));
    drop(h);
    tokio::time::timeout(Duration::from_secs(30), done)
        .await
        .unwrap()
        .unwrap();
    let res = rrx.recv().await.expect("pending clip was discarded");
    assert!(res.error.is_none() && res.suppressed.is_none(), "{:?}", res.error);
    assert!(res.file.unwrap().coverage.moment_covered());
    rec.stop();
}

#[tokio::test]
async fn obs_controller_auth_status_save_and_reconnect() {
    let Some(t) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let mock = mock_obs::start(MockObsConfig {
        password: Some("hunter2".into()),
        buffer_secs: 12,
        out_dir: dir.path().to_path_buf(),
        tools: t.clone(),
        source_file: None,
    })
    .await
    .unwrap();

    // Wrong password -> clear error, not connected.
    let bad = ObsController::spawn(ObsConfig {
        host: "127.0.0.1".into(),
        port: mock.port,
        password: Some("nope".into()),
        auto_start_replay_buffer: true,
        required_buffer_secs: 30.0,
    });
    tokio::time::sleep(Duration::from_millis(800)).await;
    let s = bad.status().borrow().clone();
    assert!(!s.connected);
    assert!(s.last_error.is_some());
    bad.shutdown();

    let obs = ObsController::spawn(ObsConfig {
        host: "127.0.0.1".into(),
        port: mock.port,
        password: Some("hunter2".into()),
        auto_start_replay_buffer: true,
        required_buffer_secs: 30.0,
    });
    let mut st = obs.status();
    if tokio::time::timeout(Duration::from_secs(5), st.wait_for(|s| s.connected && s.replay_active))
        .await
        .is_err()
    {
        panic!("not connected: {:?}", obs.status().borrow().clone());
    }
    let s = obs.status().borrow().clone();
    eprintln!("obs status {s:?}");
    assert_eq!(s.obs_version.as_deref(), Some("33.0.1"));
    assert_eq!(s.replay_buffer_secs, Some(12));
    assert!(s.buffer_too_short, "12 s buffer < 30 s required must be flagged");
    assert!(s.streaming);

    // Deferred save via the coordinator: request ends ~now, so it saves after the margin.
    let (rtx, mut rrx) = mpsc::channel(4);
    let h = coordinator::spawn(
        Backend::Obs(obs.clone()),
        t.clone(),
        CoordinatorConfig {
            policy: POLICY,
            save_margin_ms: 300,
            raw_dir: dir.path().join("raw"),
        },
        rtx,
    );
    let moment = now_ms() - 1_000;
    let req = CaptureRequest::around(
        HighlightEvent::manual("local:obs", TriggerSource::Hotkey, moment),
        4_000,
        2_000,
    );
    let t0 = now_ms();
    assert!(h.request(req));
    let res = tokio::time::timeout(Duration::from_secs(30), rrx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(now_ms() - t0 >= 1_000, "save must wait for post-roll");
    assert!(res.error.is_none(), "{:?}", res.error);
    let f = res.file.unwrap();
    assert!(
        f.path.starts_with(dir.path().join("raw")),
        "replay moved into the library"
    );
    assert!(f.coverage.full(), "{:?}", f.coverage);
    assert_eq!(mock.saves.load(std::sync::atomic::Ordering::SeqCst), 1);

    // A moment older than the buffer is reported, never silently clipped.
    let old = CaptureRequest::around(
        HighlightEvent::manual("local:obs", TriggerSource::Hotkey, now_ms() - 40_000),
        4_000,
        2_000,
    );
    assert!(h.request(old));
    let res = tokio::time::timeout(Duration::from_secs(30), rrx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        res.error
            .as_deref()
            .unwrap_or("")
            .contains("does not contain the moment"),
        "{:?}",
        res.error
    );

    // Simulated OBS crash: connections dropped; controller reconnects by itself.
    mock.drop_connections();
    tokio::time::timeout(Duration::from_secs(5), st.wait_for(|s| !s.connected))
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), st.wait_for(|s| s.connected))
        .await
        .unwrap()
        .unwrap();
    assert!(mock.connections.load(std::sync::atomic::Ordering::SeqCst) >= 3);
    obs.shutdown();
}
