# Platform support

| Area | macOS (first-class) | Windows 10/11 (second) |
|---|---|---|
| Build | ✅ built and tested (Apple Silicon, Rust 1.99.1) | ⚠️ CI job defined (`.github/workflows/ci.yml`), **not yet compiled** |
| Packaging | ✅ `scripts/package-macos.sh` → ad-hoc-signed `.app` (+ `.dmg` in release); mic usage string in Info.plist | ⚠️ `scripts/package-windows.ps1` → zip + Inno Setup script (`packaging/windows/pulsecut.iss`), untested |
| Global hotkeys | ✅ global-hotkey (Carbon), registered on the UI thread | code path: global-hotkey `RegisterHotKey` |
| Microphone | cpal/CoreAudio (resampled to 16 kHz); permission prompt via Info.plist | cpal/WASAPI |
| Secrets | Keychain (keyring 4) | Credential Manager (keyring 4) |
| Launch at login | ✅ LaunchAgent plist (`~/Library/LaunchAgents/app.pulsecut.PulseCut.plist`) | HKCU `…\Run` via `reg.exe` |
| Notifications | `osascript display notification` (argv only) + in-app toasts | in-app toasts |
| File paths | `directories` (Application Support) | `directories` (%APPDATA%) |
| Processes | no shell; kill-on-drop | no shell; `CREATE_NO_WINDOW` so FFmpeg never flashes a console |
| H.264 encoders | ✅ VideoToolbox verified (falls back to libx264 when the test encode fails) | NVENC → QSV → AMF → Media Foundation → libx264, each test-encoded before use (untested) |
| Fonts | Arial Black / Impact / Arial Bold from the system | ariblk / impact / arialbd / segoeuib from C:\Windows\Fonts |
| Capture devices | `device:` → avfoundation | `device:` → dshow |
| Intel Macs | expected to work (no arch-specific code); not tested | — |

Shared crates (`core`, `detect`, `media`, `edit`, `meta`, `store`, `capture`, `chat`, `publish`, `engine`) contain no OS-specific code apart from `cfg!` defaults for tool/font locations.

## Known macOS behaviours

* A newly built (re-signed) bundle triggers privacy prompts when it reads `~/Desktop`/`~/Documents`; the default library is in Application Support, which is not protected.
* Inside sandboxes that block hardware/Metal (e.g. dev sandboxes), VideoToolbox and whisper's GPU path fail; PulseCut detects this and uses libx264 / CPU whisper.
* Tests that bind local ports need permission to listen on 127.0.0.1.

## Windows work remaining

Compile on CI, fix any `cfg(windows)` issues, verify hotkeys/mic/autostart, benchmark hardware encoders, sign the installer.
