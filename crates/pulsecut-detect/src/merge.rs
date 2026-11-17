//! Turning (possibly overlapping) highlight events into capture windows.
//!
//! Rules:
//! * Events whose clip windows overlap — or that fall within the user cooldown of the previous
//!   clip — are merged into one clip that covers all their moments, as long as the merged clip
//!   stays within `max_len_ms`.
//! * If merging would exceed `max_len_ms`, the later event becomes its own clip only when it is
//!   outside the cooldown; otherwise it is dropped (and reported as such).
//! * Manual triggers always win: they are never dropped, and a merged clip keeps the manual
//!   trigger as its primary event.

use pulsecut_core::{HighlightEvent, TsMs};

#[derive(Debug, Clone)]
pub struct PlannedClip {
    pub start: TsMs,
    pub end: TsMs,
    pub events: Vec<HighlightEvent>,
}

impl PlannedClip {
    /// The event that best represents the clip (manual first, else highest score).
    pub fn primary(&self) -> &HighlightEvent {
        self.events.iter().find(|e| e.trigger.is_manual()).unwrap_or_else(|| {
            self.events
                .iter()
                .max_by(|a, b| a.score.total_cmp(&b.score))
                .expect("planned clip has events")
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MergePolicy {
    pub pre_ms: i64,
    pub post_ms: i64,
    pub cooldown_ms: i64,
    pub max_len_ms: i64,
}

#[derive(Debug, Default)]
pub struct MergeOutcome {
    pub clips: Vec<PlannedClip>,
    pub dropped: Vec<HighlightEvent>,
}

/// Clip end for one event: at least `post_ms` after the moment, and kept running while the
/// detected activity continues (long hype trains), within `max_len_ms`.
pub fn event_window(ev: &HighlightEvent, p: &MergePolicy) -> (TsMs, TsMs) {
    let start = ev.moment_ts - p.pre_ms;
    let end = (ev.moment_ts + p.post_ms)
        .max(ev.activity_end + 2_000)
        .min(start + p.max_len_ms);
    (start, end)
}

/// Incremental merge decision for live use: should `ev` extend `current` (still pending)?
pub fn try_extend(current: &mut PlannedClip, ev: &HighlightEvent, p: &MergePolicy) -> bool {
    let (s, e) = event_window(ev, p);
    let near = s <= current.end || ev.moment_ts - current.end < p.cooldown_ms.min(p.post_ms + p.pre_ms);
    if !near {
        return false;
    }
    let new_start = current.start.min(s);
    let new_end = current.end.max(e);
    if new_end - new_start > p.max_len_ms {
        return false;
    }
    current.start = new_start;
    current.end = new_end;
    current.events.push(ev.clone());
    true
}

pub fn plan_captures(mut events: Vec<HighlightEvent>, p: &MergePolicy) -> MergeOutcome {
    events.sort_by_key(|e| e.moment_ts);
    let mut out = MergeOutcome::default();
    let mut last_clip_moment: Option<TsMs> = None;
    for ev in events {
        if let Some(cur) = out.clips.last_mut()
            && try_extend(cur, &ev, p)
        {
            continue;
        }
        let within_cooldown = last_clip_moment.is_some_and(|m| ev.moment_ts - m < p.cooldown_ms);
        if within_cooldown && !ev.trigger.is_manual() {
            out.dropped.push(ev);
            continue;
        }
        last_clip_moment = Some(ev.moment_ts);
        let (start, end) = event_window(&ev, p);
        out.clips.push(PlannedClip {
            start,
            end,
            events: vec![ev],
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsecut_core::TriggerSource;

    fn ev(t: TsMs, trig: TriggerSource) -> HighlightEvent {
        let mut e = HighlightEvent::manual("s", trig, t);
        e.score = 0.5;
        e
    }

    const P: MergePolicy = MergePolicy {
        pre_ms: 20_000,
        post_ms: 10_000,
        cooldown_ms: 45_000,
        max_len_ms: 60_000,
    };

    #[test]
    fn merges_overlapping() {
        let r = plan_captures(
            vec![ev(100_000, TriggerSource::Chat), ev(114_000, TriggerSource::Chat)],
            &P,
        );
        assert_eq!(r.clips.len(), 1);
        assert_eq!(r.clips[0].start, 80_000);
        assert_eq!(r.clips[0].end, 124_000);
    }

    #[test]
    fn separate_when_far() {
        let r = plan_captures(
            vec![ev(100_000, TriggerSource::Chat), ev(300_000, TriggerSource::Chat)],
            &P,
        );
        assert_eq!(r.clips.len(), 2);
    }

    #[test]
    fn caps_length_and_drops_in_cooldown() {
        let evs = vec![
            ev(100_000, TriggerSource::Chat),
            ev(125_000, TriggerSource::Chat),
            ev(140_000, TriggerSource::Chat), // would make clip 80..150 = 70 s > 60 s
        ];
        let r = plan_captures(evs, &P);
        assert_eq!(r.clips.len(), 1);
        assert_eq!(r.dropped.len(), 1);
    }

    #[test]
    fn manual_never_dropped_and_is_primary() {
        let evs = vec![
            ev(100_000, TriggerSource::Chat),
            ev(125_000, TriggerSource::Chat),
            ev(140_000, TriggerSource::Hotkey),
        ];
        let r = plan_captures(evs, &P);
        assert_eq!(r.dropped.len(), 0);
        assert_eq!(r.clips.len(), 2);
        assert_eq!(r.clips[1].primary().trigger, TriggerSource::Hotkey);
    }
}
