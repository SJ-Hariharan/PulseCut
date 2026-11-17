# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-11-17

First release. Verified on macOS (Apple Silicon) only.

### Added

- Cargo workspace of 13 crates: core, detect, media, edit, meta, store, capture, chat, platform, publish, engine, cli and app. Workspace edition 2024, minimum Rust version 1.88, Apache-2.0 license.
- Desktop application `PulseCut` (egui/eframe) with Dashboard, Live Monitor and Hype Map, Clip Library, Editor, Assets, Publishing and Settings pages.
- Command-line tool `pulsecut-cli` with subcommands `demo`, `daemon`, `eval`, `render`, `chat`, `obs-check`, `encoders`, `set-secret`, `clips`, `publish`, `youtube-connect` and `settings`.
- OBS Studio connection over obs-websocket 5.x with password authentication, replay-buffer clipping with a deferred save, and a buffer-length check against the configured clip window.
- Capture backend that runs an FFmpeg rolling segment buffer for URLs, devices and files, with wall-clock anchoring and a coverage report for each capture.
- Chat ingestion from Twitch IRC (anonymous, read-only), YouTube Live Chat polling and Kick webhooks with signature verification.
- Detection from chat and stream audio, with per-stream adaptive baselines, hysteresis and a per-signal explanation for each detection. Synthetic benchmark and weight tuning through `pulsecut-cli eval`.
- Multi-channel Twitch watching. Each channel in the Twitch channel list becomes a `twitch:<channel>` source, resolved to HLS with yt-dlp, re-resolved on restart, and retried every 30 seconds while offline. The hottest sources are buffered, with a configurable limit. The first channel is the home channel: if OBS is enabled (or a rolling-buffer `segment_source` is set) it keeps that capture instead. For a pure clipper setup, turn OBS off in Settings so every listed channel is recorded from Twitch.
- Vertical rendering to 1080x1920 (also 16:9 and 1:1) with word-level animated captions, reaction text, freeze, punch-in, slow motion, replay, shake and flash effects, and CC0 sound effects with ducking.
- Title, caption, description and hashtag generation from templates, with an optional local LLM endpoint restricted to loopback addresses.
- Thumbnails, SRT subtitles and a JSON decision trail for each clip.
- Ready-to-post folder per clip, `ready-to-post/<clip>/`, containing `video.mp4`, `thumbnail.jpg`, `post.txt` and `post.json` (title, caption, hashtags, description, credit and source link).
- SQLite library with a durable job queue, crash recovery on restart, and resumable uploads.
- YouTube upload through the Data API with OAuth (loopback redirect and PKCE, `youtube.upload` scope). Default privacy is `unlisted`.
- Instagram Reels and TikTok inbox upload code paths, behind settings and not verified against the live services.
- Auto-publish, off by default. It applies only to autonomous clips that meet a minimum confidence, have full coverage, and are under a per-platform daily cap. Worker caps are 90 per day for YouTube, 45 for Instagram and 15 for TikTok.
- Local control API on `127.0.0.1` (default port `47120`, `0` disables) with `GET /api/status`, `POST /api/clip` and `POST /webhooks/kick`.
- Global hotkeys (default clip `CmdOrCtrl+Shift+K`, mark `CmdOrCtrl+Shift+M`), voice command matching for "clip it" and "clip that", and microphone analysis, both off by default.
- Secrets stored in the OS keychain under the service name `app.pulsecut.PulseCut`. They are never written to `settings.toml` or the database.
- `PULSECUT_HOME` environment variable and `--home` option to relocate config, data and cache directories.
- macOS packaging script `scripts/package-macos.sh` producing an ad-hoc-signed `PulseCut.app` and a `.dmg`.
- Windows packaging script `scripts/package-windows.ps1` and an Inno Setup script. Not run yet.
- CI workflow for macOS and Windows that runs formatting, clippy, tests and packaging.

### Changed

- The command-line binary is named `pulsecut-cli`, not `pulsecut`. The names `PulseCut` and `pulsecut` collide on case-insensitive filesystems such as the default macOS volume, so the CLI was renamed before release.

### Known issues

- Windows: the build, packaging, global hotkeys, microphone, autostart, hardware encoders and keychain path are not compiled or run on Windows.
- Linux is not a supported target.
- Intel Macs have no architecture-specific code but have not been tested.
- Live OBS Studio testing covers the connection and replay-buffer path. Long (multi-hour) runs and 1080p60 replays are not measured.
- Instagram Reels, TikTok, Kick webhooks and YouTube Live chat have not been verified against the live services.
- Instagram requires a professional account, a Graph API token and a public HTTPS media host. TikTok uploads land as inbox drafts that must be posted in the TikTok app.
- YouTube may restrict uploads from unverified OAuth apps to private visibility until Google's API audit is complete.
- Detection quality has been measured only on synthetic labelled streams. Accuracy on real, human-labelled streams is not established.
- Voice commands and the live microphone path have unit tests only. They have not been exercised with a live microphone.
- The local API has no authentication beyond binding to `127.0.0.1`.
- The macOS app bundle is ad-hoc signed and is not notarized.
