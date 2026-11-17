//! Autonomous multi-stream mode: chat for many channels is analysed continuously (cheap), and
//! rolling video buffers (expensive) are kept only for the hottest few streams that have a
//! configured source. Focus switches with hysteresis so buffers don't thrash.

use crate::Engine;
use pulsecut_capture::coordinator::{self, Backend, CoordinatorConfig};
use pulsecut_capture::segment::{SegmentConfig, SegmentRecorder};
use pulsecut_core::now_ms;
use pulsecut_detect::Detector;
use std::sync::Arc;
use std::time::Duration;

/// Mean score over the last 60 s relative to the threshold (1.0 = sitting at threshold).
pub fn hype_index(d: &Detector) -> f32 {
    let h = d.history();
    let n = h.len().min(60);
    if n == 0 {
        return 0.0;
    }
    let thr = d.cfg.threshold().max(1e-3);
    let mean: f32 = h.iter().rev().take(n).map(|p| p.score).sum::<f32>() / n as f32;
    let peak: f32 = h.iter().rev().take(n).map(|p| p.score).fold(0.0, f32::max);
    (0.6 * mean + 0.4 * peak) / thr
}

/// Pick which streams get buffers: rank by hype, keep incumbents unless a challenger beats them
/// by 25% and the incumbent has had its buffer for ≥ 2 minutes.
pub fn choose(ranked: &[(String, f32)], current: &[(String, i64)], max: usize, now: i64) -> Vec<String> {
    let mut keep: Vec<String> = vec![];
    let score = |id: &str| ranked.iter().find(|(r, _)| r == id).map(|(_, s)| *s).unwrap_or(0.0);
    let mut cur: Vec<&(String, i64)> = current.iter().collect();
    cur.sort_by(|a, b| score(&b.0).total_cmp(&score(&a.0)));
    for (id, since) in cur {
        if keep.len() >= max {
            break;
        }
        let best_challenger = ranked
            .iter()
            .filter(|(r, _)| !current.iter().any(|(c, _)| c == r) && !keep.contains(r))
            .map(|(_, s)| *s)
            .fold(0.0f32, f32::max);
        let dwell_ok = now - since >= 120_000;
        if !(dwell_ok && best_challenger > score(id) * 1.25 + 0.05) {
            keep.push(id.clone());
        }
    }
    for (id, _) in ranked {
        if keep.len() >= max {
            break;
        }
        if !keep.contains(id) {
            keep.push(id.clone());
        }
    }
    keep
}

/// Whether the home stream (first chat channel) is captured by the main backend: OBS when it is
/// enabled, or a configured rolling-buffer source.
fn home_has_own_capture(s: &pulsecut_core::Settings) -> bool {
    match s.capture.backend {
        pulsecut_core::settings::CaptureBackend::ObsReplayBuffer => s.obs.enabled,
        pulsecut_core::settings::CaptureBackend::SegmentBuffer => !s.capture.segment_source.trim().is_empty(),
    }
}

/// Configured sources plus an automatic `twitch:<channel>` video source for every watched Twitch
/// channel that has no explicit source, so adding a channel name is all a clipper has to do.
/// The first channel is the home stream; it keeps the main backend when one is configured.
pub fn effective_sources(s: &pulsecut_core::Settings) -> Vec<pulsecut_core::settings::StreamSource> {
    let mut v = s.capture.extra_sources.clone();
    v.retain(|x| !x.stream_id.trim().is_empty() && !x.input.trim().is_empty());
    let skip_home = home_has_own_capture(s);
    for (i, ch) in s.chat.twitch_channels.iter().enumerate() {
        if i == 0 && skip_home {
            continue;
        }
        let ch = ch.trim().trim_start_matches('#').to_ascii_lowercase();
        if ch.is_empty() {
            continue;
        }
        let id = format!("twitch:{ch}");
        if !v.iter().any(|x| x.stream_id == id) {
            v.push(pulsecut_core::settings::StreamSource {
                stream_id: id.clone(),
                input: id,
            });
        }
    }
    v
}

pub fn spawn(e: Arc<Engine>) {
    tokio::spawn(async move {
        let mut iv = tokio::time::interval(Duration::from_secs(15));
        loop {
            iv.tick().await;
            let s = e.settings();
            let sources = effective_sources(&s);
            if sources.is_empty() || s.capture.max_parallel_buffers == 0 {
                continue;
            }
            let ranked: Vec<(String, f32)> = {
                let dets = e.detectors.lock().unwrap();
                let mut v: Vec<(String, f32)> = sources
                    .iter()
                    .map(|src| {
                        (
                            src.stream_id.clone(),
                            dets.get(&src.stream_id).map(hype_index).unwrap_or(0.0),
                        )
                    })
                    .collect();
                v.sort_by(|a, b| b.1.total_cmp(&a.1));
                v
            };
            let current: Vec<(String, i64)> = e
                .extra_recorders
                .lock()
                .unwrap()
                .iter()
                .map(|(k, (_, since))| (k.clone(), *since))
                .collect();
            let want = choose(&ranked, &current, s.capture.max_parallel_buffers, now_ms());
            // Stop buffers that lost focus.
            for (id, _) in &current {
                if !want.contains(id) {
                    if let Some((r, _)) = e.extra_recorders.lock().unwrap().remove(id) {
                        r.stop();
                    }
                    e.unregister_capture(id);
                    e.log(format!("multi-stream: released buffer for {id}"));
                }
            }
            // Start buffers for newly focused streams.
            for id in want {
                if current.iter().any(|(c, _)| *c == id) {
                    continue;
                }
                let Some(src) = sources.iter().find(|x| x.stream_id == id) else {
                    continue;
                };
                let (atx, mut arx) = tokio::sync::mpsc::channel(256);
                let dir = e
                    .paths
                    .segments_dir()
                    .join(pulsecut_core::paths::sanitize_file_stem(&id, 40));
                match SegmentRecorder::spawn(
                    SegmentConfig {
                        input: src.input.clone(),
                        dir,
                        segment_secs: s.capture.segment_secs,
                        ring_secs: s.capture.segment_ring_secs,
                        copy_codec: true,
                        audio_analysis: true,
                    },
                    e.tools.clone(),
                    Some(atx),
                ) {
                    Ok(rec) => {
                        let tx = e.audio_tx.clone();
                        let sid = id.clone();
                        tokio::spawn(async move {
                            while let Some(a) = arx.recv().await {
                                let _ = tx.try_send((sid.clone(), a));
                            }
                        });
                        let h = coordinator::spawn(
                            Backend::Segments(Arc::clone(&rec)),
                            e.tools.clone(),
                            CoordinatorConfig {
                                policy: e.merge_policy(),
                                save_margin_ms: (s.capture.save_margin_secs * 1000.0) as i64,
                                raw_dir: e.paths.raw_dir(),
                            },
                            e.results_tx.clone(),
                        );
                        e.register_capture(&id, h);
                        e.extra_recorders.lock().unwrap().insert(id.clone(), (rec, now_ms()));
                        e.log(format!("multi-stream: buffering {id} (hype rank)"));
                    }
                    Err(err) => e.log(format!("multi-stream: cannot buffer {id}: {err}")),
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(v: &[(&str, f32)]) -> Vec<(String, f32)> {
        v.iter().map(|(a, b)| (a.to_string(), *b)).collect()
    }

    #[test]
    fn every_twitch_channel_gets_a_video_source() {
        let mut s = pulsecut_core::Settings::default();
        s.obs.enabled = false;
        s.chat.twitch_channels = vec!["Tenz".into(), "#1win_dota".into(), " ".into()];
        s.capture.extra_sources = vec![pulsecut_core::settings::StreamSource {
            stream_id: "twitch:tenz".into(),
            input: "https://example.invalid/custom.m3u8".into(),
        }];
        let v = effective_sources(&s);
        assert_eq!(v.len(), 2);
        // An explicit source wins over the automatic one.
        assert_eq!(v[0].input, "https://example.invalid/custom.m3u8");
        assert_eq!(v[1].stream_id, "twitch:1win_dota");
        assert_eq!(v[1].input, "twitch:1win_dota");
    }

    #[test]
    fn home_channel_keeps_its_own_capture() {
        let mut s = pulsecut_core::Settings::default();
        s.obs.enabled = true;
        s.chat.twitch_channels = vec!["me".into(), "friend".into()];
        let v = effective_sources(&s);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].stream_id, "twitch:friend");
        s.obs.enabled = false;
        s.capture.backend = pulsecut_core::settings::CaptureBackend::SegmentBuffer;
        s.capture.segment_source = "file:/tmp/x.mp4".into();
        assert_eq!(effective_sources(&s).len(), 1);
        s.capture.segment_source.clear();
        assert_eq!(effective_sources(&s).len(), 2);
    }

    #[test]
    fn picks_hottest_and_respects_hysteresis() {
        let ranked = r(&[("a", 3.0), ("b", 1.0), ("c", 0.5)]);
        assert_eq!(choose(&ranked, &[], 2, 0), vec!["a", "b"]);
        // c incumbent for 30 s: kept despite a hotter challenger (dwell).
        let cur = vec![("c".to_string(), 0)];
        assert_eq!(choose(&ranked, &cur, 1, 30_000), vec!["c"]);
        // After 2 min it's replaced.
        assert_eq!(choose(&ranked, &cur, 1, 130_000), vec!["a"]);
        // A slightly hotter challenger (within 25%) doesn't cause a switch.
        let close = r(&[("a", 1.1), ("c", 1.0)]);
        assert_eq!(choose(&close, &cur, 1, 300_000), vec!["c"]);
    }
}
