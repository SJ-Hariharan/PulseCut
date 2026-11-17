//! Detection evaluation against labelled synthetic scenarios, and weight tuning.
//!
//! Metric definitions (product-oriented — what matters is whether the *clip* contains the
//! moment):
//! * A label is **recalled** if some planned clip window contains the labelled moment with
//!   at least `margin_ms` of context on both sides.
//! * A clip is a **true positive** if it contains at least one labelled moment; otherwise it
//!   is a false positive.
//! * Timing error = |estimated moment − labelled moment| for the closest event (reported, not
//!   used for matching).

use crate::merge::{MergePolicy, plan_captures};
use crate::scorer::{Detector, DetectorConfig, N_SIGNALS, Weights};
use crate::synth::{Dataset, Scenario, generate};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Counts {
    pub labels: u32,
    pub recalled: u32,
    pub clips: u32,
    pub clip_tp: u32,
    pub timing_err_sum_ms: i64,
    pub timing_n: u32,
    pub within_3s: u32,
}

impl Counts {
    pub fn add(&mut self, o: &Counts) {
        self.labels += o.labels;
        self.recalled += o.recalled;
        self.clips += o.clips;
        self.clip_tp += o.clip_tp;
        self.timing_err_sum_ms += o.timing_err_sum_ms;
        self.timing_n += o.timing_n;
        self.within_3s += o.within_3s;
    }
    pub fn precision(&self) -> f64 {
        if self.clips == 0 {
            1.0
        } else {
            self.clip_tp as f64 / self.clips as f64
        }
    }
    pub fn recall(&self) -> f64 {
        if self.labels == 0 {
            1.0
        } else {
            self.recalled as f64 / self.labels as f64
        }
    }
    pub fn f1(&self) -> f64 {
        let (p, r) = (self.precision(), self.recall());
        if p + r == 0.0 { 0.0 } else { 2.0 * p * r / (p + r) }
    }
    pub fn mean_timing_err_s(&self) -> f64 {
        if self.timing_n == 0 {
            0.0
        } else {
            self.timing_err_sum_ms as f64 / self.timing_n as f64 / 1000.0
        }
    }
    /// Negatives-only scenarios: false clips per hour.
    pub fn fp(&self) -> u32 {
        self.clips - self.clip_tp
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioReport {
    pub name: String,
    pub seed: u64,
    pub counts: Counts,
    pub hours: f64,
}

pub const DEFAULT_POLICY: MergePolicy = MergePolicy {
    pre_ms: 20_000,
    post_ms: 10_000,
    cooldown_ms: 45_000,
    max_len_ms: 60_000,
};

/// Run the streaming detector over a dataset exactly as it would run live (1 s ticks).
pub fn run_detector(data: &Dataset, cfg: DetectorConfig) -> Vec<pulsecut_core::HighlightEvent> {
    let mut det = Detector::new(cfg);
    let mut events = vec![];
    let (mut mi, mut ai) = (0, 0);
    let mut now = data.start_ms;
    while now <= data.end_ms + 30_000 {
        while mi < data.messages.len() && data.messages[mi].ts <= now {
            det.push_chat(&data.messages[mi]);
            mi += 1;
        }
        // Audio second s is available once it has fully elapsed.
        while ai < data.audio.len() && (data.audio[ai].sec + 1) * 1000 <= now {
            det.push_audio(data.audio[ai]);
            ai += 1;
        }
        events.extend(det.tick(now));
        now += 1000;
    }
    events.extend(det.flush(now));
    events
}

pub fn score_scenario(sc: &Scenario, data: &Dataset, cfg: DetectorConfig, policy: &MergePolicy) -> ScenarioReport {
    let events = run_detector(data, cfg);
    let plan = plan_captures(events.clone(), policy);
    let margin = 2_000;
    let mut c = Counts {
        labels: data.labels.len() as u32,
        clips: plan.clips.len() as u32,
        ..Default::default()
    };
    for l in &data.labels {
        if plan
            .clips
            .iter()
            .any(|clip| clip.start + margin <= l.moment_ts && l.moment_ts <= clip.end - margin)
        {
            c.recalled += 1;
        }
        if let Some(err) = events.iter().map(|e| (e.moment_ts - l.moment_ts).abs()).min()
            && err <= 15_000
        {
            c.timing_err_sum_ms += err;
            c.timing_n += 1;
            if err <= 3_000 {
                c.within_3s += 1;
            }
        }
    }
    for clip in &plan.clips {
        if data
            .labels
            .iter()
            .any(|l| clip.start <= l.moment_ts && l.moment_ts <= clip.end)
        {
            c.clip_tp += 1;
        }
    }
    ScenarioReport {
        name: sc.name.clone(),
        seed: sc.seed,
        counts: c,
        hours: sc.duration_secs as f64 / 3600.0,
    }
}

pub struct Suite {
    pub scenarios: Vec<(Scenario, Dataset)>,
}

impl Suite {
    pub fn benchmark(seeds: u64) -> Suite {
        Suite::benchmark_from(1000, seeds)
    }

    pub fn benchmark_from(base: u64, seeds: u64) -> Suite {
        Suite {
            scenarios: Scenario::benchmark_suite_from(base, seeds)
                .into_par_iter()
                .map(|s| {
                    let d = generate(&s);
                    (s, d)
                })
                .collect(),
        }
    }

    pub fn evaluate(&self, weights: &Weights, sensitivity: f32, chat: bool, audio: bool) -> Vec<ScenarioReport> {
        self.scenarios
            .par_iter()
            .map(|(sc, data)| {
                let mut cfg = DetectorConfig::new(sc.name.clone());
                cfg.weights = weights.clone();
                cfg.sensitivity = sensitivity;
                cfg.reaction_delay_ms = sc.reaction_delay_ms;
                cfg.use_chat = chat;
                cfg.use_audio = audio;
                cfg.max_per_hour = 1000;
                score_scenario(sc, data, cfg, &DEFAULT_POLICY)
            })
            .collect()
    }
}

pub fn total(reports: &[ScenarioReport]) -> Counts {
    let mut t = Counts::default();
    for r in reports {
        t.add(&r.counts);
    }
    t
}

/// Per-scenario-type totals, sorted by name.
pub fn by_type(reports: &[ScenarioReport]) -> Vec<(String, Counts, f64)> {
    let mut m: std::collections::BTreeMap<String, (Counts, f64)> = Default::default();
    for r in reports {
        let e = m.entry(r.name.clone()).or_default();
        e.0.add(&r.counts);
        e.1 += r.hours;
    }
    m.into_iter().map(|(k, (c, h))| (k, c, h)).collect()
}

/// Objective: F1 with an extra penalty for false clips on negative-only material
/// (false positives are what make streamers turn auto-clipping off).
pub fn objective(reports: &[ScenarioReport]) -> f64 {
    let t = total(reports);
    let ramp_fp: u32 = reports
        .iter()
        .filter(|r| r.name.contains("negative"))
        .map(|r| r.counts.fp())
        .sum();
    t.f1() - 0.01 * ramp_fp as f64
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuneResult {
    pub weights: Weights,
    pub objective: f64,
    pub totals: Counts,
    pub baseline_objective: f64,
    pub baseline_totals: Counts,
    pub candidates_evaluated: usize,
}

/// Coordinate-descent grid search over signal weights, threshold and lead time.
/// Deterministic; runs scenarios in parallel with rayon.
pub fn tune(suite: &Suite, rounds: usize) -> TuneResult {
    let start = Weights::uniform();
    let base_reports = suite.evaluate(&start, 0.5, true, true);
    let baseline_objective = objective(&base_reports);
    let baseline_totals = total(&base_reports);
    let mut best = start;
    let mut best_obj = baseline_objective;
    let mut evaluated = 1;
    let weight_grid = [0.0f32, 0.5, 1.0, 1.5, 2.5];
    for _ in 0..rounds {
        let mut improved = false;
        for i in 0..N_SIGNALS {
            for &g in &weight_grid {
                if (best.w[i] - g).abs() < 1e-6 {
                    continue;
                }
                let mut cand = best.clone();
                cand.w[i] = g;
                let o = objective(&suite.evaluate(&cand, 0.5, true, true));
                evaluated += 1;
                if o > best_obj + 1e-9 {
                    best_obj = o;
                    best = cand;
                    improved = true;
                }
            }
        }
        for &t in &[0.30f32, 0.35, 0.40, 0.45, 0.50, 0.55, 0.60] {
            let mut cand = best.clone();
            cand.base_threshold = t;
            let o = objective(&suite.evaluate(&cand, 0.5, true, true));
            evaluated += 1;
            if o > best_obj + 1e-9 {
                best_obj = o;
                best = cand;
                improved = true;
            }
        }
        for &b in &[0.0f32, 0.1, 0.15, 0.25] {
            let mut cand = best.clone();
            cand.agreement_bonus = b;
            let o = objective(&suite.evaluate(&cand, 0.5, true, true));
            evaluated += 1;
            if o > best_obj + 1e-9 {
                best_obj = o;
                best = cand;
                improved = true;
            }
        }
        for &l in &[0.0f32, 1.0, 2.0, 3.0] {
            let mut cand = best.clone();
            cand.lead_secs = l;
            let o = objective(&suite.evaluate(&cand, 0.5, true, true));
            evaluated += 1;
            if o > best_obj + 1e-9 {
                best_obj = o;
                best = cand;
                improved = true;
            }
        }
        if !improved {
            break;
        }
    }
    let totals = total(&suite.evaluate(&best, 0.5, true, true));
    TuneResult {
        weights: best,
        objective: best_obj,
        totals,
        baseline_objective,
        baseline_totals,
        candidates_evaluated: evaluated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cargo test --release -p pulsecut-detect run_tuner -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn run_tuner() {
        let suite = Suite::benchmark(3);
        let r = tune(&suite, 3);
        println!("{}", serde_json::to_string_pretty(&r).unwrap());
    }

    #[test]
    fn tuned_defaults_meet_quality_bar() {
        // Held-out seeds (never used for tuning).
        let suite = Suite::benchmark_from(5000, 2);
        let reports = suite.evaluate(&Weights::tuned(), 0.5, true, true);
        let t = total(&reports);
        for (name, c, h) in by_type(&reports) {
            eprintln!(
                "{name:14} P={:.2} R={:.2} clips={} fp/h={:.2} timing={:.1}s",
                c.precision(),
                c.recall(),
                c.clips,
                c.fp() as f64 / h,
                c.mean_timing_err_s()
            );
        }
        let u = total(&suite.evaluate(&Weights::uniform(), 0.5, true, true));
        eprintln!(
            "HELDOUT tuned P={:.3} R={:.3} F1={:.3} | uniform P={:.3} R={:.3} F1={:.3} | timing {:.2}s, within3s {}/{}",
            t.precision(),
            t.recall(),
            t.f1(),
            u.precision(),
            u.recall(),
            u.f1(),
            t.mean_timing_err_s(),
            t.within_3s,
            t.timing_n
        );
        assert!(
            t.f1() >= u.f1(),
            "tuned weights must not be worse than uniform on held-out data"
        );
        assert!(t.precision() >= 0.85, "precision {}", t.precision());
        assert!(t.recall() >= 0.85, "recall {}", t.recall());
    }
}
