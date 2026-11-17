# Implementation plan and phase status

Legend: ✅ done and tested · 🟡 implemented, partially verified · ⬜ not started

| Phase | Scope | Status | Evidence / acceptance | Known limitations |
|---|---|---|---|---|
| 0 Research | 9 reference repos, platform APIs, reuse matrix | ✅ | RESEARCH.md, REUSE_MATRIX.md | Code read via fetched pages, not cloned/run |
| 1 Architecture | 13-crate workspace, domain model, settings, paths, safe processes | ✅ | ARCHITECTURE.md, core tests | — |
| 2 Capture | OBS controller (auth, reconnect, buffer check, deferred save); FFmpeg rolling buffer with clock anchoring; coverage reports | 🟡 | mock-OBS + real-segment integration tests | not yet run against live OBS |
| 3 Clip generation | probe, segment concat, frame-accurate trim, encoder selection | ✅ | render integration tests, demo | — |
| 4 Manual triggers | UI button, global hotkeys, local API, voice command (VAD + whisper) | 🟡 | demo hotkey path; VAD/matcher unit tests | live mic path not exercised automatically |
| 5 Chat + autonomous detection | Twitch IRC, YouTube poller, Kick webhooks; 9-signal detector; spam filters; merge | ✅ | 24 detect tests, real Twitch run, demo | YouTube/Kick not run against live services |
| 6 Audio + multi-signal | loudness/jump/transient/laughter, mic + stream audio, tuned fusion | 🟡 | held-out eval F1 0.980 (synthetic) | no real labelled streams yet; laughter is heuristic |
| 7 Editing + captions | layouts, timeline effects, whisper word captions (4 styles), loudnorm | ✅ | render tests, demo frames | facecam region is manual (no face detection) |
| 8 Meme/SFX engine | recipes with confidence gating; 13 CC0 sounds; ducking; asset packs + import | ✅ | edit tests, asset tests, demo | meme images need user/creator packs |
| 9 Titles/thumbnails | quote/chat/kind templates, optional loopback LLM, clean-frame covers | ✅ | meta tests, demo | LLM path tested only for refusal/fallback |
| 10 Desktop app | 7 pages, preview player, editor, settings, toasts | ✅ | manual screenshots of every page | preview is silent proxy (audio via OS player) |
| 11 Publishing | OAuth PKCE, YouTube resumable, IG Reels via host, TikTok inbox, queue/retry/caps/dedupe | 🟡 | mock YouTube integration test | no real uploads yet; TikTok docs unverified |
| 12 Windows | platform code paths, CI job, packaging script | 🟡 | — | never compiled on Windows |
| 13 Reliability/release | crash recovery, retention, disk guard, macOS bundle, CI | 🟡 | store/demo recovery tests, `.app` built | no notarization, no installer signing, no auto-update |

## Next steps (priority order)
1. Run against your live OBS (`pulsecut-cli obs-check`, then a stream) and fix anything the mock missed.
2. Real YouTube unlisted upload with your OAuth client (acceptance item 14).
3. Record 2–3 real streams with chat logs, label moments, add them as an evaluation set; re-tune.
4. Windows CI green, then manual test on a Windows machine.
5. Face detection for automatic facecam regions (optional ONNX model, off by default).
6. Gameplay-event detector plug-ins (e.g. kill-feed OCR) behind the existing `TriggerSource::Gameplay`.
7. Release hardening: cargo-deny in CI, Developer ID signing/notarization, Authenticode, update checks (opt-in).
