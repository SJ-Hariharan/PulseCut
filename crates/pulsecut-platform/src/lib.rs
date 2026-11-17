//! Platform abstractions. Shared crates never call OS APIs directly; they go through here.
//!
//! | Concern            | macOS                         | Windows                          |
//! |--------------------|-------------------------------|----------------------------------|
//! | Microphone         | cpal (CoreAudio)              | cpal (WASAPI)                    |
//! | Secrets            | Keychain (keyring)            | Credential Manager (keyring)     |
//! | Global hotkeys     | global-hotkey (Carbon)        | global-hotkey (RegisterHotKey)   |
//! | Launch at login    | ~/Library/LaunchAgents plist  | HKCU\…\Run registry value         |
//! | Notifications      | osascript display notification | in-app toast (no extra deps)    |
//! | Disk / CPU / RAM   | sysinfo                       | sysinfo                          |

pub mod autostart;
pub mod hotkeys;
pub mod mic;
pub mod notify;
pub mod secrets;
pub mod shell;
pub mod sysstats;
