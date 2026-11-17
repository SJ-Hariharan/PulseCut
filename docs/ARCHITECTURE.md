# Architecture

## Data flow

```
 Twitch IRC ─┐  YouTube poll ─┐  Kick webhook ─┐        mic (cpal) ─┐   stream audio (FFmpeg PCM pipe) ─┐
             ▼                ▼                ▼                    ▼                                   ▼
        StreamChat (bounded mpsc) ───────────────────────►  per-stream Detector  ◄──── AudioSecond (bounded mpsc)
                                                               │  1 s tick
                       hotkey / voice / UI / local API         ▼
                       ──── manual HighlightEvent ────►  CaptureRequest (one interface for every trigger)
                                                               │
                                     ┌─────────────────────────┴───────────────────────┐
                              Coordinator (OBS backend)                     Coordinator (segment backend, per stream)
                              deferred SaveReplayBuffer                     rolling FFmpeg -f segment ring
                                     └──────────── CapturedFile + Coverage ─┘
                                                               │
                                       SQLite: clip (Rendering) + job("render")   ← crash-safe queue
                                                               │  N bounded render workers
          probe → audio onset → motion centre → whisper.cpp → metadata → EditPlan (pulsecut-edit) → FFmpeg render
                 → clean-frame thumbnails → SRT + JSON sidecar → clip (Review)
                                                               │ approve (or gated auto-publish)
                                       SQLite: upload (unique per clip/platform/account) → upload worker
```

## Key decisions

### Capture: deferred replay-buffer save (OBS) and an owned rolling buffer (everything else)
OBS's `SaveReplayBuffer` only writes footage *already* in the ring — there is no post-roll in the protocol. PulseCut therefore schedules the save at `clip_end + save_margin` and requires the buffer to hold `pre + post + chat_reaction_delay + 4 s + margin` (`Settings::required_replay_buffer_secs`, default 42 s). The controller reads `RecRBTime` from the active OBS profile (`Output/Mode` → `SimpleOutput` or `AdvOut`) and warns in the UI when it is too short. The saved file is assumed to end at the instant the save was *requested*; the request→event delay is reported as `sync_uncertainty_ms`.

For URLs/devices/files PulseCut runs FFmpeg's segment muxer itself (`-f segment -segment_wrap -segment_list csv -reset_timestamps 1`, MPEG-TS). Wall-clock anchoring: each CSV entry's end time is observed at wall time W; `offset = min(W − end)` over a sliding window converges to the true offset plus mux latency (`capture::sync::ClockAnchor`). Extraction concatenates *whole* segments (stream copy, no keyframe snapping problems) and reports the exact covered span; the renderer trims frame-accurately while re-encoding.

**Coverage is never silent.** Every capture returns a `Coverage` (requested vs actual). If the moment itself is not covered the clip is stored as *Failed* with the reason ("increase the replay buffer"); partial pre-roll is rendered but flagged in the library and blocks auto-publish.

Alternatives considered: OBS continuous recording + `SplitRecordFile` (unbounded disk, needs pruning; kept as a future backend), OBS replay buffer saved immediately (misses post-roll — rejected).

### Detection (pulsecut-detect)
* 1-second bins per stream; spam suppression per user (1 counted msg/s, 30 s duplicate window), bot list, commands, poll votes.
* 9 signals → 0..1 "excitement" against adaptive baselines (EW mean, burst-capped updates; half-life 5 min). Counts use `min(log-ratio term, Poisson-z term)` (idea from sharkyclips, MIT) so quiet channels need a real z-score and huge channels need a real ratio.
* Chat is shifted back by the reaction delay so chat and audio align on the *moment* timeline.
* Fusion = weight-share average over *available* signals × agreement bonus. Weights are tuned by grid search on labelled synthetic streams and validated on held-out seeds (`pulsecut-cli eval`).
* Hysteresis state machine (enter at θ, stay above 0.6θ, close after 3 s), onset back-tracking, audio-onset alignment for audio-dominant events, refractory period, hourly cap.
* Every event carries per-signal contributions and human-readable reasons; the Hype Map plots stacked contributions.
* `merge::plan_captures` merges overlapping events (clip keeps running while activity continues, up to `max_clip_secs`); manual triggers are never dropped.

### Editing: decision separate from rendering
`pulsecut-edit::Planner` produces a serializable `EditPlan` (layout, crop, trim, captions style/position, effect cues with confidence + reason). It is deterministic, unit-tested without FFmpeg, editable in the UI, and re-renderable. Effects are gated by confidence vs. a bar derived from the user's intensity setting, and land on the loudest audio onset near the moment rather than on the chat reaction.

`pulsecut-media::render` turns a plan into **one** FFmpeg filter graph: layout → timeline pieces (`trim/setpts/tpad` + `concat`, with `atempo`/silence for audio) → `zoompan` for punch-in/shake → `eq` flash → caption overlay → PNG overlays → `loudnorm` → SFX `sidechaincompress` ducking → `alimiter`. A `Timeline` maps source time to output time so captions and overlays stay in sync with freezes and slow motion.

All text (captions, reaction text, hook, watermark, thumbnail titles) is rasterized in Rust with `ab_glyph` (outline via distance transform, drop shadow) because common FFmpeg builds (Homebrew's included) lack libass/drawtext. Captions are an ffconcat image sequence: one PNG per active-word state, not per frame.

### Persistence and recovery
SQLite in WAL mode with versioned migrations (`PRAGMA user_version`). Render work is a durable job queue (`claim_job` is an atomic `UPDATE … RETURNING`); on start `recover()` re-queues jobs and uploads that were running. Upload session URLs are persisted after every step so YouTube uploads resume instead of restarting.

### Concurrency and resources
Tokio runtime; all channels are bounded and drop (never block) when a consumer lags. Render concurrency defaults to 1. whisper.cpp runs as a short-lived process per clip/utterance (no resident model) and is skipped when free RAM is below a threshold. Capture pauses when free disk drops below `min_free_gb`.

### Platform abstraction
Shared crates contain no OS calls; `pulsecut-platform` holds mic (cpal), keychain (keyring), hotkeys (global-hotkey, polled from the UI thread), autostart (LaunchAgent / HKCU Run), notifications, file reveal/open, and system stats (sysinfo). Process execution (`core::process`) never uses a shell and hides console windows on Windows.

### Extensibility
* Viewer-side clipping (Mode B) fits as another `Backend` + chat source; the coordinator and pipeline are trigger-agnostic.
* Gameplay/visual detectors can push `AudioSecond`-style per-second features or emit `HighlightEvent`s with `TriggerSource::Gameplay`.
* Asset packs are directories with `pack.json`; creator packs install by copying a folder.
