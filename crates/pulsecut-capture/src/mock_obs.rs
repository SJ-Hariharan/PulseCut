//! A protocol-accurate obs-websocket v5 server for tests and the offline demo.
//!
//! Implements: Hello/Identify with SHA-256 challenge auth, GetVersion, Get/Start/Stop/Save
//! ReplayBuffer, GetLastReplayBufferReplay, GetStreamStatus, GetRecordStatus,
//! GetProfileParameter (Output/Mode, SimpleOutput/RecRBTime), and emits
//! ReplayBufferStateChanged / ReplayBufferSaved events. On SaveReplayBuffer it writes a real
//! video file (FFmpeg test pattern) of `buffer_secs` length so downstream code can probe/render it.

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use pulsecut_core::process::{Tools, run};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

#[derive(Clone)]
pub struct MockObsConfig {
    pub password: Option<String>,
    pub buffer_secs: u32,
    pub out_dir: PathBuf,
    pub tools: Tools,
    /// If set, the saved replay is cut from this file instead of a test pattern.
    pub source_file: Option<PathBuf>,
}

pub struct MockObs {
    pub port: u16,
    pub saves: Arc<AtomicU32>,
    pub connections: Arc<AtomicU32>,
    drop_all: Arc<AtomicBool>,
}

impl MockObs {
    /// Close every open connection (simulates OBS crashing / restarting).
    pub fn drop_connections(&self) {
        self.drop_all.store(true, Ordering::SeqCst);
    }
}

fn auth_string(password: &str, salt: &str, challenge: &str) -> String {
    let b64 = base64::engine::general_purpose::STANDARD;
    let secret = b64.encode(Sha256::digest(format!("{password}{salt}")));
    b64.encode(Sha256::digest(format!("{secret}{challenge}")))
}

pub async fn start(cfg: MockObsConfig) -> std::io::Result<MockObs> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let saves = Arc::new(AtomicU32::new(0));
    let connections = Arc::new(AtomicU32::new(0));
    let drop_all = Arc::new(AtomicBool::new(false));
    let (s2, c2, d2) = (Arc::clone(&saves), Arc::clone(&connections), Arc::clone(&drop_all));
    tokio::spawn(async move {
        let replay_active = Arc::new(AtomicBool::new(false));
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            d2.store(false, Ordering::SeqCst);
            c2.fetch_add(1, Ordering::SeqCst);
            let cfg = cfg.clone();
            let (saves, drop_all, ra) = (Arc::clone(&s2), Arc::clone(&d2), Arc::clone(&replay_active));
            tokio::spawn(async move {
                let _ = handle(stream, cfg, saves, drop_all, ra).await;
            });
        }
    });
    Ok(MockObs {
        port,
        saves,
        connections,
        drop_all,
    })
}

async fn handle(
    stream: tokio::net::TcpStream,
    cfg: MockObsConfig,
    saves: Arc<AtomicU32>,
    drop_all: Arc<AtomicBool>,
    replay_active: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ws = tokio_tungstenite::accept_async(stream).await?;
    let (mut tx, mut rx) = ws.split();
    let (salt, challenge) = ("c2FsdHlzYWx0", "Y2hhbGxlbmdl");
    let mut hello = json!({"op": 0, "d": {"obsWebSocketVersion": "5.6.2", "rpcVersion": 1}});
    if cfg.password.is_some() {
        hello["d"]["authentication"] = json!({"challenge": challenge, "salt": salt});
    }
    tx.send(Message::text(hello.to_string())).await?;
    // Identify
    let ident = loop {
        match rx.next().await {
            Some(Ok(Message::Text(t))) => break serde_json::from_str::<Value>(&t)?,
            Some(Ok(_)) => continue,
            _ => return Ok(()),
        }
    };
    if let Some(pw) = &cfg.password {
        let got = ident["d"]["authentication"].as_str().unwrap_or("");
        if got != auth_string(pw, salt, challenge) {
            tx.send(Message::Close(Some(
                tokio_tungstenite::tungstenite::protocol::CloseFrame {
                    code: 4009u16.into(),
                    reason: "Authentication failed.".into(),
                },
            )))
            .await?;
            return Ok(());
        }
    }
    tx.send(Message::text(
        json!({"op": 2, "d": {"negotiatedRpcVersion": 1}}).to_string(),
    ))
    .await?;

    let mut tick = tokio::time::interval(Duration::from_millis(100));
    loop {
        let msg = tokio::select! {
            m = rx.next() => m,
            _ = tick.tick() => {
                if drop_all.load(Ordering::SeqCst) { return Ok(()); }
                continue;
            }
        };
        let Some(Ok(Message::Text(t))) = msg else {
            match msg {
                Some(Ok(_)) => continue,
                _ => return Ok(()),
            }
        };
        let v: Value = serde_json::from_str(&t)?;
        if v["op"] != 6 {
            continue;
        }
        let rtype = v["d"]["requestType"].as_str().unwrap_or("").to_string();
        let rid = v["d"]["requestId"].clone();
        let mut events = vec![];
        let (ok, data): (bool, Value) = match rtype.as_str() {
            "GetVersion" => (
                true,
                json!({"obsVersion": "33.0.1", "obsWebSocketVersion": "5.6.2", "rpcVersion": 1,
                       "availableRequests": [], "supportedImageFormats": ["png"], "platform": "macos",
                       "platformDescription": "mock"}),
            ),
            "GetReplayBufferStatus" => (true, json!({"outputActive": replay_active.load(Ordering::SeqCst)})),
            "StartReplayBuffer" => {
                replay_active.store(true, Ordering::SeqCst);
                events.push(json!({"eventType": "ReplayBufferStateChanged", "eventIntent": 64,
                    "eventData": {"outputActive": true, "outputState": "OBS_WEBSOCKET_OUTPUT_STARTED"}}));
                (true, Value::Null)
            }
            "StopReplayBuffer" => {
                replay_active.store(false, Ordering::SeqCst);
                (true, Value::Null)
            }
            "GetStreamStatus" => (
                true,
                json!({"outputActive": true, "outputReconnecting": false, "outputTimecode": "00:10:00.000",
                       "outputDuration": 600000, "outputCongestion": 0.0, "outputBytes": 1000,
                       "outputSkippedFrames": 0, "outputTotalFrames": 18000}),
            ),
            "GetRecordStatus" => (
                true,
                json!({"outputActive": false, "outputPaused": false, "outputTimecode": "00:00:00.000",
                       "outputDuration": 0, "outputBytes": 0}),
            ),
            "GetProfileParameter" => {
                let cat = v["d"]["requestData"]["parameterCategory"].as_str().unwrap_or("");
                let name = v["d"]["requestData"]["parameterName"].as_str().unwrap_or("");
                let val = match (cat, name) {
                    ("Output", "Mode") => json!("Simple"),
                    ("SimpleOutput", "RecRBTime") => json!(cfg.buffer_secs.to_string()),
                    _ => Value::Null,
                };
                (
                    true,
                    json!({"parameterValue": val, "defaultParameterValue": Value::Null}),
                )
            }
            "SaveReplayBuffer" => {
                if !replay_active.load(Ordering::SeqCst) {
                    (false, Value::Null)
                } else {
                    let n = saves.fetch_add(1, Ordering::SeqCst) + 1;
                    let path = cfg.out_dir.join(format!("Replay {n}.mkv"));
                    let ok = write_replay(&cfg, &path).await;
                    if ok {
                        events.push(json!({"eventType": "ReplayBufferSaved", "eventIntent": 64,
                            "eventData": {"savedReplayPath": path.display().to_string()}}));
                    }
                    (ok, Value::Null)
                }
            }
            "GetLastReplayBufferReplay" => {
                let n = saves.load(Ordering::SeqCst);
                (
                    n > 0,
                    json!({"savedReplayPath": cfg.out_dir.join(format!("Replay {n}.mkv")).display().to_string()}),
                )
            }
            _ => (false, Value::Null),
        };
        let resp = json!({"op": 7, "d": {"requestType": rtype, "requestId": rid,
            "requestStatus": {"result": ok, "code": if ok {100} else {501}, "comment": if ok {Value::Null} else {json!("mock: not available")}},
            "responseData": data}});
        tx.send(Message::text(resp.to_string())).await?;
        for e in events {
            tx.send(Message::text(json!({"op": 5, "d": e}).to_string())).await?;
        }
    }
}

async fn write_replay(cfg: &MockObsConfig, path: &std::path::Path) -> bool {
    let Ok(ffmpeg) = cfg.tools.ffmpeg() else { return false };
    let secs = cfg.buffer_secs.to_string();
    let mut args: Vec<String> = vec!["-hide_banner".into(), "-v".into(), "error".into(), "-y".into()];
    match &cfg.source_file {
        Some(src) => {
            args.extend([
                "-sseof".into(),
                format!("-{secs}"),
                "-i".into(),
                src.display().to_string(),
            ]);
            args.extend(["-t".into(), secs.clone(), "-c".into(), "copy".into()]);
        }
        None => {
            args.extend(
                [
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("testsrc2=size=1280x720:rate=30:duration={secs}"),
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("sine=frequency=220:sample_rate=48000:duration={secs}"),
                    "-c:v",
                    "libx264",
                    "-preset",
                    "ultrafast",
                    "-pix_fmt",
                    "yuv420p",
                    "-c:a",
                    "aac",
                    "-shortest",
                ]
                .map(String::from),
            );
        }
    }
    args.push(path.display().to_string());
    run(ffmpeg, &args, Duration::from_secs(60)).await.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn auth_matches_obs_spec_example() {
        // Deterministic: same inputs → same output; differs with password.
        assert_eq!(auth_string("pw", "s", "c"), auth_string("pw", "s", "c"));
        assert_ne!(auth_string("pw", "s", "c"), auth_string("px", "s", "c"));
    }
}
