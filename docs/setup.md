# Setup: install from source

This guide builds PulseCut from source and runs it on macOS. macOS is the first-class platform for 0.1.0. Windows is included but untested (see the last section). Linux is not supported.

For day-to-day use after installation, see [USER_GUIDE.md](USER_GUIDE.md). For settings, see [configuration.md](configuration.md). For problems, see [troubleshooting.md](troubleshooting.md).

## 1. Install the tools (Homebrew)

Install [Homebrew](https://brew.sh) if you do not have it, then:

```bash
# Rust toolchain manager. PulseCut requires Rust 1.88 or newer.
brew install rustup
rustup-init -y
source "$HOME/.cargo/env"
rustup update stable
rustc --version          # must print 1.88.0 or newer

# Required: FFmpeg (rendering, segment buffer, probing).
brew install ffmpeg

# Optional but recommended: captions and voice commands.
brew install whisper-cpp
```

Optional items:

| Tool | Needed for | Install |
|---|---|---|
| whisper.cpp + a ggml model | Captions, voice commands. Without a model PulseCut still clips, just without captions. | `brew install whisper-cpp`, then see step 2 |
| yt-dlp | Clipping `twitch:<channel>` and `youtube:` inputs | `brew install yt-dlp` |
| OBS Studio | Streamer mode (OBS replay buffer) | `brew install --cask obs`, or download from the OBS website |

PulseCut looks for `ffmpeg`, `ffprobe` and `whisper-cli` in this order: the path set in `[tools]` in settings, then your `PATH`, then `/opt/homebrew/bin` and `/usr/local/bin`. yt-dlp is looked up at `/opt/homebrew/bin/yt-dlp`, then `/usr/local/bin/yt-dlp`, then `PATH`. The Homebrew whisper formula installs the binary as `whisper-cli`, which is the name PulseCut expects.

## 2. Get a whisper model (optional)

PulseCut loads the first model it finds from this list, in this order of folders: the path in `[tools] whisper_model` if set, then `~/Library/Application Support/app.PulseCut.PulseCut/models/`, then `models/` in the current directory, then a `models/` folder next to the executable.

Recommended file names, in order of preference: `ggml-base.en.bin`, `ggml-small.en.bin`, `ggml-tiny.en.bin`, `ggml-base.bin`.

Download a model with the script that ships with whisper.cpp, then copy it into the models folder:

```bash
mkdir -p "$HOME/Library/Application Support/app.PulseCut.PulseCut/models"
# run inside a whisper.cpp source checkout (its models/ folder holds the download script):
bash ./models/download-ggml-model.sh base.en
cp ./models/ggml-base.en.bin "$HOME/Library/Application Support/app.PulseCut.PulseCut/models/"
```

`ggml-base.en.bin` is about 142 MB. `ggml-tiny.en.bin` is faster for voice commands.

## 3. Build

From the repository root:

```bash
cargo build --release -p pulsecut-app -p pulsecut-cli
```

This produces two binaries in `target/release/`:

* `PulseCut`: the desktop app (GUI).
* `pulsecut-cli`: the command-line tool (headless daemon, diagnostics, evaluation, rendering).

Run the tests if you want to confirm the build (some tests bind local ports and need a shell that allows it):

```bash
cargo test --workspace --release
```

## 4. Package the app

```bash
scripts/package-macos.sh release
```

The script rebuilds in release mode if needed and writes:

* `target/dist/PulseCut.app`: the app bundle. `pulsecut-cli` is placed inside it at `Contents/MacOS/pulsecut-cli`.
* `target/dist/PulseCut-<version>-macos-<arch>.dmg`: a disk image, created when `hdiutil` is available.

The bundle is signed ad hoc (`codesign --sign -`). This lets macOS track privacy permissions for the app, but it is not a Developer ID signature, so Gatekeeper will treat it as an unidentified developer app on other machines.

Copy the app to Applications:

```bash
cp -R target/dist/PulseCut.app /Applications/
```

## 5. First launch

1. Open `PulseCut` from Applications. On first launch macOS may block it as coming from an unidentified developer. Open it through **System Settings → Privacy & Security** and allow it, or right-click the app and choose **Open**.
2. PulseCut stores its secrets (OBS password, YouTube and other tokens) in the macOS Keychain. Because the app is ad-hoc signed, **every rebuild changes its signature**. The next time the app reads a keychain item, macOS asks whether PulseCut may access it. Choose **Always Allow** to stop the prompt for that item. After you rebuild, expect the prompt again.
3. If you enable the microphone (voice commands or mic analysis), macOS asks for microphone permission once. The prompt text is set in the app bundle.
4. PulseCut creates its folders on first start. Settings and data locations are listed in [configuration.md](configuration.md#where-settings-live).

Check the installation from a terminal:

```bash
target/release/pulsecut-cli encoders   # tests the hardware and software H.264 encoders
target/release/pulsecut-cli demo       # end-to-end run with synthetic input, no accounts needed
```

`pulsecut-cli demo` is a good first test. It exercises detection, capture, rendering, captions and the library without OBS or any account.

## 6. Connect OBS (optional)

See the "Streamer mode" section of [USER_GUIDE.md](USER_GUIDE.md#2-streamer-mode-obs). In short: enable the Replay Buffer in OBS, enable the OBS WebSocket server, then save the WebSocket password in PulseCut's Settings with **Save to keychain**. Check the connection with:

```bash
target/release/pulsecut-cli obs-check
```

## Windows (untested in 0.1.0)

Windows support is included in the source tree but has **not been compiled or run for the 0.1.0 release**. Treat it as experimental.

1. Install the Rust toolchain from [rustup.rs](https://rustup.rs) (Rust 1.88 or newer).
2. Install FFmpeg and whisper.cpp, for example `winget install Gyan.FFmpeg`. Put `ffmpeg.exe`, `ffprobe.exe` and `whisper-cli.exe` on your `PATH`, or set their paths in `[tools]` in settings.
3. Build and package from PowerShell in the repository root:

   ```powershell
   scripts/package-windows.ps1
   ```

   The script builds the release binaries and produces a zip. It also includes an Inno Setup script at `packaging/windows/pulsecut.iss`.

Windows secrets go to Credential Manager, and settings go under the roaming application data folder (see [configuration.md](configuration.md#where-settings-live)).

## Linux

Not supported in 0.1.0. There is no packaging script or test target for Linux.
