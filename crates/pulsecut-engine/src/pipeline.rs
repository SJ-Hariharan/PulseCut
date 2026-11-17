//! Captured footage → finished clip.

use crate::Engine;
use pulsecut_capture::CapturedFile;
use pulsecut_core::paths::sanitize_file_stem;
use pulsecut_core::{ClipStatus, Error, LayoutMode, Result, now_ms};
use pulsecut_edit::{ClipContext, Planner};
use pulsecut_media::captions::{DEFAULT_KEYWORDS, Word, to_srt};
use pulsecut_media::render::{RenderJob, render};
use pulsecut_media::{analysis, probe, thumbnail, transcribe};
use pulsecut_meta::MetaInput;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

pub async fn render_clip(e: &Arc<Engine>, clip_id: Uuid, payload: &serde_json::Value) -> Result<()> {
    let t0 = now_ms();
    let s = e.settings();
    let mut clip = e
        .store
        .get_clip(clip_id)
        .await
        .map_err(|x| Error::other(x.to_string()))?
        .ok_or_else(|| Error::other("clip vanished"))?;
    let raw = PathBuf::from(clip.raw_path.clone().ok_or_else(|| Error::other("no raw footage"))?);
    if !raw.exists() {
        return Err(Error::other(format!("raw footage missing: {}", raw.display())));
    }
    let info = probe::probe(&e.tools, &raw).await?;
    if !info.has_video {
        return Err(Error::other("captured file has no video stream"));
    }
    let rerender = payload.get("rerender").and_then(|v| v.as_bool()).unwrap_or(false);

    // Where the requested window and the moment sit inside the raw file.
    let file_start = match payload
        .get("file")
        .and_then(|f| serde_json::from_value::<CapturedFile>(f.clone()).ok())
    {
        Some(f) => f.file_start_ts,
        None => clip.coverage.map(|c| c.actual_start).unwrap_or(clip.clip_start),
    };
    let ev = clip.event.clone();
    let win_start = (clip.clip_start - file_start).clamp(0, info.duration_ms);
    let win_end = (clip.clip_end - file_start).clamp(win_start, info.duration_ms);
    let moment = (ev.moment_ts - file_start).clamp(win_start, win_end);
    let work = e.paths.work_dir().join(clip_id.to_string());
    std::fs::create_dir_all(&work)?;

    // 1. Audio impact point near the moment (loudest onset within ±3 s).
    let mut onset = None;
    if info.has_audio {
        let a0 = (moment - 3_000).max(0);
        if let Ok(pcm) = analysis::decode_pcm(&e.tools, &raw, a0, 6_000, 16_000).await {
            let env = analysis::envelope_db(&pcm, 16_000, 50);
            onset = analysis::loudest_onset(&env, 6).map(|i| a0 + i as i64 * 50);
        }
    }
    // 2. Motion centre for smart crop.
    let motion = if s.edit.layout == LayoutMode::SmartCrop {
        analysis::motion_center(&e.tools, &raw, win_start, win_end - win_start)
            .await
            .ok()
            .map(|m| (m.x, m.focus))
    } else {
        None
    };

    // 3. Transcription (skipped when memory is tight or whisper isn't installed).
    let mut words: Vec<Word> = clip
        .words_json
        .as_deref()
        .and_then(|w| serde_json::from_str(w).ok())
        .unwrap_or_default();
    let whisper_ok = e.tools.whisper_cli.is_some() && e.tools.whisper_model.is_some();
    if words.is_empty() && whisper_ok && info.has_audio && s.edit.captions {
        let avail = pulsecut_platform::sysstats::available_mem_mb();
        if avail < s.resources.min_free_mem_mb_for_whisper {
            e.log(format!("skipping transcription: only {avail} MB RAM free"));
        } else {
            let wav = work.join("speech.wav");
            transcribe::extract_wav(&e.tools, &raw, win_start, win_end - win_start, &wav).await?;
            match transcribe::transcribe(&e.tools, &wav, 4).await {
                Ok(w) => words = w,
                Err(err) => e.log(format!("transcription failed (clip continues without captions): {err}")),
            }
        }
    }
    let transcript = words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");

    // 4. Metadata (keep user edits on re-render).
    let generated = pulsecut_meta::generate(&MetaInput {
        event: Some(ev.clone()),
        kind: ev.kind,
        trigger: Some(ev.trigger),
        transcript: transcript.clone(),
        creator: s.meta.creator_name.clone(),
        stream_title: String::new(),
        game: String::new(),
        default_hashtags: s.meta.default_hashtags.clone(),
        source_stream: ev.stream_id.clone(),
        recorded_at: pulsecut_core::time::fmt_local(ev.moment_ts),
    });
    let generated = if s.meta.local_llm_url.is_empty() {
        generated
    } else {
        pulsecut_meta::refine_with_local_llm(
            generated,
            &MetaInput {
                event: Some(ev.clone()),
                transcript: transcript.clone(),
                creator: s.meta.creator_name.clone(),
                ..Default::default()
            },
            &s.meta.local_llm_url,
            &s.meta.local_llm_model,
        )
        .await
    };
    let meta = if rerender {
        clip.meta.clone().unwrap_or(generated.meta.clone())
    } else {
        generated.meta.clone()
    };

    // 5. Edit plan (or the user-edited one on re-render).
    let plan = match (&clip.plan, rerender) {
        (Some(p), true) => p.clone(),
        _ => Planner { settings: &s.edit }.plan(&ClipContext {
            event: ev.clone(),
            source_len_ms: info.duration_ms,
            window_start_ms: win_start,
            window_end_ms: win_end,
            moment_ms: moment,
            audio_onset_ms: onset,
            motion_center: motion,
            source_w: info.width,
            source_h: info.height,
            has_speech: words.len() >= 2,
            hook_text: Some(generated.hook.clone()),
        }),
    };
    // Words are relative to win_start; the renderer wants them relative to plan.trim_start.
    let shift = win_start - plan.trim_start_ms;
    let words_rel: Vec<Word> = words
        .iter()
        .map(|w| Word {
            start_ms: w.start_ms + shift,
            end_ms: w.end_ms + shift,
            ..w.clone()
        })
        .collect();

    // 6. Render.
    let lib_dir = library_dir(e);
    let day = chrono::Local::now().format("%Y-%m-%d").to_string();
    let out_dir = lib_dir.join(&day);
    std::fs::create_dir_all(&out_dir)?;
    let stem = format!("{}_{}", sanitize_file_stem(&meta.title, 40), &clip_id.to_string()[..8]);
    let out = out_dir.join(format!("{stem}.mp4"));
    let assets = e.assets();
    let font = e.font();
    let keywords: HashSet<String> = DEFAULT_KEYWORDS.iter().map(|s| s.to_string()).collect();
    let job = RenderJob {
        source: &raw,
        info: &info,
        plan: &plan,
        words: &words_rel,
        keywords: keywords.clone(),
        font: font.as_deref(),
        assets: &assets,
        sfx_volume: s.edit.sfx_volume,
        encoder: e.encoder(),
        threads: s.resources.ffmpeg_threads,
        work_dir: &work.join("v"),
        out: &out,
    };
    let rendered = render(&e.tools, &job).await?;
    // Never report a clip as ready until the output file is verified.
    verify_output(&e.tools, &out, rendered.duration_ms).await?;

    // Optional landscape export.
    let mut landscape = None;
    if s.edit.also_landscape {
        let mut lp = plan.clone();
        lp.format = pulsecut_core::AspectFormat::Landscape16x9;
        let (w, h) = lp.format.dims(1920);
        (lp.width, lp.height) = (w, h);
        lp.caption_style.position_y = 0.84;
        let lout = out_dir.join(format!("{stem}_16x9.mp4"));
        let ljob = RenderJob {
            plan: &lp,
            out: &lout,
            work_dir: &work.join("l"),
            ..job
        };
        match render(&e.tools, &ljob).await {
            Ok(_) => landscape = Some(lout),
            Err(err) => e.log(format!("landscape export failed: {err}")),
        }
    }

    // 7. Thumbnails from clean source frames (no burnt-in captions/overlays): pick the best
    //    frame near the impact in the raw footage, then apply the clip's layout for the cover.
    let thumb_v = out_dir.join(format!("{stem}_cover.jpg"));
    let thumb_l = out_dir.join(format!("{stem}_thumb.jpg"));
    let text = s.meta.thumbnail_text.then_some(generated.thumbnail_text.as_str());
    let brand = Some(s.meta.creator_name.as_str()).filter(|b| !b.is_empty());
    let impact = onset.filter(|o| (o - moment).abs() <= 2_500).unwrap_or(moment);
    let best = thumbnail::pick_best_time(
        &e.tools,
        &raw,
        (impact - 2_000).max(0),
        (impact + 2_000).min(info.duration_ms - 100),
        impact,
    )
    .await;
    let src_at = best.map(|b| b.at_ms).unwrap_or(impact);
    let mut cover_plan = plan.clone();
    (cover_plan.width, cover_plan.height) = (plan.width.min(1080), plan.height.min(1920));
    match thumbnail::grab_with_layout(&e.tools, &raw, src_at, &cover_plan, &info).await {
        Ok(img) => thumbnail::compose_image(font.as_deref(), img, text, brand, &thumb_v)?,
        Err(_) => {
            let mapped = rendered.timeline.map_pre((src_at - plan.trim_start_ms).max(0));
            thumbnail::compose(
                &e.tools,
                font.as_deref(),
                &out,
                mapped,
                cover_plan.width,
                cover_plan.height,
                text,
                brand,
                &thumb_v,
            )
            .await?
        }
    }
    let lw = 1280;
    let lh = (lw as f32 * info.height as f32 / info.width.max(1) as f32) as u32 & !1;
    thumbnail::compose(
        &e.tools,
        font.as_deref(),
        &raw,
        src_at,
        lw,
        lh.max(2),
        text,
        brand,
        &thumb_l,
    )
    .await?;

    // 8. Side files: captions (.srt) and a JSON sidecar with the full decision trail.
    if !words_rel.is_empty() {
        let _ = std::fs::write(out.with_extension("srt"), to_srt(&words_rel, &plan.caption_style));
    }

    clip.video_path = Some(out.display().to_string());
    clip.landscape_path = landscape.map(|p| p.display().to_string());
    clip.thumb_path = Some(thumb_l.display().to_string());
    clip.thumb_vertical_path = Some(thumb_v.display().to_string());
    clip.duration_ms = rendered.duration_ms;
    clip.meta = Some(meta.clone());
    clip.plan = Some(plan.clone());
    clip.transcript = transcript;
    clip.words_json = serde_json::to_string(&words).ok();
    clip.status = ClipStatus::Review;
    clip.error = None;
    let sidecar = serde_json::json!({
        "clip_id": clip_id, "event": ev, "plan": plan, "metadata": meta, "coverage": clip.coverage,
        "audio_onset_ms": onset, "render_ms": now_ms() - t0, "encoder": e.encoder().ffmpeg_name(),
    });
    let _ = std::fs::write(
        out.with_extension("json"),
        serde_json::to_string_pretty(&sidecar).unwrap_or_default(),
    );
    if s.storage.delete_raw_after_render && !rerender {
        let _ = std::fs::remove_file(&raw);
        clip.raw_path = None;
    }
    let _ = std::fs::remove_dir_all(&work);

    // 8b. Ready-to-post package: one folder per clip with everything a person (or another
    // assistant) needs to post it by hand.
    if let Err(x) = write_post_package(&out, &thumb_v, &clip.stream_id, &meta, rendered.duration_ms) {
        e.log(format!("ready-to-post package not written: {x}"));
    }

    // 9. Auto-publish gate (off by default).
    let auto = auto_publish_allowed(e, &clip).await;
    if auto {
        clip.status = ClipStatus::Approved;
    }
    e.store
        .upsert_clip(&clip)
        .await
        .map_err(|x| Error::other(x.to_string()))?;
    e.log(format!(
        "clip ready: \"{}\" ({:.1}s, recipe {}, {} ms)",
        meta.title,
        rendered.duration_ms as f32 / 1000.0,
        plan.recipe,
        now_ms() - t0
    ));
    if auto {
        for p in enabled_platforms(&s) {
            let _ = e.publish(clip.id, p, None).await;
        }
        e.log("auto-publish: queued (confidence and daily caps satisfied)".into());
    }
    Ok(())
}

/// Probe a rendered file: it must exist, contain video and be at least half the planned length.
pub async fn verify_output(
    tools: &pulsecut_core::process::Tools,
    out: &std::path::Path,
    planned_ms: i64,
) -> Result<()> {
    let size = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        return Err(Error::other(format!(
            "render produced no output file ({})",
            out.display()
        )));
    }
    let info = probe::probe(tools, out)
        .await
        .map_err(|x| Error::other(format!("rendered file is unreadable: {x}")))?;
    if !info.has_video || info.duration_ms < planned_ms / 2 {
        return Err(Error::other(format!(
            "rendered file is incomplete ({} ms of {planned_ms} ms, video: {})",
            info.duration_ms, info.has_video
        )));
    }
    Ok(())
}

fn enabled_platforms(s: &pulsecut_core::Settings) -> Vec<&'static str> {
    let mut v = vec![];
    if s.publish.youtube_enabled {
        v.push("youtube");
    }
    if s.publish.instagram_enabled {
        v.push("instagram");
    }
    if s.publish.tiktok_enabled {
        v.push("tiktok");
    }
    v
}

async fn auto_publish_allowed(e: &Arc<Engine>, clip: &pulsecut_store::ClipRecord) -> bool {
    let s = e.settings();
    if !s.publish.auto_publish || enabled_platforms(&s).is_empty() {
        return false;
    }
    if clip.event.trigger.is_manual() || clip.event.confidence < s.publish.auto_publish_min_confidence {
        return false;
    }
    if clip.coverage.is_some_and(|c| !c.full()) {
        return false;
    }
    let since = now_ms() - 86_400_000;
    for p in enabled_platforms(&s) {
        if e.store.uploads_since(p, since).await.unwrap_or(0) >= s.publish.auto_publish_max_per_day as i64 {
            return false;
        }
    }
    true
}

pub fn library_dir(e: &Engine) -> PathBuf {
    let s = e.settings();
    if s.storage.library_dir.is_empty() {
        e.paths.library_dir_default()
    } else {
        Path::new(&s.storage.library_dir).to_path_buf()
    }
}

/// Writes `<library>/ready-to-post/<clip stem>/` with `video.mp4`, `thumbnail.jpg`, `post.txt`
/// and `post.json`. The video is hard-linked when possible (no extra disk space).
fn write_post_package(
    video: &std::path::Path,
    thumb: &std::path::Path,
    stream_id: &str,
    meta: &pulsecut_core::domain::ClipMetadata,
    duration_ms: i64,
) -> std::io::Result<()> {
    let library = video
        .parent()
        .and_then(|d| d.parent())
        .ok_or_else(|| std::io::Error::other("clip is not inside a library folder"))?;
    let stem = video.file_stem().and_then(|s| s.to_str()).unwrap_or("clip");
    let dir = library.join("ready-to-post").join(stem);
    std::fs::create_dir_all(&dir)?;
    let v = dir.join("video.mp4");
    let _ = std::fs::remove_file(&v);
    if std::fs::hard_link(video, &v).is_err() {
        std::fs::copy(video, &v)?;
    }
    if thumb.exists() {
        std::fs::copy(thumb, dir.join("thumbnail.jpg"))?;
    }
    let channel = stream_id.split_once(':').map(|(_, c)| c).unwrap_or(stream_id);
    let source = if stream_id.starts_with("twitch:") {
        format!("https://www.twitch.tv/{channel}")
    } else {
        stream_id.to_string()
    };
    let tags = meta.hashtags.join(" ");
    let credit = format!("Clip from {channel} ({source})");
    let txt = format!(
        "TITLE:\n{}\n\nCAPTION:\n{}\n\nHASHTAGS:\n{}\n\nDESCRIPTION:\n{}\n\nCREDIT:\n{}\n\nFILES:\nvideo.mp4 (vertical 9:16, {:.1}s)\nthumbnail.jpg\n",
        meta.title,
        meta.caption,
        tags,
        meta.description,
        credit,
        duration_ms as f64 / 1000.0
    );
    std::fs::write(dir.join("post.txt"), txt)?;
    let json = serde_json::json!({
        "title": meta.title, "caption": meta.caption, "description": meta.description,
        "hashtags": meta.hashtags, "credit": credit, "source": source, "stream_id": stream_id,
        "video": "video.mp4", "thumbnail": "thumbnail.jpg", "duration_s": duration_ms as f64 / 1000.0,
        "aspect": "9:16",
    });
    std::fs::write(
        dir.join("post.json"),
        serde_json::to_string_pretty(&json).unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_package_contains_video_thumbnail_and_text() {
        let root = tempfile::tempdir().unwrap();
        let day = root.path().join("2026-01-01");
        std::fs::create_dir_all(&day).unwrap();
        let video = day.join("Big_play_abc.mp4");
        let thumb = day.join("Big_play_abc_cover.jpg");
        std::fs::write(&video, b"mp4").unwrap();
        std::fs::write(&thumb, b"jpg").unwrap();
        let meta = pulsecut_core::domain::ClipMetadata {
            title: "Big play".into(),
            caption: "No way".into(),
            description: "desc".into(),
            hashtags: vec!["#clips".into(), "#shorts".into()],
            generator: "template".into(),
        };
        write_post_package(&video, &thumb, "twitch:somechannel", &meta, 30_000).unwrap();
        let dir = root.path().join("ready-to-post").join("Big_play_abc");
        assert_eq!(std::fs::read(dir.join("video.mp4")).unwrap(), b"mp4");
        assert!(dir.join("thumbnail.jpg").exists());
        let txt = std::fs::read_to_string(dir.join("post.txt")).unwrap();
        assert!(txt.contains("Big play") && txt.contains("#clips #shorts"));
        assert!(txt.contains("https://www.twitch.tv/somechannel"));
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("post.json")).unwrap()).unwrap();
        assert_eq!(json["duration_s"], 30.0);
        // Re-writing the same clip (e.g. after a re-render) must not fail.
        write_post_package(&video, &thumb, "twitch:somechannel", &meta, 30_000).unwrap();
    }

    #[tokio::test]
    async fn output_is_verified_before_a_clip_is_ready() {
        let t = pulsecut_core::process::Tools::discover(&Default::default(), &[]);
        let Some(ffmpeg) = t.ffmpeg.clone() else { return };
        if t.ffprobe.is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty.mp4");
        std::fs::write(&empty, b"").unwrap();
        assert!(verify_output(&t, &empty, 2_000).await.is_err());
        assert!(verify_output(&t, &dir.path().join("missing.mp4"), 2_000).await.is_err());
        let ok = dir.path().join("ok.mp4");
        let st = std::process::Command::new(ffmpeg)
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x240:rate=30:duration=2",
                "-pix_fmt",
                "yuv420p",
                "-y",
            ])
            .arg(&ok)
            .status()
            .unwrap();
        assert!(st.success());
        verify_output(&t, &ok, 2_000).await.unwrap();
        let err = verify_output(&t, &ok, 10_000).await.unwrap_err().to_string();
        assert!(err.contains("incomplete"), "{err}");
    }
}
