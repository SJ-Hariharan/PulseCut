//! Media-time ↔ wall-clock anchoring.
//!
//! We observe "media time X has been written" at wall time W. Observation always lags the true
//! instant, so `W - X` over-estimates the offset; the minimum over a sliding window of
//! observations converges to the true offset plus a small constant (mux latency), and still
//! tracks slow drift.

use pulsecut_core::TsMs;
use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct ClockAnchor {
    obs: VecDeque<i64>,
    cap: usize,
    /// Polling interval: the worst-case lag of one observation.
    pub resolution_ms: i64,
}

impl ClockAnchor {
    pub fn new(cap: usize, resolution_ms: i64) -> Self {
        ClockAnchor {
            obs: VecDeque::with_capacity(cap),
            cap: cap.max(1),
            resolution_ms,
        }
    }

    pub fn observe(&mut self, media_ms: i64, wall: TsMs) {
        if self.obs.len() == self.cap {
            self.obs.pop_front();
        }
        self.obs.push_back(wall - media_ms);
    }

    /// wall = media + offset
    pub fn offset(&self) -> Option<i64> {
        self.obs.iter().copied().min()
    }

    pub fn to_wall(&self, media_ms: i64) -> Option<TsMs> {
        self.offset().map(|o| media_ms + o)
    }

    pub fn to_media(&self, wall: TsMs) -> Option<i64> {
        self.offset().map(|o| wall - o)
    }

    pub fn samples(&self) -> usize {
        self.obs.len()
    }

    /// Spread of recent observations — a quality indicator shown in the UI.
    pub fn jitter_ms(&self) -> i64 {
        match (self.obs.iter().min(), self.obs.iter().max()) {
            (Some(a), Some(b)) => b - a,
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_offset_converges() {
        let mut a = ClockAnchor::new(20, 200);
        let true_offset = 1_700_000_000_000i64;
        // Observations lag by 10..190 ms.
        for (i, lag) in [150, 40, 190, 12, 90, 60].iter().enumerate() {
            let media = 2000 * (i as i64 + 1);
            a.observe(media, media + true_offset + lag);
        }
        assert_eq!(a.offset(), Some(true_offset + 12));
        assert_eq!(a.to_media(true_offset + 5_000 + 12), Some(5_000));
        assert_eq!(a.jitter_ms(), 178);
    }
}
