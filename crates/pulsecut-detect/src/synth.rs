//! Synthetic, labelled livestream data (chat + per-second audio features) so detection can be
//! developed and evaluated without a live stream. Scenarios cover quiet / medium / large
//! channels, real moments of varying strength, overlapping moments and hard negatives
//! (single-user spam, bot rings, poll votes, giveaway commands, slow audience growth,
//! loud-but-steady game audio).

use crate::audio::AudioSecond;
use crate::chat::ChatMessage;
use crate::rng::Rng;
use pulsecut_core::{MomentKind, TsMs};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MomentSpec {
    pub at_sec: i64,
    pub kind: MomentKind,
    /// Chat-rate multiplier at the burst peak.
    pub intensity: f64,
    pub duration_secs: i64,
    /// Streamer audio gets louder at the moment.
    pub audio_spike: bool,
    pub laugh_audio: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Distractor {
    /// One user flooding the same emote.
    SingleSpammer { at_sec: i64, secs: i64 },
    /// A few accounts posting the same promo text.
    BotRing { at_sec: i64, secs: i64 },
    /// Many viewers voting "1"/"2" in a poll.
    PollVotes { at_sec: i64, secs: i64 },
    /// Giveaway entry commands.
    Giveaway { at_sec: i64, secs: i64 },
    /// Audience grows `factor`× linearly over `secs`.
    SlowRamp { at_sec: i64, secs: i64, factor: f64 },
    /// Loud but steady game audio (music / gunfire bed) without any reaction.
    LoudBed { at_sec: i64, secs: i64 },
    /// Organic chat surge without reactions (streamer asked chat a question): many unique
    /// users, normal text, `factor`× rate.
    ChatSurge { at_sec: i64, secs: i64, factor: f64 },
    /// Streamer shouts / door slam with no chat response.
    Shout { at_sec: i64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scenario {
    pub name: String,
    pub seed: u64,
    pub duration_secs: i64,
    /// Baseline messages per second.
    pub base_rate: f64,
    pub population: usize,
    pub moments: Vec<MomentSpec>,
    pub distractors: Vec<Distractor>,
    /// Mean delay between the moment and the chat reaction.
    pub reaction_delay_ms: i64,
    pub audio: bool,
    pub start_ms: TsMs,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Label {
    pub moment_ts: TsMs,
    pub kind: MomentKind,
}

#[derive(Debug, Clone)]
pub struct Dataset {
    pub messages: Vec<ChatMessage>,
    pub audio: Vec<AudioSecond>,
    pub labels: Vec<Label>,
    pub start_ms: TsMs,
    pub end_ms: TsMs,
}

const NORMAL: &[&str] = &[
    "hi chat",
    "how long have you been playing this",
    "what rank are you",
    "nice",
    "this song is good",
    "hello from germany",
    "what's the build",
    "first time here",
    "can you play ranked later",
    "gg last game",
    "how do you do that",
    "chat is slow today",
    "what keyboard is that",
    "true",
    "yeah",
    "same",
    "lol nice",
    "is this the new patch",
    "back from work",
    "that was close",
    "hydrate",
    "what time is it there",
    "dont go left",
    "good morning",
    "the map looks different",
];

fn reaction_texts(kind: MomentKind) -> (&'static [&'static str], &'static [&'static str]) {
    match kind {
        MomentKind::Funny => (
            &[
                "KEKW",
                "LMAO",
                "hahahaha",
                "LUL LUL",
                "OMEGALUL",
                "im dead 😂",
                "ICANT",
                "LOL",
            ],
            &["KEKW", "LUL", "OMEGALUL"],
        ),
        MomentKind::Fail => (
            &["F", "RIP", "noooo", "unlucky", "Sadge", "bruh", "throwing", "L"],
            &["Sadge", "PepeHands"],
        ),
        MomentKind::Shock => (
            &["WHAT", "NO WAY", "wtf", "omg", "monkaS", "wait what", "HOLY"],
            &["monkaS"],
        ),
        MomentKind::Clutch => (
            &["NO WAY", "POGGERS", "CLUTCH", "WHAT", "insane", "LETS GOOO", "HOLY"],
            &["PogChamp", "POGGERS"],
        ),
        MomentKind::Wholesome => (&["<3", "aww", "so cute", "wholesome", "<3 <3"], &["<3"]),
        MomentKind::Hype | MomentKind::Generic => (
            &["POG", "LETS GOOOO", "W", "HUGE", "POGGERS", "insane", "clip it", "🔥🔥"],
            &["PogChamp", "POGGERS"],
        ),
    }
}

impl Scenario {
    fn base(name: &str, seed: u64, duration_secs: i64, base_rate: f64, population: usize) -> Scenario {
        Scenario {
            name: name.into(),
            seed,
            duration_secs,
            base_rate,
            population,
            moments: vec![],
            distractors: vec![],
            reaction_delay_ms: 6_000,
            audio: true,
            start_ms: 1_700_000_000_000,
        }
    }

    /// Moments every ~150–260 s with mixed kinds/strengths and some hard negatives.
    fn with_mixed_moments(mut self, n_moments: usize, intensity: (f64, f64)) -> Scenario {
        let mut r = Rng::new(self.seed.wrapping_mul(31).wrapping_add(7));
        let kinds = [
            MomentKind::Funny,
            MomentKind::Hype,
            MomentKind::Fail,
            MomentKind::Shock,
            MomentKind::Clutch,
            MomentKind::Wholesome,
        ];
        let mut t = 180;
        for i in 0..n_moments {
            if t > self.duration_secs - 60 {
                break;
            }
            let kind = kinds[(i + r.below(kinds.len())) % kinds.len()];
            self.moments.push(MomentSpec {
                at_sec: t,
                kind,
                intensity: r.range(intensity.0, intensity.1),
                duration_secs: 4 + r.below(8) as i64,
                audio_spike: r.chance(0.6),
                laugh_audio: kind == MomentKind::Funny && r.chance(0.7),
            });
            t += 150 + r.below(110) as i64;
        }
        self
    }

    fn with_negatives(mut self) -> Scenario {
        // Place distractors between moments.
        let gaps: Vec<i64> = self
            .moments
            .windows(2)
            .map(|w| (w[0].at_sec + w[1].at_sec) / 2)
            .collect();
        let kinds = 7;
        for (i, at) in gaps.into_iter().enumerate() {
            let d = match i % kinds {
                0 => Distractor::ChatSurge {
                    at_sec: at,
                    secs: 25,
                    factor: 2.5,
                },
                1 => Distractor::PollVotes { at_sec: at, secs: 30 },
                2 => Distractor::BotRing { at_sec: at, secs: 20 },
                3 => Distractor::Shout { at_sec: at },
                4 => Distractor::SingleSpammer { at_sec: at, secs: 25 },
                5 => Distractor::Giveaway { at_sec: at, secs: 30 },
                _ => Distractor::LoudBed { at_sec: at, secs: 40 },
            };
            self.distractors.push(d);
        }
        self
    }

    pub fn quiet_channel(seed: u64, duration_secs: i64, n: usize) -> Scenario {
        Scenario::base("quiet", seed, duration_secs, 0.15, 40).with_mixed_moments(n, (6.0, 14.0))
    }

    pub fn medium_channel(seed: u64, duration_secs: i64, n: usize) -> Scenario {
        Scenario::base("medium", seed, duration_secs, 2.0, 400).with_mixed_moments(n, (3.0, 7.0))
    }

    pub fn large_channel(seed: u64, duration_secs: i64, n: usize) -> Scenario {
        Scenario::base("large", seed, duration_secs, 25.0, 6000).with_mixed_moments(n, (2.0, 4.5))
    }

    /// The standard benchmark suite used by `eval` and the `pulsecut-cli eval` command.
    pub fn benchmark_suite(seeds: u64) -> Vec<Scenario> {
        Scenario::benchmark_suite_from(1000, seeds)
    }

    /// Same suite with a different seed base (held-out validation uses 5000+).
    pub fn benchmark_suite_from(base: u64, seeds: u64) -> Vec<Scenario> {
        let mut v = vec![];
        for s in 0..seeds {
            let seed = base + s;
            v.push(Scenario::quiet_channel(seed, 2400, 10).with_negatives());
            v.push(Scenario::medium_channel(seed, 2400, 10).with_negatives());
            v.push(Scenario::large_channel(seed, 2400, 10).with_negatives());
            // Weak moments (hard positives) on a medium channel.
            let mut weak = Scenario::base("medium-weak", seed + 50, 2400, 2.0, 400).with_mixed_moments(10, (2.0, 3.0));
            weak.name = "medium-weak".into();
            v.push(weak);
            // Overlapping moments: pairs 14 s apart.
            let mut ov = Scenario::base("overlap", seed + 77, 1800, 3.0, 500);
            for k in 0..6 {
                let t = 200 + k * 250;
                for dt in [0, 14] {
                    ov.moments.push(MomentSpec {
                        at_sec: t + dt,
                        kind: MomentKind::Hype,
                        intensity: 5.0,
                        duration_secs: 5,
                        audio_spike: true,
                        laugh_audio: false,
                    });
                }
            }
            v.push(ov);
            // Slow audience growth with no moments at all: everything detected is a FP.
            let mut ramp = Scenario::base("ramp-negative", seed + 99, 1800, 1.0, 600);
            ramp.distractors.push(Distractor::SlowRamp {
                at_sec: 120,
                secs: 1500,
                factor: 4.0,
            });
            v.push(ramp);
        }
        v
    }

    pub fn labels(&self) -> Vec<Label> {
        self.moments
            .iter()
            .map(|m| Label {
                moment_ts: self.start_ms + m.at_sec * 1000,
                kind: m.kind,
            })
            .collect()
    }
}

/// Generate the dataset for a scenario. Deterministic for a given seed.
pub fn generate(sc: &Scenario) -> Dataset {
    let mut r = Rng::new(sc.seed);
    let mut msgs: Vec<ChatMessage> = Vec::new();
    // Zipf-ish user activity weights: regulars talk much more than lurkers.
    let pop = sc.population.max(5);
    let weights: Vec<f64> = (1..=pop).map(|k| 1.0 / (k as f64).powf(0.9)).collect();
    let wsum: f64 = weights.iter().sum();
    let cdf: Vec<f64> = weights
        .iter()
        .scan(0.0, |acc, w| {
            *acc += w / wsum;
            Some(*acc)
        })
        .collect();
    let pick_regular = |r: &mut Rng| -> usize {
        let u = r.f64();
        cdf.partition_point(|c| *c < u).min(pop - 1)
    };

    // Each moment's actual reaction delay has jitter (viewers' latency differs).
    let delays: Vec<i64> = sc
        .moments
        .iter()
        .map(|_| sc.reaction_delay_ms + (r.normal() * 1200.0) as i64)
        .collect();

    for sec in 0..sc.duration_secs {
        let t0 = sc.start_ms + sec * 1000;
        let mut rate = sc.base_rate;
        for d in &sc.distractors {
            if let Distractor::SlowRamp { at_sec, secs, factor } = d
                && sec >= *at_sec
            {
                let f = ((sec - at_sec) as f64 / *secs as f64).min(1.0);
                rate *= 1.0 + (factor - 1.0) * f;
            }
        }
        // Normal chatter.
        for _ in 0..r.poisson(rate) {
            let u = pick_regular(&mut r);
            msgs.push(ChatMessage {
                ts: t0 + (r.f64() * 1000.0) as i64,
                user: format!("user{u}"),
                text: r.pick(NORMAL).to_string(),
                emotes: vec![],
                is_mod: false,
                is_subscriber: r.chance(0.2),
            });
        }
        // Reaction bursts.
        for (m, delay) in sc.moments.iter().zip(&delays) {
            let burst_start = m.at_sec * 1000 + delay;
            let rel = t0 - sc.start_ms - burst_start;
            let len = m.duration_secs * 1000;
            if rel < -1000 || rel > len + 4000 {
                continue;
            }
            // Fast attack, exponential decay.
            let shape = if rel < 0 {
                0.3
            } else if rel < len {
                1.0
            } else {
                (-(rel - len) as f64 / 1500.0).exp()
            };
            let extra = sc.base_rate.max(0.4) * (m.intensity - 1.0) * shape;
            let (texts, emotes) = reaction_texts(m.kind);
            for _ in 0..r.poisson(extra) {
                // Bursts pull in many lurkers -> unique chatter spike.
                let u = if r.chance(0.6) {
                    r.below(pop)
                } else {
                    pick_regular(&mut r)
                };
                let react = r.chance(0.75);
                let text = if react {
                    r.pick(texts).to_string()
                } else {
                    r.pick(NORMAL).to_string()
                };
                let em = if react && r.chance(0.5) {
                    vec![r.pick(emotes).to_string()]
                } else {
                    vec![]
                };
                msgs.push(ChatMessage {
                    ts: t0 + (r.f64() * 1000.0) as i64,
                    user: format!("user{u}"),
                    text,
                    emotes: em,
                    is_mod: false,
                    is_subscriber: r.chance(0.3),
                });
            }
        }
        // Distractors.
        for d in &sc.distractors {
            match d {
                Distractor::SingleSpammer { at_sec, secs } if sec >= *at_sec && sec < at_sec + secs => {
                    for k in 0..4 {
                        msgs.push(ChatMessage {
                            ts: t0 + k * 250,
                            user: "spam_guy".into(),
                            text: "LUL LUL LUL".into(),
                            emotes: vec!["LUL".into()],
                            is_mod: false,
                            is_subscriber: false,
                        });
                    }
                }
                Distractor::BotRing { at_sec, secs } if sec >= *at_sec && sec < at_sec + secs => {
                    for b in 0..3 {
                        msgs.push(ChatMessage {
                            ts: t0 + b * 300,
                            user: format!("promo_bot{b}"),
                            text: format!("cheap viewers at bestviewz dot com {sec}"),
                            emotes: vec![],
                            is_mod: false,
                            is_subscriber: false,
                        });
                    }
                }
                Distractor::PollVotes { at_sec, secs } if sec >= *at_sec && sec < at_sec + secs => {
                    for _ in 0..r.poisson(sc.base_rate.max(0.5) * 3.0) {
                        msgs.push(ChatMessage {
                            ts: t0 + (r.f64() * 1000.0) as i64,
                            user: format!("user{}", r.below(pop)),
                            text: if r.chance(0.5) { "1".into() } else { "2".into() },
                            emotes: vec![],
                            is_mod: false,
                            is_subscriber: false,
                        });
                    }
                }
                Distractor::ChatSurge { at_sec, secs, factor } if sec >= *at_sec && sec < at_sec + secs => {
                    for _ in 0..r.poisson(sc.base_rate.max(0.4) * (factor - 1.0)) {
                        msgs.push(ChatMessage {
                            ts: t0 + (r.f64() * 1000.0) as i64,
                            user: format!("user{}", r.below(pop)),
                            text: r.pick(NORMAL).to_string(),
                            emotes: vec![],
                            is_mod: false,
                            is_subscriber: false,
                        });
                    }
                }
                Distractor::Giveaway { at_sec, secs } if sec >= *at_sec && sec < at_sec + secs => {
                    for _ in 0..r.poisson(sc.base_rate.max(0.5) * 3.0) {
                        msgs.push(ChatMessage {
                            ts: t0 + (r.f64() * 1000.0) as i64,
                            user: format!("user{}", r.below(pop)),
                            text: "!enter".into(),
                            emotes: vec![],
                            is_mod: false,
                            is_subscriber: false,
                        });
                    }
                }
                _ => {}
            }
        }
    }
    msgs.sort_by_key(|m| m.ts);

    // Audio features at the moment timeline.
    let mut audio = Vec::new();
    if sc.audio {
        let mut level = -28.0f32;
        for sec in 0..sc.duration_secs {
            level += (r.normal() as f32) * 0.4 + (-28.0 - level) * 0.1;
            let mut db = level + (r.normal() as f32) * 1.5;
            let mut voiced = (0.45 + r.normal() * 0.15).clamp(0.0, 1.0) as f32;
            let mut laughter = (r.f64() * 0.12) as f32;
            let mut jump = (r.f64() * 6.0) as f32;
            for m in &sc.moments {
                let rel = sec - m.at_sec;
                if m.audio_spike && (0..=3).contains(&rel) {
                    db += [13.0, 10.0, 7.0, 4.0][rel as usize] + r.normal() as f32;
                    voiced = 0.9;
                    if rel == 0 {
                        jump = 18.0 + (r.f64() * 6.0) as f32;
                    }
                }
                if m.laugh_audio && (0..=4).contains(&rel) {
                    laughter = (0.55 + r.f64() * 0.35) as f32;
                    db += 5.0;
                    voiced = 0.95;
                }
            }
            for d in &sc.distractors {
                if let Distractor::Shout { at_sec } = d {
                    let rel = sec - at_sec;
                    if (0..=1).contains(&rel) {
                        db += [12.0, 6.0][rel as usize];
                        voiced = 0.9;
                        if rel == 0 {
                            jump = 17.0;
                        }
                    }
                }
                if let Distractor::LoudBed { at_sec, secs } = d
                    && sec >= *at_sec
                    && sec < at_sec + secs
                {
                    // Rises quickly and stays: should be absorbed by jump/baseline logic.
                    let ramp = ((sec - at_sec) as f32 / 4.0).min(1.0);
                    db += 9.0 * ramp;
                    voiced = 0.3;
                }
            }
            audio.push(AudioSecond {
                sec: (sc.start_ms / 1000) + sec,
                loudness_db: db,
                peak_db: db + 6.0,
                max_jump_db: jump,
                voiced,
                laughter,
            });
        }
    }

    Dataset {
        messages: msgs,
        audio,
        labels: sc.labels(),
        start_ms: sc.start_ms,
        end_ms: sc.start_ms + sc.duration_secs * 1000,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic() {
        let sc = Scenario::medium_channel(5, 600, 2);
        let a = generate(&sc);
        let b = generate(&sc);
        assert_eq!(a.messages.len(), b.messages.len());
        assert_eq!(a.messages[10], b.messages[10]);
    }

    #[test]
    fn rates_match_spec() {
        let sc = Scenario::large_channel(1, 300, 0);
        let d = generate(&sc);
        let rate = d.messages.len() as f64 / 300.0;
        assert!((rate - 25.0).abs() < 2.0, "{rate}");
    }
}
