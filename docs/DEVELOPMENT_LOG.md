# Development log

## 2026-11-17 — initial build

**Environment audit.** Empty project directory. Rust 1.99.1 (Homebrew rustup), OBS 33.0.1 installed (WebSocket server disabled), whisper.cpp 1.9.5, no FFmpeg. With approval: installed FFmpeg 9.0.2 via Homebrew and downloaded `ggml-base.en.bin`.

**Research** (three parallel agents) → RESEARCH.md / REUSE_MATRIX.md. Key findings that shaped the design: OBS `SaveReplayBuffer` has no post-roll (→ deferred save); Twitch anonymous IRC still supported; YouTube `videos.insert` now has its own ~100/day bucket; Instagram needs a public video URL; reference detection ideas worth reimplementing are in sharkyclips (MIT).

**Decisions made during implementation**
* Homebrew FFmpeg has no libass/drawtext → all text rendered in Rust (ab_glyph) and overlaid as PNG/ffconcat. More portable anyway.
* Detection weights tuned on synthetic labelled data, validated on held-out seeds; raw chat velocity ended up weighted 0 after adding "organic surge" negatives.
* Overlapping-moment recall was 0.75 because clips ended at moment+post-roll; fixed by extending clips over the activity span (recall 1.00).
* Laughter heuristic flagged an impact sound as laughter in the live app; fixed by high-passing the envelope and requiring oscillation + ≥3 bursts (new regression test).
* Thumbnails grabbed from the rendered clip duplicated burnt-in captions → covers now come from clean raw frames through the clip's layout.
* Sandbox findings: VideoToolbox and whisper Metal fail inside the dev sandbox (auto-fallback verified); local port binding and `.gitmodules` crate unpacks need the sandbox disabled.
* egui 0.37 API (`App::ui`, `Panel`) and sysinfo 0.40 (`Result` constructors), sqlx 0.9 (`AssertSqlSafe`) adapted after reading crate sources.
* Default egui fonts lack some symbols (●, →, icons) → removed from the UI.

**Verified end-to-end** with `pulsecut-cli demo` (149 s) and in the packaged desktop app; 96 tests green; clippy/fmt clean. See TESTING.md.

**Open**: live OBS, real uploads, Windows, real-stream evaluation.
