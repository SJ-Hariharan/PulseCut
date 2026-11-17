//! Multi-signal fusion, hysteresis event detection, cooldowns, explanations and the Hype Map.

use crate::audio::AudioSecond;
use crate::baseline::{CountBaseline, RollingMedian, count_excitement};
use crate::chat::{ChatAggregator, ChatMessage};
use crate::text::Category;
use pulsecut_core::{HighlightEvent, MomentKind, Reason, TriggerSource, TsMs};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use uuid::Uuid;

/// Individual signals. Order is the index into `Weights::w` and `HypePoint::comps`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Signal {
    ChatVelocity = 0,
    ChatUnique = 1,
    ChatReactions = 2,
    ChatEmotes = 3,
    ChatCopypasta = 4,
    ChatAccel = 5,
    AudioLoudness = 6,
    AudioLaughter = 7,
    AudioTransient = 8,
}

pub const N_SIGNALS: usize = 9;

impl Signal {
    pub const ALL: [Signal; N_SIGNALS] = [
        Signal::ChatVelocity,
        Signal::ChatUnique,
        Signal::ChatReactions,
        Signal::ChatEmotes,
        Signal::ChatCopypasta,
        Signal::ChatAccel,
        Signal::AudioLoudness,
        Signal::AudioLaughter,
        Signal::AudioTransient,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Signal::ChatVelocity => "chat.velocity",
            Signal::ChatUnique => "chat.unique_chatters",
            Signal::ChatReactions => "chat.reactions",
            Signal::ChatEmotes => "chat.emotes",
            Signal::ChatCopypasta => "chat.copypasta",
            Signal::ChatAccel => "chat.acceleration",
            Signal::AudioLoudness => "audio.loudness",
            Signal::AudioLaughter => "audio.laughter",
            Signal::AudioTransient => "audio.transient",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Signal::ChatVelocity => "Chat velocity",
            Signal::ChatUnique => "Unique chatters",
            Signal::ChatReactions => "Reactions",
            Signal::ChatEmotes => "Emotes",
            Signal::ChatCopypasta => "Copy-paste wave",
            Signal::ChatAccel => "Chat acceleration",
            Signal::AudioLoudness => "Loudness",
            Signal::AudioLaughter => "Laughter",
            Signal::AudioTransient => "Sudden sound",
        }
    }
    pub fn is_audio(self) -> bool {
        (self as usize) >= Signal::AudioLoudness as usize
    }
}

/// Fusion parameters. Defaults are the result of `eval::tune` on the synthetic benchmark
/// (see PERFORMANCE.md for the run and its precision/recall); do not hand-edit without
/// re-running the evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Weights {
    pub w: [f32; N_SIGNALS],
    pub agreement_bonus: f32,
    /// Threshold at sensitivity 0.5.
    pub base_threshold: f32,
    /// Ratio at which a count signal saturates.
    pub ratio_full: f32,
    /// z-score at which a count signal saturates.
    pub z_full: f32,
    /// Seconds subtracted from the chat-burst onset to estimate the moment.
    pub lead_secs: f32,
}

impl Default for Weights {
    fn default() -> Self {
        Weights::tuned()
    }
}

impl Weights {
    /// Untuned, uniform starting point used as the grid-search origin.
    pub fn uniform() -> Self {
        Weights {
            w: [1.0; N_SIGNALS],
            agreement_bonus: 0.15,
            base_threshold: 0.45,
            ratio_full: 6.0,
            z_full: 5.0,
            lead_secs: 1.0,
        }
    }

    /// Tuned defaults (from `pulsecut-cli eval --tune`, recorded in PERFORMANCE.md).
    pub fn tuned() -> Self {
        Weights {
            // velocity, unique, reactions, emotes, copypasta, accel, loudness, laughter, transient
            // Tuned 2026-11-17 on seeds 1000-1002 (18 scenarios, 11.2 stream-hours):
            // P 1.00 / R 0.974 vs uniform P 0.99 / R 0.885. Raw velocity is redundant with
            // unique chatters + reactions and is fooled by organic surges, so it gets 0.
            w: [0.0, 0.5, 2.5, 2.5, 1.0, 1.0, 1.0, 1.0, 1.0],
            agreement_bonus: 0.15,
            base_threshold: 0.45,
            ratio_full: 6.0,
            z_full: 5.0,
            lead_secs: 1.0,
        }
    }

    pub fn load(path: &std::path::Path) -> std::io::Result<Self> {
        let s = std::fs::read_to_string(path)?;
        serde_json::from_str(&s).map_err(std::io::Error::other)
    }
}

#[derive(Debug, Clone)]
pub struct DetectorConfig {
    pub stream_id: String,
    pub weights: Weights,
    pub sensitivity: f32,
    /// Minimum spacing between separate detector events (duplicate suppression). Spacing
    /// between *clips* is handled by `merge::plan_captures` using the user cooldown.
    pub refractory_ms: i64,
    pub reaction_delay_ms: i64,
    pub use_chat: bool,
    pub use_audio: bool,
    pub max_per_hour: u32,
    pub extra_vocab: Vec<String>,
    /// Max event length before it is force-closed.
    pub max_event_secs: i64,
}

impl DetectorConfig {
    pub fn new(stream_id: impl Into<String>) -> Self {
        DetectorConfig {
            stream_id: stream_id.into(),
            weights: Weights::default(),
            sensitivity: 0.5,
            refractory_ms: 8_000,
            reaction_delay_ms: 6_000,
            use_chat: true,
            use_audio: true,
            max_per_hour: 30,
            extra_vocab: vec![],
            max_event_secs: 30,
        }
    }

    pub fn from_settings(stream_id: impl Into<String>, s: &pulsecut_core::Settings, weights: Weights) -> Self {
        DetectorConfig {
            stream_id: stream_id.into(),
            weights,
            sensitivity: s.detection.sensitivity,
            refractory_ms: 8_000,
            reaction_delay_ms: (s.detection.chat_reaction_delay_secs * 1000.0) as i64,
            use_chat: s.detection.use_chat,
            use_audio: s.detection.use_audio,
            max_per_hour: s.detection.max_clips_per_hour,
            extra_vocab: s.chat.custom_reactions.clone(),
            max_event_secs: 30,
        }
    }

    pub fn threshold(&self) -> f32 {
        // sensitivity 0 -> 1.5x base (strict), 1 -> 0.5x base (permissive).
        self.weights.base_threshold * (1.5 - self.sensitivity.clamp(0.0, 1.0))
    }
}

/// One point of the Hype Map (per moment-second).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HypePoint {
    pub sec: i64,
    pub score: f32,
    pub threshold: f32,
    pub comps: [f32; N_SIGNALS],
    pub msgs: u32,
    pub loudness_db: f32,
}

#[derive(Debug, Clone, Copy, Default)]
struct Raw {
    rate5: f32,
    rate_base: f32,
    uniq10: f32,
    uniq_base: f32,
    react5: f32,
    react_base: f32,
    emote5: f32,
    copy5: f32,
    loud_excess: f32,
    loud_db: f32,
    laughter: f32,
    jump: f32,
}

#[derive(Debug, Clone)]
struct Active {
    start: i64,
    last_above: i64,
    secs_above: u32,
    peak: f32,
    peak_sec: i64,
    peak_comps: [f32; N_SIGNALS],
    peak_raw: Raw,
    agree_at_peak: u32,
    best_audio: (f32, i64),
}

/// Live summary for dashboards.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct LiveStats {
    pub msgs_per_sec: f32,
    pub baseline_msgs_per_sec: f32,
    pub unique_chatters_60s: u32,
    pub last_score: f32,
    pub threshold: f32,
    pub loudness_db: f32,
    pub total_messages: u64,
    pub suppressed_messages: u64,
    pub events_emitted: u32,
}

pub struct Detector {
    pub cfg: DetectorConfig,
    chat: ChatAggregator,
    audio: BTreeMap<i64, AudioSecond>,
    rate_base: CountBaseline,
    uniq_base: CountBaseline,
    react_base: CountBaseline,
    emote_base: CountBaseline,
    copy_base: CountBaseline,
    loud_median: RollingMedian,
    loud_recent: VecDeque<f32>,
    next_eval: Option<i64>,
    active: Option<Active>,
    last_emitted_moment: Option<TsMs>,
    emitted_times: VecDeque<TsMs>,
    history: VecDeque<HypePoint>,
    last_audio_sec: Option<i64>,
    stats: LiveStats,
}

const HISTORY_SECS: usize = 4 * 3600;

impl Detector {
    pub fn new(cfg: DetectorConfig) -> Self {
        let extra = cfg.extra_vocab.clone();
        Detector {
            chat: ChatAggregator::new(180, extra),
            audio: BTreeMap::new(),
            rate_base: CountBaseline::new(300.0, 0.05),
            uniq_base: CountBaseline::new(300.0, 0.3),
            react_base: CountBaseline::new(300.0, 0.02),
            emote_base: CountBaseline::new(300.0, 0.02),
            copy_base: CountBaseline::new(300.0, 0.01),
            loud_median: RollingMedian::new(120),
            loud_recent: VecDeque::with_capacity(4),
            next_eval: None,
            active: None,
            last_emitted_moment: None,
            emitted_times: VecDeque::new(),
            history: VecDeque::new(),
            last_audio_sec: None,
            stats: LiveStats::default(),
            cfg,
        }
    }

    pub fn push_chat(&mut self, msg: &ChatMessage) {
        if !self.cfg.use_chat {
            return;
        }
        // Shift onto the moment timeline when evaluating, not here; bins stay in chat time.
        self.chat.push(msg);
    }

    pub fn push_audio(&mut self, a: AudioSecond) {
        if !self.cfg.use_audio {
            return;
        }
        self.last_audio_sec = Some(self.last_audio_sec.map_or(a.sec, |s| s.max(a.sec)));
        self.audio.insert(a.sec, a);
        while self.audio.len() > 600 {
            let first = *self.audio.keys().next().unwrap();
            self.audio.remove(&first);
        }
    }

    pub fn history(&self) -> &VecDeque<HypePoint> {
        &self.history
    }

    pub fn stats(&self) -> LiveStats {
        self.stats
    }

    pub fn chat(&self) -> &ChatAggregator {
        &self.chat
    }

    fn delay_secs(&self) -> i64 {
        (self.cfg.reaction_delay_ms as f64 / 1000.0).round() as i64
    }

    /// Advance the evaluation clock to `now`. Returns events that completed.
    pub fn tick(&mut self, now: TsMs) -> Vec<HighlightEvent> {
        let d = if self.cfg.use_chat { self.delay_secs() } else { 0 };
        // A moment-second m is evaluable once chat second m+d is closed (+750 ms grace for
        // late messages) and audio for m had a chance to arrive.
        let limit = (now - 750).div_euclid(1000) - 1 - d;
        let mut out = Vec::new();
        let start = match self.next_eval {
            Some(s) => s,
            None => {
                let first_chat = self.chat.latest_sec().map(|s| s - d);
                let first_audio = self.audio.keys().next().copied();
                match (first_chat, first_audio) {
                    (Some(a), Some(b)) => a.min(b),
                    (Some(a), None) => a,
                    (None, Some(b)) => b,
                    (None, None) => return out,
                }
            }
        };
        let start = start.max(limit - 3600);
        let mut m = start;
        while m <= limit {
            if let Some(ev) = self.evaluate(m, d, now) {
                out.push(ev);
            }
            m += 1;
        }
        self.next_eval = Some(m.max(start));
        out
    }

    /// Force-close any active event (e.g. on stream end).
    pub fn flush(&mut self, now: TsMs) -> Vec<HighlightEvent> {
        let d = self.delay_secs();
        self.active
            .take()
            .and_then(|a| self.finalize(a, d, now))
            .into_iter()
            .collect()
    }

    fn evaluate(&mut self, m: i64, d: i64, now: TsMs) -> Option<HighlightEvent> {
        let w = self.cfg.weights.clone();
        let (rf, zf) = (w.ratio_full as f64, w.z_full as f64);
        let c = m + d;
        let mut e = [0f32; N_SIGNALS];
        let mut avail = [false; N_SIGNALS];
        let mut raw = Raw::default();

        let chat_on = self.cfg.use_chat && self.chat.total_seen > 0;
        let mut msgs_now = 0;
        if chat_on {
            let msgs = self.chat.bin(c).map(|b| b.msgs).unwrap_or(0) as f64;
            msgs_now = msgs as u32;
            let rate5 = self.chat.sum(c - 4, c, |b| b.msgs) as f64;
            let uniq10 = self.chat.unique_users(c - 9, c) as f64;
            let react5 = self.chat.sum(c - 4, c, |b| b.reaction_total()) as f64;
            let emote5 = self.chat.sum(c - 4, c, |b| b.emotes) as f64;
            let copy5 = self.chat.sum(c - 4, c, |b| b.copypasta) as f64;
            let recent3 = self.chat.sum(c - 2, c, |b| b.msgs) as f64;
            let prev5 = self.chat.sum(c - 7, c - 3, |b| b.msgs) as f64;
            let warm = self.rate_base.warmed_up();
            if warm {
                e[Signal::ChatVelocity as usize] = count_excitement(rate5, self.rate_base.mean(), 5.0, rf, zf) as f32;
                e[Signal::ChatUnique as usize] = count_excitement(uniq10, self.uniq_base.mean(), 1.0, rf, zf) as f32;
                e[Signal::ChatReactions as usize] =
                    count_excitement(react5, self.react_base.mean(), 5.0, rf, zf) as f32;
                e[Signal::ChatEmotes as usize] = count_excitement(emote5, self.emote_base.mean(), 5.0, rf, zf) as f32;
                e[Signal::ChatCopypasta as usize] = count_excitement(copy5, self.copy_base.mean(), 5.0, rf, zf) as f32;
                let prev_rate = (prev5 / 5.0).max(self.rate_base.mean());
                e[Signal::ChatAccel as usize] = count_excitement(recent3, prev_rate, 3.0, 4.0, 4.0) as f32;
            }
            raw.rate5 = rate5 as f32;
            raw.rate_base = self.rate_base.mean() as f32;
            raw.uniq10 = uniq10 as f32;
            raw.uniq_base = self.uniq_base.mean() as f32;
            raw.react5 = react5 as f32;
            raw.react_base = self.react_base.mean() as f32;
            raw.emote5 = emote5 as f32;
            raw.copy5 = copy5 as f32;
            // Update baselines after scoring.
            self.rate_base.update(msgs);
            self.uniq_base.update(uniq10);
            self.react_base
                .update(self.chat.bin(c).map(|b| b.reaction_total()).unwrap_or(0) as f64);
            self.emote_base
                .update(self.chat.bin(c).map(|b| b.emotes).unwrap_or(0) as f64);
            self.copy_base
                .update(self.chat.bin(c).map(|b| b.copypasta).unwrap_or(0) as f64);
            for s in &Signal::ALL[..6] {
                avail[*s as usize] = warm;
            }
        }

        let audio_on = self.cfg.use_audio && self.last_audio_sec.is_some_and(|s| s >= m - 5);
        let mut loud_db = -90.0;
        if audio_on {
            if let Some(a) = self.audio.get(&m).copied() {
                loud_db = a.loudness_db;
                let silent = a.loudness_db <= -55.0;
                let median = self.loud_median.median().unwrap_or(a.loudness_db as f64) as f32;
                let excess = a.loudness_db - median;
                let prev_mean = if self.loud_recent.is_empty() {
                    a.loudness_db
                } else {
                    self.loud_recent.iter().sum::<f32>() / self.loud_recent.len() as f32
                };
                let jump = a.loudness_db - prev_mean;
                if !silent && self.loud_median.len() >= 20 {
                    e[Signal::AudioLoudness as usize] =
                        0.6 * ((excess - 3.0) / 12.0).clamp(0.0, 1.0) + 0.4 * ((jump - 4.0) / 10.0).clamp(0.0, 1.0);
                    e[Signal::AudioLaughter as usize] = a.laughter.clamp(0.0, 1.0);
                    e[Signal::AudioTransient as usize] =
                        ((a.max_jump_db - 12.0) / 18.0).clamp(0.0, 1.0) * if a.peak_db > -30.0 { 1.0 } else { 0.0 };
                }
                if !silent {
                    self.loud_median.push(a.loudness_db as f64);
                }
                if self.loud_recent.len() == 3 {
                    self.loud_recent.pop_front();
                }
                self.loud_recent.push_back(a.loudness_db);
                raw.loud_excess = excess;
                raw.loud_db = a.loudness_db;
                raw.laughter = a.laughter;
                raw.jump = jump;
            }
            for s in &Signal::ALL[6..] {
                avail[*s as usize] = self.loud_median.len() >= 20;
            }
        }

        // Fusion: weight shares over available signals, agreement bonus.
        let total_w: f32 = (0..N_SIGNALS).filter(|i| avail[*i]).map(|i| w.w[i].max(0.0)).sum();
        let mut comps = [0f32; N_SIGNALS];
        let mut score = 0.0;
        let mut agree = 0u32;
        if total_w > 0.0 {
            for i in 0..N_SIGNALS {
                if !avail[i] {
                    continue;
                }
                let share = w.w[i].max(0.0) / total_w;
                comps[i] = share * e[i];
                score += comps[i];
                if e[i] >= 0.5 && share >= 0.05 {
                    agree += 1;
                }
            }
            score *= 1.0 + w.agreement_bonus * agree.saturating_sub(1) as f32;
            score = score.clamp(0.0, 1.0);
        }

        let thr = self.cfg.threshold();
        if self.history.len() == HISTORY_SECS {
            self.history.pop_front();
        }
        self.history.push_back(HypePoint {
            sec: m,
            score,
            threshold: thr,
            comps,
            msgs: msgs_now,
            loudness_db: loud_db,
        });
        self.stats.last_score = score;
        self.stats.threshold = thr;
        self.stats.msgs_per_sec = raw.rate5 / 5.0;
        self.stats.baseline_msgs_per_sec = self.rate_base.mean() as f32;
        self.stats.loudness_db = loud_db;
        self.stats.total_messages = self.chat.total_seen;
        self.stats.suppressed_messages = self.chat.total_suppressed;
        if chat_on {
            self.stats.unique_chatters_60s = self.chat.unique_users(c - 59, c);
        }

        // Hysteresis state machine.
        let audio_comp = comps[6] + comps[7] + comps[8];
        let low = thr * 0.6;
        match self.active.as_mut() {
            None => {
                if score >= thr {
                    self.active = Some(Active {
                        start: m,
                        last_above: m,
                        secs_above: 1,
                        peak: score,
                        peak_sec: m,
                        peak_comps: comps,
                        peak_raw: raw,
                        agree_at_peak: agree,
                        best_audio: (audio_comp, m),
                    });
                } else if score >= low {
                    // Track a possible onset just below threshold: handled via lookback on fire.
                }
                None
            }
            Some(a) => {
                if score >= low {
                    a.last_above = m;
                    if score >= thr {
                        a.secs_above += 1;
                    }
                    if score > a.peak {
                        a.peak = score;
                        a.peak_sec = m;
                        a.peak_comps = comps;
                        a.peak_raw = raw;
                        a.agree_at_peak = agree;
                    }
                    if audio_comp > a.best_audio.0 {
                        a.best_audio = (audio_comp, m);
                    }
                }
                let done = m - a.last_above >= 3 || m - a.start >= self.cfg.max_event_secs;
                if done {
                    let a = self.active.take().unwrap();
                    self.finalize(a, d, now)
                } else {
                    None
                }
            }
        }
    }

    fn onset(&self, a: &Active) -> i64 {
        // Walk back from the threshold crossing while the score is still rising from the
        // low threshold — the burst onset is earlier than the crossing.
        let low = self.cfg.threshold() * 0.6;
        let mut s = a.start;
        for p in self.history.iter().rev() {
            if p.sec >= a.start {
                continue;
            }
            if p.sec < a.start - 4 || p.score < low * 0.5 {
                break;
            }
            s = p.sec;
        }
        s
    }

    fn finalize(&mut self, a: Active, d: i64, now: TsMs) -> Option<HighlightEvent> {
        let thr = self.cfg.threshold();
        if a.secs_above < 2 && a.peak < thr * 1.4 {
            return None; // single-second blip
        }
        let audio_share: f32 = a.peak_comps[6..].iter().sum::<f32>() / a.peak.max(1e-6);
        let onset = self.onset(&a);
        let moment_sec = if audio_share > 0.5 {
            a.best_audio.1 as f64
        } else {
            onset as f64 - self.cfg.weights.lead_secs as f64
        };
        let moment_ts = (moment_sec * 1000.0) as i64 + 500;

        if let Some(last) = self.last_emitted_moment
            && moment_ts - last < self.cfg.refractory_ms
        {
            tracing::debug!(stream = %self.cfg.stream_id, "event suppressed (refractory)");
            return None;
        }
        while self.emitted_times.front().is_some_and(|t| now - *t > 3_600_000) {
            self.emitted_times.pop_front();
        }
        if self.emitted_times.len() as u32 >= self.cfg.max_per_hour {
            tracing::info!(stream = %self.cfg.stream_id, "hourly clip cap reached; event dropped");
            return None;
        }

        // Chat window that reacted to this moment.
        let (cf, ct) = (a.start + d - 1, a.last_above + d + 1);
        let reactions = self.chat.reactions(cf, ct);
        let dominant = self.chat.dominant_category(cf, ct);
        let kind = classify_kind(dominant, &reactions, a.peak_raw.laughter, audio_share);

        let reasons = explain(&a, self.cfg.reaction_delay_ms);
        let z = 5.0 * (a.peak / thr - 1.0)
            + 0.4 * (a.agree_at_peak as f32 - 1.0)
            + 0.2 * (a.secs_above as f32 - 2.0).min(3.0);
        let confidence = 1.0 / (1.0 + (-z).exp());
        let trigger = if audio_share > 0.65 {
            TriggerSource::Audio
        } else if audio_share < 0.1 {
            TriggerSource::Chat
        } else {
            TriggerSource::MultiSignal
        };

        let ev = HighlightEvent {
            id: Uuid::new_v4(),
            stream_id: self.cfg.stream_id.clone(),
            trigger,
            moment_ts,
            detected_at: now,
            activity_start: a.start * 1000,
            activity_end: (a.last_above + 1) * 1000,
            score: a.peak,
            confidence,
            kind,
            reasons,
            top_phrases: self.chat.top_phrases(cf, ct, 3).into_iter().map(|p| p.0).collect(),
            top_emotes: self.chat.top_emotes(cf, ct, 3).into_iter().map(|p| p.0).collect(),
            unique_chatters: self.chat.unique_users(cf, ct),
            messages: self.chat.sum(cf, ct, |b| b.msgs),
        };
        self.last_emitted_moment = Some(moment_ts);
        self.emitted_times.push_back(now);
        self.stats.events_emitted += 1;
        Some(ev)
    }
}

fn classify_kind(
    dominant: Option<(Category, f32)>,
    reactions: &[u32; crate::text::N_CATEGORIES],
    laughter: f32,
    audio_share: f32,
) -> MomentKind {
    let total: u32 = reactions.iter().sum();
    let share = |c: Category| {
        if total == 0 {
            0.0
        } else {
            reactions[c as usize] as f32 / total as f32
        }
    };
    if let Some((cat, frac)) = dominant
        && total >= 3
        && frac >= 0.3
    {
        return match cat {
            Category::Laugh => MomentKind::Funny,
            Category::Hype if share(Category::Shock) >= 0.25 => MomentKind::Clutch,
            Category::Hype => MomentKind::Hype,
            Category::Shock if share(Category::Hype) >= 0.25 => MomentKind::Clutch,
            Category::Shock => MomentKind::Shock,
            Category::Fail => MomentKind::Fail,
            Category::Wholesome => MomentKind::Wholesome,
            Category::ClipRequest => MomentKind::Hype,
        };
    }
    if laughter >= 0.5 {
        return MomentKind::Funny;
    }
    if audio_share > 0.5 {
        return MomentKind::Shock;
    }
    MomentKind::Generic
}

fn explain(a: &Active, delay_ms: i64) -> Vec<Reason> {
    let total: f32 = a.peak_comps.iter().sum::<f32>().max(1e-6);
    let r = &a.peak_raw;
    let mut v: Vec<(f32, Reason)> = Signal::ALL
        .iter()
        .filter(|s| a.peak_comps[**s as usize] / total >= 0.05)
        .map(|s| {
            let c = a.peak_comps[*s as usize] / total;
            let detail = match s {
                Signal::ChatVelocity => format!(
                    "chat at {:.1} msg/s vs {:.2} baseline ({:.1}×)",
                    r.rate5 / 5.0,
                    r.rate_base,
                    (r.rate5 / 5.0 + 0.1) / (r.rate_base + 0.1)
                ),
                Signal::ChatUnique => format!("{} unique chatters in 10 s (baseline {:.1})", r.uniq10, r.uniq_base),
                Signal::ChatReactions => {
                    format!("{} reaction messages in 5 s (baseline {:.2}/s)", r.react5, r.react_base)
                }
                Signal::ChatEmotes => format!("{} emotes in 5 s", r.emote5),
                Signal::ChatCopypasta => format!("copy-paste wave: {} matching messages", r.copy5),
                Signal::ChatAccel => "chat accelerating sharply".to_string(),
                Signal::AudioLoudness if r.jump > 3.0 => format!(
                    "audio {:.1} dB above recent level (sudden +{:.1} dB)",
                    r.loud_excess, r.jump
                ),
                Signal::AudioLoudness => format!("audio {:.1} dB above recent level", r.loud_excess),
                Signal::AudioLaughter => format!("laughter-like audio ({:.0}%)", r.laughter * 100.0),
                Signal::AudioTransient => "sudden loud sound".to_string(),
            };
            (
                c,
                Reason {
                    signal: s.key().to_string(),
                    detail,
                    contribution: c,
                },
            )
        })
        .collect();
    v.sort_by(|x, y| y.0.total_cmp(&x.0));
    let mut out: Vec<Reason> = v.into_iter().map(|x| x.1).collect();
    if out.iter().any(|r| r.signal.starts_with("chat")) {
        out.push(Reason {
            signal: "timing".into(),
            detail: format!("moment placed {:.1} s before chat reaction", delay_ms as f32 / 1000.0),
            contribution: 0.0,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::{Scenario, generate};

    #[test]
    fn detects_burst_and_ignores_calm() {
        let sc = Scenario::medium_channel(42, 900, 3);
        let data = generate(&sc);
        let mut det = Detector::new(DetectorConfig::new("test"));
        let mut events = vec![];
        let mut i = 0;
        for now in (data.start_ms..data.end_ms + 20_000).step_by(1000) {
            while i < data.messages.len() && data.messages[i].ts <= now {
                det.push_chat(&data.messages[i]);
                i += 1;
            }
            events.extend(det.tick(now));
        }
        assert!(!events.is_empty());
        for e in &events {
            assert!(!e.reasons.is_empty());
            assert!(e.confidence > 0.0 && e.confidence <= 1.0);
        }
        // Every labelled moment found within 8 s.
        let found = sc
            .labels()
            .iter()
            .filter(|l| events.iter().any(|e| (e.moment_ts - l.moment_ts).abs() <= 8_000))
            .count();
        assert_eq!(
            found,
            sc.labels().len(),
            "events: {:?}",
            events.iter().map(|e| e.moment_ts).collect::<Vec<_>>()
        );
    }
}
