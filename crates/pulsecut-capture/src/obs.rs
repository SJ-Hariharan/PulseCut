//! OBS Studio controller over obs-websocket v5 (via `obws`).
//!
//! * authenticated connect, automatic reconnection with backoff, health polling
//! * live status (streaming / recording / replay buffer) published on a `watch` channel
//! * reads the replay-buffer length from the active profile and flags it if too short
//! * `save_replay` = SaveReplayBuffer + wait for `ReplayBufferSaved` (falls back to
//!   GetLastReplayBufferReplay if the event is missed)

use futures_util::StreamExt;
use obws::Client;
use obws::events::Event;
use pulsecut_core::{Error, Result, TsMs, now_ms};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, broadcast, watch};

#[derive(Debug, Clone)]
pub struct ObsConfig {
    pub host: String,
    pub port: u16,
    pub password: Option<String>,
    pub auto_start_replay_buffer: bool,
    /// Minimum replay buffer seconds needed for the configured clip window.
    pub required_buffer_secs: f32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ObsStatus {
    pub connected: bool,
    pub obs_version: Option<String>,
    pub websocket_version: Option<String>,
    pub streaming: bool,
    pub recording: bool,
    pub replay_active: bool,
    /// Replay buffer length from the OBS profile, if readable.
    pub replay_buffer_secs: Option<u32>,
    pub buffer_too_short: bool,
    pub last_error: Option<String>,
    pub reconnect_attempts: u32,
    pub last_saved: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SavedReplay {
    pub path: PathBuf,
    /// When SaveReplayBuffer was sent — the saved file ends at (approximately) this instant.
    pub requested_at: TsMs,
    pub saved_at: TsMs,
}

pub struct ObsController {
    cfg: ObsConfig,
    client: Mutex<Option<Arc<Client>>>,
    status_tx: watch::Sender<ObsStatus>,
    saved_tx: broadcast::Sender<PathBuf>,
    shutdown: watch::Sender<bool>,
}

impl ObsController {
    /// Start the connection manager in the background.
    pub fn spawn(cfg: ObsConfig) -> Arc<ObsController> {
        let (status_tx, _) = watch::channel(ObsStatus::default());
        let (saved_tx, _) = broadcast::channel(16);
        let (shutdown, _) = watch::channel(false);
        let me = Arc::new(ObsController {
            cfg,
            client: Mutex::new(None),
            status_tx,
            saved_tx,
            shutdown,
        });
        let m = Arc::clone(&me);
        tokio::spawn(async move { m.run().await });
        me
    }

    pub fn status(&self) -> watch::Receiver<ObsStatus> {
        self.status_tx.subscribe()
    }

    pub fn shutdown(&self) {
        let _ = self.shutdown.send(true);
    }

    fn set(&self, f: impl FnOnce(&mut ObsStatus)) {
        self.status_tx.send_modify(f);
    }

    async fn run(self: Arc<Self>) {
        let mut backoff = Duration::from_secs(1);
        let mut stop = self.shutdown.subscribe();
        loop {
            if *stop.borrow() {
                break;
            }
            match Client::connect(&self.cfg.host, self.cfg.port, self.cfg.password.as_deref()).await {
                Ok(client) => {
                    backoff = Duration::from_secs(1);
                    let client = Arc::new(client);
                    *self.client.lock().await = Some(Arc::clone(&client));
                    self.set(|s| {
                        s.connected = true;
                        s.last_error = None;
                    });
                    tracing::info!("connected to OBS at {}:{}", self.cfg.host, self.cfg.port);
                    if let Err(e) = self.on_connected(&client).await {
                        tracing::warn!("OBS setup: {e}");
                        self.set(|s| s.last_error = Some(e.to_string()));
                    }
                    self.session(&client, &mut stop).await;
                    *self.client.lock().await = None;
                    self.set(|s| {
                        s.connected = false;
                        s.streaming = false;
                        s.recording = false;
                        s.replay_active = false;
                    });
                    tracing::warn!("OBS connection lost");
                }
                Err(e) => {
                    let msg = match &e {
                        obws::error::Error::Connect(_) | obws::error::Error::Timeout => format!(
                            "cannot reach OBS WebSocket at {}:{} — is OBS running with Tools → WebSocket Server enabled?",
                            self.cfg.host, self.cfg.port
                        ),
                        obws::error::Error::Handshake(_) => {
                            "OBS rejected the connection (check the WebSocket password)".into()
                        }
                        other => other.to_string(),
                    };
                    self.set(|s| {
                        s.connected = false;
                        s.last_error = Some(msg);
                        s.reconnect_attempts += 1;
                    });
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(backoff) => {}
                _ = stop.changed() => {}
            }
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    }

    async fn on_connected(&self, client: &Client) -> Result<()> {
        let v = client.general().version().await.map_err(obs_err)?;
        let rb = client.replay_buffer().status().await.unwrap_or(false);
        let stream = client.streaming().status().await.map(|s| s.active).unwrap_or(false);
        let rec = client.recording().status().await.map(|s| s.active).unwrap_or(false);
        let buf = read_buffer_secs(client).await;
        let too_short = buf.is_some_and(|b| (b as f32) < self.cfg.required_buffer_secs);
        if too_short {
            tracing::warn!(
                "OBS replay buffer is {}s but PulseCut needs ≥{:.0}s for the configured clip window",
                buf.unwrap_or(0),
                self.cfg.required_buffer_secs
            );
        }
        self.set(|s| {
            s.obs_version = Some(v.obs_studio_version.to_string());
            s.websocket_version = Some(v.obs_web_socket_version.to_string());
            s.replay_active = rb;
            s.streaming = stream;
            s.recording = rec;
            s.replay_buffer_secs = buf;
            s.buffer_too_short = too_short;
        });
        if !rb && self.cfg.auto_start_replay_buffer {
            match client.replay_buffer().start().await {
                Ok(()) => self.set(|s| s.replay_active = true),
                Err(e) => {
                    let m = format!("could not start OBS replay buffer (enable it in OBS Settings → Output): {e}");
                    self.set(|s| s.last_error = Some(m.clone()));
                    return Err(Error::other(m));
                }
            }
        }
        Ok(())
    }

    /// Process events and poll health until the connection drops or shutdown.
    async fn session(&self, client: &Client, stop: &mut watch::Receiver<bool>) {
        let mut events = match client.events() {
            Ok(e) => Box::pin(e),
            Err(e) => {
                tracing::warn!("OBS events unavailable: {e}");
                return;
            }
        };
        let mut health = tokio::time::interval(Duration::from_secs(5));
        loop {
            tokio::select! {
                ev = events.next() => match ev {
                    Some(Event::ReplayBufferSaved { path }) => {
                        self.set(|s| s.last_saved = Some(path.display().to_string()));
                        let _ = self.saved_tx.send(path);
                    }
                    Some(Event::ReplayBufferStateChanged { active, .. }) => self.set(|s| s.replay_active = active),
                    Some(Event::StreamStateChanged { active, .. }) => self.set(|s| s.streaming = active),
                    Some(Event::RecordStateChanged { active, .. }) => self.set(|s| s.recording = active),
                    Some(Event::ExitStarted) => { tracing::info!("OBS is exiting"); return; }
                    Some(_) => {}
                    None => return,
                },
                _ = health.tick() => {
                    if client.general().version().await.is_err() {
                        return;
                    }
                }
                _ = stop.changed() => return,
            }
        }
    }

    async fn client(&self) -> Result<Arc<Client>> {
        self.client
            .lock()
            .await
            .clone()
            .ok_or_else(|| Error::other("OBS is not connected"))
    }

    pub async fn set_replay_buffer(&self, on: bool) -> Result<()> {
        let c = self.client().await?;
        let r = if on {
            c.replay_buffer().start().await
        } else {
            c.replay_buffer().stop().await
        };
        r.map_err(obs_err)?;
        self.set(|s| s.replay_active = on);
        Ok(())
    }

    /// Save the replay buffer now and return the saved file.
    pub async fn save_replay(&self, timeout: Duration) -> Result<SavedReplay> {
        let c = self.client().await?;
        let mut saved = self.saved_tx.subscribe();
        let requested_at = now_ms();
        c.replay_buffer().save().await.map_err(|e| {
            Error::other(format!(
                "OBS SaveReplayBuffer failed (is the replay buffer running?): {e}"
            ))
        })?;
        let path = match tokio::time::timeout(timeout, saved.recv()).await {
            Ok(Ok(p)) => p,
            _ => {
                // Event missed (e.g. events disabled): ask OBS directly.
                let p = c.replay_buffer().last_replay().await.map_err(obs_err)?;
                PathBuf::from(p)
            }
        };
        Ok(SavedReplay {
            path,
            requested_at,
            saved_at: now_ms(),
        })
    }
}

fn obs_err(e: obws::error::Error) -> Error {
    Error::other(format!("OBS: {e}"))
}

/// Replay buffer length (seconds) from the current profile: `Output/Mode` decides between
/// `SimpleOutput/RecRBTime` and `AdvOut/RecRBTime` (OBS profile ini keys).
async fn read_buffer_secs(client: &Client) -> Option<u32> {
    let mode = client.profiles().parameter("Output", "Mode").await.ok()?.value;
    let cat = if mode.as_deref() == Some("Advanced") {
        "AdvOut"
    } else {
        "SimpleOutput"
    };
    let p = client.profiles().parameter(cat, "RecRBTime").await.ok()?;
    p.value.or(p.default_value)?.parse().ok()
}
