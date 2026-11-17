# Performance

Machine: Apple M1 (4P+4E), 8 GB RAM, macOS 15, FFmpeg 9.0.2 (Homebrew), whisper.cpp 1.9.5. Release build. Single runs unless stated; expect ±10–15 % noise.

## Rendering

30.9 s clip, 1080×1920 @ 30 fps, full `fail_freeze` recipe (blur-fill layout, freeze, punch-in via zoompan, reaction text, 2 SFX with ducking, loudnorm), 1280×720 H.264 source.

| Encoder | Wall time | CPU time (user) | Peak RSS (FFmpeg + PulseCut) |
|---|---|---|---|
| h264_videotoolbox (hardware) | **15.6 s** (2.0× real time) | 45 s | 0.90 GB |
| libx264 veryfast crf 20 | 19.7 s (1.6× real time) | 81 s | 1.23 GB |

* Hardware encoding is chosen only after a real test encode passes on the machine (`pulsecut-cli encoders`). Inside a restricted macOS sandbox, VideoToolbox cannot create a compression session (`-12908`) and PulseCut falls back to libx264 — verified.
* Most remaining time is filtering (blur-fill scaling, zoompan), not encoding. The blurred background is computed at ¼ resolution.
* Default render concurrency is 1 to keep peak memory around 1 GB on 8 GB machines.

## Transcription

whisper.cpp `base.en`, CPU (`-ng`, 4 threads): 11 s of speech in 4.5 s. GPU (Metal) is attempted first and is used when available outside the sandbox. Transcription is skipped when free RAM < 700 MB (configurable).

## Detection (synthetic benchmark)

`pulsecut-cli eval --seeds 3` — 18 scenarios × 3 seeds (quiet 0.15 msg/s, medium 2 msg/s, large 25 msg/s channels, weak moments, overlapping moments, a slow-growth negative stream, and hard negatives: single-user spam, bot rings, poll votes, giveaway commands, organic chat surges, streamer shouts, loud steady game audio). A label is recalled when a planned clip contains the moment with ≥ 2 s of context.

| Weights | Data | Precision | Recall | F1 | Moment within 3 s |
|---|---|---|---|---|---|
| Shipped (tuned) | training seeds | 1.000 | 0.974 | 0.987 | 137/151 |
| Shipped (tuned) | **held-out seeds** | **1.000** | **0.962** | **0.980** | 139/150 |
| Uniform (untuned) | held-out seeds | 0.991 | 0.859 | 0.920 | 127/134 |
| Shipped, chat only | held-out | 0.985 | 0.981 | 0.983 | |
| Shipped, audio only | held-out | 1.000 | 0.096 | 0.175 | |

Tuning (coordinate-descent grid search, 105 candidates, ~60 s on 4 cores) set raw chat velocity's weight to 0 — organic surges fooled it — and raised reactions/emotes. Weakest category: "medium-weak" moments (2–3× chat rate), recall 0.80.

**Caveat:** the generator and the detector were written by the same people; these numbers show the detector behaves as designed on the modelled phenomena, not that it is accurate on real streams. Audio-only detection is weak in this benchmark (synthetic audio spikes exist on 60 % of moments and the audio score is deliberately conservative). The laughter feature is a heuristic (envelope pulsing at 3–7 Hz with ≥ 3 bursts) and is not a trained classifier.

Full output: `research/eval-report.txt`.

## Runtime footprint

* Desktop app idle with one rolling buffer and Twitch chat: ~80–100 MB RSS (PulseCut process), CPU dominated by the FFmpeg buffer process (stream copy for real sources; libx264 re-encode for the demo's file source).
* Detection costs < 1 ms per stream per second; tested with 7 simultaneous Twitch channels.
* Chat/audio channels are bounded (8192 / 512); slow consumers drop data rather than block capture.

## Not measured yet

Windows encoders (NVENC/QSV/AMF/MF), long (multi-hour) runs, real 1080p60 OBS replays, multi-stream buffers on real HLS sources.
