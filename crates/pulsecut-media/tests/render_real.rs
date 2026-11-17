//! Real FFmpeg renders (skipped when ffmpeg/ffprobe are not installed).

use pulsecut_core::process::{Tools, run};
use pulsecut_core::settings::ToolSettings;
use pulsecut_core::{AspectFormat, CaptionStyle, EditPlan, EffectCue, EffectKind, LayoutMode};
use pulsecut_media::assets::{AssetLibrary, install_builtin_pack};
use pulsecut_media::captions::Word;
use pulsecut_media::encoder::{VideoEncoder, choose, probe_encoders};
use pulsecut_media::render::{RenderJob, render};
use pulsecut_media::textimg::Font;
use pulsecut_media::{probe, thumbnail};
use std::path::Path;
use std::time::Duration;

fn tools() -> Option<Tools> {
    let t = Tools::discover(&ToolSettings::default(), &[]);
    (t.ffmpeg.is_some() && t.ffprobe.is_some()).then_some(t)
}

async fn make_source(t: &Tools, out: &Path, secs: u32) {
    let args: Vec<String> = [
        "-hide_banner",
        "-v",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        &format!("testsrc2=size=1280x720:rate=30:duration={secs}"),
        "-f",
        "lavfi",
        "-i",
        &format!("sine=frequency=330:sample_rate=48000:duration={secs}"),
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-shortest",
        &out.to_string_lossy(),
    ]
    .map(String::from)
    .to_vec();
    run(t.ffmpeg().unwrap(), &args, Duration::from_secs(60)).await.unwrap();
}

fn cue(kind: EffectKind, s: i64, e: i64) -> EffectCue {
    EffectCue {
        kind,
        start_ms: s,
        end_ms: e,
        intensity: 0.7,
        sound_asset: None,
        text: None,
        image_asset: None,
        confidence: 0.9,
        reason: "test".into(),
    }
}

#[tokio::test]
async fn renders_vertical_clip_with_all_effects() {
    let Some(t) = tools() else {
        eprintln!("ffmpeg missing; skipped");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.mp4");
    make_source(&t, &src, 8).await;
    let info = probe::probe(&t, &src).await.unwrap();
    assert_eq!((info.width, info.height), (1280, 720));

    let assets_root = dir.path().join("assets");
    install_builtin_pack(&assets_root).unwrap();
    let lib = AssetLibrary::load(&assets_root);
    let font = Font::load("").ok();

    let mut freeze = cue(EffectKind::FreezeFrame, 3000, 4000);
    freeze.sound_asset = Some("scratch".into());
    let mut react = cue(EffectKind::ReactionText, 3000, 3600);
    react.text = Some("WAIT WHAT".into());
    let mut punch = cue(EffectKind::PunchIn, 2900, 4200);
    punch.sound_asset = Some("impact_boom".into());
    let plan = EditPlan {
        recipe: "test-all".into(),
        format: AspectFormat::Vertical9x16,
        width: 540,
        height: 960,
        layout: LayoutMode::BlurFill,
        crop_center_x: 0.5,
        facecam: None,
        trim_start_ms: 1000,
        trim_end_ms: 7000,
        effects: vec![
            freeze,
            react,
            punch,
            cue(EffectKind::ScreenShake, 3000, 3400),
            cue(EffectKind::Flash, 3000, 3100),
            cue(EffectKind::SlowMotion, 4500, 5500),
        ],
        captions: true,
        caption_style: CaptionStyle::default(),
        normalize_audio: true,
        watermark_text: Some("@pulsecut".into()),
        intro_text: Some("watch this".into()),
        outro_text: None,
        fps: 30,
        reasons: vec![],
    };
    let words = vec![
        Word {
            text: "no".into(),
            start_ms: 500,
            end_ms: 800,
            prob: 1.0,
        },
        Word {
            text: "way".into(),
            start_ms: 800,
            end_ms: 1200,
            prob: 1.0,
        },
        Word {
            text: "that".into(),
            start_ms: 2000,
            end_ms: 2300,
            prob: 1.0,
        },
        Word {
            text: "happened!".into(),
            start_ms: 2300,
            end_ms: 2900,
            prob: 1.0,
        },
    ];
    let probes = probe_encoders(&t).await.unwrap();
    for p in &probes {
        eprintln!("encoder {:?}: ok={} {}ms {:?}", p.encoder, p.ok, p.ms, p.error);
    }
    let enc = choose(&probes, true);
    let out = dir.path().join("out.mp4");
    let job = RenderJob {
        source: &src,
        info: &info,
        plan: &plan,
        words: &words,
        keywords: Default::default(),
        font: font.as_ref(),
        assets: &lib,
        sfx_volume: 0.8,
        encoder: enc,
        threads: 0,
        work_dir: &dir.path().join("work"),
        out: &out,
    };
    let r = render(&t, &job).await.unwrap_or_else(|e| panic!("render failed: {e}"));
    let oi = probe::probe(&t, &out).await.unwrap();
    eprintln!(
        "rendered {:?} in timeline {:?}; info {:?}",
        r.duration_ms, r.timeline.pieces, oi
    );
    assert_eq!((oi.width, oi.height), (540, 960));
    assert!(oi.has_audio);
    // 6 s source + 1 s freeze + slow-mo stretch.
    assert!(
        (oi.duration_ms - r.duration_ms).abs() < 200,
        "{} vs {}",
        oi.duration_ms,
        r.duration_ms
    );
    assert!(r.duration_ms > 7_000);
    assert_eq!(r.sounds, 2);
    if font.is_some() {
        assert!(r.captions_rendered);
        assert_eq!(r.overlays, 3); // reaction, hook, watermark
    }
    let thumb = dir.path().join("thumb.jpg");
    let best = thumbnail::pick_best_time(&t, &out, 0, r.duration_ms - 100, 3000)
        .await
        .unwrap();
    thumbnail::compose(
        &t,
        font.as_ref(),
        &out,
        best.at_ms,
        540,
        960,
        Some("No way"),
        Some("@pulsecut"),
        &thumb,
    )
    .await
    .unwrap();
    assert!(std::fs::metadata(&thumb).unwrap().len() > 5_000);
    let _ = VideoEncoder::X264;
    // Keep artefacts for manual inspection when requested.
    if let Ok(keep) = std::env::var("PULSECUT_KEEP_RENDER") {
        std::fs::copy(&out, Path::new(&keep).join("render_all_effects.mp4")).unwrap();
        std::fs::copy(&thumb, Path::new(&keep).join("render_all_effects_thumb.jpg")).unwrap();
    }
}

#[tokio::test]
async fn renders_every_layout() {
    let Some(t) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.mp4");
    make_source(&t, &src, 3).await;
    let info = probe::probe(&t, &src).await.unwrap();
    let lib = AssetLibrary::default();
    for layout in LayoutMode::ALL {
        for (fmt, (w, h)) in [
            (AspectFormat::Vertical9x16, (360, 640)),
            (AspectFormat::Landscape16x9, (640, 360)),
        ] {
            let plan = EditPlan {
                recipe: "layout".into(),
                format: fmt,
                width: w,
                height: h,
                layout,
                crop_center_x: 0.7,
                facecam: Some(pulsecut_core::NormRect {
                    x: 0.7,
                    y: 0.65,
                    w: 0.28,
                    h: 0.3,
                }),
                trim_start_ms: 0,
                trim_end_ms: 2000,
                effects: vec![],
                captions: false,
                caption_style: CaptionStyle::default(),
                normalize_audio: false,
                watermark_text: None,
                intro_text: None,
                outro_text: None,
                fps: 30,
                reasons: vec![],
            };
            let out = dir.path().join(format!("{layout:?}_{w}.mp4"));
            let job = RenderJob {
                source: &src,
                info: &info,
                plan: &plan,
                words: &[],
                keywords: Default::default(),
                font: None,
                assets: &lib,
                sfx_volume: 1.0,
                encoder: VideoEncoder::X264,
                threads: 2,
                work_dir: &dir.path().join(format!("w{layout:?}{w}")),
                out: &out,
            };
            render(&t, &job)
                .await
                .unwrap_or_else(|e| panic!("{layout:?} {fmt:?}: {e}"));
            let oi = probe::probe(&t, &out).await.unwrap();
            assert_eq!((oi.width, oi.height), (w, h), "{layout:?}");
        }
    }
}
