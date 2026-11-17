//! Capture coordinator: one queue for every trigger (hotkey, voice, chat, audio, API).
//!
//! Requests are merged while still pending (`merge::try_extend`), saved once
//! `clip_end + margin` has passed, and returned with a coverage report. Coverage that misses
//! the moment is reported as an error result, never silently produced.

use crate::obs::ObsController;
use crate::segment::SegmentRecorder;
use crate::{CapturedFile, coverage_for};
use pulsecut_core::process::Tools;
use pulsecut_core::{CaptureRequest, Error, HighlightEvent, Result, TsMs, now_ms};
use pulsecut_detect::merge::{MergePolicy, PlannedClip, try_extend};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Clone)]
pub enum Backend {
    Obs(Arc<ObsController>),
    Segments(Arc<SegmentRecorder>),
}

impl Backend {
    pub fn name(&self) -> &'static str {
        match self {
            Backend::Obs(_) => "obs-replay-buffer",
            Backend::Segments(_) => "segment-buffer",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CaptureResult {
    pub clip_start: TsMs,
    pub clip_end: TsMs,
    pub events: Vec<HighlightEvent>,
    pub primary: HighlightEvent,
    pub file: Option<CapturedFile>,
    pub error: Option<String>,
}

pub struct CoordinatorConfig {
    pub policy: MergePolicy,
    pub save_margin_ms: i64,
    pub raw_dir: PathBuf,
}

#[derive(Clone)]
pub struct CaptureHandle {
    tx: mpsc::Sender<CaptureRequest>,
}

impl CaptureHandle {
    /// Queue a capture. Returns false if the queue is full (bounded; caller logs/drops).
    pub fn request(&self, r: CaptureRequest) -> bool {
        self.tx.try_send(r).is_ok()
    }
}

pub fn spawn(
    backend: Backend,
    tools: Tools,
    cfg: CoordinatorConfig,
    results: mpsc::Sender<CaptureResult>,
) -> CaptureHandle {
    let (tx, rx) = mpsc::channel(64);
    tokio::spawn(run(backend, tools, cfg, rx, results));
    CaptureHandle { tx }
}

async fn run(
    backend: Backend,
    tools: Tools,
    cfg: CoordinatorConfig,
    mut rx: mpsc::Receiver<CaptureRequest>,
    results: mpsc::Sender<CaptureResult>,
) {
    let mut pending: Vec<PlannedClip> = vec![];
    loop {
        let next_due = pending.iter().map(|c| c.end + cfg.save_margin_ms).min();
        let sleep = next_due
            .map(|d| Duration::from_millis((d - now_ms()).clamp(0, 60_000) as u64))
            .unwrap_or(Duration::from_secs(3600));
        tokio::select! {
            req = rx.recv() => {
                let Some(req) = req else { break };
                let ev = req.event.clone();
                let mut merged = false;
                for c in pending.iter_mut() {
                    if try_extend(c, &ev, &cfg.policy) {
                        // Respect explicit windows from manual requests.
                        c.start = c.start.min(req.clip_start);
                        c.end = c.end.max(req.clip_end).min(c.start + cfg.policy.max_len_ms);
                        merged = true;
                        break;
                    }
                }
                if !merged {
                    pending.push(PlannedClip { start: req.clip_start, end: req.clip_end, events: vec![ev] });
                }
            }
            _ = tokio::time::sleep(sleep) => {
                let now = now_ms();
                let (due, rest): (Vec<_>, Vec<_>) = pending.into_iter().partition(|c| c.end + cfg.save_margin_ms <= now);
                pending = rest;
                for clip in due {
                    let r = capture_one(&backend, &tools, &cfg, &clip).await;
                    let primary = clip.primary().clone();
                    let res = match r {
                        Ok(file) => {
                            let err = if !file.coverage.moment_covered() {
                                Some(format!("captured footage does not contain the moment (missing {} ms); \
                                    increase the replay buffer length", file.coverage.missing_ms()))
                            } else { None };
                            CaptureResult { clip_start: clip.start, clip_end: clip.end, events: clip.events.clone(), primary, file: Some(file), error: err }
                        }
                        Err(e) => CaptureResult { clip_start: clip.start, clip_end: clip.end, events: clip.events.clone(), primary, file: None, error: Some(e.to_string()) },
                    };
                    if results.send(res).await.is_err() { return; }
                }
            }
        }
    }
}

async fn capture_one(
    backend: &Backend,
    tools: &Tools,
    cfg: &CoordinatorConfig,
    clip: &PlannedClip,
) -> Result<CapturedFile> {
    std::fs::create_dir_all(&cfg.raw_dir)?;
    let primary = clip.primary();
    let stem = format!("{}_{}", chrono_stamp(primary.moment_ts), &primary.id.to_string()[..8]);
    match backend {
        Backend::Obs(obs) => {
            let saved = obs.save_replay(Duration::from_secs(20)).await?;
            let ext = saved
                .path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("mkv")
                .to_string();
            let dest = cfg.raw_dir.join(format!("{stem}.{ext}"));
            // Move into PulseCut's library (copy+delete across volumes).
            if std::fs::rename(&saved.path, &dest).is_err() {
                std::fs::copy(&saved.path, &dest)?;
                let _ = std::fs::remove_file(&saved.path);
            }
            let info = pulsecut_media::probe::probe(tools, &dest).await?;
            if info.duration_ms <= 0 {
                return Err(Error::other("OBS saved an empty replay"));
            }
            // The replay ends at (approximately) the moment the save was requested.
            let file_start = saved.requested_at - info.duration_ms;
            Ok(CapturedFile {
                path: dest,
                file_start_ts: file_start,
                duration_ms: info.duration_ms,
                coverage: coverage_for(file_start, info.duration_ms, clip.start, clip.end, primary.moment_ts),
                sync_uncertainty_ms: (saved.saved_at - saved.requested_at).clamp(50, 2000),
                backend: "obs-replay-buffer".into(),
            })
        }
        Backend::Segments(rec) => {
            // Wait (bounded) until the buffer has caught up with the window end.
            for _ in 0..40 {
                if rec.buffered_until().is_some_and(|t| t >= clip.end) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            let dest = cfg.raw_dir.join(format!("{stem}.mkv"));
            rec.extract(clip.start, clip.end, primary.moment_ts, &dest).await
        }
    }
}

fn chrono_stamp(ts: TsMs) -> String {
    pulsecut_core::time::fmt_local(ts).replace([' ', ':'], "-")
}
