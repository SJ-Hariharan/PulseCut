# Licensing

## PulseCut's own code
PulseCut is licensed under **Apache-2.0** (see [LICENSE](../LICENSE)).

## Code reuse
**No source code was copied from any reference project.** Ideas from MIT-licensed projects were independently reimplemented in Rust and are attributed in [REUSE_MATRIX.md](REUSE_MATRIX.md) (sharkyclips: count-excitement min(ratio, z) and agreement bonus; loudness excess/jump scoring; stream-clipper: overlap merging; PeakCut: caption pagination on pauses). AGPL-3.0 projects (clips-studio, LocalClip) were studied for ideas only.

## Media assets
* **Sound effects**: the 13 built-in sounds (`pulsecut-core` pack) are procedurally synthesized by PulseCut's own code (`crates/pulsecut-media/src/sfx.rs`) and released as **CC0-1.0**. No third-party or meme audio is bundled.
* **Fonts**: none bundled; PulseCut uses fonts installed on the user's system at runtime (e.g. Arial Black on macOS).
* **Speech sample in the demo**: whisper.cpp's `jfk.wav` (a U.S. government recording, public domain) is read from the user's whisper.cpp install, not redistributed.
* **User-imported assets** require a license or usage-rights statement and record attribution; creator packs must declare a license in `pack.json`.

## External tools (not bundled)
| Tool | License | Notes |
|---|---|---|
| FFmpeg | LGPL-2.1+/GPL (build-dependent; Homebrew build is GPL with x264/x265) | invoked as a separate process; if you redistribute FFmpeg binaries with PulseCut, comply with its license for that build |
| whisper.cpp | MIT | separate process |
| whisper models (ggml) | MIT (OpenAI Whisper weights) | downloaded by the user |
| OBS Studio | GPL-2.0 | controlled over its WebSocket API only |

## Rust dependencies
Key crates and their published licenses are listed in REUSE_MATRIX.md. All are permissive (MIT/Apache-2.0/BSD/Zlib/ISC/Unicode/MPL-2.0 for some transitive crates) as commonly published; **run `cargo deny check licenses` before a release** to produce the authoritative list.
