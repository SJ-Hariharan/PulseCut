# PulseCut research: repos 1-5 (livestream auto-clippers)

Audit date: 2026-11-17. Method: GitHub API (repo, trees, issues, releases), raw.githubusercontent.com source files, READMEs. Nothing cloned or run.

Caveat: source was read through a summarizing fetch, so quoted constants and formulas are close to the source but not guaranteed byte-exact. Re-check each formula against the raw file before porting.

## Summary table

| # | Repo | License (API spdx) | Lang | Stars | Open issues | Last push | Verdict |
|---|------|---------|------|-------|-------------|-----------|---------|
| 1 | utkualsir/sharkyclips | MIT | Python (+React UI) | 7 | 1 | 2026-11-16 | Reimplement algorithms (best signal design) |
| 2 | Mayukh135/peakcut (fork of haziqahmedqasmi/PeakCut) | MIT | Python | fork 0 / parent 694 | 0 / parent 14 | fork 2026-11-14 / parent 2026-11-17 | Reimplement LLM-selection prompts and caption logic |
| 3 | darryl-17/dawassi-ai-clipper | MIT | JS/Node (repo tagged HTML) | 2 | 0 | 2026-07-18 | Reimplement ffmpeg recipes; avoid app |
| 4 | bendawg2010/Auto-clipper | MIT | Python | 11 | 0 | 2026-04-19 | Reimplement audio/chat formulas; avoid YOLO stack (AGPL dep) |
| 5 | nirvagold/stream-clipper | MIT | Rust (Tauri 2) + Svelte | 5 | 0 | 2025-12-31 | Reimplement RMS-percentile + merge; avoid Pro/license code |

## 1. utkualsir/sharkyclips

- **License:** MIT. Repo is 41 MB and has no fork flag. Release v1.0.0 is named "ClipForge 1.0.0" (2026-10-24), which does not match the repo name. Verify this before relying on it.
- **Deps (backend/requirements.txt):** faster-whisper, yt-dlp, numpy, scipy, librosa, sherpa-onnx (laughter tagger), mlx-whisper (macOS arm64), fastapi, uvicorn, opencv-python-headless, rapidocr (HUD OCR), playwright (TikTok upload), matplotlib, psutil, rich, wordfreq, pystray.
- **Issues:** 1 open, #1 "Spanish language support and better AI gameplay context" (2026-11-16). It asks for Warzone-style kill/win event detection.
- **Architecture:** Local-first. VOD/chat download via yt-dlp and chat-replay parsing, then faster-whisper transcription. Signals are computed per second and fused into a "hype" curve, and candidates are cut from it. No OBS websocket; OBS is watched as a recording folder. Other modules include game HUD OCR, multi-platform publishing, a web app, and a voice-dubbing path. Much of this is outside PulseCut's scope.
- **Modes (highlights/modes.py):**
  - gaming weights: loudness 0.30, laughter 0.25, keywords 0.25, speech 0.20, chat 0.30, game 0.6; agreement_bonus 0.15; smooth_sigma 2.0; clip 15-60 s; pre-roll 6-10 s; post-roll 3 s; min_gap 45 s; boundary "reaction".
  - study weights: llm 0.55, keywords 0.20, steady_speech 0.15, speech 0.05, loudness 0.02, laughter 0.01, chat 0.05; bonus 0.10; sigma 5.0; clip 30-60 s; pre 1-4 s; post 1 s; min_gap 60 s; boundary "sentence".
  - music weights: music 0.80, loudness 0.12, laughter 0.08, chat 0.10; bonus 0.05; sigma 3.0; clip 15-30 s; min_gap 30 s; boundary "music".

### Detection algorithms

**Fusion (highlights/scorer.py, `compute_hype`).**
- Keep only weights > 0. Signals that fail or are missing are skipped, and failures do not abort the run. Each signal's score is clipped to [0,1].
- `share_i = w_i / Σ w_j` over signals that ran.
- `combined = Σ share_i * s_i(t)`.
- `agreeing(t) = #{ i : s_i(t) >= 0.5 and share_i >= 0.08 }`.
- `combined *= 1 + agreement_bonus * max(0, agreeing - 1)`, then clip to [0,1].
- If n > 3, apply a Gaussian filter with sigma = `smooth_sigma`.

**Loudness (signals/loudness.py).**
- Frame grid: 0.5 s non-overlapping frames (FPS = 2). Per frame, `dB = 10*log10(mean(x^2) + 1e-10)`.
- Silence: frames with dB <= -55 are excluded from the baseline. Missing values are filled with the median of voiced frames, or -40 if none.
- Baseline: rolling median over 121 frames (60 s), mode "nearest". `excess = dB - baseline`.
- Jump: `jump = dB - mean(dB over previous 6 frames)`, computed with a cumsum. The first frame uses its own value.
- Frame score: `0.6*clip((excess-3)/12, 0, 1) + 0.4*clip((jump-4)/10, 0, 1)`. Frames at or below -55 dB are forced to 0.
- Events: local peaks >= 0.45, min spacing 5 frames, and only if excess >= 3 dB. Label `loudness_spike`.

**Chat (signals/chat.py).**
- Requires at least 30 messages. Each message's timestamp is shifted by `chat_delay` (default 5 s). Messages are counted into 1 s bins.
- `rate5 = uniform_filter1d(rate, 5)`. `baseline = rolling_median(rate5, 121)`, with floor `max(baseline, 0.4*mean(rate))`. `ratio = rate5/baseline`.
- Spike term A: `log2(ratio)/log2(6)`, clipped to [0,1].
- Spike term B: `z = (count10 - baseline*10)/sqrt(baseline*10 + 1)`, where `count10` is the 10 s count. Term B is `clip(z/5)`.
- `spike = min(A, B)`. The min keeps sparse chats from scoring high.
- Reactions: case-insensitive regex from `triggers/chat_reactions.json`. Entries are whole-message or contains-type. Pre-normalization collapses runs of 3+ identical characters to 2.
- Copy-paste waves: the first 40 normalized chars posted by >= 3 distinct authors within 5 s count as reactions. Pure 1-2 digit numbers are excluded as poll votes.
- `burst = min(clip(react5/(baseline*1.5)), clip(react5*5/4))`.
- `chat_score = clip(0.65*spike + 0.35*burst)`. Peaks at >= 0.4 with spacing 20 samples. Event value is `f"{ratio:.1f}x"`.

**Laughter (signals/laughter.py).**
- CED-mini AudioSet tagger (527 classes) through sherpa-onnx on CPU. Windows are 2 s, non-overlapping. Windows under 0.5 s or with peak < 1e-3 are skipped.
- Groups: laughter, shouting, and cheering. Each group takes the max probability over its labels.
- Scale: laughter x0.45, shouting x0.45, cheering x0.5 then x0.8. Clip to [0,1]. Per second, take the max across groups.
- Peaks at >= 0.5 with spacing 4.

**Keywords (signals/keywords.py).**
- Normalize: casefold, strip accents and punctuation, unify Arabic letter variants, collapse runs of 3+ letters. CJK keeps two repeats.
- Phrases: JSON lists per language. `en.json` has "reaction" (25 phrases, weights 0.3-1.0, e.g. "no way" 1.0, "let's go" 1.0, "lol" 0.5) and "study" (17 phrases, 0.5-1.0, e.g. "important" 1.0, "common mistake" 1.0, "for example" 0.5). Custom phrases default to weight 1.0.
- Matching: token-sequence match for spaced languages. Substring `str.find` for zh/ja/th.
- Scoring: a per-second impulse of summed weights, Gaussian-smoothed with sigma 1.5 and scaled by `sqrt(2π)*1.5`. Then `score = 1 - exp(-1.5*spread)`.

**Candidates (highlights/candidates.py).**
- Peaks: `find_peaks(curve, distance=max(1, int(min_gap/3)), prominence=0.01)` with threshold `floor + 0.1*(max - floor)`.
- Window: expand left and right while the curve stays above half the peak value. Then apply the mode's pre-roll, post-roll, and length limits.
- Length filter: drop if `e - s < min(clip_min*0.5, duration*0.5)`.
- Overlap (NMS): reject if overlapping an existing clip with a 1.0 s buffer.
- Game events: `fit_spans` adds 4.0 s build-up and 2.0 s after. Events with strength >= 0.8 (`MUST_HAVE_STRENGTH`) are always kept.
- Word snapping: start at word_start - 0.25 s, end at word_end + 0.35 s. Mode windows: sentence 8/3 and 4/8 s; music 2 s each side; reaction 3/2 and 3/4 s.
- Default clip count: `clip(round(duration/240), 3, 40)`.

**Rendering (clips.py, `_filter_graph`).** Vertical is built as one `-filter_complex` (no `-vf`):
- Fill crop: `scale={W}:{H}:force_original_aspect_ratio=increase:flags=bicubic,crop={W}:{H}`
- Blurred background: `boxblur=12:2,eq=brightness=-0.10:saturation=1.1`, with foreground `scale={W}:{H}:force_original_aspect_ratio=decrease:flags=bicubic` then `overlay=(W-w)/2:(H-h)/2,setsar=1`.
- Split layout: facecam `crop=...,scale={OUT_W}:{SPLIT_TOP}:flags=bicubic`, then `vstack=inputs=2,setsar=1`.
- Captions: word-level timestamps are written to `subs.ass` and burned in with `[base]ass=subs.ass,format=yuv420p[v]`. Bold style gives one event per word and highlights the spoken word.
- Encoders: NVENC `-cq 30 -preset p5 -tune hq`; libx264 `-preset veryfast -crf 23`.

**Titles.** `titles/` uses banks, game-specific lexicons (cs2, lol, valorant, delta_force), and a style module, with optional local LLM. Not analyzed in depth.

**Limitations seen.** Heavy surface area (ingest, dubbing, TikTok browser automation, multi-tenant web app). Keyword match is literal. Most game detection depends on OCR presets for specific titles. Speech and chat timing rely on a fixed 5 s chat delay.

## 2. Mayukh135/peakcut (fork) of haziqahmedqasmi/PeakCut (parent)

- **Fork status:** The fork has 0 stars, 0 issues, no releases, and the same file tree as the parent. The parent is the real project, with 694 stars, 14 open issues, MIT license, and last push 2026-11-17. I audited the parent's code.
- **License:** MIT (both). LICENSE file is 1077 bytes.
- **Deps (requirements.txt):** fastapi 0.162.5, httpx, numpy 2.2.6, psycopg 3.3.9, SQLAlchemy 2.0.57, sherpa-onnx 2.0.1 (ASR), soundfile, yt-dlp 2026.10.21, pillow 11.3.0, uvicorn. Docker, Caddy/Traefik/nginx deploy files.
- **Architecture:** Multi-tenant web backend (studio/). Files of note: `brain.py` (clip selection), `live.py` (live buffer), `asr_worker.py`, `transcribe.py`, `render.py`, `ai.py`, `accounts.py`, `jobs.py`.
- **Capture:** Live mode keeps a rolling buffer. Clips are cut while the stream is still running. Live detection uses loudness, chat activity, and transcript (per README). Chat code path not fully read.

### Detection (studio/brain.py, LLM-driven)

- Windows: 15 min, 60 s overlap (`WINDOW_SEC, OVERLAP_SEC = 15*60, 60`). Windows with < 3 caption lines are skipped.
- Clips per window: `max(2, min(8, ceil(max_clips*1.5/len(windows)) + 1))`.
- Loudness: z-scored loudness. A line is "loud" if `seg.max() > 1.5`. Returns zeros if < 5 samples.
- Caption grouping: new line on gap > 0.7 s, duration > 10 s, or sentence end after >= 4 words.
- Boundary snap: clip start snaps to a caption line within 4 s. Sentence end = pause of 1.0 s (unpunctuated) or 0.5 s. Tail of 0.35 s after last word.
- Overlap reject: `min(e, k.end) - max(s, k.start) > 0.3*(e - s)`.
- Prompt: "You are a clip scout for short-form gaming and streaming videos." Score 0-100 with "most clips 50-80". Hook <= 5 words, all caps. Length 15-60 s by default. Returns a JSON array with start, end, score, hook, title, description, hashtags, reason.
- Live mode: "Most checks should return []." Transcript judge uses `n=2` and a `must` flag. `MUST_PICK`: "return the single best moment ... even if it is only decent." Signal judge reads 2 s slices (`range(0, len(z), 2)`), up to 3 moments.
- Retries: `ask_ai` tries each model from `ai.models()` up to 3 times. On 429 or 5xx it sleeps `4*(attempt+1)` s. Other errors move to the next model.
- Fallback without AI: `fallback_by_loudness` picks the loudest speech stretches and assigns score 50.
- Languages: `KEEP_LANGS = ("en","hi","ur")` are kept as spoken. Hindi and Urdu are romanized. Other languages use YouTube English captions or AI. A caption fix is rejected unless `_same_word` similarity >= 0.45.

**Limitations.** Clip quality depends on the LLM. Fallback is loudness only. The fork has no code of its own. Heavy stack for a Rust rewrite.

**Reusable assets.** Fonts in studio/fonts: Anton, Archivo Black, Poppins ExtraBold, all SIL OFL with license files included. Fonts can be reused with OFL notices.

## 3. darryl-17/dawassi-ai-clipper

- **License:** MIT. Release v2.0.11 (2026-07-18). Three releases published the same day (v2.0.9-v2.0.11).
- **Deps (package.json):** only `express ^5.2.1`. Script `node server.js`. Runs ffmpeg and yt-dlp as external processes.
- **Architecture:** Node/Express server.js (63 KB), single-file UI in public/app.html (128 KB) and templates. No Python.
- **Capture:** Live recorder watches Twitch/YouTube streams via yt-dlp with reconnect flags. Segments are written by the ffmpeg segment muxer. There is no OBS websocket or replay buffer. README claims a rolling buffer of up to 10 h, with up to 5 streams at once.

### ffmpeg and yt-dlp commands (verbatim where short)

- Live recorder: `-reconnect 1 -reconnect_streamed 1 -reconnect_delay_max 10 ... -map 0 -c copy -f segment -segment_time <SEG> -reset_timestamps 1 -segment_format mpegts seg_%08d.ts`
- Clip concat: `-fflags +genpts -f concat -safe 0 -i list.txt -map 0:v -map 0:a -c:v libx264 -preset veryfast -crf 20 -c:a aac -b:a 160k -movflags +faststart`
- Vertical 9:16: `-vf crop=min(iw\,ih*9/16):ih,scale=1080:1920:flags=lanczos,setsar=1`
- Auto-shorts crop: `crop='min(iw,ih*9/16)':'min(ih,iw/9*16)':'(iw-ow)/2':'(ih-oh)/2',scale=1080:1920:flags=lanczos,setsar=1`
- Loudness: `-af ebur128=metadata=1 -f null -`. Per-second peaks: `-af astats=metadata=1:reset=48000,ametadata=print:key=lavfi.astats.Overall.Peak_level:file=-`
- Speech enhancement: `highpass=f=90,afftdn=nf=-25,acompressor=threshold=-18dB:ratio=3:attack=20:release=250,loudnorm=I=-16:TP=-1.5:LRA=11`
- VOD section: yt-dlp `--download-sections *<s>-<e> -f bv*[height<=?1080]+ba/b[height<=?1080]/b --merge-output-format mp4 --force-keyframes-at-cuts`
- Thumbnails: yt-dlp `--skip-download --write-thumbnail --convert-thumbnails jpg`. Twitch listing: `--flat-playlist --dump-json --playlist-end N` on `twitch.tv/<ch>/videos?filter=archives&sort=time`.

### Detection

- **Live hype (every 15 s per stream):** Measure mean volume of up to 5 new segments, skipping the newest. Ignore reads below -70 dB. Keep last 150 readings. Baseline is their median. Spike when >= 12 readings exist and segment > baseline + 6 dB.
- **Auto-shorts:** Clip length 30 s, with >= 40 s between peaks. Shorts count clamped to [1, 6] and to `floor(duration/30)`. Videos <= 34 s yield one short. Otherwise sort loudness peaks loudest first. Skip peaks in the first 2 s or last 4 s. If nothing is found, space shorts evenly. Window starts 8 s before peak, clamped to bounds.

**Limitations.** Loudness-only detection, no chat, no transcript. Explicit shell-style command building. Test coverage is one smoke file (`tests/test_smoke.py`).

## 4. bendawg2010/Auto-clipper

- **Default branch:** `claude/twitch-clip-analyzer-MPT08`, not `main`. Audit used this branch.
- **License:** MIT. Last push 2026-04-19. Release v0.13.0 (2026-04-19), v0.12.0 (2026-04-18).
- **Deps (requirements.txt):** flask, yt-dlp, opencv-python-headless, numpy, Pillow, pywebview, requests, ultralytics, tqdm, openai-whisper, faster-whisper.
- **Architecture:** Python. `analysis/` has `detector.py`, `audio_detector.py`, `chat_detector.py`, `hybrid_detector.py`, `clip_trigger_detector.py` (Whisper "clip it" voice triggers), `motion_detector.py`, `scene_detector.py`, `yolo_local_analyzer.py`, `roboflow_*`, `ai_analyzer.py`, `game_profiles.py` (201 KB of per-game profiles), `arc_clip_detector.py` (77 KB).
- **Models:** `models/best.pt` (5.4 MB) is a YOLOv11n trained on Arc Raiders classes. Weights license is not stated.
- **Capture:** VOD download via yt-dlp, not live. Analysis runs on a recording with OpenCV, then clips are cut with FFmpeg. Voice triggers scan recorded audio, not a live mic.

### Detection

**Audio (audio_detector.py).**
- No RMS. ffmpeg `astats` gives per-second peak levels (`reset=48000`), bucketed by `int(current_time)` with max kept.
- Score: `0` if `db < threshold_db`, else `clip((db - threshold_db)/max(ceiling_db - threshold_db, 1), 0, 1)`.
- Defaults: `audio_threshold_db = -8`, `audio_ceiling_db = -1`, `intensity_threshold = 0.35`, `fallback_threshold_ratio = 0.50`. `min_spike_gap = 1.0` is defined but never used.
- Labels by dB: >= -3 "Loud Combat", >= -6 "Gunfire/Explosion", >= -10 "Combat Audio", >= threshold "Ambient Activity", else "Ambient".
- Fallback: if no second reaches `intensity_threshold`, take top 3 scores at >= `intensity_threshold * 0.5`, excluding Ambient labels.
- Merge: seconds within `merge_gap` (default 8 s) join one cluster.
- Clip: `dur = clip(raw + extension, min_dur, max_dur)` with defaults 20, 60, 10. `pre_pad = 8`. `confidence = min(avg_score*1.5, 1)`.

**Chat (chat_detector.py, `ChatSpikeDetector`).**
- 5 s buckets: `bucket = (offset // 5) * 5`.
- `avg` and `std` over bucket counts. `thr = avg + std*(2.0 - 2*intensity)`, with intensity 0.35 giving multiplier 1.3. Then `thr = max(thr, 1.5*avg)`. The code comment says "mean + 1.5 std", which does not match the formula.
- `score = min(count/max_msgs, 1)`. Labels: count >= 4*avg "Chat Explosion", >= 2.5*avg "Chat Spike", else "Chat Active".
- Merge gap 8 s, clip duration as above. Fetches chat with urlopen timeout 15, `max_pages = 200`.
- No emote or keyword lists, no cooldown in this file. Profile values come from `game_profiles.py`.

**Hybrid (hybrid_detector.py).** Audio, motion, and scene only. No chat, which contradicts the README's "Hybrid" description.
- Per frame: `max_score = max(audio, motion, scene)`. Count signals >= 0.2. If 3 active, `fused = min(max*1.3, 1)`. If 2 active, `min(max*1.15, 1)`. Else max.
- Motion: `min(motion_energy*8 + diff_std*4 + accel, 1)`, with `accel = min((motion_energy - prev)*5, 0.3)`.
- Scene: `min(color_shift*3 + chi_shift*2 + flash, 1)`, with `flash = min(bd*5, 0.5)` only if `bd > 0.08`.
- Labels: fused < 0.2 "Idle". Otherwise by largest signal: "Loud Combat" / "Combat Audio" (audio, threshold 0.6), "Intense Action" / "Active Movement" (motion), "Visual Disruption" / "Scene Shift" (scene).
- Window: `window_frames = max(1, int(3*sample_fps))` (3 s, sample_fps 4). `blended = 0.5*avg + 0.5*peak`. Keep >= `intensity_threshold` (0.35).
- Fallback: `thr * 0.15`. Candidates capped at `max(5, min_expected*2)`, with `min_expected = max(1, duration/600)`.

**Limitations.** Chat not in hybrid. YOLO is game-specific (Arc Raiders). Dead-ish branch. Heavy dependency set. Exact `min_spike_gap` bug. Clip windows are fixed-length padding rather than speech-aware.

**License risk.** `ultralytics` is AGPL-3.0. Shipping or linking it may impose AGPL obligations. Avoid it in the Rust rewrite.

## 5. nirvagold/stream-clipper

- **License:** MIT (LICENSE 1068 bytes). Release v0.1.0 (2025-12-31) is the only release. Last push 2025-12-31, about 10.5 months stale. `updated_at` is 2026-11-16, so a non-code change happened recently.
- **Stack:** Tauri 2 + Rust + Svelte. Cargo deps: tauri 2, tauri-plugin-opener/dialog/fs, serde, `hound 3.5` (WAV reading), `webrtc-vad 0.4`, tokio, rayon 1.10, thiserror, anyhow, chrono, dirs, regex, uuid, hostname, whoami, `rand 0.9.2`.
- **Bundled binary:** `src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe` (99 MB) is committed. Check the ffmpeg build license before reuse.
- **Architecture:** File-based. No live capture. Import a local video (MP4, MKV, MOV, AVI, WebM, FLV, TS). Chat logs are imported as files (Twitch JSON, YouTube JSON, TXT).
- **Repo contents:** `src-tauri/src/pro/` (`_c.rs`, `_v.rs`, `mod.rs`) and `src-tauri/src/bin/keygen.rs` (a key generator) are in the repo. The license validator in `license/validator.rs` is fully offline, with checksum and XOR-keystream local storage. It is a client-side gate, and the author's own summary says it can be bypassed.

### Detection

**Audio (audio/analyzer.rs).**
- Non-overlapping chunks of `chunk_duration` seconds. `rms = sqrt(sum(s^2)/n)/32768`. `peak = max|s|/32768`.
- Percentile threshold by sensitivity: <=1.0 gives p75, <=2.0 gives p82, <=3.0 gives p90, else p95.
- Fallback: if `threshold <= baseline_rms*1.15`, set `threshold = baseline_rms*(1 + 0.35*sensitivity)`.
- Per chunk: `above = rms > threshold`. `sudden = delta > avg_delta*1.5`. `crest = peak/rms` (1 if rms <= 0.001). `transient = crest > 2.0`. `sustained = next.rms > baseline*1.2`. `well_above = rms > baseline + 1.5*std`. `voice = has_voice || voice_ratio > 0.2`.
- Valid if `above && (voice || criteria_met >= 3)`.
- Quality points: 20, 15, 10, 15, 10, and 30 for voice. Keep if quality >= 40 and duration >= `min_duration`. Merge adjacent if `gap <= merge_gap`.
- Score: `intensity` clamped to [0, 40]. Quality bonus `max(quality-40, 0)*0.6`. Voice bonus `voice_ratio*20`. Total clamped to [0,100].

**Chat (chat/analyzer.rs).**
- Contiguous windows of `window_size` seconds. `baseline_rate = median(window message counts)`. `threshold = baseline*rate_multiplier`.
- Metrics per window: emote density `emote_count/message_count`, caps ratio (uppercase/alphabetic), keyword matches (case-insensitive substring, once per keyword per message).
- Spike if `message_count > threshold` OR `keyword_matches >= keyword_threshold` OR `emote_density > emote_threshold`. Merge adjacent spikes with gap < 0.1 s.
- Score: `rate_score = clamp((peak_rate/baseline - 1)*30, 0, 70)`, or 35 if baseline is 0. `keyword_score = min(matches*10, 30)`. `score = min(rate+keyword, 100)`.

**Combo (highlight/scorer.rs).**
- `combined = (audio.score*audio_weight + chat.score*chat_weight) * combo_bonus`, capped at 100. Span is the union of both intervals.
- The UI text "+50% bonus" is hardcoded, not derived from `combo_bonus`.
- Audio-only: `audio.score*audio_weight`. Voice flag if audio score > 60.
- Overlap: `start1 < end2 && start2 < end1`.
- Free tier (`max_clips`): **shuffle with a time-seeded RNG, truncate, then re-sort by time**. Clips are picked randomly, not by score. This is likely a bug.

**Merge (highlight/merger.rs).** Merge if `overlap_ratio > threshold`, where `overlap_ratio = overlap / min(d1, d2)`. Merged end is `max(ends)`. Higher score wins. Padding: `start = max(0, start-pad_before)`, `end = min(duration, end+pad_after)`.

**Clip cut (video/clipper.rs).**
- Cut: `-ss <s:.3f> -i <src> -t <d:.3f>`. Always re-encodes.
- MP4: `-c:v libx264 -preset fast -crf 23 -c:a aac -b:a 128k`.
- WebM (Pro): `-c:v libvpx-vp9 -crf 30 -b:v 0 -c:a libopus -b:a 128k`.
- Scale: `scale=-2:<h>`. Free tier watermark: `drawtext=text='Stream Clipper':fontsize=24:fontcolor=white@0.7:x=w-tw-20:y=h-th-20`.
- No 9:16 crop or captions were seen in the excerpt read. README's "9:16 is Pro" is not verified.

**Limitations.** Chat is Pro-gated. Free tier is 5 clips per video, 720p, watermark. Last release is 10 months old. No captions. The code has no tests in the excerpts read. Windows binary committed.

## Reuse recommendations for the Rust rewrite

| Repo | Recommendation | Reason |
|------|----------------|--------|
| sharkyclips | Reimplement the fusion design, loudness, chat-spike, keyword, and candidate logic (sections above) | MIT, strongest algorithm set. Port the formulas, not the codebase. |
| PeakCut (parent) | Reimplement caption grouping, 15-min windowing with 60 s overlap, overlap rejection, and LLM prompt structure. Reuse OFL fonts (Anton, Archivo Black, Poppins) with license notices | MIT, but backend is heavy. Prompts are the valuable part. |
| dawassi | Reimplement ffmpeg recipes (segment recorder, vertical crop, speech chain, live hype 6 dB rule) | MIT, but the app itself is JS/Express with no reusable core. |
| bendawg | Reimplement audio astats threshold and merge logic and chat 5 s bucket spike rule. Avoid YOLO | MIT, but `ultralytics` is AGPL-3.0 and the model weights have no stated license. |
| stream-clipper | Reimplement RMS percentile threshold, merge logic, and ffmpeg cut command. Do not copy `pro/`, `keygen.rs`, or `license/`. Do not ship its bundled ffmpeg .exe without checking | MIT, Rust-native and closest to PulseCut's stack, but stale and its gating code is not worth reusing. |

Overall: all five are MIT, so the algorithms are reusable in principle. The highest-value logic is in sharkyclips (multi-signal fusion with agreement bonus and keyword/loudness/chat scoring) and in stream-clipper (Rust RMS percentile + merge). Avoid the AGPL dependency (ultralytics) and the Pro gating code.

## Open questions for follow-up

1. Verify sharkyclips release name "ClipForge 1.0.0" against the repo.
2. Pull the raw sharkyclips `signals/base.py` and `context.py` to confirm the RMS hop and `ctx.rms_db` source.
3. Check the sharkyclips mode weights against the raw `modes.py` (the `game` weight of 0.6 looked unusual).
4. Confirm the bendawg `clip_trigger_detector.py` voice-trigger phrase list.
