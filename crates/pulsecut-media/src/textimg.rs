//! Outlined text rasterization with `ab_glyph`. Used for captions, reaction text, hook/outro
//! cards, watermarks and thumbnail titles, so PulseCut does not depend on FFmpeg being built
//! with libass/libfreetype (Homebrew's default FFmpeg has neither).

use ab_glyph::{Font as _, FontVec, PxScale, ScaleFont};
use image::{Rgba, RgbaImage};
use pulsecut_core::{Error, Result};
use std::path::{Path, PathBuf};

pub struct Font {
    font: FontVec,
    pub path: PathBuf,
}

/// Bold, widely available system fonts. Fonts are used at runtime from the user's system and
/// never bundled.
pub fn default_font_candidates() -> Vec<PathBuf> {
    let list: &[&str] = if cfg!(target_os = "macos") {
        &[
            "/System/Library/Fonts/Supplemental/Arial Black.ttf",
            "/System/Library/Fonts/Supplemental/Impact.ttf",
            "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
            "/System/Library/Fonts/Supplemental/Verdana Bold.ttf",
            "/Library/Fonts/Arial Bold.ttf",
        ]
    } else if cfg!(windows) {
        &[
            "C:\\Windows\\Fonts\\ariblk.ttf",
            "C:\\Windows\\Fonts\\impact.ttf",
            "C:\\Windows\\Fonts\\arialbd.ttf",
            "C:\\Windows\\Fonts\\segoeuib.ttf",
        ]
    } else {
        &[
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
            "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
        ]
    };
    list.iter().map(PathBuf::from).collect()
}

impl Font {
    pub fn from_file(path: &Path) -> Result<Font> {
        let data = std::fs::read(path)?;
        let font = FontVec::try_from_vec_and_index(data, 0)
            .map_err(|e| Error::other(format!("invalid font {}: {e}", path.display())))?;
        Ok(Font {
            font,
            path: path.to_path_buf(),
        })
    }

    /// Configured font if set and valid, otherwise the first available system font.
    pub fn load(configured: &str) -> Result<Font> {
        if !configured.is_empty() {
            return Font::from_file(Path::new(configured));
        }
        for p in default_font_candidates() {
            if p.is_file()
                && let Ok(f) = Font::from_file(&p)
            {
                return Ok(f);
            }
        }
        Err(Error::other("no usable bold font found; set Edit → Font in Settings"))
    }

    pub fn measure(&self, text: &str, px: f32) -> f32 {
        let sf = self.font.as_scaled(PxScale::from(px));
        let mut w = 0.0;
        let mut prev = None;
        for ch in text.chars() {
            let id = sf.glyph_id(ch);
            if let Some(p) = prev {
                w += sf.kern(p, id);
            }
            w += sf.h_advance(id);
            prev = Some(id);
        }
        w
    }

    pub fn has_glyph(&self, ch: char) -> bool {
        self.font.glyph_id(ch).0 != 0
    }
}

#[derive(Debug, Clone)]
pub struct Run {
    pub text: String,
    pub color: [u8; 4],
    /// Relative size (1.0 = base px). Used for the "pop" on the active caption word.
    pub scale: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct TextStyle {
    pub px: f32,
    pub outline: [u8; 4],
    pub outline_px: f32,
    pub shadow: bool,
    pub line_spacing: f32,
}

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle {
            px: 72.0,
            outline: [0, 0, 0, 255],
            outline_px: 6.0,
            shadow: true,
            line_spacing: 1.12,
        }
    }
}

/// Greedy word wrap to `max_width` pixels.
pub fn wrap(font: &Font, text: &str, px: f32, max_width: f32) -> Vec<String> {
    let mut lines = vec![];
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let cand = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur} {word}")
        };
        if font.measure(&cand, px) <= max_width || cur.is_empty() {
            cur = cand;
        } else {
            lines.push(std::mem::take(&mut cur));
            cur = word.to_string();
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Largest px ≤ `px` at which every line fits `max_width`.
pub fn fit_px(font: &Font, lines: &[String], px: f32, max_width: f32) -> f32 {
    let widest = lines.iter().map(|l| font.measure(l, px)).fold(0.0f32, f32::max);
    if widest <= max_width || widest <= 0.0 {
        px
    } else {
        (px * max_width / widest).max(8.0)
    }
}

/// Render lines of colored runs into a tight RGBA block (with room for outline/shadow).
pub fn render_block(font: &Font, lines: &[Vec<Run>], style: &TextStyle) -> RgbaImage {
    let pad = (style.outline_px.ceil() as i32 + if style.shadow { 8 } else { 0 }).max(2);
    let max_scale = lines.iter().flatten().map(|r| r.scale).fold(1.0f32, f32::max);
    let line_h = style.px * style.line_spacing * max_scale.max(1.0);
    let widths: Vec<f32> = lines
        .iter()
        .map(|l| l.iter().map(|r| font.measure(&r.text, style.px * r.scale)).sum())
        .collect();
    let block_w = widths.iter().copied().fold(0.0f32, f32::max).ceil() as i32 + 2 * pad;
    let block_h = (line_h * lines.len() as f32).ceil() as i32 + 2 * pad;
    let (w, h) = (block_w.max(1) as u32, block_h.max(1) as u32);

    // Coverage mask (for outline) + premultiplied fill layer.
    let mut cov = vec![0f32; (w * h) as usize];
    let mut fill = vec![[0f32; 4]; (w * h) as usize];

    for (li, line) in lines.iter().enumerate() {
        let base_sf = font.font.as_scaled(PxScale::from(style.px * max_scale));
        let baseline = pad as f32 + line_h * li as f32 + base_sf.ascent() + (line_h - base_sf.height()) / 2.0;
        let mut x = pad as f32 + (block_w as f32 - 2.0 * pad as f32 - widths[li]) / 2.0;
        for run in line {
            let sf = font.font.as_scaled(PxScale::from(style.px * run.scale));
            let mut prev = None;
            for ch in run.text.chars() {
                let id = sf.glyph_id(ch);
                if let Some(p) = prev {
                    x += sf.kern(p, id);
                }
                let glyph =
                    id.with_scale_and_position(PxScale::from(style.px * run.scale), ab_glyph::point(x, baseline));
                if let Some(og) = font.font.outline_glyph(glyph) {
                    let b = og.px_bounds();
                    let c = run.color;
                    og.draw(|gx, gy, v| {
                        let px = b.min.x as i32 + gx as i32;
                        let py = b.min.y as i32 + gy as i32;
                        if px >= 0 && py >= 0 && (px as u32) < w && (py as u32) < h {
                            let i = (py as u32 * w + px as u32) as usize;
                            let v = v.clamp(0.0, 1.0);
                            cov[i] = (cov[i] + v).min(1.0);
                            let a = v * c[3] as f32 / 255.0;
                            let f = &mut fill[i];
                            for k in 0..3 {
                                f[k] = f[k] * (1.0 - a) + c[k] as f32 / 255.0 * a;
                            }
                            f[3] = f[3] * (1.0 - a) + a;
                        }
                    });
                }
                x += sf.h_advance(id);
                prev = Some(id);
            }
        }
    }

    // Outline: distance transform of the glyph mask, then a soft stroke of radius outline_px.
    let outline_a = if style.outline_px > 0.0 {
        let dist = distance_field(&cov, w as usize, h as usize);
        dist.iter()
            .map(|d| (style.outline_px + 0.8 - d).clamp(0.0, 1.0))
            .collect::<Vec<f32>>()
    } else {
        vec![0.0; cov.len()]
    };

    let mut img = RgbaImage::new(w, h);
    let (sx, sy) = (4i32, 6i32);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let i = (y as u32 * w + x as u32) as usize;
            let mut out = [0f32; 4]; // premultiplied
            if style.shadow {
                let (qx, qy) = (x - sx, y - sy);
                if qx >= 0 && qy >= 0 && (qx as u32) < w && (qy as u32) < h {
                    let j = (qy as u32 * w + qx as u32) as usize;
                    let a = outline_a[j].max(cov[j]) * 0.45;
                    out = over(out, [0.0, 0.0, 0.0, a]);
                }
            }
            let oa = outline_a[i] * style.outline[3] as f32 / 255.0;
            if oa > 0.0 {
                let o = style.outline;
                out = over(
                    out,
                    [
                        o[0] as f32 / 255.0 * oa,
                        o[1] as f32 / 255.0 * oa,
                        o[2] as f32 / 255.0 * oa,
                        oa,
                    ],
                );
            }
            let f = fill[i];
            if f[3] > 0.0 {
                out = over(out, [f[0] * f[3], f[1] * f[3], f[2] * f[3], f[3]]);
            }
            img.put_pixel(x as u32, y as u32, unpremultiply(out));
        }
    }
    img
}

fn over(dst: [f32; 4], src: [f32; 4]) -> [f32; 4] {
    let k = 1.0 - src[3];
    [
        src[0] + dst[0] * k,
        src[1] + dst[1] * k,
        src[2] + dst[2] * k,
        src[3] + dst[3] * k,
    ]
}

fn unpremultiply(p: [f32; 4]) -> Rgba<u8> {
    if p[3] <= 1e-4 {
        return Rgba([0, 0, 0, 0]);
    }
    let a = p[3].min(1.0);
    let c = |v: f32| ((v / a).clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgba([c(p[0]), c(p[1]), c(p[2]), (a * 255.0).round() as u8])
}

/// Two-pass chamfer (3-4) distance to the nearest pixel with coverage ≥ 0.5, in pixels.
fn distance_field(cov: &[f32], w: usize, h: usize) -> Vec<f32> {
    const INF: f32 = 1e9;
    let mut d: Vec<f32> = cov.iter().map(|c| if *c >= 0.5 { 0.0 } else { INF }).collect();
    let (a, b) = (1.0f32, std::f32::consts::SQRT_2);
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let mut v = d[i];
            if x > 0 {
                v = v.min(d[i - 1] + a);
            }
            if y > 0 {
                v = v.min(d[i - w] + a);
                if x > 0 {
                    v = v.min(d[i - w - 1] + b);
                }
                if x + 1 < w {
                    v = v.min(d[i - w + 1] + b);
                }
            }
            d[i] = v;
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let i = y * w + x;
            let mut v = d[i];
            if x + 1 < w {
                v = v.min(d[i + 1] + a);
            }
            if y + 1 < h {
                v = v.min(d[i + w] + a);
                if x + 1 < w {
                    v = v.min(d[i + w + 1] + b);
                }
                if x > 0 {
                    v = v.min(d[i + w - 1] + b);
                }
            }
            d[i] = v;
        }
    }
    d
}

/// Alpha-blend `src` onto `dst` with its top-left at (x, y).
pub fn blit(dst: &mut RgbaImage, src: &RgbaImage, x: i32, y: i32) {
    for sy in 0..src.height() as i32 {
        for sx in 0..src.width() as i32 {
            let (dx, dy) = (x + sx, y + sy);
            if dx < 0 || dy < 0 || dx as u32 >= dst.width() || dy as u32 >= dst.height() {
                continue;
            }
            let s = src.get_pixel(sx as u32, sy as u32);
            let sa = s[3] as f32 / 255.0;
            if sa <= 0.0 {
                continue;
            }
            let d = dst.get_pixel_mut(dx as u32, dy as u32);
            let da = d[3] as f32 / 255.0;
            let oa = sa + da * (1.0 - sa);
            for k in 0..3 {
                let v = (s[k] as f32 * sa + d[k] as f32 * da * (1.0 - sa)) / oa.max(1e-6);
                d[k] = v.round().clamp(0.0, 255.0) as u8;
            }
            d[3] = (oa * 255.0).round() as u8;
        }
    }
}

/// Render `text` (wrapped to fit `max_width`) centred at (cx, cy) on a transparent canvas.
#[allow(clippy::too_many_arguments)]
pub fn text_card(
    font: &Font,
    canvas_w: u32,
    canvas_h: u32,
    text: &str,
    cx: f32,
    cy: f32,
    color: [u8; 4],
    style: TextStyle,
    max_width: f32,
) -> RgbaImage {
    let mut canvas = RgbaImage::new(canvas_w, canvas_h);
    let lines = wrap(font, text, style.px, max_width);
    let px = fit_px(font, &lines, style.px, max_width);
    let st = TextStyle { px, ..style };
    let runs: Vec<Vec<Run>> = lines
        .into_iter()
        .map(|l| {
            vec![Run {
                text: l,
                color,
                scale: 1.0,
            }]
        })
        .collect();
    let block = render_block(font, &runs, &st);
    blit(
        &mut canvas,
        &block,
        (cx - block.width() as f32 / 2.0).round() as i32,
        (cy - block.height() as f32 / 2.0).round() as i32,
    );
    canvas
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font() -> Option<Font> {
        Font::load("").ok()
    }

    #[test]
    fn renders_outlined_text() {
        let Some(f) = font() else { return };
        let img = render_block(
            &f,
            &[vec![Run {
                text: "HELLO".into(),
                color: [255, 255, 255, 255],
                scale: 1.0,
            }]],
            &TextStyle::default(),
        );
        assert!(img.width() > 100 && img.height() > 50);
        let white = img.pixels().filter(|p| p[0] > 240 && p[3] > 240).count();
        let black = img.pixels().filter(|p| p[0] < 20 && p[3] > 200).count();
        assert!(white > 500, "fill pixels {white}");
        assert!(black > 500, "outline pixels {black}");
    }

    #[test]
    fn wraps_and_fits() {
        let Some(f) = font() else { return };
        let lines = wrap(&f, "this is a fairly long caption line that must wrap", 60.0, 500.0);
        assert!(lines.len() >= 2);
        for l in &lines {
            assert!(f.measure(l, 60.0) <= 500.0 || !l.contains(' '));
        }
        let px = fit_px(&f, &["SUPERCALIFRAGILISTICEXPIALIDOCIOUS".into()], 80.0, 300.0);
        assert!(px < 80.0);
    }
}
