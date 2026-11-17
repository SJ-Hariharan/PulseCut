//! Source→output time mapping for timeline-altering effects (freeze, slow motion, speed ramp,
//! instant replay). Overlays, captions and sound effects are authored in *source* time and
//! mapped through this so they stay in sync with the edited video.

use pulsecut_core::{EffectCue, EffectKind};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Piece {
    /// Play source `[a, b)` at `speed` (1 = normal, 0.5 = half speed).
    Play { a: i64, b: i64, speed: f32 },
    /// Hold the frame at source time `at` for `dur` ms (audio silent).
    Freeze { at: i64, dur: i64 },
}

impl Piece {
    pub fn out_len(&self) -> i64 {
        match *self {
            Piece::Play { a, b, speed } => ((b - a) as f32 / speed).round() as i64,
            Piece::Freeze { dur, .. } => dur,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Timeline {
    pub pieces: Vec<Piece>,
    pub src_len: i64,
}

pub fn slow_speed(intensity: f32) -> f32 {
    (1.0 - 0.6 * intensity.clamp(0.0, 1.0)).clamp(0.4, 1.0)
}

impl Timeline {
    pub fn identity(src_len: i64) -> Timeline {
        Timeline {
            pieces: vec![Piece::Play {
                a: 0,
                b: src_len,
                speed: 1.0,
            }],
            src_len,
        }
    }

    pub fn build(src_len: i64, effects: &[EffectCue]) -> Timeline {
        let mut fx: Vec<&EffectCue> = effects
            .iter()
            .filter(|e| e.kind.alters_timeline())
            .filter(|e| e.start_ms >= 0 && e.start_ms < src_len)
            .collect();
        fx.sort_by_key(|e| e.start_ms);
        let mut pieces = vec![];
        let mut cur = 0i64;
        let push_play = |pieces: &mut Vec<Piece>, a: i64, b: i64, speed: f32| {
            if b - a >= 40 {
                pieces.push(Piece::Play { a, b, speed });
            }
        };
        for e in fx {
            if e.start_ms < cur {
                continue; // overlapping timeline effects: keep the first
            }
            let end = e.end_ms.min(src_len).max(e.start_ms);
            push_play(&mut pieces, cur, e.start_ms, 1.0);
            match e.kind {
                EffectKind::FreezeFrame => {
                    let dur = (e.end_ms - e.start_ms).clamp(200, 3000);
                    pieces.push(Piece::Freeze { at: e.start_ms, dur });
                    cur = e.start_ms;
                }
                EffectKind::SlowMotion => {
                    push_play(&mut pieces, e.start_ms, end, slow_speed(e.intensity));
                    cur = end;
                }
                EffectKind::SpeedRamp => {
                    let mid = e.start_ms + (end - e.start_ms) / 2;
                    push_play(&mut pieces, e.start_ms, mid, 1.6);
                    push_play(&mut pieces, mid, end, 0.5);
                    cur = end;
                }
                EffectKind::Replay => {
                    push_play(&mut pieces, e.start_ms, end, 1.0);
                    push_play(&mut pieces, e.start_ms, end, slow_speed(e.intensity.max(0.5)));
                    cur = end;
                }
                _ => {}
            }
        }
        push_play(&mut pieces, cur, src_len, 1.0);
        if pieces.is_empty() {
            return Timeline::identity(src_len);
        }
        Timeline { pieces, src_len }
    }

    pub fn is_identity(&self) -> bool {
        matches!(self.pieces.as_slice(), [Piece::Play { speed, .. }] if (*speed - 1.0).abs() < 1e-6)
    }

    pub fn out_len(&self) -> i64 {
        self.pieces.iter().map(|p| p.out_len()).sum()
    }

    /// Output time of the first frame showing source `t`, *before* a freeze at `t`.
    pub fn map_pre(&self, t: i64) -> i64 {
        self.map(t, true)
    }

    /// Output time of source `t`, *after* any freeze at `t`.
    pub fn map_post(&self, t: i64) -> i64 {
        self.map(t, false)
    }

    fn map(&self, t: i64, stop_at_freeze: bool) -> i64 {
        let mut off = 0;
        for p in &self.pieces {
            match *p {
                Piece::Freeze { at, dur } => {
                    if stop_at_freeze && (at - t).abs() <= 1 {
                        return off;
                    }
                    off += dur;
                }
                Piece::Play { a, b, speed } => {
                    if t >= a && t < b {
                        return off + ((t - a) as f32 / speed).round() as i64;
                    }
                    off += p.out_len();
                }
            }
        }
        off
    }

    /// Map a source range to the output, never returning an empty range.
    pub fn map_range(&self, a: i64, b: i64) -> (i64, i64) {
        let oa = self.map_pre(a);
        let ob = self.map_post(b.max(a)).max(oa + 40);
        (oa, ob)
    }

    /// Output ranges occupied by freeze pieces (used to silence/duck audio and hold captions).
    pub fn freeze_ranges(&self) -> Vec<(i64, i64)> {
        let mut off = 0;
        let mut v = vec![];
        for p in &self.pieces {
            if let Piece::Freeze { dur, .. } = p {
                v.push((off, off + dur));
            }
            off += p.out_len();
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cue(kind: EffectKind, s: i64, e: i64) -> EffectCue {
        EffectCue {
            kind,
            start_ms: s,
            end_ms: e,
            intensity: 0.5,
            sound_asset: None,
            text: None,
            image_asset: None,
            confidence: 1.0,
            reason: String::new(),
        }
    }

    #[test]
    fn freeze_inserts_time() {
        let t = Timeline::build(10_000, &[cue(EffectKind::FreezeFrame, 4_000, 5_200)]);
        assert_eq!(t.out_len(), 11_200);
        assert_eq!(t.map_pre(4_000), 4_000);
        assert_eq!(t.map_post(4_000), 5_200);
        assert_eq!(t.map_post(6_000), 7_200);
        assert_eq!(t.freeze_ranges(), vec![(4_000, 5_200)]);
    }

    #[test]
    fn slowmo_stretches() {
        let t = Timeline::build(10_000, &[cue(EffectKind::SlowMotion, 2_000, 4_000)]);
        // intensity 0.5 -> speed 0.7 -> 2000/0.7 = 2857
        assert_eq!(t.out_len(), 8_000 + 2_857);
        assert_eq!(t.map_pre(3_000), 2_000 + 1_429);
        assert_eq!(t.map_pre(5_000), 2_000 + 2_857 + 1_000);
    }

    #[test]
    fn replay_duplicates() {
        let t = Timeline::build(10_000, &[cue(EffectKind::Replay, 6_000, 8_000)]);
        assert_eq!(t.pieces.len(), 4);
        assert!(t.out_len() > 12_000);
        assert_eq!(t.map_pre(7_000), 7_000);
    }

    #[test]
    fn identity_and_overlaps() {
        assert!(Timeline::build(5_000, &[]).is_identity());
        let t = Timeline::build(
            10_000,
            &[
                cue(EffectKind::SlowMotion, 2_000, 6_000),
                cue(EffectKind::FreezeFrame, 3_000, 4_000),
            ],
        );
        assert!(!t.pieces.iter().any(|p| matches!(p, Piece::Freeze { .. })));
    }
}
