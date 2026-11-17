# Contributing to PulseCut

Thanks for taking the time to contribute. PulseCut is an Apache-2.0 licensed Rust desktop app and CLI for local-first livestream clipping. This guide covers how to set up a development environment, what CI checks, and how to get a change merged.

## Ways to help

* Report bugs and request features using the issue templates.
* Improve documentation under `docs/` and the README.
* Send focused pull requests for bug fixes, tests, and features that fit the project's local-first goals.
* For security problems, do not open a public issue. See [SECURITY.md](SECURITY.md).

For larger changes, open an issue first so the approach can be agreed before you invest time in it.

## Development setup

Requirements:

* **Rust 1.88 or newer** (the workspace sets `rust-version = "1.88"`), installed with `rustup`. Include the `clippy` and `rustfmt` components.
* **FFmpeg and ffprobe on your `PATH`**. Required for rendering and for the render integration tests.
  * macOS: `brew install ffmpeg`
  * Windows: `choco install ffmpeg -y`, or any FFmpeg build with `ffmpeg.exe` and `ffprobe.exe` on `PATH`
* **Optional: whisper.cpp** (`whisper-cli`) for caption transcription. macOS: `brew install whisper-cpp`. Download a model into `models/` as described in the README.
* **Optional: yt-dlp** for resolving platform live URLs in the segment capture path.

Clone the repository and build:

```sh
cargo build --workspace
```

On macOS you may want to run `source ./env.sh` first. It points Cargo at the repository-local `.cargo-home` and adjusts `PATH` for Homebrew's rustup.

Run the application:

```sh
cargo run -p pulsecut-app           # desktop app (binary: PulseCut)
cargo run -p pulsecut-cli -- --help # command-line tool (binary: pulsecut-cli)
```

## Checks that CI runs

CI runs on macOS (`macos-14`) and Windows (`windows-latest`). Run these locally before opening a pull request:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --release
```

Notes:

* Clippy warnings fail the build. Fix them rather than adding `#[allow(...)]`, unless the allow is narrowly scoped and explained in a comment.
* Some integration tests start a local mock OBS server or a mock YouTube API server bound to `127.0.0.1` on an ephemeral port. Your environment or sandbox must allow binding to loopback addresses for those tests to pass.
* The real-render tests (`crates/pulsecut-media/tests/render_real.rs`) need FFmpeg on `PATH` and return early when it is not found.
* Run `cargo fmt --all` (without `--check`) to format your changes.

## Workspace layout

The repository is a Cargo workspace. Each crate has one responsibility:

| Crate | Responsibility |
|---|---|
| `pulsecut-core` | Domain model, settings, paths, safe process runner |
| `pulsecut-detect` | Streaming detection, synthetic datasets, evaluation and tuning |
| `pulsecut-media` | FFmpeg render graphs, captions, whisper, thumbnails, SFX, asset packs |
| `pulsecut-edit` | Effect decision engine (recipes; no rendering) |
| `pulsecut-meta` | Titles, captions, hashtags (templates and optional local LLM) |
| `pulsecut-store` | SQLite: clips, durable job queue, uploads, events |
| `pulsecut-capture` | OBS controller, rolling segment buffer, capture coordinator, mock OBS |
| `pulsecut-chat` | Twitch IRC, YouTube Live Chat, Kick webhooks |
| `pulsecut-publish` | OAuth (PKCE), YouTube, Instagram, TikTok, upload worker |
| `pulsecut-platform` | Microphone, OS keychain, hotkeys, autostart, notifications, system stats |
| `pulsecut-engine` | Orchestrator, render pipeline, multi-stream, voice, local API, demo |
| `pulsecut-cli` | `pulsecut-cli` command-line binary |
| `pulsecut-app` | `PulseCut` desktop application (egui) |

## Coding guidelines

* Formatting is controlled by `rustfmt.toml` (`max_width = 120`). Do not hand-tune formatting against it.
* Write code that compiles cleanly under `clippy --all-targets -D warnings`, including tests.
* Prefer returning `Result` with the crate's error type over panicking in library code. `unwrap` and `expect` are acceptable in tests and for invariants that are truly impossible to violate.
* Run external programs (FFmpeg, ffprobe, whisper-cli, yt-dlp) with an argument vector, never through a shell, and give each invocation a timeout.
* Keep the default behaviour local-first. New network calls, telemetry, or background recording need a clear reason and must be opt-in, and the user must be able to see them in Settings.
* Add or update tests with the code they cover. Tests that need external tools should skip cleanly when the tool is missing.
* Keep comments short and explain *why*, not *what*.
* Update the relevant file under `docs/` (for example `USER_GUIDE.md`, `TESTING.md`, or `PLATFORM_SUPPORT.md`) when user-facing behaviour, setup, or platform status changes.

## Secrets and personal data

* **Never commit secrets.** This includes OAuth client secrets, access or refresh tokens, API keys, OBS passwords, webhook public keys you consider private, and signing certificates.
* Credentials belong in the OS credential store (macOS Keychain or Windows Credential Manager), entered from the Settings screen. They are not read from `.env` files or from environment variables.
* User settings and databases live outside the repository in PulseCut's data directory, or in the directory set by `PULSECUT_HOME` (see `.env.example`). Do not check in `settings.toml`, `*.sqlite`, clips, recordings, or logs.
* Do not paste real tokens, channel credentials, or private file paths into issues, pull requests, test fixtures, or sample output. Use placeholders.
* If you accidentally commit a secret, treat it as compromised: revoke it at the provider first, then remove it from history, and tell the maintainer.

## Commit messages

* Write the subject in the imperative mood, for example `Add Kick signature replay window test`, not `Added` or `Adds`.
* Keep the subject line concise (about 72 characters or fewer). Use the body to explain why the change is needed when that is not obvious.
* Keep one logical change per commit where practical.

## Pull request process

1. Fork the repository and create a topic branch from `main`.
2. Make your change, including tests and documentation updates.
3. Run the three CI checks listed above and confirm they pass locally.
4. Open a pull request. Fill in the template: summary, testing done, and the checklist.
5. A maintainer reviews the pull request. Please respond to review comments, and push follow-up commits rather than force-pushing over reviewed history, unless asked.
6. Once CI is green and the review is approved, a maintainer merges the pull request.

Keep pull requests focused. A small, well-tested change is easier to review and more likely to merge quickly than a large mixed one.

## Reporting security issues

Do not file security vulnerabilities as public issues or pull requests. Use GitHub's private vulnerability reporting (Security tab, then **Report a vulnerability**). The full process, supported versions, and expectations are in [SECURITY.md](SECURITY.md).

## Code of conduct

Participation in this project is governed by the [Code of Conduct](CODE_OF_CONDUCT.md).

## License

By contributing, you agree that your contributions are licensed under the Apache License, Version 2.0, the same license as the project. See [LICENSE](LICENSE).
