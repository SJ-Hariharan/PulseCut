//! Original sound effects, procedurally synthesized at install time. They are PulseCut's own
//! work, released as CC0-1.0, so the app ships no third-party or meme audio.

use pulsecut_core::{Error, Result};
use std::f32::consts::PI;
use std::path::Path;

pub const SR: u32 = 48_000;

#[derive(Debug, Clone, Copy)]
pub struct SfxSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub tags: &'static [&'static str],
    pub synth: fn() -> Vec<f32>,
}

pub const BUILTIN: &[SfxSpec] = &[
    SfxSpec {
        id: "impact_boom",
        name: "Deep Impact",
        category: "impact",
        tags: &["boom", "hit", "win", "drop"],
        synth: impact_boom,
    },
    SfxSpec {
        id: "dramatic_hit",
        name: "Dramatic Hit",
        category: "dramatic",
        tags: &["stab", "reveal", "shock"],
        synth: dramatic_hit,
    },
    SfxSpec {
        id: "whoosh",
        name: "Whoosh",
        category: "transition",
        tags: &["swipe", "zoom", "transition"],
        synth: whoosh,
    },
    SfxSpec {
        id: "pop",
        name: "Pop",
        category: "transition",
        tags: &["text", "appear", "blip"],
        synth: pop,
    },
    SfxSpec {
        id: "ding",
        name: "Ding",
        category: "reaction",
        tags: &["correct", "nice", "bell"],
        synth: ding,
    },
    SfxSpec {
        id: "tada",
        name: "Ta-da Arpeggio",
        category: "reaction",
        tags: &["win", "success", "celebrate"],
        synth: tada,
    },
    SfxSpec {
        id: "boing",
        name: "Boing",
        category: "comedy",
        tags: &["bounce", "silly", "funny"],
        synth: boing,
    },
    SfxSpec {
        id: "slide_down",
        name: "Slide Whistle Down",
        category: "comedy",
        tags: &["fail", "fall", "funny"],
        synth: slide_down,
    },
    SfxSpec {
        id: "fail_buzzer",
        name: "Fail Buzzer",
        category: "comedy",
        tags: &["wrong", "fail", "lose"],
        synth: fail_buzzer,
    },
    SfxSpec {
        id: "scratch",
        name: "Record Stop",
        category: "comedy",
        tags: &["stop", "freeze", "wait what"],
        synth: scratch,
    },
    SfxSpec {
        id: "riser",
        name: "Tension Riser",
        category: "suspense",
        tags: &["build", "tension", "before"],
        synth: riser,
    },
    SfxSpec {
        id: "heartbeat",
        name: "Heartbeat",
        category: "suspense",
        tags: &["tense", "clutch", "nervous"],
        synth: heartbeat,
    },
    SfxSpec {
        id: "hype_horn",
        name: "Hype Horn",
        category: "impact",
        tags: &["hype", "win", "celebrate"],
        synth: hype_horn,
    },
];

/// Small deterministic noise source (no RNG dependency, identical output on every platform).
struct Noise(u32);
impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

fn n(secs: f32) -> usize {
    (secs * SR as f32) as usize
}

fn env_exp(i: usize, tau: f32) -> f32 {
    (-(i as f32 / SR as f32) / tau).exp()
}

fn attack(i: usize, secs: f32) -> f32 {
    ((i as f32 / SR as f32) / secs).min(1.0)
}

fn impact_boom() -> Vec<f32> {
    let mut nz = Noise(7);
    let mut phase = 0.0f32;
    (0..n(1.4))
        .map(|i| {
            let t = i as f32 / SR as f32;
            let f = 40.0 + 110.0 * (-t * 9.0).exp();
            phase += 2.0 * PI * f / SR as f32;
            let body = phase.sin() * env_exp(i, 0.35);
            let click = nz.next() * env_exp(i, 0.012) * 0.6;
            (body * 0.95 + click) * attack(i, 0.002)
        })
        .collect()
}

fn dramatic_hit() -> Vec<f32> {
    let mut boom = impact_boom();
    let chord = [110.0f32, 130.81, 164.81, 220.0];
    for (i, s) in boom.iter_mut().enumerate() {
        let t = i as f32 / SR as f32;
        let mut saw = 0.0;
        for f in chord {
            saw += 2.0 * ((t * f).fract()) - 1.0;
        }
        *s = *s * 0.7 + saw / chord.len() as f32 * 0.35 * env_exp(i, 0.5);
    }
    boom.extend((0..n(0.4)).map(|_| 0.0));
    boom
}

fn lowpass(x: &mut [f32], cutoff: impl Fn(usize) -> f32) {
    let mut y = 0.0f32;
    for (i, s) in x.iter_mut().enumerate() {
        let a = 1.0 - (-2.0 * PI * cutoff(i) / SR as f32).exp();
        y += a * (*s - y);
        *s = y;
    }
}

fn whoosh() -> Vec<f32> {
    let len = n(0.7);
    let mut nz = Noise(11);
    let mut x: Vec<f32> = (0..len).map(|_| nz.next()).collect();
    lowpass(&mut x, |i| 300.0 + 5000.0 * (PI * i as f32 / len as f32).sin());
    for (i, s) in x.iter_mut().enumerate() {
        let p = i as f32 / len as f32;
        *s *= (PI * p).sin().powf(1.5) * 2.5;
    }
    x
}

fn pop() -> Vec<f32> {
    let mut phase = 0.0f32;
    (0..n(0.09))
        .map(|i| {
            let t = i as f32 / SR as f32;
            phase += 2.0 * PI * (1200.0 - 5000.0 * t) / SR as f32;
            phase.sin() * env_exp(i, 0.02) * attack(i, 0.001)
        })
        .collect()
}

fn bell(f: f32, len: f32, tau: f32) -> Vec<f32> {
    (0..n(len))
        .map(|i| {
            let t = i as f32 / SR as f32;
            ((2.0 * PI * f * t).sin() + 0.5 * (2.0 * PI * f * 2.01 * t).sin() + 0.25 * (2.0 * PI * f * 3.02 * t).sin())
                / 1.75
                * env_exp(i, tau)
                * attack(i, 0.003)
        })
        .collect()
}

fn ding() -> Vec<f32> {
    bell(1318.5, 1.2, 0.35)
}

fn tada() -> Vec<f32> {
    let notes = [523.25f32, 659.25, 783.99, 1046.5];
    let mut out = vec![0.0; n(1.6)];
    for (k, f) in notes.iter().enumerate() {
        let start = n(0.09 * k as f32);
        for (i, v) in bell(*f, 1.6 - 0.09 * k as f32, 0.45).into_iter().enumerate() {
            if start + i < out.len() {
                out[start + i] += v * 0.45;
            }
        }
    }
    out
}

fn boing() -> Vec<f32> {
    let mut phase = 0.0f32;
    (0..n(0.7))
        .map(|i| {
            let t = i as f32 / SR as f32;
            let f = 180.0 + 420.0 * (1.0 - (-t * 6.0).exp()) + 40.0 * (2.0 * PI * 14.0 * t).sin();
            phase += 2.0 * PI * f / SR as f32;
            phase.sin() * env_exp(i, 0.25) * attack(i, 0.004)
        })
        .collect()
}

fn slide_down() -> Vec<f32> {
    let mut phase = 0.0f32;
    let len = n(0.9);
    (0..len)
        .map(|i| {
            let p = i as f32 / len as f32;
            let t = i as f32 / SR as f32;
            let f = 1500.0 * (300.0f32 / 1500.0).powf(p) * (1.0 + 0.02 * (2.0 * PI * 6.0 * t).sin());
            phase += 2.0 * PI * f / SR as f32;
            phase.sin() * 0.8 * attack(i, 0.02) * (1.0 - p).powf(0.3)
        })
        .collect()
}

fn fail_buzzer() -> Vec<f32> {
    let len = n(0.75);
    (0..len)
        .map(|i| {
            let t = i as f32 / SR as f32;
            let sq = if (t * 110.0).fract() < 0.5 { 1.0 } else { -1.0 };
            let sq2 = if (t * 116.0).fract() < 0.5 { 1.0 } else { -1.0 };
            (sq + sq2) * 0.25 * attack(i, 0.01) * (1.0 - i as f32 / len as f32).powf(0.2)
        })
        .collect()
}

fn scratch() -> Vec<f32> {
    let len = n(0.45);
    let mut nz = Noise(5);
    let mut x: Vec<f32> = (0..len).map(|_| nz.next()).collect();
    lowpass(&mut x, |i| {
        let p = i as f32 / len as f32;
        3000.0 * (1.0 - p) + 200.0
    });
    let mut phase = 0.0f32;
    for (i, s) in x.iter_mut().enumerate() {
        let p = i as f32 / len as f32;
        phase += 2.0 * PI * (600.0 * (1.0 - p) + 60.0) / SR as f32;
        *s = (*s * 1.5 + phase.sin() * 0.5) * (1.0 - p) * attack(i, 0.003);
    }
    x
}

fn riser() -> Vec<f32> {
    let len = n(2.2);
    let mut nz = Noise(23);
    let mut phase = 0.0f32;
    let mut noise: Vec<f32> = (0..len).map(|_| nz.next()).collect();
    lowpass(&mut noise, |i| 400.0 + 6000.0 * (i as f32 / len as f32).powi(2));
    noise
        .into_iter()
        .enumerate()
        .map(|(i, nv)| {
            let p = i as f32 / len as f32;
            phase += 2.0 * PI * (200.0 + 1800.0 * p * p) / SR as f32;
            (nv * 0.6 + phase.sin() * 0.4) * p.powf(1.5)
        })
        .collect()
}

fn heartbeat() -> Vec<f32> {
    let mut out = vec![0.0; n(1.2)];
    for start in [0.0f32, 0.28] {
        let s0 = n(start);
        let mut phase = 0.0f32;
        for i in 0..n(0.25) {
            let t = i as f32 / SR as f32;
            phase += 2.0 * PI * (60.0 - 20.0 * t) / SR as f32;
            if s0 + i < out.len() {
                out[s0 + i] += phase.sin() * env_exp(i, 0.06) * attack(i, 0.004);
            }
        }
    }
    out
}

fn hype_horn() -> Vec<f32> {
    let mut out = vec![];
    for (len, gap) in [(0.18f32, 0.07f32), (0.18, 0.07), (0.6, 0.0)] {
        for i in 0..n(len) {
            let t = i as f32 / SR as f32;
            let mut v = 0.0;
            for f in [440.0f32, 554.37, 659.25] {
                v += 2.0 * ((t * f).fract()) - 1.0;
            }
            out.push(v / 3.0 * 0.6 * attack(i, 0.01) * (1.0 - (i as f32 / n(len) as f32)).powf(0.15));
        }
        out.extend(std::iter::repeat_n(0.0, n(gap)));
    }
    let mut x = out;
    lowpass(&mut x, |_| 3500.0);
    x
}

/// Peak-normalize to -1 dBFS and write a 16-bit mono WAV.
pub fn write_wav(path: &Path, samples: &[f32]) -> Result<()> {
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs())).max(1e-6);
    let gain = 0.891 / peak;
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SR,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| Error::other(format!("wav: {e}")))?;
    // 5 ms fade-out to avoid clicks.
    let fade = n(0.005);
    let len = samples.len();
    for (i, s) in samples.iter().enumerate() {
        let f = if len - i < fade {
            (len - i) as f32 / fade as f32
        } else {
            1.0
        };
        w.write_sample((s * gain * f * 32767.0).clamp(-32768.0, 32767.0) as i16)
            .map_err(|e| Error::other(format!("wav: {e}")))?;
    }
    w.finalize().map_err(|e| Error::other(format!("wav: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_builtins_generate_valid_audio() {
        let dir = tempfile::tempdir().unwrap();
        for s in BUILTIN {
            let x = (s.synth)();
            assert!(x.len() > 1000, "{}", s.id);
            assert!(x.iter().all(|v| v.is_finite()), "{}", s.id);
            assert!(x.iter().any(|v| v.abs() > 0.05), "{} silent", s.id);
            let p = dir.path().join(format!("{}.wav", s.id));
            write_wav(&p, &x).unwrap();
            let r = hound::WavReader::open(&p).unwrap();
            assert_eq!(r.spec().sample_rate, SR);
        }
    }
}
