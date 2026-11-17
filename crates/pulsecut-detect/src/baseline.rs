//! Adaptive per-channel baselines and the excitement transfer function.

use std::collections::VecDeque;

/// Exponentially-weighted baseline for a non-negative count signal, robust to bursts:
/// each update is clipped to `3·mean + 3` so a hype moment barely moves the baseline,
/// while a channel that genuinely grows (raid, prime time) is followed within a few minutes.
#[derive(Debug, Clone)]
pub struct CountBaseline {
    mean: f64,
    n: u64,
    alpha: f64,
    floor: f64,
}

impl CountBaseline {
    /// `half_life_secs`: how quickly the baseline adapts. `floor`: minimum expected rate.
    pub fn new(half_life_secs: f64, floor: f64) -> Self {
        CountBaseline {
            mean: 0.0,
            n: 0,
            alpha: 1.0 - 0.5f64.powf(1.0 / half_life_secs.max(1.0)),
            floor,
        }
    }

    pub fn update(&mut self, x: f64) {
        self.n += 1;
        if self.n <= 30 {
            // Warm-up: plain running mean so the first minute isn't dominated by priors.
            self.mean += (x - self.mean) / self.n as f64;
            return;
        }
        let capped = x.min(3.0 * self.mean + 3.0);
        self.mean += self.alpha * (capped - self.mean);
    }

    pub fn mean(&self) -> f64 {
        self.mean.max(self.floor)
    }

    pub fn warmed_up(&self) -> bool {
        self.n >= 30
    }

    /// Pre-seed (e.g. from a previous session's stored baseline).
    pub fn seed(&mut self, mean: f64, n: u64) {
        self.mean = mean;
        self.n = n;
    }
}

/// Map an observed count `x` over a window of `w` seconds, against a per-second baseline
/// `lambda`, to 0..1. Both a log-ratio and a Poisson z-score must be high: the min() keeps
/// tiny channels from firing on 1→3 messages and huge channels from firing on noise.
pub fn count_excitement(x: f64, lambda_per_sec: f64, window_secs: f64, ratio_full: f64, z_full: f64) -> f64 {
    let expected = lambda_per_sec * window_secs;
    let ratio = (x + 0.5) / (expected + 0.5);
    let ratio_term = (ratio.log2() / ratio_full.log2()).clamp(0.0, 1.0);
    let z = (x - expected) / (expected + 1.0).sqrt();
    let z_term = (z / z_full).clamp(0.0, 1.0);
    ratio_term.min(z_term)
}

/// Rolling median over the last `cap` values (small windows; sorting a copy is fine).
#[derive(Debug, Clone)]
pub struct RollingMedian {
    buf: VecDeque<f64>,
    cap: usize,
}

impl RollingMedian {
    pub fn new(cap: usize) -> Self {
        RollingMedian {
            buf: VecDeque::with_capacity(cap),
            cap: cap.max(1),
        }
    }
    pub fn push(&mut self, v: f64) {
        if self.buf.len() == self.cap {
            self.buf.pop_front();
        }
        self.buf.push_back(v);
    }
    pub fn median(&self) -> Option<f64> {
        if self.buf.is_empty() {
            return None;
        }
        let mut v: Vec<f64> = self.buf.iter().copied().collect();
        v.sort_by(|a, b| a.total_cmp(b));
        Some(v[v.len() / 2])
    }
    pub fn len(&self) -> usize {
        self.buf.len()
    }
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_resists_bursts() {
        let mut b = CountBaseline::new(300.0, 0.05);
        for _ in 0..600 {
            b.update(2.0);
        }
        for _ in 0..10 {
            b.update(60.0);
        }
        assert!(b.mean() < 2.5, "{}", b.mean());
    }

    #[test]
    fn baseline_follows_growth() {
        let mut b = CountBaseline::new(120.0, 0.05);
        for _ in 0..300 {
            b.update(1.0);
        }
        for _ in 0..900 {
            b.update(4.0);
        }
        assert!(b.mean() > 3.5, "{}", b.mean());
    }

    #[test]
    fn excitement_needs_both_terms() {
        // Quiet channel: 3 msgs in 5s vs 0.1/s baseline -> big ratio but small z.
        let quiet = count_excitement(3.0, 0.1, 5.0, 6.0, 5.0);
        // Big channel: 160 vs 30/s*5=150 -> tiny ratio.
        let big_noise = count_excitement(160.0, 30.0, 5.0, 6.0, 5.0);
        // Real burst on a big channel.
        let big_burst = count_excitement(600.0, 30.0, 5.0, 6.0, 5.0);
        assert!(quiet < 0.5, "{quiet}");
        assert!(big_noise < 0.1, "{big_noise}");
        assert!(big_burst > 0.7, "{big_burst}");
    }

    #[test]
    fn rolling_median() {
        let mut m = RollingMedian::new(3);
        for v in [1.0, 9.0, 2.0, 3.0] {
            m.push(v);
        }
        assert_eq!(m.median(), Some(3.0));
    }
}
