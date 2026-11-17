//! Lightweight audio feature extraction (no ML): 50 ms frames → per-second loudness,
//! transient jumps, speech activity and a laughter heuristic.
//!
//! The laughter heuristic looks for the characteristic 3–7 Hz "ha-ha-ha" energy pulsing in
//! voiced audio (normalized autocorrelation of the frame-energy envelope) combined with
//! modulation depth. It is cheap enough to run continuously and is evaluated on synthetic
//! signals in the tests; it is a heuristic, not a trained classifier (see PERFORMANCE.md).

use pulsecut_core::TsMs;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AudioSecond {
    /// Epoch second.
    pub sec: i64,
    /// Energy-mean loudness in dBFS.
    pub loudness_db: f32,
    pub peak_db: f32,
    /// Largest frame-to-frame rise (dB) — sudden sound events.
    pub max_jump_db: f32,
    /// Fraction of voiced 50 ms frames.
    pub voiced: f32,
    /// 0..1 laughter likelihood.
    pub laughter: f32,
}

const FRAME_MS: usize = 50;
const FRAMES_PER_SEC: usize = 1000 / FRAME_MS;
const ENV_LEN: usize = 40; // 2 s envelope for laughter periodicity

#[derive(Debug)]
pub struct AudioAnalyzer {
    sample_rate: u32,
    frame_len: usize,
    start_ts: TsMs,
    samples_seen: u64,
    cur: Vec<f32>,
    frames: Vec<(f32, f32)>, // (dB, zcr) for the current second
    prev_db: f32,
    env: VecDeque<f32>,
    voiced_env: VecDeque<bool>,
    noise_floor: f32,
}

impl AudioAnalyzer {
    /// `start_ts`: wall-clock ms of the first sample that will be pushed.
    pub fn new(sample_rate: u32, start_ts: TsMs) -> Self {
        let frame_len = (sample_rate as usize * FRAME_MS / 1000).max(1);
        AudioAnalyzer {
            sample_rate,
            frame_len,
            start_ts,
            samples_seen: 0,
            cur: Vec::with_capacity(frame_len),
            frames: Vec::with_capacity(FRAMES_PER_SEC),
            prev_db: -90.0,
            env: VecDeque::with_capacity(ENV_LEN),
            voiced_env: VecDeque::with_capacity(ENV_LEN),
            noise_floor: -60.0,
        }
    }

    /// Re-anchor the clock (e.g. after a capture restart).
    pub fn reanchor(&mut self, start_ts: TsMs) {
        self.start_ts = start_ts;
        self.samples_seen = 0;
        self.cur.clear();
        self.frames.clear();
    }

    /// Push mono samples in [-1, 1]; returns any completed seconds.
    pub fn push(&mut self, samples: &[f32]) -> Vec<AudioSecond> {
        let mut out = Vec::new();
        for &s in samples {
            self.cur.push(s);
            self.samples_seen += 1;
            if self.cur.len() == self.frame_len {
                self.finish_frame();
                if self.frames.len() == FRAMES_PER_SEC {
                    out.push(self.finish_second());
                }
            }
        }
        out
    }

    fn finish_frame(&mut self) {
        let n = self.cur.len() as f32;
        let energy: f32 = self.cur.iter().map(|x| x * x).sum::<f32>() / n;
        let db = 10.0 * (energy + 1e-10).log10();
        let zc = self.cur.windows(2).filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0)).count() as f32 / n;
        self.cur.clear();
        // Slowly-tracking noise floor (fast down, slow up).
        if db < self.noise_floor {
            self.noise_floor = 0.7 * self.noise_floor + 0.3 * db;
        } else {
            self.noise_floor += 0.002 * (db - self.noise_floor);
        }
        let voiced = db > self.noise_floor + 9.0 && db > -50.0 && zc < 0.3;
        if self.env.len() == ENV_LEN {
            self.env.pop_front();
            self.voiced_env.pop_front();
        }
        self.env.push_back(db);
        self.voiced_env.push_back(voiced);
        self.frames.push((db, zc));
    }

    fn finish_second(&mut self) -> AudioSecond {
        let frames = std::mem::take(&mut self.frames);
        let mean_energy: f32 = frames.iter().map(|(db, _)| 10f32.powf(db / 10.0)).sum::<f32>() / frames.len() as f32;
        let loudness_db = 10.0 * (mean_energy + 1e-10).log10();
        let peak_db = frames.iter().map(|f| f.0).fold(-100.0f32, f32::max);
        let mut max_jump = 0.0f32;
        let mut prev = self.prev_db;
        for (db, _) in &frames {
            max_jump = max_jump.max(db - prev);
            prev = *db;
        }
        self.prev_db = prev;
        let voiced_n = self
            .voiced_env
            .iter()
            .rev()
            .take(FRAMES_PER_SEC)
            .filter(|v| **v)
            .count();
        let voiced = voiced_n as f32 / FRAMES_PER_SEC as f32;
        let laughter = self.laughter_score();
        // The second that just completed ends at samples_seen.
        let end_ms = self.start_ts + (self.samples_seen as i64 * 1000) / self.sample_rate as i64;
        AudioSecond {
            sec: (end_ms - 1).div_euclid(1000),
            loudness_db,
            peak_db,
            max_jump_db: max_jump,
            voiced,
            laughter,
        }
    }

    fn laughter_score(&self) -> f32 {
        if self.env.len() < ENV_LEN {
            return 0.0;
        }
        let env: Vec<f32> = self.env.iter().copied().collect();
        let mean = env.iter().sum::<f32>() / env.len() as f32;
        // High-pass the envelope (subtract a 7-frame moving average) so smooth swells and
        // decays (impacts, music fades) don't look periodic; only pulsing survives.
        let n = env.len();
        let x: Vec<f32> = (0..n)
            .map(|i| {
                let (a, b) = (i.saturating_sub(3), (i + 4).min(n));
                env[i] - env[a..b].iter().sum::<f32>() / (b - a) as f32
            })
            .collect();
        let var: f32 = x.iter().map(|v| v * v).sum::<f32>();
        if var <= 1e-6 {
            return 0.0;
        }
        let depth = (var / n as f32).sqrt();
        let r = |lag: usize| (lag..n).map(|i| x[i] * x[i - lag]).sum::<f32>() / var;
        // True pulsing: positive correlation at the period, negative at half the period.
        let mut best = 0.0f32;
        for lag in 3..=6usize {
            let osc = r(lag) - r(lag / 2 + lag % 2).max(-1.0);
            best = best.max(osc / 2.0);
        }
        let periodicity = best.clamp(0.0, 1.0);
        // Count distinct bursts: laughter has several ("ha-ha-ha"), an impact has one.
        let bursts = (1..n)
            .filter(|&i| x[i] > depth * 0.5 && x[i - 1] <= depth * 0.5)
            .count();
        let burst_term = ((bursts as f32 - 2.0) / 3.0).clamp(0.0, 1.0);
        let voiced_frac = self.voiced_env.iter().filter(|v| **v).count() as f32 / ENV_LEN as f32;
        let depth_term = ((depth - 2.0) / 5.0).clamp(0.0, 1.0);
        let mean_term = ((mean + 45.0) / 15.0).clamp(0.0, 1.0);
        (periodicity * 1.5).min(1.0) * burst_term * depth_term * voiced_frac.min(1.0).sqrt() * mean_term
    }
}

/// Decode raw little-endian s16 mono PCM into f32 samples.
pub fn s16le_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Rng;

    const SR: u32 = 16_000;

    fn noise(r: &mut Rng, n: usize, amp: f32) -> Vec<f32> {
        (0..n).map(|_| (r.f64() as f32 * 2.0 - 1.0) * amp).collect()
    }

    /// Voiced-ish pulses: a 220 Hz tone gated at `rate_hz` with 50% duty.
    fn pulsed(n: usize, rate_hz: f32, amp: f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = i as f32 / SR as f32;
                let gate = if (t * rate_hz).fract() < 0.5 { 1.0 } else { 0.05 };
                (2.0 * std::f32::consts::PI * 220.0 * t).sin() * amp * gate
            })
            .collect()
    }

    #[test]
    fn loudness_and_jump() {
        let mut r = Rng::new(3);
        let mut a = AudioAnalyzer::new(SR, 0);
        let quiet = a.push(&noise(&mut r, SR as usize * 3, 0.01));
        let loud = a.push(&noise(&mut r, SR as usize, 0.5));
        assert_eq!(quiet.len(), 3);
        assert_eq!(quiet[2].sec, 2);
        assert!(loud[0].loudness_db > quiet[2].loudness_db + 25.0);
        assert!(loud[0].max_jump_db > 20.0);
    }

    #[test]
    fn laughter_heuristic_separates_pulses_from_steady_tone() {
        let mut a = AudioAnalyzer::new(SR, 0);
        let mut r = Rng::new(9);
        a.push(&noise(&mut r, SR as usize * 2, 0.003));
        let laugh = a.push(&pulsed(SR as usize * 3, 4.8, 0.4));
        let mut b = AudioAnalyzer::new(SR, 0);
        b.push(&noise(&mut r, SR as usize * 2, 0.003));
        let steady = b.push(&pulsed(SR as usize * 3, 0.0001, 0.4));
        let l = laugh.last().unwrap().laughter;
        let s = steady.last().unwrap().laughter;
        assert!(l > 0.4, "laugh {l}");
        assert!(s < 0.1, "steady {s}");
    }

    #[test]
    fn impact_is_not_laughter() {
        let mut a = AudioAnalyzer::new(SR, 0);
        let mut r = Rng::new(4);
        a.push(&noise(&mut r, SR as usize * 2, 0.003));
        let boom: Vec<f32> = (0..SR as usize * 3)
            .map(|i| {
                let t = i as f32 / SR as f32;
                0.9 * (2.0 * std::f32::consts::PI * 70.0 * t).sin() * (-3.0 * t).exp()
            })
            .collect();
        let out = a.push(&boom);
        let max = out.iter().map(|s| s.laughter).fold(0.0f32, f32::max);
        assert!(max < 0.15, "impact scored as laughter: {max}");
    }

    #[test]
    fn pcm_decode() {
        let v = s16le_to_f32(&[0x00, 0x40, 0x00, 0xC0]);
        assert_eq!(v, vec![0.5, -0.5]);
    }
}
