# PulseCut Reuse Matrix

Audit date: 2026-11-17. Licenses are from the GitHub API SPDX field; LICENSE files were not fetched for clips-studio and LocalClip. Code was read through summarizing fetches (nothing cloned or run), so formulas are described, not copied.

**Overall position: no code was copied from any reference repository.** Where PulseCut uses an idea, it was independently reimplemented in Rust. The two AGPL-3.0 projects (clips-studio, LocalClip) were studied for ideas only.

Decision key: **Idea (reimplemented)** = concept adopted, own code. **Ideas only** = studied, nothing adopted into code. **Avoid** = deliberately not used. **Knowledge** = guidance read, no code or assets taken.

## Feature reuse table

| Project | Feature | License | Reuse decision | Reason |
|---|---|---|---|---|
| sharkyclips | Multi-signal fusion with renormalized weights | MIT | Idea (reimplemented) | Best-designed fusion; PulseCut fuses 9 streaming signals with its own tuned weights |
| sharkyclips | Agreement bonus (more agreeing signals boost score) | MIT | Idea (reimplemented) | Cheap precision gain; own thresholds and tuning |
| sharkyclips | Chat spike = min(log-ratio, Poisson z) | MIT | Idea (reimplemented) | Prevents sparse chats scoring high; rebuilt for streaming input with adaptive baselines |
| sharkyclips | Loudness score (excess + jump over rolling-median baseline) | MIT | Idea (adapted) | Formula 0.6*clip((excess-3)/12)+0.4*clip((jump-4)/10) adapted; verify constants against raw source |
| sharkyclips | Copy-paste wave detection | MIT | Idea (reimplemented) | Concept shared; PulseCut's own implementation and spam suppression |
| sharkyclips | Laughter tagger (sherpa-onnx AudioSet) | MIT | Avoid | PulseCut uses a laughter heuristic instead; ONNX model dependency out of scope |
| sharkyclips | Game HUD OCR, TikTok playwright upload, dubbing, web app | MIT | Avoid | Outside PulseCut scope |
| sharkyclips | Mode presets (gaming/study/music) | MIT | Ideas only | Not adopted |
| PeakCut | Caption pagination on pauses (gap > 0.7 s) | MIT | Idea (reimplemented) | Simple, robust rule; implemented in `paginate` |
| PeakCut | LLM window clip selection (15 min / 60 s overlap) | MIT | Ideas only | PulseCut detection is signal-driven; LLM limited to optional metadata |
| PeakCut | Fonts (Anton, Archivo Black, Poppins ExtraBold; SIL OFL) | OFL | Not adopted in this audit | Reusable with OFL notices if bundled later |
| PeakCut | Multi-tenant web backend (Postgres, accounts) | MIT | Avoid | Heavy; PulseCut is local-first |
| dawassi-ai-clipper | Segment-muxer live recorder recipe | MIT | Idea (reimplemented) | Supports PulseCut's ring buffer; own implementation with wall-clock anchoring |
| dawassi-ai-clipper | Live loudness spike (median baseline + 6 dB) | MIT | Ideas only | PulseCut uses fused, adaptive baselines |
| dawassi-ai-clipper | Node/Express app | MIT | Avoid | No reusable core; JS stack |
| Auto-clipper (bendawg) | Per-second astats peak levels, chat 5 s buckets | MIT | Ideas only | Superseded by PulseCut's own signals |
| Auto-clipper (bendawg) | YOLO model and ultralytics | AGPL-3.0 dep; model weights license not stated | Avoid | Licensing risk; game-specific |
| stream-clipper | Overlap merge (overlap / min duration, keep higher score) | MIT | Idea (reimplemented) | Inspired `merge.rs` rule for overlapping events |
| stream-clipper | RMS percentile threshold | MIT | Ideas only | PulseCut uses rolling-median and EW baselines |
| stream-clipper | Pro gating, `license/`, `keygen.rs` | MIT (repo) | Avoid | Client-side gate; not worth reusing |
| stream-clipper | Bundled ffmpeg.exe (99 MB) | Unchecked | Avoid | Build license unverified |
| clips-studio | Per-word ASS recolour captions | AGPL-3.0 | Ideas only | Copying would bind PulseCut to AGPL; PulseCut renders captions itself (no libass in dev FFmpeg) |
| clips-studio | LLM chunked scoring and dedupe | AGPL-3.0 | Ideas only | Not adopted |
| clips-studio | YouTube resumable upload retry policy | AGPL-3.0 | Ideas only | PulseCut follows Google's documented protocol instead |
| clips-studio | Auto-publish and campaigns | AGPL-3.0 | Avoid | Out of scope |
| agent-video-editor | Pacing, hook and audio guidance | MIT | Knowledge | Read as guidance; no code taken |
| agent-video-editor | CC0 SFX/music library | CC0 claimed; per-file terms unchecked | Not used | PulseCut ships 13 original procedurally synthesized CC0 effects |
| agent-video-editor | HyperFrames HTML rendering | MIT | Avoid | Different render stack |
| LocalClip | Reframing (face sampling, spring smoothing, face/fit plan) | AGPL-3.0 | Ideas only | Not implemented; PulseCut uses blur-fill, center/smart crop, facecam split |
| LocalClip | Blurred-background compositor | AGPL-3.0 | Ideas only | PulseCut builds its own FFmpeg graph (blur-fill is a common technique across repos) |
| LocalClip | LLM validation layer (snap, dedupe, drop weak) | AGPL-3.0 | Ideas only | Not adopted |
| LocalClip | Rust `tao`/`wry` WebView shell | AGPL-3.0 | Avoid | PulseCut uses eframe/egui |
| LocalClip | faster-whisper GPU batching | AGPL-3.0 | Avoid | PulseCut uses whisper.cpp `whisper-cli` |
| streamclip-ai | Weighted fusion with renormalization | MIT | Ideas only | Same family as sharkyclips fusion |
| streamclip-ai | LLM pairwise tournament ranking | MIT | Ideas only | Not adopted; no LLM ranking |
| streamclip-ai | SFX placement with silence checks | MIT | Ideas only | Noted; PulseCut has its own SFX and sidechain ducking |
| streamclip-ai | YAMNet laughter, static Haar facecam crop | MIT | Avoid | Noisy proxy, static box limitation |
| twitch-highlight-finder | Rust chat-frequency/emote highlights | MIT | Not inspected | Source not read; stale (2024) |
| autocut, ripclips, Auto-YT-Clipper | Misc. vertical/caption clippers | MIT | Not inspected | Reference only |

## Third-party dependencies

Licenses below are as commonly published for these crates; none has been verified in this audit. Run `cargo deny check licenses` (cargo-deny) and review the transitive tree before any release.

| Crate | Purpose | License (as commonly published) |
|---|---|---|
| tokio | Async runtime | MIT (verify with cargo-deny) |
| axum | HTTP server | MIT (verify with cargo-deny) |
| reqwest 0.13 | HTTP client | MIT OR Apache-2.0 (verify with cargo-deny) |
| serde | Serialization | MIT OR Apache-2.0 (verify with cargo-deny) |
| sqlx 0.9 (sqlite) | Database | MIT OR Apache-2.0 (verify with cargo-deny) |
| tracing | Logging | MIT (verify with cargo-deny) |
| rayon | Data parallelism | MIT OR Apache-2.0 (verify with cargo-deny) |
| tokio-tungstenite 0.30 | WebSocket | MIT (verify with cargo-deny) |
| obws 0.15 | OBS WebSocket client | verify with cargo-deny |
| cpal 0.18 | Audio capture | Apache-2.0 (verify with cargo-deny) |
| global-hotkey 0.10 | Global hotkeys | verify with cargo-deny |
| eframe / egui 0.37 | GUI | MIT OR Apache-2.0 (verify with cargo-deny) |
| ab_glyph | Glyph rasterization | Apache-2.0 (verify with cargo-deny) |
| image | Image encode/decode | MIT OR Apache-2.0 (verify with cargo-deny) |
| hound | WAV I/O | Apache-2.0 (verify with cargo-deny) |
| keyring 4 | Credential storage | verify with cargo-deny |
| sha2 | Hashing | MIT OR Apache-2.0 (verify with cargo-deny) |
| base64 | Encoding | MIT OR Apache-2.0 (verify with cargo-deny) |
| chrono | Time | MIT OR Apache-2.0 (verify with cargo-deny) |
| uuid | IDs | MIT OR Apache-2.0 (verify with cargo-deny) |
| clap | CLI | MIT OR Apache-2.0 (verify with cargo-deny) |
| directories | Paths | verify with cargo-deny |
| sysinfo | System info | MIT (verify with cargo-deny) |

External programs invoked as separate processes (not linked): FFmpeg (build license depends on configuration, verify), whisper.cpp `whisper-cli` (MIT per its repository; verify), llama.cpp `llama-server` (optional; verify).

## Borrowed ideas to PulseCut implementation

| Idea | Source project (license) | PulseCut file | What PulseCut did |
|---|---|---|---|
| Count score = min(log-ratio term, Poisson z term) | sharkyclips (MIT) | `crates/pulsecut-detect/src/baseline.rs` (`count_excitement`) | Independently reimplemented for streaming input with adaptive baselines |
| Signal fusion over renormalized weights | sharkyclips (MIT) | `crates/pulsecut-detect/src/scorer.rs` | Fuses 9 signals per second; weights tuned by grid search on synthetic data |
| Agreement bonus | sharkyclips (MIT) | `crates/pulsecut-detect/src/scorer.rs` | Reimplemented with PulseCut's own thresholds |
| Loudness score 0.6*clip((excess-3)/12)+0.4*clip((jump-4)/10) with rolling-median baseline | sharkyclips (MIT) | `crates/pulsecut-detect/src/scorer.rs` | Adapted from sharkyclips' loudness signal |
| Merge rule for overlapping events | stream-clipper (MIT) | `crates/pulsecut-detect/src/merge.rs` | Inspired by its overlap merge; own implementation |
| Caption pagination on 0.7 s pauses | PeakCut (MIT) | `crates/pulsecut-media/src/captions.rs` (`paginate`) | Rule reimplemented; captions rendered with ab_glyph |

Own work, not borrowed: spam suppression, poll/giveaway/bot filtering, hysteresis, adaptive exponentially weighted baselines, OBS deferred save with coverage reports, FFmpeg segment ring buffer with wall-clock anchoring, PNG/ffconcat caption rendering, the 13 synthesized sound effects, sidechain ducking, deterministic metadata templates.

Caveat: synthetic-data tuning only (held-out F1 0.985 vs 0.922 uniform); not validated on real streams.
