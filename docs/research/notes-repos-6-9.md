# Repo audit 6-9: auto-clip / livestream highlight tools

Scope: source inspected via GitHub API trees and raw.githubusercontent.com files. Nothing cloned or run.
Caveat: code excerpts below come from a summarizing fetch of the raw file, not a byte-exact copy. Quoted strings are the ones the fetch returned verbatim, but verify against the raw file before copying anything.

## Summary table

| # | Repo | License (API SPDX) | Lang | Stars | Last push | Releases | Open issues | Verdict |
|---|------|------------------|------|-------|-----------|----------|-------------|---------|
| 6 | ColinGPT9/clips-studio ("Clips Kitty") | AGPL-3.0 | Python (+ Electron UI, TS) | 286 | 2026-11-17 | v2.1.0 (2026-10-21) | 64 (incl. PRs) | reimplement ideas only |
| 7 | kmilo93sd/agent-video-editor | MIT | Markdown skill + HTML | 8 | 2026-10-04 | none | 0 | reuse knowledge and CC0 assets |
| 8 | MarkyADHD/LocalClip | AGPL-3.0 | Python backend, Rust desktop shell | 5 | 2026-11-15 | v1.0.0 to v1.4.0 (latest 2026-11-13) | 0 (16 PRs, all closed) | reimplement ideas only |
| 9 | iamzimozic/streamclip-ai | MIT | Python | 0 | 2026-09-24 | none | 0 | reuse code and ideas (with attribution) |

Important licensing note: 6 and 8 are AGPL-3.0. Copying their code into PulseCut would bind PulseCut to AGPL if distributed. Use them as a spec only. Repo 9 is MIT and can be reused with attribution.

## Repo 6: ColinGPT9/clips-studio ("Clips Kitty")

- **License:** AGPL-3.0 (API). I did not read the LICENSE file directly.
- **Metadata:** created 2026-06-11, 59 forks, default branch `main`, not archived, not a fork. The 64 open issues include PRs. Recent issue themes: "Editor suite" steps (new editor, export compiler, media serving), a PyTorch DLL load failure on Windows, caption/subtitle filter escaping, and feature requests for "better viral detection" and faster processing.
- **Architecture:** top-level dirs are `analysis/`, `config/prompts/`, `connected_accounts/`, `campaigns/`, `llm/providers/`, `longform/`, `publish/`, `remote_render/`, `sdk/python/clipskitty_sdk/`, `sports/`, `tests/`, `ui/` (Electron-style renderer with TSX), `video/`, `video_editor/`, `web/`.
- **Implemented vs README:** the README claims auto-posting, 24/7 operation, and Ollama/Whisper/OpenRouter. `publish/youtube_shorts.py` and `publish/uploadpost.py` exist. I did not confirm the 24/7 scheduler.
- **Highlight detection:**
  - `analysis/highlights.py` is LLM-driven, not signal-driven. `find_highlights` makes one LLM call per chunk. `score_windows` scores signal-peak windows passed in as an `events` list of `(second, description)` pairs. The module does not compute audio, chat, or scene signals itself.
  - Chunking: videos over `long_video_threshold_seconds` (default 1800 s) use `chunk_seconds=1200.0` with `chunk_overlap_seconds=60.0`. `WINDOW_BATCH=8`.
  - Thresholds: `min_score=60`, `max_clips=3`, `min_duration=10.0`, `max_duration=60.0`, `max_overlap=0.4`, `max_text_similarity=0.7`, `max_segment_reuse=0.4`, neutral score 50 for unanswered windows, `LEAD_IN_SECONDS=6.0`, `SENTENCE_REACH_SECONDS=8.0`, `SENTENCE_PAUSE_SECONDS=2.0`.
  - Dedup (`_select_unique` then `_check_against_kept`): reject if timestamp overlap > 0.4, reject if `SequenceMatcher(None, a, b).ratio() > 0.7`, reject if segment reuse > 0.4.
  - Scoring prompt (`config/prompts/score_clips.txt`): "score is 0-100 predicted viral potential", "Be critical: most moments score below 50", "Only truly strong moments score above 80", "Return at most 5 clips", "Each clip must make sense on its own with no extra context", "TRENDING & DRAMA (weight this heavily)". It also returns an `engagement` field (0-100) and a `trending` boolean.
- **Editing pipeline:**
  - `video/captions.py`: ASS output only (`build_captions`, `_header`). Default style: `Arial`, size 84, `#FFFFFF`, bottom, 3 words per caption, uppercase on, highlight colour `#FFE600`. Style line starts `Style: Default,{font},{size},{color},{color},&H00000000,&H7F000000,-1,...` with outline width 7.
  - Word-level highlight without `\k`/`\kf`: each word gets its own Dialogue event, and the active word is recoloured with `\1c`. The docstring says progressive ASS karaoke is avoided because it leaves passed words coloured. `_word_times` uses Whisper word timings when token counts match, otherwise splits the span in proportion to word length.
  - Speaker tint: `second_speaker_color` default `#5CE1FF`, applied via inline `\1c`.
  - Font whitelist (`FONTS`), script fonts (`SCRIPT_FONTS`, e.g. `"hi": "Nirmala UI"`, `"ja": "Yu Gothic UI"`). Style values are scaled from a 1920 px canvas, with a minimum font size of 24.
  - Vertical crop, blurred background, zoom, freeze, and slow-mo: the clips-studio `ffmpeg.py`, `reframing.py`, and `speaker` modules were not located. The `backend/src/media/...` paths I tried returned 404 because they belong to LocalClip, not this repo. Treat the clips-studio render filter graph as unverified.
  - `remote_render/` (queue, service, protocol) exists for offloading renders.
- **Transcription:** the tree has `transcription/transcriber.py` and `transcription/cloud.py`, plus `scripts/fetch_whisper.py`, `tests/test_whisper_gpu_libraries.py`, and `tests/test_cloud_transcription.py`. Whisper variant and parameters not read.
- **Titles and hashtags:** `publish/metadata.py` and `publish/schedule.py` exist. Prompt contents not read.
- **Publishing:**
  - `publish/youtube_shorts.py`: YouTube Data API v3 `videos.insert` with a resumable `MediaFileUpload` in 8 MB chunks. `next_chunk(num_retries=3)`, up to four retries, never retries a 404. OAuth scopes `https://www.googleapis.com/auth/youtube.upload` and `https://www.googleapis.com/auth/youtube.readonly`; `yt-analytics.readonly` is added for interactive auth. Tokens stored per channel as `youtube_token__{channel_id}`. Default privacy `unlisted`. Unaudited projects are forced to private. Quota: 100 `videos.insert` per project per day.
  - Also `publish/uploadpost.py`, `publish/woopsocial.py`, and a Discord webhook integration.
- **Known limitations (from issues):** Windows DLL failures with PyTorch, subtitle filename escaping (PR draft #236), caption position presets requested (#238), and requests for faster processing and better viral detection (#235).
- **Reuse:** reimplement ideas only (AGPL). Good ideas to copy: the ASS per-word `\1c` highlight approach, the 60/80 score calibration, the chunked LLM windowing, and the YouTube resumable-upload retry policy.

## Repo 7: kmilo93sd/agent-video-editor

- **License:** MIT (API).
- **Metadata:** created 2026-10-04, 1 fork, no releases, no open issues. Last push 2026-10-04.
- **What it is:** not a clipper. It is an Agent Skills package (`skills/editing-videos/SKILL.md`) that gives an AI agent editing judgment (pacing, cuts, captions, audio, colour, thumbnails). It uses HyperFrames (HTML-based compositions) as the render target.
- **Implemented:** `skills/editing-videos/references/` (`pacing-and-cuts.md`, `hook-and-retention.md`, `audio-and-sfx.md`, `color.md`, `graphics-and-text.md`, `thumbnail-title-description.md`, `tools.md`, `licenses.md`), `recipes/shorts-from-long-form.md`, `review-checklist.md`.
- **Assets:** 128 CC0 SFX (`library/sfx/*.wav`), 15 music tracks (`library/music/*.mp3`), 26 CC0 graphics (HTML/SVG under `graphics/`), a meme catalog, and `library/tools/synthesize.py`.
- **Highlight detection / FFmpeg / captions / transcription / publishing:** none. No FFmpeg filter graphs, no Whisper, no publishing code. The "shorts" recipe is prose.
- **Limitations:** no code path for PulseCut's core job.
- **Reuse:** reuse knowledge (pacing and hook guidelines are useful for scoring heuristics and reviewer prompts). Reuse CC0 SFX and music only after checking `library/LICENSES.md` for per-file terms. Avoid the HTML/HyperFrames rendering stack.

## Repo 8: MarkyADHD/LocalClip

- **License:** AGPL-3.0 (API). `LICENSE` and `NOTICE` present.
- **Metadata:** created 2026-10-13 as v1.0.0. 5 stars, 1 fork. Releases v1.0.0 through v1.4.0 (latest 2026-11-13). Open issues 0. 16 PRs, all closed; themes are the video editor (multi-track long-form, facecam layouts), Windows export reliability, auto-update, code signing with SignPath, and "clip that" handling.
- **Architecture:** `backend/` (Python, `src/`, Postgres-style migrations, Next.js frontend in `frontend/`, Prisma), `desktop/` (Rust shell), `docker-compose.yml`, `LocalClip.ps1` and `Install-LocalClip.cmd` for Windows.
- **Rust desktop shell (`desktop/src/main.rs`):** uses `tao` (window and event loop) and `wry` (WebView). It runs `LocalClip.ps1 -NoBrowser` hidden through PowerShell, polls `http://localhost:3107`, and loads it. On close it kills the launcher and runs `Close LocalClip.ps1`. Auto-update: `newer_release`, `download_setup`, `start_installer` (silent `LocalClip-Setup.exe`, reopens with `--updated`). This is a useful reference for a Rust desktop shell. No ffmpeg or detection logic in Rust.
- **Highlight detection (`backend/src/ai.py`):**
  - LLM-driven, not signal-driven. Prompt lines (verbatim from the fetch): "You only pick moments that would actually do well posted on their own.", "Every returned segment must be 15-60 seconds long. Prefer 25-50 seconds.", "Be harsh. Most stream moments are 30-55. Reserve 75+ for clips you'd confidently post.", "Quality over quantity: returning one great clip, or none, beats padding the list with weak ones."
  - Windowing: transcripts up to 15 min are a single window. Longer ones use 12-minute windows with a 2-minute overlap. Windows run concurrently (semaphore 5) under a 1800 s total budget.
  - Chat and loudness signals are filtered per window (`_filter_clip_signals_for_window`) and passed as hints. A "clip that" voice command adds a clip covering the ~45 s before the phrase (`add_requested_clips`).
  - Output schema (pydantic): `start_time`, `end_time` (MM:SS), `relevance_score` (0-1), `hook_title` (3-9 words, max 64 chars / 10 words), `hook_start_time` / `hook_end_time` (cold open at most 8 s), `hashtags` (3-5), and `virality` (`hook_score`, `engagement_score`, `value_score`, `shareability_score` each 0-25; `total_score` 0-100; `hook_type` enum).
  - Validation: `_validate_segments` snaps timestamps to transcript lines (`_snap_segment_bounds`), repairs bounds, and replaces model text with transcript text. `dedupe_overlapping_segments` drops overlaps > 50% or candidates within 20 s. `drop_weak_candidates` keeps at least 3 and drops score < 50. `_rescale_virality` keeps subscores proportional.
  - Retries: 3 attempts for HTTP 408/429/500/502/503/504/529 and network errors; backoff starts at 5 s and doubles, with jitter. 600 s overall timeout; rerank timeout 240 s.
  - Providers (`SUPPORTED_LLM_PROVIDERS`): google, openai, openrouter, anthropic, ollama, local (any OpenAI-compatible server), claude-code.
- **Editing pipeline:**
  - `backend/src/media/ffmpeg.py`: trim/atrim/setpts/asetpts per kept range, `xfade` + `acrossfade` between segments (fade length from `crossfade_fade_for_ranges`), fallback `concat=n=...:v=1:a=1`, `subtitles` filter with `fontsdir`, `-ss` before `-i` for fast seek, `loudnorm` via `LOUDNORM_FILTER`, `-movflags +faststart`. Encoders: NVENC `h264_nvenc -preset p1/p5 -tune hq -rc vbr -cq -b:v 0`; libx264 `-crf`, plus `-x264-params keyint=120:min-keyint=30:scenecut=40`. CPU fallback on NVENC failure; `VIDEO_ENCODER=cpu` forces libx264. Audio `aac -ar 48000`.
  - `backend/src/media/reframing.py` (the real vertical logic):
    - Detection: one ffmpeg decode at `fps=3.0`, scaled to 480 px wide, piped as `bgr24`. YuNet first (score `fw*fh*conf`), Haar fallback (`scaleFactor=1.2`, `minNeighbors=3`). Scene cuts from 32x18 thumbnail mean-absolute-difference, threshold `max(14.0, mean + 3.5*std)`.
    - Smoothing (`build_crop_trajectory`): forward/back fill, median filter window 3, deadzone `max(2.0, crop_w*0.05)`, critically damped spring `omega = 2/smooth_time`, `smooth_time=0.9`, speed cap `width*0.4`, final smoothing window 5, keyframe simplification to at most 90 keys, movement gate `max(8, crop_w*0.04)` px.
    - Layout (`build_layout_plan`): "face" vs "fit" shots, sliding window `FACE_RATE_WINDOW` / `FACE_PRESENCE_RATE`, short runs merged (`MIN_LAYOUT_SECONDS`), boundaries snapped to scene cuts (`LAYOUT_SNAP_WINDOW`).
    - Compositor (`build_vertical_compositor_filter`):
      ```
      [bgsrc]scale=540:960:force_original_aspect_ratio=increase,crop=540:960,gblur=sigma=14:steps=2,scale=1080:1920:flags=bilinear,setsar=1[bg];
      [crsrc]{crop_chain}[face];   ... [bg][face]overlay=0:0:enable='...'[t1];
      [ftsrc]scale=1080:1920:force_original_aspect_ratio=decrease:flags=lanczos,... [t1][fit]overlay=(W-w)/2:(H-h)/2:enable='...'[vout]
      ```
      Shot timing uses `enable` expressions of `between(t\,a\,b)` joined by `+`.
    - Moving crop: `crop={crop_w}:{crop_h}:x='{x_expr}':y=0`. `x_expr` is piecewise linear, `if(lt(t\,t1)\,lerp\,next)`, wrapped in `trunc((...)/2)*2`.
    - Ken Burns punch-in (`kenburns_zoom_fragment`) for static face-only clips and narrow sources: `zoom` = `if(isnan(it)\,1\,1+{KENBURNS_ZOOM_DELTA}*min(it/{duration}\,1))`, `x='(iw-iw/zoom)/2'`, `y='(ih-ih/zoom)*0.35'`, `s=1080x1920`, supersampled first.
    - Split mode (`vertical_split`): two tiles at `1080:960 flags=lanczos`, then `[lv][rv]vstack,setsar=1`.
  - `backend/src/media/speaker_framing.py`: speaker-turn tracking. Uses diarized transcript turns (AssemblyAI path), maps speakers to face clusters by mouth motion (`assign_speakers_to_clusters`, `shot_contradicts_mapping`). Constants: `SPEAKER_MIN_TURN_SECONDS=1.2`, `SPEAKER_CUT_LEAD_SECONDS=0.15`, `SPEAKER_ANALYSIS_FPS=10.0`, `WIDE_SHOT_MIN_RATE=0.4`, `MAPPING_MIN_MARGIN=0.1`, `SHOT_CONTRADICTION_MARGIN=0.2`, `MAX_FRAMED_SPEAKERS=4`, `MAX_FACE_CLUSTERS=6`. Hard cuts at speaker turns; smooth pan for single-face shots.
  - `backend/src/media/captions.py`: ASS only, via `build_assemblyai_ass_subtitles` (word timings from AssemblyAI cache). Default style template: `Style: Default,{font_name},{font_px},{primary},&H000000FF,{outline},{back_color},1,0,0,0,100,100,0,0,{border_style},...`, alignment 5. Hook line via `build_hook_title_ass`, top aligned (alignment 8), size ~82% of caption, clamped 34-66 px. Karaoke: no `\k`/`\kf`. Each word gets its own Dialogue event that runs until the next word's start, with the active word coloured via `\c` and an optional box (`\3c`, `\bord`, `\shad0`). Avoids per-word `\fscx` because it reflows the centred line. Single-word `\t` pop on first word only. Chunks of `max_words_per_line` (default 4) placed with `\pos`. Emoji via `\fn{EMOJI_FONT_NAME}` after a probe (`emoji_rendering_supported`). Font size `get_scaled_font_size`: width/560, clamped 26-132 px.
  - Blurred background: yes (see compositor above).
  - Zoom, freeze, slow-mo: Ken Burns punch-in only. No freeze or slow-mo found in the files read.
  - Sound effects: not checked in LocalClip.
- **Transcription (`backend/src/media/transcription.py`):** faster-whisper. Model from `runtime_config.whisper_model`, default `"base"`. CUDA: `device="cuda"`, `compute_type="float16"`, `BatchedInferencePipeline` with `WHISPER_GPU_BATCH_SIZE=16`. CPU: `device="cpu"`, `compute_type="int8"`. `word_timestamps=True`, `vad_filter=True`, `condition_on_previous_text=False`. Audio prepped to mono 16 kHz 64 kbps MP3. Speaker labels only from AssemblyAI (`speaker_labels=True`).
- **Known limitations:** Windows-first (PowerShell launcher). Chat and loudness are hints to the LLM, not computed scores. No freeze/slow-mo.
- **Reuse:** reimplement ideas (AGPL). Strongest reusable ideas: the reframing pipeline (face sampling, spring smoothing, keyframe simplification, face/fit layout plan, blurred-background compositor), the LLM validation layer (snap timestamps to transcript, dedupe, weak-candidate drop), and the Rust WebView shell pattern.

## Repo 9: iamzimozic/streamclip-ai

- **License:** MIT (`LICENSE` read: "MIT License, Copyright (c) 2026 iamzimozic").
- **Metadata:** created 2026-09-24, last push 2026-09-24, 0 stars, 0 forks, 0 open issues, no releases. Size 3282 KB, 43 files. Language Python.
- **Architecture:** `src/pipeline.py` runs the steps. `download`, `transcription/transcribe.py`, `audio/features.py`, `audio/analyze.py`, `audio/laughter.py`, `chat.py`, `detection/fusion.py`, `detection_heuristic.py`, `llm/evaluate.py`, `ranking/rank.py`, `ranking/tournament.py`, `ranking/diversity.py`, `audio/ranking_features.py`, `render.py`, `render_vertical.py`, `render_captions.py`, `captions.py`, `sfx.py`, `metadata.py`, `comedy.py`, `feedback.py`, `ui/server.py`. Comedy-focused. Assets: `assets/fonts/LuckiestGuy-Regular.ttf`, `assets/models/yamnet.tflite`, `yamnet_class_map.csv`.
- **Implemented vs README:** pipeline ran as described. Chat is optional. No publishing integration in the file list.
- **Highlight detection:**
  - Fusion (`detection/fusion.py`, `score_candidates`): base weights `audio 0.40`, `transcript 0.20`, `comedy 0.30`, `laughter 0.10`. Laughter becomes `0.20` when YAMNet features exist. `chat 0.40` is added when a chat signal exists. Combined score = `sum(w*s)/total_weight`, rounded to 3 dp. Total weight is 1.50 with all signals, 1.00 with only base.
  - Per-signal: audio is min-max normalized `strongest_delta`. Transcript is `min(matches/3.0, 1.0)` over reaction words. Comedy is `comedy_cue_score` (raw 0-1). Laughter is `yamnet_laughter_score(features, start, end+2.0)`, otherwise `laughter_energy_score`.
  - Constants: `PRE_ROLL=22.0`, `POST_ROLL=12.0`, `TARGET_WINDOW=35.0`. `find_event_windows`: `min_delta=0.20`, `max_event_gap=6.0`. `build_window`: `start = max(0, center - pre_roll)`, `end = center + post_roll`, end extension capped at `+4.0`. `find_comedy_windows`: `min_comedy=0.30`, `context=1`, punchline at `segment["end"]`. `laughter_peaks`: `threshold=0.3`, `min_gap=20`, `moment = max(ts - 2.0, 0)`.
  - Heuristic (`detection_heuristic.py`): context window is segments `i-2` to `i+2`. Reaction score `min(matches/3.0, 1.0)` over 17-term `REACTION_WORDS` (substring match, so "no" can false-match). Speech activity `min(words_per_sec/4.0, 1.0)`. Combined `0.6*reaction + 0.4*activity`. Candidates with `score < 0.25` dropped.
  - Audio (`audio/analyze.py`): RMS precomputed (not computed here). `delta = round(rms[i] - rms[i-1], 4)`. Ranked by delta descending. No threshold; just ranked.
  - Comedy (`comedy.py`, `comedy_cue_score`): four weighted components, each count divided by a saturation point and capped at 1. Laughter tokens (`haha`, `lol`, `(laugh`, `i'm dead`) weight 0.35, saturate at 2. Banter phrases (`shut up`, `are you serious`, `bozo`) weight 0.30, saturate at 3. Repetition (`Salted! Salted!` style) weight 0.20, saturate at 3. Short punchy lines (≤6 words ending `!`/`?`) weight 0.15, saturate at 3. Final `round(min(sum,1.0),3)`. `laughter_energy_score` is a weak audio proxy: loud (RMS ≥ 0.35) windows with no words nearby (padding 0.25 s, speech ended within 3.0 s).
  - LLM refine (`llm/evaluate.py`) and tournament (`ranking/tournament.py`): see below.
- **LLM tournament (`ranking/tournament.py`, `run_tournament`):** one round-robin over the top `NUM_CLIPS + 2` finalists. Every unordered pair via `itertools.combinations`. Position bias cancelled by alternating order (odd matches `(i,j)`, even `(j,i)`). Prompt asks "Which clip would a general streamer audience find FUNNIER?" and includes "Gameplay advice and calm commentary are not funny." Temperature 0.0. Output `{"reason": "<one sentence>", "funnier": "A"|"B"}`. Winner gets `humor_wins += 1`. Sorted by `humor_wins` descending, ties keep original order. Failed comparisons are skipped. Optional taste examples from `feedback.py` (past kept/rejected clips).
- **Editing pipeline:**
  - `render_vertical.py`, face found (two-panel stack):
    ```
    [0:v]crop={face_w}:{face_h}:{face_x}:{face_y},scale=1080:810,setsar=1{face_effects}[face];
    [0:v]crop={game_w}:{game_h}:{game_x}:{game_y},scale=1080:1110,setsar=1[game];
    [face][game]vstack=inputs=2,subtitles=filename=...:fontsdir=...[v]
    ```
    `face_effects` is optional zoom/shake/tint from `facecam_effects_filter()`.
  - No face: `crop={crop_w}:{SOURCE_HEIGHT}:{(SOURCE_WIDTH - crop_w)//2}:0,scale=1080:1920,setsar=1,subtitles=...[v]`, with `crop_w` = source height × 9/16, rounded even.
  - Face detection: 24 sampled frames, Haar cascade, `dominant_face_box()` picks the most-agreed cluster and takes the median. One static box for the whole clip. No tracking. `gameplay_crop()` shifts horizontally by at most `MAX_GAME_SHIFT` to avoid the facecam.
  - Audio: `loudnorm` to -14 LUFS, `aresample=48000`. SFX cues via `adelay`, `amix` (`normalize=0`, `duration=first`), then `loudnorm`.
  - Blurred background: no. Zoom/freeze/slow-mo: no.
  - SFX (`sfx.py`): LLM picks up to `MAX_CUES = 2` effects per clip (`bamboo_hit`, `blush`, `vine_boom`, `record_scratch`, `crickets`) from `assets/sfx/<name>.{mp3,wav,ogg}`. `snap_to_word_end()` moves cues to word ends. `MIN_SILENCE`: 1.5 s for `crickets`, 0.5 s for `bamboo_hit`. `MIN_CUE_GAP = 2.0`. Temperature 0.2. The LLM cannot hear audio, so the silence checks enforce it.
  - Captions (`render_captions.py`): ASS, `Style: Default,DejaVu Sans,52,&H00FFFFFF,&H00FFFFFF,&H00000000,&H00000000,1,0,0,0,100,100,0,0,1,4,0,2,80,80,180,1`, PlayRes 1920x1080, bottom-centre. One Dialogue event per segment. No override tags (`\k`, `\kf`, `\t`, `\fscx` all absent). No word-level karaoke. Burned with the `subtitles` filter.
- **Transcription (`transcription/transcribe.py`):** faster-whisper, model `"small"`, `device="cpu"`, `compute_type="int8"`, `beam_size=5`, `vad_filter=True`, `word_timestamps=True`. Word start/end rounded to 3 dp, segment times to 2 dp. Output `data/processed/transcript.json` with `source`, `language`, `language_probability`, `segments[].words[]`.
- **Audio features:** YAMNet (`assets/models/yamnet.tflite`) for laughter detection, with an RMS-based fallback.
- **Titles and hashtags (`metadata.py`):** one LLM call per clip at `temperature=0.6`. Prompt key lines: "hook: 2 to 6 words shown on screen in the first seconds", "It MUST mention something specific from THIS clip", "title: a short, curiosity-driven title, under 70 characters.", "description: one or two casual sentences.", "hashtags: 5 relevant hashtags without the # sign." Output JSON `{hook, title, description, hashtags}`. `clean_hook` uppercases and truncates to `MAX_HOOK_CHARS = 32` at a word boundary. Used hooks are passed as "off limits". Output `data/processed/vertical_clips/clip_XX.txt` is a copy-paste template. The 70-char title limit is prompt-only, not enforced.
- **Publishing:** none.
- **Known limitations:** face position is one static box, so it breaks when the streamer moves. Haar-only detection. Transcript reaction vocabulary is substring-matched. Laughter-energy proxy is noisy (gunfire triggers it). Title length not enforced. Ships only Windows-friendly fonts. Only 0 stars, so untested in the wild.
- **Reuse:** reuse code and ideas (MIT). Best pieces: the fusion-with-renormalized-weights scheme, the pairwise-tournament ranking with position-bias cancellation, the SFX silence-gap check, and the ASS style line. Replace the Haar-static-box with the reframing approach from LocalClip (re-implemented).

## Repo 7 and 8 coverage note

Repo 8's `LICENSE`, `NOTICE`, and `CLAUDE.md` were not fetched; license is from the API. Repo 6's LICENSE was not fetched either. Use the API SPDX until the files are checked.

## Additional candidates (permissive license, from GitHub search)

Search queries: `twitch highlight language:rust` (sorted by stars) and `auto clip ffmpeg captions vertical license:mit`.

| Repo | Lang | License | Stars | Last push | What it does | Reuse |
|------|------|---------|-------|-----------|--------------|-------|
| hougesen/twitch-highlight-finder | Rust | MIT | 9 | 2024-05-10 | Finds Twitch highlights from chat message frequency and emotes | Reuse code (MIT). Directly relevant to Rust chat-spike detection. Stale. |
| nirvagold/stream-clipper | Rust (Tauri + Svelte) | MIT | 5 | 2025-12-31 | Desktop app, highlights from audio and chat analysis | Reuse code (MIT). Closest Rust match. Read before relying on it. |
| huntermixhunter/autocut | Python | MIT | 0 | 2026-10-31 | Local FFmpeg + faster-whisper vertical captioned clips | Reference only. |
| xniperbuilds/ripclips | Python | MIT | 0 | 2026-07-15 | yt-dlp + FFmpeg vertical clips with face tracking | Reference only. Check tracking approach. |
| satsushis/Auto-YT-Clipper | JavaScript | MIT | 2 | 2026-08-25 | Captioned vertical clips with AI moment selection and uploads | Reference only. |

Nothing in the Rust results (other than the two above) exceeds 10 stars. Neither Rust repo has been checked at the source level here.

## Reuse recommendations (summary)

- Repo 6 clips-studio (AGPL-3.0): reimplement ideas. Per-word `\1c` ASS highlight, 60/80 score calibration, YouTube resumable upload with retry policy.
- Repo 7 agent-video-editor (MIT): reuse pacing and hook guidance as heuristic notes. Reuse CC0 SFX/music after checking `library/LICENSES.md`. Avoid HyperFrames rendering.
- Repo 8 LocalClip (AGPL-3.0): reimplement ideas. Reframing pipeline (sampling, spring smoothing, face/fit layout, blurred background), LLM validation and dedupe, Rust WebView shell pattern.
- Repo 9 streamclip-ai (MIT): reuse code and ideas with attribution. Fusion weights, tournament ranking, SFX silence gating, ASS style line.
- Rust candidates: `hougesen/twitch-highlight-finder` and `nirvagold/stream-clipper` (both MIT) for chat-spike and audio highlight ideas.
