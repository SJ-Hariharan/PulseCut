//! EditPlan → one FFmpeg invocation.
//!
//! Graph outline:
//! ```text
//! [0:v] fps,setsar → layout (blur-fill | crop | facecam split) ─┐
//!                                       timeline pieces (trim/setpts/tpad) + concat ─→ zoompan (punch-in/shake)
//!   → eq (flash) → overlay(captions ffconcat) → overlay(reaction/meme/hook/outro/watermark PNGs) → [vout]
//! [0:a] pieces (atrim/atempo/silence) + concat → loudnorm → sidechain-duck under SFX → amix → alimiter → [aout]
//! ```
//! All text is pre-rendered to PNG in Rust (`textimg`), so no libass/drawtext is needed.

use crate::assets::{AssetKind, AssetLibrary};
use crate::captions::{Word, render_track};
use crate::encoder::VideoEncoder;
use crate::probe::MediaInfo;
use crate::textimg::{Font, TextStyle, text_card};
use crate::timeline::{Piece, Timeline};
use image::RgbaImage;
use pulsecut_core::process::{Tools, run};
use pulsecut_core::{EditPlan, EffectKind, Error, LayoutMode, Result};
use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct RenderJob<'a> {
    pub source: &'a Path,
    pub info: &'a MediaInfo,
    pub plan: &'a EditPlan,
    /// Words in source-clip time (0 = plan.trim_start_ms).
    pub words: &'a [Word],
    pub keywords: HashSet<String>,
    pub font: Option<&'a Font>,
    pub assets: &'a AssetLibrary,
    pub sfx_volume: f32,
    pub encoder: VideoEncoder,
    pub threads: u32,
    pub work_dir: &'a Path,
    pub out: &'a Path,
}

#[derive(Debug, Clone)]
pub struct RenderOutput {
    pub duration_ms: i64,
    pub timeline: Timeline,
    pub captions_rendered: bool,
    pub overlays: usize,
    pub sounds: usize,
    pub args: Vec<String>,
}

fn s(ms: i64) -> String {
    format!("{:.3}", ms as f64 / 1000.0)
}

fn even(v: f32) -> u32 {
    ((v / 2.0).round() as u32 * 2).max(2)
}

/// Layout filter chain from `[inp]` to `[out]` producing exactly W x H.
pub fn layout_filter(plan: &EditPlan, info: &MediaInfo, inp: &str, out: &str) -> String {
    let (w, h) = (plan.width, plan.height);
    let (sw, sh) = (info.width.max(2) as f32, info.height.max(2) as f32);
    let src_aspect = sw / sh;
    let out_aspect = w as f32 / h as f32;
    let cx = plan.crop_center_x.clamp(0.0, 1.0);
    if (src_aspect - out_aspect).abs() < 0.03 {
        return format!("[{inp}]scale={w}:{h}:flags=lanczos[{out}]");
    }
    match plan.layout {
        LayoutMode::BlurFill => format!(
            "[{inp}]split=2[bfa][bfb];\
             [bfa]scale={qw}:{qh}:force_original_aspect_ratio=increase,crop={qw}:{qh},boxblur=8:2,scale={w}:{h},eq=brightness=-0.07:saturation=1.15[bfbg];\
             [bfb]scale={w}:{h}:force_original_aspect_ratio=decrease:flags=lanczos[bffg];\
             [bfbg][bffg]overlay=(W-w)/2:(H-h)/2[{out}]",
            qw = even(w as f32 / 4.0),
            qh = even(h as f32 / 4.0),
        ),
        LayoutMode::CenterCrop | LayoutMode::SmartCrop => {
            let cx = if plan.layout == LayoutMode::CenterCrop { 0.5 } else { cx };
            format!(
                "[{inp}]scale={w}:{h}:force_original_aspect_ratio=increase:flags=lanczos,\
                 crop={w}:{h}:'max(0,min(iw-{w},{cx:.4}*iw-{w}/2))':'(ih-{h})/2'[{out}]"
            )
        }
        LayoutMode::Facecam => {
            let r = plan.facecam.unwrap_or(pulsecut_core::NormRect {
                x: 0.0,
                y: 0.0,
                w: 0.25,
                h: 0.3,
            });
            let r = r.clamped();
            let top_h = even(h as f32 * 0.38);
            let bot_h = h - top_h;
            format!(
                "[{inp}]split=2[fca][fcb];\
                 [fca]crop=iw*{rw:.4}:ih*{rh:.4}:iw*{rx:.4}:ih*{ry:.4},scale={w}:{top_h}:force_original_aspect_ratio=increase,crop={w}:{top_h}[fctop];\
                 [fcb]scale={w}:{bot_h}:force_original_aspect_ratio=increase:flags=lanczos,\
                 crop={w}:{bot_h}:'max(0,min(iw-{w},{cx:.4}*iw-{w}/2))':'(ih-{bot_h})/2'[fcbot];\
                 [fctop][fcbot]vstack[{out}]",
                rw = r.w,
                rh = r.h,
                rx = r.x,
                ry = r.y,
            )
        }
    }
}

fn atempo_chain(speed: f32) -> String {
    // atempo accepts 0.5..100 per instance; chain for slower speeds.
    let mut s = speed.max(0.25);
    let mut parts = vec![];
    while s < 0.5 {
        parts.push("atempo=0.5".to_string());
        s /= 0.5;
    }
    if (s - 1.0).abs() > 1e-3 {
        parts.push(format!("atempo={s:.4}"));
    }
    if parts.is_empty() {
        "anull".into()
    } else {
        parts.join(",")
    }
}

/// Zoom/shake expressions (in output seconds) for zoompan, or None if unused.
fn zoom_exprs(plan: &EditPlan, tl: &Timeline) -> Option<(String, String, String)> {
    let mut z = String::from("1");
    let mut sx = String::from("0");
    let mut sy = String::from("0");
    let mut any = false;
    let t = format!("(on/{})", plan.fps);
    for e in &plan.effects {
        let (a, b) = tl.map_range(e.start_ms, e.end_ms);
        let (a, b) = (a as f64 / 1000.0, b as f64 / 1000.0);
        match e.kind {
            EffectKind::PunchIn => {
                any = true;
                let amp = 0.12 + 0.28 * e.intensity.clamp(0.0, 1.0);
                let _ = write!(z, "+{amp:.3}*clip(({t}-{a:.3})/0.12,0,1)*clip(({b:.3}-{t})/0.2,0,1)");
            }
            EffectKind::ScreenShake => {
                any = true;
                let amp = 6.0 + 18.0 * e.intensity.clamp(0.0, 1.0);
                let gate = format!("between({t},{a:.3},{b:.3})");
                let _ = write!(z, "+0.06*{gate}");
                let _ = write!(sx, "+{amp:.1}*{gate}*sin({t}*71)");
                let _ = write!(sy, "+{amp:.1}*{gate}*cos({t}*53)");
            }
            _ => {}
        }
    }
    any.then_some((z, sx, sy))
}

fn flash_expr(plan: &EditPlan, tl: &Timeline) -> Option<String> {
    let mut e = String::new();
    for c in plan.effects.iter().filter(|c| c.kind == EffectKind::Flash) {
        let a = tl.map_pre(c.start_ms) as f64 / 1000.0;
        let _ = write!(e, "+0.45*clip(1-(t-{a:.3})/0.18,0,1)*gte(t,{a:.3})");
    }
    (!e.is_empty()).then(|| format!("0{e}"))
}

struct OverlayImg {
    path: PathBuf,
    start_ms: i64,
    end_ms: i64,
}

struct SoundCue {
    path: PathBuf,
    at_ms: i64,
    gain: f32,
}

/// Build the full argument list (pure except for writing overlay PNGs/caption images).
pub fn build(job: &RenderJob) -> Result<(Vec<String>, Timeline, bool, usize, usize)> {
    let plan = job.plan;
    let src_len = (plan.trim_end_ms - plan.trim_start_ms).max(500);
    let tl = Timeline::build(src_len, &plan.effects);
    let out_len = tl.out_len();
    let (w, h) = (plan.width, plan.height);
    std::fs::create_dir_all(job.work_dir)?;

    let mut args: Vec<String> = vec!["-hide_banner".into(), "-v".into(), "error".into(), "-y".into()];
    args.extend([
        "-ss".into(),
        s(plan.trim_start_ms),
        "-t".into(),
        s(src_len),
        "-i".into(),
    ]);
    args.push(job.source.to_string_lossy().to_string());
    let mut next_input = 1usize;

    // Captions.
    let mut caption_input = None;
    if plan.captions
        && let Some(font) = job.font
    {
        let style = &plan.caption_style;
        if let Some(list) = render_track(
            font,
            job.words,
            &tl,
            style,
            &job.keywords,
            w,
            h,
            &job.work_dir.join("captions"),
        )? {
            args.extend(["-f", "concat", "-safe", "0", "-i"].map(String::from));
            args.push(list.to_string_lossy().to_string());
            caption_input = Some(next_input);
            next_input += 1;
        }
    }

    // Overlay images (reaction text, memes, hook, outro, watermark).
    let mut overlays: Vec<OverlayImg> = vec![];
    let mut save_png = |img: RgbaImage, name: &str, a: i64, b: i64| -> Result<()> {
        let p = job.work_dir.join(format!("{name}.png"));
        img.save(&p).map_err(|e| Error::other(format!("overlay png: {e}")))?;
        overlays.push(OverlayImg {
            path: p,
            start_ms: a,
            end_ms: b,
        });
        Ok(())
    };
    if let Some(font) = job.font {
        let big = TextStyle {
            px: w as f32 * 0.11,
            outline_px: w as f32 * 0.011,
            ..TextStyle::default()
        };
        for (i, e) in plan.effects.iter().enumerate() {
            if e.kind == EffectKind::ReactionText
                && let Some(text) = &e.text
            {
                let (a, b) = tl.map_range(e.start_ms, e.end_ms);
                let img = text_card(
                    font,
                    w,
                    h,
                    text,
                    w as f32 / 2.0,
                    h as f32 * 0.30,
                    [255, 255, 255, 255],
                    big,
                    w as f32 * 0.88,
                );
                save_png(img, &format!("react_{i}"), a, b)?;
            }
        }
        if let Some(t) = plan.intro_text.as_deref().filter(|t| !t.trim().is_empty()) {
            let img = text_card(
                font,
                w,
                h,
                &t.to_uppercase(),
                w as f32 / 2.0,
                h as f32 * 0.16,
                [255, 230, 0, 255],
                TextStyle {
                    px: w as f32 * 0.085,
                    outline_px: w as f32 * 0.009,
                    ..TextStyle::default()
                },
                w as f32 * 0.9,
            );
            save_png(img, "hook", 0, 2600.min(out_len))?;
        }
        if let Some(t) = plan.outro_text.as_deref().filter(|t| !t.trim().is_empty()) {
            let img = text_card(
                font,
                w,
                h,
                t,
                w as f32 / 2.0,
                h as f32 * 0.5,
                [255, 255, 255, 255],
                big,
                w as f32 * 0.85,
            );
            save_png(img, "outro", (out_len - 2500).max(0), out_len)?;
        }
        if let Some(t) = plan.watermark_text.as_deref().filter(|t| !t.trim().is_empty()) {
            let px = w as f32 * 0.035;
            let img = text_card(
                font,
                w,
                h,
                t,
                w as f32 - font.measure(t, px) / 2.0 - w as f32 * 0.04,
                h as f32 * 0.045,
                [255, 255, 255, 170],
                TextStyle {
                    px,
                    outline_px: px * 0.1,
                    shadow: false,
                    ..TextStyle::default()
                },
                w as f32 * 0.6,
            );
            save_png(img, "watermark", 0, out_len)?;
        }
    }
    for (i, e) in plan.effects.iter().enumerate() {
        if e.kind != EffectKind::MemeOverlay {
            continue;
        }
        let Some(asset) = e.image_asset.as_deref().and_then(|q| job.assets.find(q)) else {
            continue;
        };
        if asset.entry.kind != AssetKind::Image {
            continue;
        }
        let Ok(src) = image::open(&asset.path) else { continue };
        let target_w = (w as f32 * 0.55) as u32;
        let scaled = src
            .resize(target_w, h / 3, image::imageops::FilterType::Lanczos3)
            .to_rgba8();
        let mut canvas = RgbaImage::new(w, h);
        crate::textimg::blit(
            &mut canvas,
            &scaled,
            ((w - scaled.width()) / 2) as i32,
            (h as f32 * 0.12) as i32,
        );
        let (a, b) = tl.map_range(e.start_ms, e.end_ms);
        save_png(canvas, &format!("meme_{i}"), a, b)?;
    }
    let overlay_base = next_input;
    for o in &overlays {
        args.extend(["-loop".into(), "1".into(), "-t".into(), s(out_len + 500), "-i".into()]);
        args.push(o.path.to_string_lossy().to_string());
        next_input += 1;
    }

    // Sound effects.
    let mut sounds: Vec<SoundCue> = vec![];
    for e in &plan.effects {
        if let Some(q) = e.sound_asset.as_deref()
            && let Some(a) = job.assets.find(q).filter(|a| a.entry.kind == AssetKind::Sound)
        {
            sounds.push(SoundCue {
                path: a.path.clone(),
                at_ms: tl.map_pre(e.start_ms.clamp(0, src_len)),
                gain: job.sfx_volume * a.entry.volume * (0.6 + 0.4 * e.intensity),
            });
        }
    }
    let sound_base = next_input;
    for snd in &sounds {
        args.extend(["-i".into(), snd.path.to_string_lossy().to_string()]);
        next_input += 1;
    }
    let _ = next_input;

    // ---- filter graph ----
    let mut g = String::new();
    let fps = plan.fps;
    let _ = write!(g, "[0:v]fps={fps},setsar=1,format=yuv420p[v0];");
    let _ = write!(g, "{};", layout_filter(plan, job.info, "v0", "vl"));
    let audio_src = if job.info.has_audio {
        "[0:a]aformat=sample_rates=48000:channel_layouts=stereo[a0];".to_string()
    } else {
        format!("anullsrc=r=48000:cl=stereo,atrim=0:{}[a0];", s(src_len))
    };
    g.push_str(&audio_src);

    if tl.is_identity() {
        g.push_str("[vl]null[vt];[a0]anull[at];");
    } else {
        let n = tl.pieces.len();
        let _ = write!(g, "[vl]split={n}");
        for i in 0..n {
            let _ = write!(g, "[vs{i}]");
        }
        let _ = write!(g, ";[a0]asplit={n}");
        for i in 0..n {
            let _ = write!(g, "[as{i}]");
        }
        g.push(';');
        for (i, p) in tl.pieces.iter().enumerate() {
            match *p {
                Piece::Play { a, b, speed } => {
                    let _ = write!(
                        g,
                        "[vs{i}]trim=start={}:end={},setpts=(PTS-STARTPTS)/{speed:.4}[pv{i}];",
                        s(a),
                        s(b)
                    );
                    let _ = write!(
                        g,
                        "[as{i}]atrim=start={}:end={},asetpts=PTS-STARTPTS,{}[pa{i}];",
                        s(a),
                        s(b),
                        atempo_chain(speed)
                    );
                }
                Piece::Freeze { at, dur } => {
                    let at = at.min(src_len - 1000 / fps as i64).max(0);
                    let _ = write!(
                        g,
                        "[vs{i}]trim=start={},setpts=PTS-STARTPTS,trim=end_frame=1,tpad=stop_mode=clone:stop_duration={}[pv{i}];",
                        s(at),
                        s(dur)
                    );
                    let _ = write!(g, "[as{i}]anullsink;aevalsrc=0|0:s=48000:d={}[pa{i}];", s(dur));
                }
            }
        }
        for i in 0..n {
            let _ = write!(g, "[pv{i}][pa{i}]");
        }
        let _ = write!(g, "concat=n={n}:v=1:a=1[vt][at];");
    }

    // Zoom / shake, flash.
    let mut vcur = "vt".to_string();
    if let Some((z, sx, sy)) = zoom_exprs(plan, &tl) {
        let _ = write!(
            g,
            "[{vcur}]zoompan=z='{z}':x='iw/2-iw/zoom/2+({sx})':y='ih/2-ih/zoom/2+({sy})':d=1:s={w}x{h}:fps={fps}[vz];"
        );
        vcur = "vz".into();
    }
    if let Some(fe) = flash_expr(plan, &tl) {
        let _ = write!(g, "[{vcur}]eq=brightness='{fe}':eval=frame[vf];");
        vcur = "vf".into();
    }
    if let Some(ci) = caption_input {
        let _ = write!(
            g,
            "[{ci}:v]format=rgba[capv];[{vcur}][capv]overlay=0:0:eof_action=pass:format=auto[vc];"
        );
        vcur = "vc".into();
    }
    for (k, o) in overlays.iter().enumerate() {
        let idx = overlay_base + k;
        let (a, b) = (o.start_ms as f64 / 1000.0, o.end_ms as f64 / 1000.0);
        let fade_out = (b - 0.15).max(a);
        let _ = write!(
            g,
            "[{idx}:v]format=rgba,fade=t=in:st={a:.3}:d=0.12:alpha=1,fade=t=out:st={fade_out:.3}:d=0.15:alpha=1[ov{k}];\
             [{vcur}][ov{k}]overlay=0:0:enable='between(t,{a:.3},{b:.3})':format=auto[vo{k}];"
        );
        vcur = format!("vo{k}");
    }
    let _ = write!(g, "[{vcur}]format=yuv420p[vout];");

    // Audio: normalize, then duck under SFX.
    let mut acur = "at".to_string();
    if plan.normalize_audio {
        let _ = write!(
            g,
            "[{acur}]loudnorm=I=-14:TP=-1.5:LRA=11,aformat=sample_rates=48000:channel_layouts=stereo[an];"
        );
        acur = "an".into();
    }
    if !sounds.is_empty() {
        for (k, snd) in sounds.iter().enumerate() {
            let idx = sound_base + k;
            let d = snd.at_ms.max(0);
            let _ = write!(
                g,
                "[{idx}:a]aformat=sample_rates=48000:channel_layouts=stereo,volume={:.3},adelay={d}|{d}[sx{k}];",
                snd.gain
            );
        }
        for k in 0..sounds.len() {
            let _ = write!(g, "[sx{k}]");
        }
        let _ = write!(
            g,
            "amix=inputs={}:normalize=0:duration=longest,asplit=2[sfxa][sfxb];\
             [{acur}][sfxa]sidechaincompress=threshold=0.04:ratio=6:attack=5:release=350[duck];\
             [duck][sfxb]amix=inputs=2:normalize=0:duration=first[am];",
            sounds.len()
        );
        acur = "am".into();
    }
    let _ = write!(g, "[{acur}]alimiter=limit=0.95,atrim=0:{}[aout]", s(out_len));

    args.extend(["-filter_complex".into(), g]);
    args.extend(["-map", "[vout]", "-map", "[aout]"].map(String::from));
    args.extend(job.encoder.args(job.threads));
    args.extend(
        [
            "-r",
            &fps.to_string(),
            "-c:a",
            "aac",
            "-b:a",
            "192k",
            "-ar",
            "48000",
            "-movflags",
            "+faststart",
        ]
        .map(String::from),
    );
    args.extend(["-t".into(), s(out_len)]);
    args.push(job.out.to_string_lossy().to_string());
    Ok((args, tl, caption_input.is_some(), overlays.len(), sounds.len()))
}

pub async fn render(tools: &Tools, job: &RenderJob<'_>) -> Result<RenderOutput> {
    let (args, timeline, captions, overlays, sounds) = build(job)?;
    let timeout = Duration::from_secs(60 + (timeline.out_len() as u64 / 1000) * 20);
    let r = run(tools.ffmpeg()?, &args, timeout).await;
    if let Err(e) = r {
        // Hardware encoders can fail on odd sizes/sessions; retry once in software.
        if job.encoder.is_hardware() {
            tracing::warn!("hardware encode failed, retrying with libx264: {e}");
            let mut sw = args.clone();
            if let Some(i) = sw.iter().position(|a| a == "-c:v") {
                let enc_args = job.encoder.args(job.threads);
                let sw_args = VideoEncoder::X264.args(job.threads);
                sw.splice(i..i + enc_args.len(), sw_args);
            }
            run(tools.ffmpeg()?, &sw, timeout).await?;
        } else {
            return Err(e);
        }
    }
    Ok(RenderOutput {
        duration_ms: timeline.out_len(),
        timeline,
        captions_rendered: captions,
        overlays,
        sounds,
        args,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsecut_core::{AspectFormat, CaptionStyle, EffectCue};

    pub(crate) fn plan(effects: Vec<EffectCue>, layout: LayoutMode) -> EditPlan {
        EditPlan {
            recipe: "test".into(),
            format: AspectFormat::Vertical9x16,
            width: 540,
            height: 960,
            layout,
            crop_center_x: 0.5,
            facecam: None,
            trim_start_ms: 0,
            trim_end_ms: 6000,
            effects,
            captions: false,
            caption_style: CaptionStyle::default(),
            normalize_audio: true,
            watermark_text: None,
            intro_text: None,
            outro_text: None,
            fps: 30,
            reasons: vec![],
        }
    }

    #[test]
    fn atempo_chains() {
        assert_eq!(atempo_chain(1.0), "anull");
        assert_eq!(atempo_chain(0.4), "atempo=0.5,atempo=0.8000");
    }

    #[test]
    fn layout_variants_produce_labels() {
        let info = MediaInfo {
            width: 1920,
            height: 1080,
            ..Default::default()
        };
        for l in LayoutMode::ALL {
            let f = layout_filter(&plan(vec![], l), &info, "a", "b");
            assert!(f.starts_with("[a]") && f.ends_with("[b]"), "{f}");
        }
    }
}
