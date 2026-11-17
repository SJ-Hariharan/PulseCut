//! Cheap content analysis for editing decisions.

use crate::frames::decode_gray;
use pulsecut_core::Result;
use pulsecut_core::process::{Tools, run};
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotionCenter {
    /// Horizontal centre of motion energy, 0..1.
    pub x: f32,
    /// How concentrated the motion is (0 = uniform, 1 = a narrow column).
    pub focus: f32,
}

/// Horizontal motion centre from grey 96x54 frames: column histogram of frame differences.
#[allow(clippy::needless_range_loop)] // 2-D indexing is clearer here
pub fn motion_center_from_frames(frames: &[Vec<u8>], w: usize, h: usize) -> MotionCenter {
    let mut cols = vec![0f64; w];
    for pair in frames.windows(2) {
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let d = (pair[1][i] as i32 - pair[0][i] as i32).unsigned_abs() as f64;
                if d > 8.0 {
                    cols[x] += d;
                }
            }
        }
    }
    let total: f64 = cols.iter().sum();
    if total <= 1.0 {
        return MotionCenter { x: 0.5, focus: 0.0 };
    }
    // Smooth with a 9-column box filter, take the energy-weighted mean around the peak region.
    let k = 4isize;
    let smooth: Vec<f64> = (0..w as isize)
        .map(|x| {
            (-k..=k)
                .map(|d| cols[(x + d).clamp(0, w as isize - 1) as usize])
                .sum::<f64>()
        })
        .collect();
    let mean_x = cols.iter().enumerate().map(|(x, v)| x as f64 * v).sum::<f64>() / total;
    let var = cols
        .iter()
        .enumerate()
        .map(|(x, v)| (x as f64 - mean_x).powi(2) * v)
        .sum::<f64>()
        / total;
    let peak = smooth
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(x, _)| x)
        .unwrap_or(w / 2);
    // Blend the peak and the mean: robust to two-sided motion.
    let cx = (0.6 * peak as f64 + 0.4 * mean_x) / (w - 1).max(1) as f64;
    let uniform_std = w as f64 / 12f64.sqrt();
    let focus = (1.0 - var.sqrt() / uniform_std).clamp(0.0, 1.0);
    MotionCenter {
        x: cx.clamp(0.0, 1.0) as f32,
        focus: focus as f32,
    }
}

pub async fn motion_center(tools: &Tools, input: &Path, start_ms: i64, dur_ms: i64) -> Result<MotionCenter> {
    let (w, h) = (96u32, 54u32);
    let frames = decode_gray(tools, input, start_ms, dur_ms, 3.0, w, h).await?;
    Ok(motion_center_from_frames(&frames, w as usize, h as usize))
}

/// Decode mono f32 PCM at `sr` Hz.
pub async fn decode_pcm(tools: &Tools, input: &Path, start_ms: i64, dur_ms: i64, sr: u32) -> Result<Vec<f32>> {
    let mut args: Vec<String> = vec!["-hide_banner".into(), "-v".into(), "error".into()];
    if start_ms > 0 {
        args.extend(["-ss".into(), format!("{:.3}", start_ms as f64 / 1000.0)]);
    }
    args.extend(["-i".into(), input.to_string_lossy().to_string()]);
    if dur_ms > 0 {
        args.extend(["-t".into(), format!("{:.3}", dur_ms as f64 / 1000.0)]);
    }
    args.extend(["-vn", "-ac", "1", "-ar", &sr.to_string(), "-f", "s16le", "pipe:1"].map(String::from));
    let out = run(tools.ffmpeg()?, &args, Duration::from_secs(120)).await?;
    Ok(out
        .stdout
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
        .collect())
}

/// RMS loudness envelope in dBFS per `frame_ms`.
pub fn envelope_db(samples: &[f32], sr: u32, frame_ms: u32) -> Vec<f32> {
    let n = (sr as usize * frame_ms as usize / 1000).max(1);
    samples
        .chunks(n)
        .map(|c| 10.0 * (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32 + 1e-10).log10())
        .collect()
}

/// Index of the loudest rise (largest positive jump over ~300 ms) — a good "impact" point.
pub fn loudest_onset(env_db: &[f32], window: usize) -> Option<usize> {
    if env_db.len() <= window {
        return None;
    }
    // First index with the maximal rise (earliest onset wins ties).
    let mut best: Option<(usize, f32)> = None;
    for i in window..env_db.len() {
        let j = env_db[i] - env_db[i - window];
        if best.is_none_or(|(_, b)| j > b) {
            best = Some((i, j));
        }
    }
    best.filter(|(_, j)| *j > 6.0).map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_motion_on_the_right() {
        let (w, h) = (96, 54);
        let mut frames = vec![];
        for f in 0..6u8 {
            let mut img = vec![50u8; w * h];
            for y in 10..40 {
                for x in 70..80 {
                    img[y * w + x] = if f % 2 == 0 { 250 } else { 0 };
                }
            }
            frames.push(img);
        }
        let m = motion_center_from_frames(&frames, w, h);
        assert!(m.x > 0.7 && m.x < 0.85, "{m:?}");
        assert!(m.focus > 0.5);
        let still = motion_center_from_frames(&vec![vec![0u8; w * h]; 4], w, h);
        assert_eq!(still.x, 0.5);
    }

    #[test]
    fn onset() {
        let mut env = vec![-40.0f32; 50];
        for v in env.iter_mut().skip(30) {
            *v = -12.0;
        }
        assert_eq!(loudest_onset(&env, 3), Some(30));
        assert_eq!(loudest_onset(&[-30.0; 50], 3), None);
    }
}
