//! Animated word-level captions rendered to an image sequence (PNG + ffconcat) that FFmpeg
//! overlays. One image per "active word" state, so cost scales with word count, not frames.

use crate::textimg::{Font, Run, TextStyle, blit, render_block};
use crate::timeline::Timeline;
use image::RgbaImage;
use pulsecut_core::{CaptionStyle, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub text: String,
    /// Source-clip time (ms from clip start).
    pub start_ms: i64,
    pub end_ms: i64,
    #[serde(default)]
    pub prob: f32,
}

/// Group words into caption pages: break on long pauses, sentence ends and size limits.
pub fn paginate(words: &[Word], style: &CaptionStyle) -> Vec<Vec<Word>> {
    let per_page = (style.max_words_per_line * style.max_lines).max(1);
    let mut pages: Vec<Vec<Word>> = vec![];
    let mut cur: Vec<Word> = vec![];
    for w in words {
        let pause = cur.last().map(|p: &Word| w.start_ms - p.end_ms).unwrap_or(0);
        let sentence_end = cur.last().map(|p| p.text.ends_with(['.', '!', '?'])).unwrap_or(false);
        if !cur.is_empty() && (cur.len() >= per_page || pause > 700 || sentence_end) {
            pages.push(std::mem::take(&mut cur));
        }
        cur.push(w.clone());
    }
    if !cur.is_empty() {
        pages.push(cur);
    }
    pages
}

fn display_text(w: &str, style: &CaptionStyle) -> String {
    let t = w.trim();
    let t = t.trim_matches(|c: char| c == ',' || c == '"');
    if style.uppercase {
        t.to_uppercase()
    } else {
        t.to_string()
    }
}

fn is_keyword(w: &str, keywords: &HashSet<String>) -> bool {
    let k: String = w
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    !k.is_empty() && (keywords.contains(&k) || k.chars().all(|c| c.is_ascii_digit()) || w.ends_with('!'))
}

pub const DEFAULT_KEYWORDS: &[&str] = &[
    "no", "what", "wait", "insane", "crazy", "never", "win", "won", "dead", "omg", "wow", "huge", "clutch", "why",
    "how", "yes", "lets", "go", "best", "worst", "help", "run", "stop", "bro",
];

/// One caption image state.
struct Frame {
    out_start: i64,
    out_end: i64,
    page: usize,
    active: Option<usize>,
}

/// Render the caption track for a clip. Returns the ffconcat path, or None if there are no words.
#[allow(clippy::too_many_arguments)]
pub fn render_track(
    font: &Font,
    words: &[Word],
    timeline: &Timeline,
    style: &CaptionStyle,
    keywords: &HashSet<String>,
    width: u32,
    height: u32,
    dir: &Path,
) -> Result<Option<PathBuf>> {
    let words: Vec<Word> = words
        .iter()
        .filter(|w| !w.text.trim().is_empty() && w.end_ms > 0 && w.start_ms < timeline.src_len)
        .cloned()
        .collect();
    if words.is_empty() {
        return Ok(None);
    }
    std::fs::create_dir_all(dir)?;
    let pages = paginate(&words, style);
    let out_len = timeline.out_len();

    // Build the frame list on the output timeline.
    let mut frames: Vec<Frame> = vec![];
    for (pi, page) in pages.iter().enumerate() {
        let page_end_src = page.last().unwrap().end_ms + 250;
        let next_start_src = pages.get(pi + 1).map(|p| p[0].start_ms).unwrap_or(i64::MAX);
        for (wi, w) in page.iter().enumerate() {
            let s = timeline.map_pre(w.start_ms.max(0));
            let e_src = page
                .get(wi + 1)
                .map(|n| n.start_ms)
                .unwrap_or(page_end_src.min(next_start_src));
            let e = timeline.map_post(e_src.min(timeline.src_len)).min(out_len);
            if e > s {
                frames.push(Frame {
                    out_start: s,
                    out_end: e,
                    page: pi,
                    active: Some(wi),
                });
            }
        }
    }
    frames.sort_by_key(|f| f.out_start);

    // Render images and the ffconcat list, filling gaps with a blank frame.
    let blank_path = dir.join("blank.png");
    RgbaImage::new(width, height).save(&blank_path).map_err(io_err)?;
    let max_w = width as f32 * 0.86;
    let base_style = TextStyle {
        px: style.font_size * (width as f32 / 1080.0),
        outline: style.outline_rgba,
        outline_px: style.outline_px * (width as f32 / 1080.0),
        shadow: style.shadow,
        line_spacing: 1.1,
    };
    let mut list = String::from("ffconcat version 1.0\n");
    let mut t = 0i64;
    let mut last_file = blank_path.clone();
    let mut n = 0;
    for f in &frames {
        let start = f.out_start.max(t);
        if start >= f.out_end {
            continue;
        }
        if start > t {
            let _ = writeln!(
                list,
                "file '{}'\nduration {:.3}",
                blank_path.display(),
                (start - t) as f64 / 1000.0
            );
        }
        let page = &pages[f.page];
        let img = render_page(font, page, f.active, style, keywords, &base_style, width, height, max_w);
        n += 1;
        let p = dir.join(format!("cap_{n:04}.png"));
        img.save(&p).map_err(io_err)?;
        let _ = writeln!(
            list,
            "file '{}'\nduration {:.3}",
            p.display(),
            (f.out_end - start) as f64 / 1000.0
        );
        last_file = p;
        t = f.out_end;
    }
    if t < out_len {
        let _ = writeln!(
            list,
            "file '{}'\nduration {:.3}",
            blank_path.display(),
            (out_len - t) as f64 / 1000.0
        );
        last_file = blank_path.clone();
    }
    // The concat demuxer ignores the last entry's duration unless the file is repeated.
    let _ = writeln!(list, "file '{}'", last_file.display());
    let path = dir.join("captions.ffconcat");
    std::fs::write(&path, list)?;
    Ok(Some(path))
}

#[allow(clippy::too_many_arguments)]
fn render_page(
    font: &Font,
    page: &[Word],
    active: Option<usize>,
    style: &CaptionStyle,
    keywords: &HashSet<String>,
    base: &TextStyle,
    width: u32,
    height: u32,
    max_w: f32,
) -> RgbaImage {
    let per_line = style.max_words_per_line.max(1);
    let mut lines: Vec<Vec<Run>> = vec![];
    for (li, chunk) in page.chunks(per_line).enumerate() {
        let mut runs = vec![];
        for (k, w) in chunk.iter().enumerate() {
            let idx = li * per_line + k;
            let is_active = active == Some(idx);
            let color = if is_active {
                style.highlight_rgba
            } else if style.preset == "bold_pop" && is_keyword(&w.text, keywords) {
                [80, 255, 140, 255]
            } else {
                style.text_rgba
            };
            let mut text = display_text(&w.text, style);
            if k + 1 < chunk.len() {
                text.push(' ');
            }
            runs.push(Run {
                text,
                color,
                scale: if is_active && style.animate { 1.12 } else { 1.0 },
            });
        }
        lines.push(runs);
    }
    // Shrink to fit the safe width.
    let mut st = *base;
    let widest = lines
        .iter()
        .map(|l| l.iter().map(|r| font.measure(&r.text, st.px * r.scale)).sum::<f32>())
        .fold(0.0f32, f32::max);
    if widest > max_w {
        st.px *= max_w / widest;
    }
    let block = render_block(font, &lines, &st);
    let mut canvas = RgbaImage::new(width, height);
    let cy = style.position_y.clamp(0.08, 0.92) * height as f32;
    // Keep inside vertical safe area (platform UI covers ~12% top/bottom).
    let y = (cy - block.height() as f32 / 2.0)
        .clamp(height as f32 * 0.06, height as f32 * 0.86 - block.height() as f32)
        .round() as i32;
    blit(
        &mut canvas,
        &block,
        ((width as f32 - block.width() as f32) / 2.0).round() as i32,
        y,
    );
    canvas
}

fn io_err(e: image::ImageError) -> pulsecut_core::Error {
    pulsecut_core::Error::other(format!("image write: {e}"))
}

/// Plain SRT export (for platforms that accept a caption file).
pub fn to_srt(words: &[Word], style: &CaptionStyle) -> String {
    let fmt = |ms: i64| {
        let ms = ms.max(0);
        format!(
            "{:02}:{:02}:{:02},{:03}",
            ms / 3_600_000,
            (ms / 60_000) % 60,
            (ms / 1000) % 60,
            ms % 1000
        )
    };
    let mut s = String::new();
    for (i, p) in paginate(words, style).iter().enumerate() {
        let text: Vec<String> = p.iter().map(|w| w.text.trim().to_string()).collect();
        let _ = write!(
            s,
            "{}\n{} --> {}\n{}\n\n",
            i + 1,
            fmt(p[0].start_ms),
            fmt(p.last().unwrap().end_ms),
            text.join(" ")
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(t: &str, s: i64, e: i64) -> Word {
        Word {
            text: t.into(),
            start_ms: s,
            end_ms: e,
            prob: 1.0,
        }
    }

    #[test]
    fn paginates_on_pause_and_size() {
        let style = CaptionStyle::preset("bold_pop"); // 3 words x 2 lines
        let words = vec![
            w("one", 0, 200),
            w("two", 200, 400),
            w("three.", 400, 600),
            w("four", 700, 900),
            w("five", 2000, 2200),
        ];
        let p = paginate(&words, &style);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].len(), 3);
    }

    #[test]
    fn renders_track_with_continuous_timing() {
        let Ok(font) = Font::load("") else { return };
        let dir = tempfile::tempdir().unwrap();
        let words = vec![w("no", 1000, 1300), w("way!", 1300, 1800), w("bro", 3000, 3300)];
        let tl = Timeline::identity(5000);
        let kw: HashSet<String> = DEFAULT_KEYWORDS.iter().map(|s| s.to_string()).collect();
        let p = render_track(&font, &words, &tl, &CaptionStyle::default(), &kw, 540, 960, dir.path())
            .unwrap()
            .unwrap();
        let list = std::fs::read_to_string(p).unwrap();
        let total: f64 = list
            .lines()
            .filter_map(|l| l.strip_prefix("duration "))
            .map(|d| d.parse::<f64>().unwrap())
            .sum();
        assert!((total - 5.0).abs() < 0.01, "durations sum to clip length: {total}");
        assert!(list.matches("cap_").count() >= 3);
    }

    #[test]
    fn srt_format() {
        let s = to_srt(&[w("hi", 0, 500)], &CaptionStyle::default());
        assert!(s.contains("00:00:00,000 --> 00:00:00,500"));
    }
}
