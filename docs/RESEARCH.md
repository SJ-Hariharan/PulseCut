# PulseCut Research Notes

Audit date: 2026-11-17. This document consolidates the research that informed PulseCut's design: nine reference projects, five additional candidates, and eight platform or tool surfaces. Raw notes live in `research/notes-repos-1-5.md`, `research/notes-repos-6-9.md` and `research/notes-platforms.md`. Licensing decisions are tabulated in `REUSE_MATRIX.md`.

## 1. Method

**What was inspected.** Nine open-source livestream/auto-clipping projects (see section 2), a GitHub search for additional candidates, and the official documentation or package listings for OBS WebSocket, Twitch chat, YouTube Live chat and upload, Kick, Instagram Reels, TikTok, whisper.cpp and FFmpeg.

**How.** The GitHub API (repo metadata, trees, issues, releases), raw source files from raw.githubusercontent.com, READMEs, official platform docs and crates.io/docs.rs pages.

**Caveats, stated plainly:**

- No repository was cloned, built or run. All code was read through fetched pages.
- The fetch tool summarized pages. Quoted constants, thresholds and formulas are close to the source but not guaranteed byte-exact. Every formula was re-implemented from its description, not copied, and must not be assumed identical to the original.
- Licenses come from the GitHub API SPDX field. For clips-studio and LocalClip the LICENSE file itself was not fetched; for LocalClip the `NOTICE` and `CLAUDE.md` files were not fetched either.
- Platform claims carry the notes' own tags: **[V]** verified against official docs fetched on the audit date, **[M]** from memory or third-party sources and not verified. Those tags are preserved below where they matter.
- Synthetic-data caveat for PulseCut itself: detection weights were tuned on synthetic labelled data only (see section 6). Nothing here is validated on real streams.

## 2. Reference projects

Summary:

| # | Project | License | Language | Stars | Last push | Role for PulseCut |
|---|---------|---------|----------|-------|-----------|-------------------|
| 1 | utkualsir/sharkyclips | MIT | Python + React | 7 | 2026-11-16 | Best signal design; ideas re-implemented |
| 2 | haziqahmedqasmi/PeakCut (audited via fork Mayukh135/peakcut) | MIT | Python | parent 694 | 2026-11-17 | Caption and LLM-window ideas |
| 3 | darryl-17/dawassi-ai-clipper | MIT | Node/Express | 2 | 2026-07-18 | FFmpeg recipes (ideas) |
| 4 | bendawg2010/Auto-clipper | MIT | Python | 11 | 2026-04-19 | Audio/chat formulas (ideas) |
| 5 | nirvagold/stream-clipper | MIT | Rust (Tauri 2) + Svelte | 5 | 2025-12-31 | Merge rule, Rust reference |
| 6 | ColinGPT9/clips-studio | AGPL-3.0 | Python + Electron UI | 286 | 2026-11-17 | Ideas only |
| 7 | kmilo93sd/agent-video-editor | MIT | Markdown skill + HTML | 8 | 2026-10-04 | Editing knowledge |
| 8 | MarkyADHD/LocalClip | AGPL-3.0 | Python + Rust shell | 5 | 2026-11-15 | Ideas only |
| 9 | iamzimozic/streamclip-ai | MIT | Python | 0 | 2026-09-24 | Fusion/tournament ideas |

No code was copied from any of these repositories. The two AGPL projects were studied for ideas only.

### 2.1 utkualsir/sharkyclips (MIT)

- **Architecture.** Local-first. VOD and chat are downloaded with yt-dlp, transcribed with faster-whisper, and per-second signals are fused into a "hype" curve from which candidates are cut. OBS is only watched as a recording folder, with no WebSocket integration. Surrounding modules (game HUD OCR, TikTok browser upload via playwright, web app, voice dubbing) are far outside PulseCut's scope.
- **Deps.** faster-whisper, yt-dlp, numpy, scipy, librosa, sherpa-onnx (laughter tagger), mlx-whisper, fastapi, opencv-python-headless, rapidocr, playwright and others.
- **Activity and issues.** 1 open issue (Spanish support and game-event detection). The v1.0.0 release is titled "ClipForge 1.0.0", which does not match the repo name; this is flagged as an open verification item.
- **Implemented vs claims.** The fusion, loudness, chat, laughter and keyword signals exist as described. Game detection relies on per-title OCR presets.
- **Key algorithms (as read through summarizing fetch).**
  - Fusion: weights renormalized over signals that actually ran; each signal clipped to [0,1]; an agreement bonus multiplies the combined score by `1 + bonus * (agreeing - 1)` where a signal "agrees" at score >= 0.5 and a minimum share; Gaussian smoothing afterward.
  - Chat spike: `min(log-ratio term, Poisson-style z term)` so sparse chats cannot score high from a handful of messages. Reaction vocabulary and copy-paste waves (same normalized text from >= 3 authors within 5 s) feed a separate burst term.
  - Loudness: 0.5 s frames, rolling-median baseline over about 60 s, `excess` over baseline and `jump` over the previous few frames, scored as `0.6*clip((excess-3)/12) + 0.4*clip((jump-4)/10)`.
  - Laughter via an AudioSet tagger (sherpa-onnx), keyword impulses, then NMS over candidate windows.
- **Limitations.** Heavy surface area; literal keyword matching; a fixed 5 s chat delay; batch (VOD) oriented rather than streaming; game detection is title-specific. The `game` weight of 0.6 in the gaming mode looked unusual and is an open check against the raw file.
- **What PulseCut learned.** The min-of-two-terms count score and the agreement bonus are the best-designed ideas in the survey. PulseCut re-implemented them for streaming input (`count_excitement`, agreement bonus in `scorer.rs`), adapted the loudness score with its rolling-median baseline, and does not use the game, OCR, laughter-model or dubbing parts.

### 2.2 PeakCut (MIT; audited via parent haziqahmedqasmi/PeakCut)

- The audited fork (Mayukh135/peakcut) has the same tree as its parent and no code of its own. The parent has 694 stars and 14 open issues.
- **Architecture.** Multi-tenant web backend (FastAPI, Postgres, Docker, reverse-proxy configs). A live mode keeps a rolling buffer and cuts clips while the stream runs, using loudness, chat activity and transcript (per README; the chat code path was not fully read).
- **Detection is LLM-driven.** 15-minute windows with 60 s overlap, a "clip scout" prompt scoring 0-100 with most clips in 50-80, hook <= 5 words, JSON output. Loudness is z-scored and passed as a hint. The fallback without an LLM is loudness only.
- **Caption grouping.** A new caption line starts on a gap > 0.7 s, duration > 10 s, or sentence end after >= 4 words. Boundary snapping uses a 0.35 s tail after the last word.
- **Limitations.** Clip quality depends entirely on the LLM; the stack (Postgres, multi-tenant accounts) is heavy for a local-first Rust app.
- **What PulseCut learned.** The 0.7 s pause rule for caption pagination (implemented in `captions.rs`, `paginate`). The LLM-selection prompts and 15-minute windowing were noted but PulseCut's detection is signal-driven and its LLM use is optional and limited to metadata. The repository ships SIL OFL fonts (Anton, Archivo Black, Poppins ExtraBold) that are legally reusable with notices; this audit did not establish that PulseCut bundles them.

### 2.3 darryl-17/dawassi-ai-clipper (MIT)

- **Architecture.** A Node/Express server plus a single-file HTML UI, with ffmpeg and yt-dlp as external processes. Only dependency is `express`. A live recorder uses the FFmpeg segment muxer on a yt-dlp stream, with reconnect flags; no OBS WebSocket.
- **Detection.** Loudness-only: every 15 s it measures new segments and flags a spike when a segment exceeds the median of the last 150 readings by 6 dB (after >= 12 readings). No chat and no transcript.
- **Limitations.** One smoke test file; shell-style command construction; three releases published on the same day.
- **What PulseCut learned.** The segment-muxer recorder recipe (`-c copy -f segment -reset_timestamps 1`, MPEG-TS segments) validated the ring-buffer approach, and the median-baseline-plus-dB-margin idea matches PulseCut's rolling baselines.

### 2.4 bendawg2010/Auto-clipper (MIT)

- **Architecture.** Python/Flask with analysis modules for audio, chat, motion, scene and a YOLO model specific to one game. Works on downloaded VODs, not live. Default branch is `claude/twitch-clip-analyzer-MPT08`, not `main`.
- **Implemented vs claims.** The README calls the detector "hybrid", but the hybrid detector reads audio, motion and scene only; chat is a separate module. A `min_spike_gap` setting is defined and never used; a chat-threshold code comment ("mean + 1.5 std") does not match the formula.
- **Deps and licensing risk.** `ultralytics` is AGPL-3.0, and the bundled YOLO weights have no stated license. PulseCut avoided this stack entirely.
- **What PulseCut learned.** Per-second astats peak levels, a 5-second chat-bucket baseline and the idea of labelled event kinds. The documented bugs are a reminder to verify behavior, not trust README claims.

### 2.5 nirvagold/stream-clipper (MIT)

- **Architecture.** Tauri 2 + Rust + Svelte desktop app, file-based (import a video and a chat log); no live capture. Rust deps include hound, webrtc-vad, tokio, rayon. A 99 MB Windows ffmpeg.exe is committed to the repo (build license unchecked).
- **Detection.** RMS chunks with a percentile threshold chosen by sensitivity, multi-criteria quality points, chat windows scored by rate/emote density/caps/keywords, and a combo bonus when audio and chat overlap.
- **Notable issues.** The free tier picks clips with a time-seeded shuffle, not by score (likely a bug). The Pro license gate (`pro/`, `license/`, `keygen.rs`) is client-side, and the author's own summary says it can be bypassed. Last push was 2025-12-31; no captions or 9:16 crop were seen.
- **What PulseCut learned.** The overlap-merge rule (merge when `overlap / min(d1, d2)` exceeds a threshold, keep the higher score, pad boundaries) inspired `merge.rs`. The Pro/license code and bundled binary were deliberately not used.

### 2.6 ColinGPT9/clips-studio "Clips Kitty" (AGPL-3.0)

- 286 stars, 64 open issues (including PRs), very active. LICENSE file not read; AGPL taken from the API.
- **Detection is LLM-driven** (chunked transcripts with 60 s overlap, scores 0-100, dedupe by timestamp overlap, text similarity and segment reuse). It does not compute audio or chat signals itself.
- **Editing.** ASS captions with one Dialogue event per word and a per-word `\1c` recolour, avoiding `\k` karaoke because it leaves spoken words coloured. The vertical-crop and render modules were not located, so its render graph is unverified.
- **Publishing.** YouTube resumable upload in 8 MB chunks with retry policy; unaudited projects forced to private (which the platform notes now report as reversed on 2026-11-02, see section 4).
- **Limitations (from issues).** Windows PyTorch DLL failures, subtitle path escaping, requests for faster processing.
- **Reuse.** Ideas only (AGPL). PulseCut does not use ASS at all (see section 5).

### 2.7 kmilo93sd/agent-video-editor (MIT)

- Not a clipper. It is an Agent Skills package of editing judgment (pacing, hooks, audio and SFX, color, thumbnails) with 128 CC0 SFX, 15 music tracks and 26 CC0 graphics, rendering through an HTML-based tool. No detection, FFmpeg, Whisper or publishing code.
- **Reuse.** Knowledge only; asset terms need checking against its `library/LICENSES.md` per file. PulseCut's 13 sound effects are original, procedurally synthesized and CC0, not taken from this library.

### 2.8 MarkyADHD/LocalClip (AGPL-3.0)

- 5 stars, releases v1.0.0 to v1.4.0, Windows-first (PowerShell launcher), Python backend with a Next.js frontend and a Rust `tao`/`wry` desktop shell that only hosts a WebView.
- **Detection** is LLM-driven; chat and loudness are hints. A "clip that" voice command adds a clip covering the previous ~45 s. The LLM output is validated by snapping to transcript lines, removing overlaps and dropping weak candidates.
- **Editing.** The richest reframing pipeline in the survey: face sampling at 3 fps (YuNet then Haar), spring-damped crop trajectory with deadzone and keyframe simplification, a face/fit layout plan snapped to scene cuts, blurred-background compositor, Ken Burns punch-in, speaker-turn framing. ASS per-word captions. faster-whisper with GPU batching.
- **Limitations.** Windows-first; no freeze or slow-mo found; chat/loudness are only LLM hints.
- **Reuse.** Ideas only (AGPL). PulseCut implemented blur-fill, center/smart crop and facecam split with its own FFmpeg graphs; it did not implement spring-smoothed face tracking.

### 2.9 iamzimozic/streamclip-ai (MIT)

- Python, comedy-focused, 0 stars, 43 files, no releases. Pipeline: faster-whisper, audio features, YAMNet laughter, optional chat, weighted fusion, LLM refine, pairwise LLM tournament ranking with position-bias cancellation, SFX cues with silence checks, vertical render with a static Haar face box, LLM metadata.
- **Weaknesses.** One static face box for the whole clip; substring reaction matching (so "no" false-matches); a noisy laughter-energy proxy; title length limit enforced only in the prompt.
- **What PulseCut learned.** Renormalized-weight fusion and the principle that an LLM cannot hear audio, so SFX placement needs programmatic silence checks. PulseCut's own SFX/ducking logic is separate.

## 3. Additional projects found

From GitHub search (`twitch highlight language:rust`, `auto clip ffmpeg captions vertical license:mit`). None was inspected at source level.

| Repo | Language | License | Stars | Last push | Note |
|------|----------|---------|-------|-----------|------|
| hougesen/twitch-highlight-finder | Rust | MIT | 9 | 2024-05-10 | Chat frequency and emote highlights; stale; not read |
| huntermixhunter/autocut | Python | MIT | 0 | 2026-10-31 | Local FFmpeg + faster-whisper captions; reference only |
| xniperbuilds/ripclips | Python | MIT | 0 | 2026-07-15 | Face-tracked vertical clips; approach unchecked |
| satsushis/Auto-YT-Clipper | JavaScript | MIT | 2 | 2026-08-25 | AI moment selection and uploads; reference only |

nirvagold/stream-clipper also appeared in the Rust search (covered in 2.5). Nothing else in the Rust results exceeded 10 stars.

## 4. Platform and API findings

### OBS WebSocket v5 [V unless noted]
- Handshake: Hello, Identify (auth = base64(sha256(base64(sha256(password+salt)) + challenge))), Identified. Event subscriptions are bit flags; Outputs = 64.
- `SaveReplayBuffer` returns immediately and writes asynchronously; completion is signalled by `ReplayBufferSaved`, with `GetLastReplayBufferReplay` as a polling fallback. **It saves only footage already in the ring; there is no post-roll option.**
- `SplitRecordFile`, `CreateRecordChapter` and `RecordFileChanged` arrived in protocol 5.5.0 (OBS 30.2).
- Replay buffer length keys (`RecRBTime`, `RecRBSize` under `SimpleOutput`/`AdvOut`) are **[M]**: not in the protocol docs.
- **obws** 0.15.0 (published 2026-03-13) covers `replay_buffer().save()/last_replay()` and `recording().split_file()/create_chapter()`. Its repo moved to forge.dnaka91.rocks.
- **Decision.** Use OBS replay buffer with a deferred save: wait `clip_end + margin`, then `SaveReplayBuffer`, then await `ReplayBufferSaved`. Merge overlapping triggers into one save. Capture via obws 0.15; raw v5 over tokio-tungstenite + sha2 + base64 stays available as a fallback if the crate lags.
- **Open items.** Confirm the `RecRBTime` category names on a real install, whether a running buffer picks up a changed length without restart, and that `client.profiles().set_parameter` exists in obws.

### Twitch chat
- Anonymous IRC over WebSocket (`wss://irc-ws.chat.twitch.tv:443`) with `CAP REQ :twitch.tv/tags twitch.tv/commands` **[V]**. Anonymous `justinfan` login is long-standing community behavior **[M]**; the fetched docs do not mention it.
- The `emotes` tag uses zero-based inclusive code-point offsets **[V]**; Rust code must count chars, not bytes. `tmi-sent-ts` is the timing source.
- IRC is not deprecated; Twitch recommends EventSub but gives no sunset **[V]**. EventSub `channel.chat.message` needs a user token with `user:read:chat`, so there is no anonymous EventSub **[V]**.
- **Decision.** Anonymous IRC is the default chat source, behind a trait so EventSub can be added later.

### YouTube Live chat
- `liveChatMessages.list` costs 5 units per call **[V]**; at a 5 s poll that is about 3,600 units per hour against a 10,000/day default, so polling does not scale. `streamList` (server-streaming, gRPC) costs 5 units per call/connection and is the recommended path **[V]**; API-key acceptance for public chats is medium confidence.
- **Decision and open item.** Treat as an optional, quota-budgeted source honoring `pollingIntervalMillis`. Verify `streamList` with an API key and its real quota consumption over a long stream. Scraping InnerTube was rejected (unofficial, ToS gray).

### Kick
- Official events are **webhook-only**; `chat.message.sent` is capped at 1,000 subscriptions for unverified apps **[V]**. Webhook signatures use RSA PKCS#1 v1.5 + SHA-256 over `{message_id}.{timestamp}.{raw_body}` with a runtime-fetched public key **[V]**.
- A desktop app has no public HTTPS endpoint; the options are a relay service or a user tunnel. The unofficial Pusher WebSocket is community-documented only **[M]** and can break at any time.
- **Decision.** Treat Kick as experimental behind the chat-source trait. The webhook endpoint registration step is uncertain and needs verification.

### YouTube upload
- **[V]** `videos.insert` quota changed: a dedicated bucket of 100 calls per day (June 2026), and on 2026-11-02 the restriction forcing unverified projects to private viewing was reversed. Older guides are outdated. Always set `privacyStatus` explicitly.
- Resumable protocol (session URI, 256 KB-multiple chunks, `308` resume) **[V]**. Shorts classification (vertical/square, <= 3 min) is **[M]**; OAuth for desktop (loopback + PKCE, refresh token in Keychain via `keyring`) is **[M]**.
- **Decision.** Export first; upload is a later step. Open item: confirm the Shorts length rule against YouTube Help.

### Instagram Reels
- Container, poll `status_code`, then `media_publish` **[V]**. A resumable upload path exists but the docs suggest it may be limited to Facebook Login for Business, which is ambiguous for Instagram Login **[V, ambiguous]**. The docs contradict themselves on the daily publish cap (50 vs 100); query `content_publishing_limit`.
- App Review is a heavy gate for a distributed desktop app. **Decision:** export and reveal in Finder by default. Open item: test resumable upload eligibility with a real account.

### TikTok
- Official docs returned HTTP 503 / JS-only on the audit date, so everything is **[M]** from third-party summaries: Direct Post vs inbox upload, an audit gate forcing private posts for unaudited clients, chunked FILE_UPLOAD limits.
- **Decision.** Inbox/draft upload or export-only. Re-verify all numbers against official docs before relying on them.

### whisper.cpp
- Binary renamed to `whisper-cli` **[V]**. Input must be 16 kHz mono WAV **[V]**. `-ojf` writes full JSON with per-token entries and millisecond offsets **[V]**, so word timing can be built by merging tokens (a token starting with a space begins a new word) without `-ml 1` hacks.
- **Decision.** Shell out to `whisper-cli` with `-ojf`, merge tokens into words, try GPU first and fall back to CPU (`-ng`). VAD (`--vad` with a Silero model) exists **[V]** but timestamp re-mapping behavior was not tested; short-clip flags like `-mc 0` are **[M]**.

### FFmpeg
- The segment muxer ring buffer and `ffconcat` cutting are **[V]** from FFmpeg docs; `-c copy` inpoints snap to keyframes, so accurate clips need a re-encode.
- `h264_videotoolbox` options are **[M]** (the fetched codec page had no VideoToolbox section); verify with `ffmpeg -h encoder=h264_videotoolbox`.
- **Build finding that shaped the design.** The Homebrew FFmpeg 9 build used for development lacks libass and drawtext, so the `subtitles`/`ass`/`drawtext` approach seen in clips-studio, LocalClip, sharkyclips and streamclip-ai was not viable. PulseCut renders captions and text itself with ab_glyph to PNG and overlays them via ffconcat.
- Shipping an FFmpeg binary inside an app raises LGPL/GPL compliance questions **[M]**; this remains a distribution question to settle.

## 5. Editing and creator patterns observed

- **Blur-fill vertical** (blurred scaled copy behind a fitted foreground) appears in sharkyclips and LocalClip; static center crop in dawassi, streamclip-ai and stream-clipper; facecam/gameplay vstack split in sharkyclips and streamclip-ai. PulseCut implemented blur-fill, center/smart crop and facecam split.
- **Word-highlight captions** are common (per-word Dialogue events in clips-studio and LocalClip). Because the build lacked libass, PulseCut paginates and renders captions itself; the 0.7 s pause pagination rule comes from PeakCut.
- **Hooks and titles**: short all-caps on-screen hooks (<= 5-6 words, <= 32 characters in streamclip-ai) that reference something specific in the clip. PulseCut uses deterministic templates (best transcript quote, chat phrases, moment kind) with an optional loopback-only local LLM.
- **SFX placement**: the LLM cannot hear audio, so silence checks gate sound effects (streamclip-ai). PulseCut adds sidechain ducking and loudnorm.
- **Pacing effects**: Ken Burns punch-in and facecam zoom/shake exist in a few projects; freeze and slow-mo were not found in any inspected project. PulseCut implements freeze, slow-mo, replay and speed-ramp via trim/setpts/tpad/concat, punch-in and shake via zoompan, and flash via eq.
- **Score calibration**: LLM prompts consistently push critical scoring ("most moments below 50-55"); PulseCut instead calibrates signal weights.

## 6. What PulseCut implemented (and how far the claims go)

- **Detection:** streaming per-second fusion of 9 signals: chat velocity, unique chatters, reactions vocabulary, emotes, copy-paste waves, chat acceleration, audio loudness excess/jump, a laughter heuristic, and audio transients. Count signals use `min(log-ratio, Poisson z)` (idea from sharkyclips, independently reimplemented). The agreement bonus idea and the loudness score (`0.6*clip((excess-3)/12)+0.4*clip((jump-4)/10)`, rolling-median baseline) are adapted from sharkyclips. The overlap-merge rule is inspired by stream-clipper. Spam suppression, poll/giveaway/bot filtering, hysteresis and adaptive exponentially weighted baselines are PulseCut's own.
- **Tuning:** weights by grid search on synthetic labelled data. Held-out: precision 1.000, recall 0.971, F1 0.985, versus uniform weights F1 0.922. **This is synthetic data only and has not been validated on real streams.** The perfect precision should be read as a property of the synthetic generator, not a performance claim.
- **Capture:** OBS replay buffer with deferred save (obws 0.15); FFmpeg segment-muxer ring buffer with wall-clock anchoring; coverage reports.
- **Transcription, editing, metadata:** as described in section 4 and 5.

## 7. Gaps none of the references fill

1. **Live-streaming detection with adaptive baselines.** Every inspected detector is batch (VOD) or fixed-threshold, or LLM-driven. None fuses chat and audio signals in a streaming per-second loop with adaptive EW baselines, hysteresis and spam suppression. PeakCut's live mode is the closest, but its chat path was not fully read and it relies on LLM calls.
2. **Deferred replay save with coverage guarantees.** None of the projects use OBS `SaveReplayBuffer` at all; none wait for post-roll before saving, merge overlapping triggers into one save, or report coverage (how much of the requested window the saved file actually contains). The OBS protocol has no post-roll option, so this must be built client-side.
3. **Rust-native end to end.** The only Rust project (stream-clipper) is file-based, stale and license-gated; its chat analysis is Pro-only. The rest are Python or Node. No project combines Rust async capture, detection, FFmpeg editing and a native UI.
4. **Editing without libass/drawtext.** The surveyed projects assume ASS or `subtitles` filters; none render captions independently of FFmpeg's text filters.
5. **Honest evaluation.** None of the references publish a labelled evaluation. PulseCut's own is synthetic only; a real-stream benchmark remains future work.

## 8. Open verification items

1. OBS `RecRBTime` category names; whether a running buffer picks up changes; obws `profiles().set_parameter`.
2. YouTube `streamList` with an API key and quota over a long stream.
3. Instagram resumable upload under Instagram Login.
4. TikTok limits and audit rules against official docs.
5. sharkyclips: "ClipForge 1.0.0" release name, raw `modes.py` weights, raw `signals/base.py`.
6. Licenses of clips-studio and LocalClip from their LICENSE/NOTICE files; third-party crate licenses via `cargo-deny`.
7. Real-stream validation of detection weights.
