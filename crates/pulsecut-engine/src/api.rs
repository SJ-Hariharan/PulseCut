//! Local control API (127.0.0.1 only) for Stream Deck buttons, chatbots and scripts, plus the
//! Kick webhook receiver.
//!
//! * `POST /api/clip`     → manual clip (same path as the hotkey)
//! * `GET  /api/status`   → JSON status
//! * `POST /webhooks/kick` → verified Kick `chat.message.sent` deliveries (needs a tunnel)

use crate::Engine;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use pulsecut_core::TriggerSource;
use std::sync::{Arc, OnceLock};

struct ApiState {
    engine: Arc<Engine>,
    kick: OnceLock<Option<pulsecut_chat::kick::KickVerifier>>,
}

pub fn router(engine: Arc<Engine>) -> Router {
    let st = Arc::new(ApiState {
        engine,
        kick: OnceLock::new(),
    });
    Router::new()
        .route("/api/clip", post(clip))
        .route("/api/status", get(status))
        .route("/api/monitor", get(monitor))
        .route("/webhooks/kick", post(kick))
        .with_state(st)
}

pub fn spawn(engine: Arc<Engine>, port: u16) {
    tokio::spawn(async move {
        match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Ok(l) => {
                engine.log(format!("local API on http://127.0.0.1:{port}"));
                let _ = axum::serve(l, router(engine)).await;
            }
            Err(e) => engine.log(format!("local API not started on port {port}: {e}")),
        }
    });
}

async fn clip(State(s): State<Arc<ApiState>>) -> Json<serde_json::Value> {
    let id = s.engine.clip_now(TriggerSource::LocalApi);
    Json(serde_json::json!({"ok": true, "event_id": id}))
}

async fn status(State(s): State<Arc<ApiState>>) -> Json<serde_json::Value> {
    let e = &s.engine;
    let streams: Vec<_> = e
        .streams()
        .into_iter()
        .map(|v| serde_json::json!({"stream": v.stream_id, "msgs_per_sec": v.stats.msgs_per_sec, "score": v.stats.last_score, "hype_index": v.hype_index, "capturing": v.capturing}))
        .collect();
    Json(serde_json::json!({
        "obs": e.obs_status(),
        "home_stream": e.home_stream(),
        "streams": streams,
        "clips": e.clips().len(),
        "capture_paused": e.capture_paused(),
        "monitoring": {
            "enabled": e.settings().watch.monitoring_enabled,
            "selected": e.monitor_state().decision.selected,
            "buffers": e.monitored_streams().into_iter().map(|(id, since)| serde_json::json!({"stream": id, "since": since})).collect::<Vec<_>>(),
        },
    }))
}

/// Discovery, selection (with per-candidate scores and reasons), clip-job counts and the
/// trigger log explaining why moments were or were not clipped.
async fn monitor(State(s): State<Arc<ApiState>>) -> Json<serde_json::Value> {
    let e = &s.engine;
    let m = e.monitor_state();
    let clips = e.clips();
    let count = |st: pulsecut_core::ClipStatus| clips.iter().filter(|c| c.status == st).count();
    Json(serde_json::json!({
        "monitoring_enabled": e.settings().watch.monitoring_enabled,
        "auto_clip_enabled": e.settings().detection.autonomous_enabled,
        "discovery": {"api_ok": m.api_ok, "note": m.api_note, "last_poll": m.last_poll, "live": m.live, "trending": m.trending},
        "selection": {"decided_at": m.decided_at, "last_switch": m.last_switch, "decision": m.decision},
        "buffers": e.monitored_streams().into_iter().map(|(id, since)| serde_json::json!({"stream": id, "since": since})).collect::<Vec<_>>(),
        "clip_jobs": {
            "rendering": count(pulsecut_core::ClipStatus::Rendering),
            "failed": count(pulsecut_core::ClipStatus::Failed),
            "review": count(pulsecut_core::ClipStatus::Review),
        },
        "trigger_log": e.trigger_log().into_iter().rev().take(100).collect::<Vec<_>>(),
    }))
}

async fn kick(State(s): State<Arc<ApiState>>, h: HeaderMap, body: Bytes) -> StatusCode {
    let verifier = s.kick.get_or_init(|| {
        let pem = s.engine.secrets.get("kick-public-key").ok().flatten()?;
        pulsecut_chat::kick::KickVerifier::from_pem(&pem).ok()
    });
    let Some(v) = verifier else {
        return StatusCode::SERVICE_UNAVAILABLE;
    };
    let hdr = |k: &str| h.get(k).and_then(|x| x.to_str().ok()).unwrap_or("");
    if hdr("Kick-Event-Type") != "chat.message.sent" {
        return StatusCode::NO_CONTENT;
    }
    let ok = v
        .verify(
            hdr("Kick-Event-Message-Id"),
            hdr("Kick-Event-Message-Timestamp"),
            &body,
            hdr("Kick-Event-Signature"),
            pulsecut_core::now_ms() / 1000,
        )
        .is_ok();
    if !ok {
        return StatusCode::UNAUTHORIZED;
    }
    if let Some((slug, msg)) = pulsecut_chat::kick::parse_chat(&body, pulsecut_core::now_ms()) {
        s.engine.inject_chat(&format!("kick:{slug}"), msg);
    }
    StatusCode::OK
}
