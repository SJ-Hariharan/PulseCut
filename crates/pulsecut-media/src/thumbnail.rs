//! Thumbnail selection and composition.
//!
//! Candidate frames around the moment are scored on sharpness (variance of the Laplacian),
//! colourfulness (Hasler–Süsstrunk), exposure and closeness to the moment; the winner gets a
//! contrast/saturation lift, an outlined hook title and optional creator branding.

use crate::frames::{decode_rgba, grab};
use crate::textimg::{Font, TextStyle, blit, text_card};
use image::{Rgba, RgbaImage};
use pulsecut_core::process::Tools;
use pulsecut_core::{Error, Result};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameScore {
    pub at_ms: i64,
    pub sharpness: f32,
    pub colorfulness: f32,
    pub exposure: f32,
    pub total: f32,
}

pub fn score_frame(img: &RgbaImage) -> (f32, f32, f32) {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let luma: Vec<f32> = img
        .pixels()
        .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
        .collect();
    // Laplacian variance.
    let mut lap = Vec::with_capacity(w * h);
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let i = y * w + x;
            lap.push(4.0 * luma[i] - luma[i - 1] - luma[i + 1] - luma[i - w] - luma[i + w]);
        }
    }
    let lm = lap.iter().sum::<f32>() / lap.len().max(1) as f32;
    let sharp = lap.iter().map(|v| (v - lm).powi(2)).sum::<f32>() / lap.len().max(1) as f32;
    // Colourfulness.
    let (mut rg, mut yb) = (vec![], vec![]);
    for p in img.pixels() {
        let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
        rg.push(r - g);
        yb.push(0.5 * (r + g) - b);
    }
    let stats = |v: &[f32]| {
        let m = v.iter().sum::<f32>() / v.len().max(1) as f32;
        let s = (v.iter().map(|x| (x - m).powi(2)).sum::<f32>() / v.len().max(1) as f32).sqrt();
        (m, s)
    };
    let ((mrg, srg), (myb, syb)) = (stats(&rg), stats(&yb));
    let colorful = (srg.powi(2) + syb.powi(2)).sqrt() + 0.3 * (mrg.powi(2) + myb.powi(2)).sqrt();
    let mean_luma = luma.iter().sum::<f32>() / luma.len().max(1) as f32 / 255.0;
    let exposure = 1.0 - ((mean_luma - 0.5).abs() * 2.0).min(1.0);
    (sharp, colorful, exposure)
}

/// Pick the best frame time in `[from_ms, to_ms]` of `input`, preferring times near `moment_ms`.
pub async fn pick_best_time(
    tools: &Tools,
    input: &Path,
    from_ms: i64,
    to_ms: i64,
    moment_ms: i64,
) -> Result<FrameScore> {
    let span = (to_ms - from_ms).max(1000);
    let fps = (12_000.0 / span as f32).clamp(0.5, 4.0);
    let count = ((span as f32 / 1000.0) * fps).ceil() as u32;
    let frames = decode_rgba(tools, input, from_ms, count.min(24), fps, 192, 108).await?;
    if frames.is_empty() {
        return Err(Error::other("no frames decoded for thumbnail"));
    }
    let raw: Vec<(i64, (f32, f32, f32))> = frames
        .iter()
        .enumerate()
        .map(|(i, f)| (from_ms + (i as f32 * 1000.0 / fps) as i64, score_frame(f)))
        .collect();
    let max_s = raw.iter().map(|r| r.1.0).fold(1e-6, f32::max);
    let max_c = raw.iter().map(|r| r.1.1).fold(1e-6, f32::max);
    let best = raw
        .iter()
        .map(|(t, (s, c, e))| {
            let near = (-((t - moment_ms).abs() as f32 / 3000.0)).exp();
            FrameScore {
                at_ms: *t,
                sharpness: s / max_s,
                colorfulness: c / max_c,
                exposure: *e,
                total: 0.4 * s / max_s + 0.25 * c / max_c + 0.15 * e + 0.2 * near,
            }
        })
        .max_by(|a, b| a.total.total_cmp(&b.total))
        .unwrap();
    Ok(best)
}

/// Contrast and saturation lift for punchier thumbnails.
pub fn punch_up(img: &mut RgbaImage, contrast: f32, saturation: f32) {
    for p in img.pixels_mut() {
        let l = 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
        for k in 0..3 {
            let v = p[k] as f32;
            let sat = l + (v - l) * saturation;
            let con = (sat - 128.0) * contrast + 128.0;
            p[k] = con.clamp(0.0, 255.0) as u8;
        }
    }
}

/// Darken the bottom/top band so title text stays readable.
fn gradient_band(img: &mut RgbaImage, top: bool, frac: f32, strength: f32) {
    let h = img.height() as f32;
    let band = h * frac;
    for (_, y, p) in img.enumerate_pixels_mut() {
        let d = if top { y as f32 } else { h - y as f32 };
        if d < band {
            let a = strength * (1.0 - d / band);
            for k in 0..3 {
                p[k] = (p[k] as f32 * (1.0 - a)) as u8;
            }
        }
    }
}

/// Compose a thumbnail from a frame of `input` at `at_ms`.
#[allow(clippy::too_many_arguments)]
pub async fn compose(
    tools: &Tools,
    font: Option<&Font>,
    input: &Path,
    at_ms: i64,
    w: u32,
    h: u32,
    title: Option<&str>,
    branding: Option<&str>,
    out: &Path,
) -> Result<()> {
    let img = grab(tools, input, at_ms, w, h).await?;
    compose_image(font, img, title, branding, out)
}

/// One clean frame of `input` at `at_ms` passed through the clip's layout (blur-fill, crop,
/// facecam…) — i.e. what the clip looks like *without* captions or overlays.
pub async fn grab_with_layout(
    tools: &Tools,
    input: &Path,
    at_ms: i64,
    plan: &pulsecut_core::EditPlan,
    info: &crate::probe::MediaInfo,
) -> Result<RgbaImage> {
    let graph = format!(
        "[0:v]setsar=1[v0];{}",
        crate::render::layout_filter(plan, info, "v0", "out")
    );
    let args: Vec<String> = vec![
        "-hide_banner".into(),
        "-v".into(),
        "error".into(),
        "-ss".into(),
        format!("{:.3}", at_ms.max(0) as f64 / 1000.0),
        "-i".into(),
        input.to_string_lossy().to_string(),
        "-filter_complex".into(),
        graph,
        "-map".into(),
        "[out]".into(),
        "-frames:v".into(),
        "1".into(),
        "-f".into(),
        "rawvideo".into(),
        "-pix_fmt".into(),
        "rgba".into(),
        "pipe:1".into(),
    ];
    let o = pulsecut_core::process::run(tools.ffmpeg()?, &args, std::time::Duration::from_secs(60)).await?;
    RgbaImage::from_raw(plan.width, plan.height, o.stdout)
        .ok_or_else(|| Error::other("layout frame had unexpected size"))
}

/// Compose a thumbnail from an already-decoded frame.
pub fn compose_image(
    font: Option<&Font>,
    mut img: RgbaImage,
    title: Option<&str>,
    branding: Option<&str>,
    out: &Path,
) -> Result<()> {
    let (w, h) = (img.width(), img.height());
    punch_up(&mut img, 1.12, 1.18);
    if let (Some(font), Some(title)) = (font, title.filter(|t| !t.trim().is_empty())) {
        let vertical = h > w;
        gradient_band(&mut img, !vertical, 0.38, 0.55);
        let px = if vertical { w as f32 * 0.12 } else { h as f32 * 0.16 };
        let cy = if vertical { h as f32 * 0.22 } else { h as f32 * 0.78 };
        let layer = text_card(
            font,
            w,
            h,
            &title.to_uppercase(),
            w as f32 / 2.0,
            cy,
            [255, 230, 0, 255],
            TextStyle {
                px,
                outline_px: px * 0.09,
                ..TextStyle::default()
            },
            w as f32 * 0.9,
        );
        blit(&mut img, &layer, 0, 0);
    }
    if let (Some(font), Some(b)) = (font, branding.filter(|b| !b.trim().is_empty())) {
        let px = (w.min(h) as f32 * 0.045).max(14.0);
        let layer = text_card(
            font,
            w,
            h,
            b,
            w as f32 - font.measure(b, px) / 2.0 - px,
            px * 1.3,
            [255, 255, 255, 230],
            TextStyle {
                px,
                outline_px: px * 0.12,
                shadow: false,
                ..TextStyle::default()
            },
            w as f32 * 0.5,
        );
        blit(&mut img, &layer, 0, 0);
    }
    let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
    rgb.save_with_format(out, image::ImageFormat::Jpeg)
        .map_err(|e| Error::other(format!("thumbnail save: {e}")))?;
    Ok(())
}

/// Solid placeholder (used when a clip has no decodable video).
pub fn placeholder(w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_pixel(w, h, Rgba([24, 24, 32, 255]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sharp_beats_flat() {
        let flat = RgbaImage::from_pixel(64, 36, Rgba([120, 120, 120, 255]));
        let mut checker = flat.clone();
        for (x, y, p) in checker.enumerate_pixels_mut() {
            if (x / 4 + y / 4) % 2 == 0 {
                *p = Rgba([250, 40, 40, 255]);
            }
        }
        let (s1, c1, _) = score_frame(&flat);
        let (s2, c2, _) = score_frame(&checker);
        assert!(s2 > s1 * 10.0 + 1.0);
        assert!(c2 > c1);
    }
}
