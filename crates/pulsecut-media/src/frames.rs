//! Decoded frames via an FFmpeg rawvideo pipe (thumbnails, smart crop, UI preview).

use image::RgbaImage;
use pulsecut_core::process::{Tools, run};
use pulsecut_core::{Error, Result};
use std::path::Path;
use std::time::Duration;

fn secs(ms: i64) -> String {
    format!("{:.3}", ms.max(0) as f64 / 1000.0)
}

/// Decode up to `count` frames at `fps` starting at `start_ms`, scaled to exactly `w`x`h`
/// (letterboxing is the caller's concern). Returns RGBA images.
pub async fn decode_rgba(
    tools: &Tools,
    input: &Path,
    start_ms: i64,
    count: u32,
    fps: f32,
    w: u32,
    h: u32,
) -> Result<Vec<RgbaImage>> {
    let args: Vec<String> = vec![
        "-hide_banner".into(),
        "-v".into(),
        "error".into(),
        "-ss".into(),
        secs(start_ms),
        "-i".into(),
        input.to_string_lossy().to_string(),
        "-an".into(),
        "-vf".into(),
        format!("fps={fps},scale={w}:{h}:flags=bilinear"),
        "-frames:v".into(),
        count.to_string(),
        "-f".into(),
        "rawvideo".into(),
        "-pix_fmt".into(),
        "rgba".into(),
        "pipe:1".into(),
    ];
    let out = run(tools.ffmpeg()?, &args, Duration::from_secs(120)).await?;
    let fsz = (w * h * 4) as usize;
    Ok(out
        .stdout
        .chunks_exact(fsz)
        .filter_map(|c| RgbaImage::from_raw(w, h, c.to_vec()))
        .collect())
}

/// Grey frames (1 byte/pixel) — cheaper for motion analysis.
pub async fn decode_gray(
    tools: &Tools,
    input: &Path,
    start_ms: i64,
    dur_ms: i64,
    fps: f32,
    w: u32,
    h: u32,
) -> Result<Vec<Vec<u8>>> {
    let args: Vec<String> = vec![
        "-hide_banner".into(),
        "-v".into(),
        "error".into(),
        "-ss".into(),
        secs(start_ms),
        "-t".into(),
        secs(dur_ms),
        "-i".into(),
        input.to_string_lossy().to_string(),
        "-an".into(),
        "-vf".into(),
        format!("fps={fps},scale={w}:{h}:flags=area"),
        "-f".into(),
        "rawvideo".into(),
        "-pix_fmt".into(),
        "gray".into(),
        "pipe:1".into(),
    ];
    let out = run(tools.ffmpeg()?, &args, Duration::from_secs(120)).await?;
    Ok(out.stdout.chunks_exact((w * h) as usize).map(|c| c.to_vec()).collect())
}

/// One frame at `at_ms`, full quality, scaled to fit within `max_w`x`max_h` keeping aspect.
pub async fn grab(tools: &Tools, input: &Path, at_ms: i64, w: u32, h: u32) -> Result<RgbaImage> {
    decode_rgba(tools, input, at_ms, 1, 30.0, w, h)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| Error::other(format!("no frame at {at_ms} ms in {}", input.display())))
}
