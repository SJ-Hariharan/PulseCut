//! Stream-selection scheduler: the single place that decides which streams get a rolling video
//! buffer. Chat for every watched channel is analysed continuously (cheap); buffers (FFmpeg +
//! yt-dlp, expensive) are kept only for the streams chosen by [`crate::selection::decide`].
//!
//! Every 10 s, or immediately when discovery or the settings change, the scheduler builds the
//! candidate list (watchlist + live status + hype + trending fallback), asks the selection
//! engine, and applies the difference. Released streams finish their pending clips before the
//! buffer stops (see [`Engine::release_buffer`]).

use crate::selection::{self, Active, Candidate, Origin, Rules};
use crate::{ActiveBuffer, Engine};
use pulsecut_capture::coordinator::{self, Backend, CoordinatorConfig};
use pulsecut_capture::segment::{SegmentConfig, SegmentRecorder};
use pulsecut_core::now_ms;
use pulsecut_detect::Detector;
use std::collections::BTreeSet;
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

/// Whether the home stream (first chat channel) is captured by the main backend: OBS when it is
/// enabled, or a configured rolling-buffer source.
pub fn home_has_own_capture(s: &pulsecut_core::Settings) -> bool {
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

/// Build selection candidates from settings, discovery results and detector hype.
pub fn candidates(e: &Engine, s: &pulsecut_core::Settings) -> Vec<Candidate> {
    let m = e.monitor.lock().unwrap().clone();
    let dets = e.detectors.lock().unwrap();
    let hype = |id: &str| dets.get(id).map(hype_index).unwrap_or(0.0);
    let mut v: Vec<Candidate> = vec![];
    for w in crate::discovery::watchlist(s) {
        let id = format!("twitch:{}", w.login);
        let live = m.live.get(&w.login);
        v.push(Candidate {
            login: id.clone(),
            origin: Origin::Watchlist,
            enabled: w.enabled,
            favorite: w.favorite,
            priority: w.priority,
            live: if m.api_ok { Some(live.is_some()) } else { None },
            viewers: live.map(|x| x.viewers),
            hype: hype(&id),
        });
    }
    // Explicit non-Twitch sources (RTMP/SRT/HLS URLs, youtube:…) behave like watchlist entries
    // with unknown live status.
    for src in effective_sources(s) {
        if v.iter().any(|c| c.login == src.stream_id) {
            continue;
        }
        v.push(Candidate {
            login: src.stream_id.clone(),
            origin: Origin::Watchlist,
            enabled: true,
            favorite: false,
            priority: 0,
            live: None,
            viewers: None,
            hype: hype(&src.stream_id),
        });
    }
    if m.api_ok {
        for t in &m.trending {
            let id = format!("twitch:{}", t.login);
            if v.iter().any(|c| c.login == id) {
                continue;
            }
            v.push(Candidate {
                login: id.clone(),
                origin: Origin::Trending,
                enabled: true,
                favorite: false,
                priority: 0,
                live: Some(true),
                viewers: Some(t.viewers),
                hype: hype(&id),
            });
        }
    }
    v
}

/// Rules from settings; the manual override is a login, candidates use `twitch:<login>` ids.
pub fn rules(s: &pulsecut_core::Settings) -> Rules {
    let max = if s.watch.monitoring_enabled {
        s.capture.max_parallel_buffers
    } else {
        0
    };
    let mut r = Rules::from_settings(&s.watch, max);
    r.manual_override = r
        .manual_override
        .map(|o| if o.contains(':') { o } else { format!("twitch:{o}") });
    r
}

pub fn spawn(e: Arc<Engine>) {
    tokio::spawn(async move {
        let mut last_selected: Vec<String> = vec![];
        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(10)) => {}
                _ = e.monitor_wake.notified() => {}
            }
            let s = e.settings();
            let cands = candidates(&e, &s);
            let current: Vec<Active> = e
                .extra_recorders
                .lock()
                .unwrap()
                .iter()
                .map(|(k, b)| Active {
                    login: k.clone(),
                    since: b.since,
                })
                .collect();
            let last_switch = e.monitor.lock().unwrap().last_switch;
            let now = now_ms();
            let d = selection::decide(&cands, &current, last_switch, &rules(&s), now);

            // Release streams that lost their slot (pending clips finish first).
            for a in &current {
                if !d.selected.contains(&a.login) {
                    let b = e.extra_recorders.lock().unwrap().remove(&a.login);
                    if let Some(b) = b {
                        e.release_buffer(&a.login, b);
                        e.log(format!("monitor: released {}", a.login));
                    }
                }
            }
            // Start buffers for newly selected streams.
            for id in &d.selected {
                if current.iter().any(|a| &a.login == id) {
                    continue;
                }
                start_buffer(&e, &s, id);
            }
            if d.selected != last_selected {
                let why = d.reasons.join("; ");
                e.log(format!("monitor: {why}"));
                e.note("monitor", "selection", why, None);
                last_selected = d.selected.clone();
            }
            update_watch_chat(&e, &s, &d);
            let mut m = e.monitor.lock().unwrap();
            if d.switched {
                m.last_switch = Some(now);
            }
            m.decision = d;
            m.decided_at = Some(now);
        }
    });
}

fn start_buffer(e: &Arc<Engine>, s: &pulsecut_core::Settings, id: &str) {
    let input = effective_sources(s)
        .into_iter()
        .find(|x| x.stream_id == id)
        .map(|x| x.input)
        .unwrap_or_else(|| id.to_string());
    let (atx, mut arx) = tokio::sync::mpsc::channel(256);
    let dir = e
        .paths
        .segments_dir()
        .join(pulsecut_core::paths::sanitize_file_stem(id, 40));
    match SegmentRecorder::spawn(
        SegmentConfig {
            input,
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
            let sid = id.to_string();
            tokio::spawn(async move {
                while let Some(a) = arx.recv().await {
                    let _ = tx.try_send((sid.clone(), a));
                }
            });
            let (h, done) = coordinator::spawn_tracked(
                Backend::Segments(Arc::clone(&rec)),
                e.tools.clone(),
                CoordinatorConfig {
                    policy: e.merge_policy(),
                    save_margin_ms: (s.capture.save_margin_secs * 1000.0) as i64,
                    raw_dir: e.paths.raw_dir(),
                },
                e.results_tx.clone(),
            );
            e.register_capture(id, h);
            e.extra_recorders.lock().unwrap().insert(
                id.to_string(),
                ActiveBuffer {
                    rec,
                    since: now_ms(),
                    done: Some(done),
                },
            );
            e.log(format!("monitor: buffering {id}"));
        }
        Err(err) => {
            e.log(format!("monitor: cannot buffer {id}: {err}"));
            e.note(id, "failed", format!("cannot start buffer: {err}"), None);
        }
    }
}

/// Chat for watchlist channels that are not in the static chat list, plus selected trending
/// streams. The connection is only restarted when that set changes.
fn update_watch_chat(e: &Arc<Engine>, s: &pulsecut_core::Settings, d: &selection::Decision) {
    let fixed: BTreeSet<String> = s
        .chat
        .twitch_channels
        .iter()
        .map(|c| pulsecut_core::settings::normalize_login(c))
        .collect();
    let mut want: BTreeSet<String> = BTreeSet::new();
    if s.watch.monitoring_enabled {
        for w in crate::discovery::watchlist(s) {
            if w.enabled {
                want.insert(w.login);
            }
        }
        for id in &d.selected {
            if let Some(l) = id.strip_prefix("twitch:") {
                want.insert(l.to_string());
            }
        }
    }
    let want: Vec<String> = want.into_iter().filter(|l| !fixed.contains(l)).collect();
    let mut wc = e.watch_chat.lock().unwrap();
    if wc.as_ref().map(|(v, _)| v) == Some(&want) || (wc.is_none() && want.is_empty()) {
        return;
    }
    if let Some((_, c)) = wc.take() {
        c.stop();
    }
    if !want.is_empty() {
        let c = pulsecut_chat::twitch::spawn(want.clone(), e.chat_sender(), None);
        e.log(format!("monitor: chat joined for {}", want.join(", ")));
        *wc = Some((want, c));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
