# Testing

```bash
source env.sh && cargo test --workspace --release
```

Tests that start local mock servers (OBS, YouTube) bind `127.0.0.1:0`. In a sandbox without local-port permission they fail with `PermissionDenied`; run them outside the sandbox.

## Results (2026-11-17, macOS 15, M1 8 GB)

**96 passed, 0 failed, 1 ignored** (the slow weight tuner, run explicitly). `cargo clippy --workspace --all-targets`: 0 warnings. `cargo fmt --check`: clean.

| Crate | Tests | What they prove |
|---|---|---|
| core | 11 | settings round-trip/partial files/clamping, safe path joins, coverage maths, process runner errors |
| detect | 24 (+1 ignored) | normalization & vocabulary, spam/bot/poll suppression, copy-paste waves, baselines resist bursts and follow growth, excitement needs ratio *and* z, audio loudness/jump, laughter vs steady tone vs impact, merge rules, synthetic determinism, **held-out precision/recall gate** |
| media | 21 + 2 integration | ffprobe parsing, timeline maths, caption pagination and continuous timing, outlined text rendering, whisper JSON parsing, voice-command matching, SFX synthesis, asset-pack validation (path traversal, fake magic bytes, license required), **real FFmpeg renders of every layout × orientation and a clip with all effects + captions + SFX + thumbnail** |
| edit | 7 | recipe choice, audio-onset alignment, confidence gating, intensity 0, SFX off, caption band avoidance, smart crop, facecam fallback |
| meta | 5 | quote selection, contextual titles, hashtag limits, clamping, LLM loopback-only + fallback |
| store | 3 | clip round-trip + search, **job claim + crash recovery across reopen**, upload de-duplication |
| capture | 4 + 2 integration | CSV parsing, input forms, clock anchoring, mock auth; **real segment ring with wall-clock anchoring + live audio analysis**, **OBS controller vs mock: wrong password, buffer-too-short warning, deferred save timing, file moved into library, too-old moment reported, reconnect after OBS "crash"** |
| chat | 5 | IRCv3 tags/emotes/unicode/actions, YouTube page parsing, **Kick RSA signature verification incl. tamper/replay** |
| publish | 6 + 1 integration | PKCE S256, auth URL, video resource limits/scheduling, TikTok chunking, backoff, RFC 3339; **worker vs mock YouTube: token refresh, 503 mid-upload, resume of same session, every byte exactly once, clip → Published** |
| platform | 3 | resampler continuity, hotkey parsing, LaunchAgent plist |
| engine | 2 | multi-stream focus hysteresis, VAD segmentation |

## End-to-end acceptance (`pulsecut-cli demo`)

| # | Requirement | Result |
|---|---|---|
| 1 | Launch PulseCut | engine starts (store migrate, asset pack install, encoder probe) |
| 2 | Connect capture | rolling FFmpeg buffer reading a 75 s synthetic stream in real time. OBS path covered by the mock integration test; **live OBS not yet tested** |
| 3 | Streaming | synthetic stream (test pattern + public-domain speech + impact at 40 s) |
| 4 | Receive chat | 224 synthetic messages delivered in real time (2 msg/s baseline + reaction burst) |
| 5 | Detect a burst | ✅ Fail, 100 % confidence, moment estimated −1.9 s from truth, 7 explained reasons |
| 6 | Capture the correct footage | ✅ coverage: moment covered, full window |
| 7 | ~30 s clip | ✅ 30.9 s (30 s window + 0.9 s freeze) |
| 8 | Vertical video | ✅ 720×1280 (demo uses 1280 long edge for speed; default 1080×1920) |
| 9 | Subtitles | ✅ 22 words by whisper.cpp base.en, word-highlighted captions + SRT |
| 10 | Editing recipe | ✅ `fail_freeze`: freeze on the audio impact, punch-in, reaction text from chat ("THROWING") |
| 11 | Sound effect | ✅ `scratch` + `slide_down`, ducked |
| 12 | Title + thumbnail | ✅ "DemoStreamer's plan went horribly wrong (chat: "THROWING")", vertical cover + 16:9 thumbnail |
| 13 | Library | ✅ status *Needs review*, JSON decision trail |
| 14 | Approve + upload unlisted YouTube | ⚠️ approve ✅, queue ✅, duplicate prevented ✅; real upload **not run** (needs your OAuth client). Upload code path proven against a mock API |
| 15 | Recover from simulated failure | ✅ job interrupted mid-run re-queued and resumed (attempt 2); clip re-rendered from raw footage |
| + | Manual hotkey clip | ✅ hotkey path produced a 30 s clip (`clean` recipe — manual clips without a known moment type get no meme effects by design) |
| + | Voice command | VAD + whisper + phrase matcher unit-tested; **live microphone path not exercised in CI** (needs mic permission) |

Report: `demo-output/demo-report.json`. Visual checks: `research/demo-sheet.png`, `research/render-sheet.png`.

## Real-world checks run manually

* `pulsecut-cli chat --twitch xqc kaicenat zackrawrr jynxzi caseoh_ ibai tarik --secs 60` → connected anonymously, 70 live messages parsed across channels, detectors scored live (needs network outside the sandbox).
* `pulsecut-cli encoders` → VideoToolbox and libx264 both pass a real 1080×1920 test encode outside the sandbox (VideoToolbox fails inside the sandbox; PulseCut falls back to libx264 automatically).
* Desktop app launched as a bundle; all 7 pages exercised with screenshots (library preview playback, editor plan, asset previews, publishing queue, settings).

## Manual integration test: monitoring and clipping with real services

Automated tests use a mock OBS, a mock Helix server and simulated capture; they do not prove real-world reliability. Run this procedure before a release:

1. **Selection without credentials.** Add two live Twitch streamers on the Live Monitor page, leave the Twitch client id empty, set max streams to 1. Expect: dashboard shows one `buffering` stream and `live status unknown`; `GET /api/monitor` lists the decision reasons.
2. **Selection with Helix.** Enter a client id + secret. Expect within one poll: live/offline and viewer counts in the table. Mark the non-selected streamer as favorite: after `min_watch_secs` the favorite replaces the current stream (`switch … favorite … is live`).
3. **Trending fallback.** Disable every watchlisted streamer and enable trending. Expect a trending stream to be selected; re-enable a live watchlisted streamer and expect the trending stream to be released immediately.
4. **No lost clips on switch.** Trigger `POST /api/clip` and within 5 s force a switch (press **monitor now** on another streamer). Expect the clip to appear in the library (`saved` then `clip ready`), not a failure.
5. **Cooldown/dedup.** With autonomous clipping on, watch the trigger log during a busy stream: moments within the cooldown show `suppressed`.
6. **OBS failure.** In streamer mode, stop the OBS replay buffer and press Clip now. Expect `failed … OutputNotRunning (after 3 attempts)` in the trigger log and a failed clip in the library; start the buffer again and the next clip succeeds.
7. **Disk space.** Set `storage.min_free_gb` above the free space. Expect `disk space low: autonomous capture paused` and failed-clip records for autonomous moments; manual clips still work.
8. **Restart recovery.** Quit PulseCut while a clip is rendering and restart: the job is re-queued and finishes.

## Not yet tested

* Live OBS Studio (protocol mock only), real YouTube/Instagram/TikTok uploads, YouTube Live Chat with a real API key, Kick webhooks from Kick's servers.
* Windows build and runtime.
* Detection on recorded real streams with human labels (the most important next validation step).
