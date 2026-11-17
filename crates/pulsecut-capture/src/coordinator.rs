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
    /// Set when the request was intentionally not captured (cooldown / duplicate of a clip that
    /// was already saved). Never set for manual triggers.
    pub suppressed: Option<String>,
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
    spawn_tracked(backend, tools, cfg, results).0
}

/// Like [`spawn`], but also returns the task handle. When every [`CaptureHandle`] clone is
/// dropped the coordinator stops accepting work, **finishes every pending clip** (waiting for
/// its post-roll) and then exits, so callers can await the handle before stopping the buffer.
pub fn spawn_tracked(
    backend: Backend,
    tools: Tools,
    cfg: CoordinatorConfig,
    results: mpsc::Sender<CaptureResult>,
) -> (CaptureHandle, tokio::task::JoinHandle<()>) {
    let (tx, rx) = mpsc::channel(64);
    let j = tokio::spawn(run(backend, tools, cfg, rx, results));
    (CaptureHandle { tx }, j)
}

/// A clip that was already saved by this coordinator (for cooldown / dedup decisions).
#[derive(Debug, Clone, Copy)]
pub struct SavedClip {
    pub start: TsMs,
    pub end: TsMs,
    pub moment: TsMs,
}

/// Decide whether an autonomous request must be suppressed because a clip was already saved:
/// * its moment is within the cooldown of a saved clip's moment, or
/// * at least 60% of its window is already covered by one saved clip.
///
/// Manual requests are always admitted. Returns the reason when suppressed.
pub fn suppression_reason(req: &CaptureRequest, saved: &[SavedClip], cooldown_ms: i64) -> Option<String> {
    if req.event.trigger.is_manual() {
        return None;
    }
    let len = (req.clip_end - req.clip_start).max(1);
    for c in saved.iter().rev() {
        let gap = (req.event.moment_ts - c.moment).abs();
        if gap < cooldown_ms {
            return Some(format!(
                "cooldown: {:.0}s after a saved clip (cooldown {:.0}s)",
                gap as f64 / 1000.0,
                cooldown_ms as f64 / 1000.0
            ));
        }
        let overlap = (req.clip_end.min(c.end) - req.clip_start.max(c.start)).max(0);
        if overlap * 100 >= len * 60 {
            return Some(format!(
                "duplicate: {}% of the window is already in a saved clip",
                overlap * 100 / len
            ));
        }
    }
    None
}

const MAX_ATTEMPTS: u32 = 3;

async fn run(
    backend: Backend,
    tools: Tools,
    cfg: CoordinatorConfig,
    mut rx: mpsc::Receiver<CaptureRequest>,
    results: mpsc::Sender<CaptureResult>,
) {
    let mut pending: Vec<PlannedClip> = vec![];
    let mut saved: std::collections::VecDeque<SavedClip> = Default::default();
    let mut accepting = true;
    loop {
        if !accepting && pending.is_empty() {
            break; // every handle dropped and every pending clip finished
        }
        let next_due = pending.iter().map(|c| c.end + cfg.save_margin_ms).min();
        let sleep = next_due
            .map(|d| Duration::from_millis((d - now_ms()).clamp(0, 60_000) as u64))
            .unwrap_or(Duration::from_secs(3600));
        tokio::select! {
            req = rx.recv(), if accepting => {
                let Some(req) = req else {
                    accepting = false;
                    if !pending.is_empty() {
                        tracing::info!("capture source released; finishing {} pending clip(s)", pending.len());
                    }
                    continue;
                };
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
                if merged {
                    continue;
                }
                let history: Vec<SavedClip> = saved.iter().copied().collect();
                if let Some(why) = suppression_reason(&req, &history, cfg.policy.cooldown_ms) {
                    let r = CaptureResult {
                        clip_start: req.clip_start,
                        clip_end: req.clip_end,
                        events: vec![ev.clone()],
                        primary: ev,
                        file: None,
                        error: None,
                        suppressed: Some(why),
                    };
                    if results.send(r).await.is_err() { return; }
                    continue;
                }
                pending.push(PlannedClip { start: req.clip_start, end: req.clip_end, events: vec![ev] });
            }
            _ = tokio::time::sleep(sleep) => {
                let now = now_ms();
                let (due, rest): (Vec<_>, Vec<_>) = pending.into_iter().partition(|c| c.end + cfg.save_margin_ms <= now);
                pending = rest;
                for clip in due {
                    let mut r = capture_one(&backend, &tools, &cfg, &clip).await;
                    let mut attempt = 1;
                    while let Err(e) = &r {
                        if attempt >= MAX_ATTEMPTS {
                            break;
                        }
                        tracing::warn!("capture attempt {attempt} failed ({e}); retrying");
                        tokio::time::sleep(Duration::from_millis(1000 * attempt as u64)).await;
                        attempt += 1;
                        r = capture_one(&backend, &tools, &cfg, &clip).await;
                    }
                    let primary = clip.primary().clone();
                    let res = match r {
                        Ok(file) => {
                            let err = if !file.coverage.moment_covered() {
                                Some(format!("captured footage does not contain the moment (missing {} ms); \
                                    increase the replay buffer length", file.coverage.missing_ms()))
                            } else {
                                saved.push_back(SavedClip { start: clip.start, end: clip.end, moment: primary.moment_ts });
                                if saved.len() > 32 { saved.pop_front(); }
                                None
                            };
                            CaptureResult { clip_start: clip.start, clip_end: clip.end, events: clip.events.clone(), primary, file: Some(file), error: err, suppressed: None }
                        }
                        Err(e) => CaptureResult {
                            clip_start: clip.start, clip_end: clip.end, events: clip.events.clone(), primary, file: None,
                            error: Some(format!("{e} (after {attempt} attempt{})", if attempt == 1 { "" } else { "s" })),
                            suppressed: None,
                        },
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

#[cfg(test)]
mod tests {
    use super::*;
    use pulsecut_core::TriggerSource;

    fn req(trigger: TriggerSource, moment: TsMs) -> CaptureRequest {
        let mut ev = HighlightEvent::manual("twitch:x", TriggerSource::Manual, moment);
        ev.trigger = trigger;
        CaptureRequest::around(ev, 20_000, 10_000)
    }
    fn saved(moment: TsMs) -> SavedClip {
        SavedClip {
            start: moment - 20_000,
            end: moment + 10_000,
            moment,
        }
    }

    #[test]
    fn cooldown_suppresses_autonomous_but_not_manual() {
        let s = [saved(100_000)];
        let why = suppression_reason(&req(TriggerSource::MultiSignal, 130_000), &s, 45_000).unwrap();
        assert!(why.starts_with("cooldown"), "{why}");
        assert!(suppression_reason(&req(TriggerSource::Hotkey, 130_000), &s, 45_000).is_none());
        assert!(suppression_reason(&req(TriggerSource::Chat, 150_000), &s, 45_000).is_none());
    }

    #[test]
    fn overlap_dedup_without_cooldown() {
        // Cooldown 0: only the overlap rule applies.
        let s = [saved(100_000)];
        let dup = suppression_reason(&req(TriggerSource::Audio, 105_000), &s, 0).unwrap();
        assert!(dup.starts_with("duplicate"), "{dup}");
        // 20 s later: window 105..135 vs saved 80..110 => 5 s of 30 s overlap => admitted.
        assert!(suppression_reason(&req(TriggerSource::Audio, 125_000), &s, 0).is_none());
        assert!(suppression_reason(&req(TriggerSource::Audio, 105_000), &[], 45_000).is_none());
    }
}
