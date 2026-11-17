//! Global hotkeys. Must be created on the main thread of an app with an event loop
//! (eframe/winit provides it); events are polled from the UI loop.

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use pulsecut_core::{Error, Result};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    Clip,
    Mark,
}

pub struct Hotkeys {
    _mgr: GlobalHotKeyManager,
    bindings: Vec<(u32, HotkeyAction, String)>,
}

/// Validate an accelerator string like "CmdOrCtrl+Shift+K".
pub fn parse(accel: &str) -> Result<HotKey> {
    HotKey::from_str(accel).map_err(|e| Error::Settings(format!("invalid hotkey '{accel}': {e}")))
}

impl Hotkeys {
    pub fn register(clip: &str, mark: &str) -> Result<Hotkeys> {
        let mgr = GlobalHotKeyManager::new().map_err(|e| Error::other(format!("hotkeys unavailable: {e}")))?;
        let mut bindings = vec![];
        for (accel, action) in [(clip, HotkeyAction::Clip), (mark, HotkeyAction::Mark)] {
            if accel.trim().is_empty() {
                continue;
            }
            let hk = parse(accel)?;
            mgr.register(hk)
                .map_err(|e| Error::other(format!("could not register {accel} (in use by another app?): {e}")))?;
            bindings.push((hk.id(), action, accel.to_string()));
        }
        Ok(Hotkeys { _mgr: mgr, bindings })
    }

    /// Drain pending key presses.
    pub fn poll(&self) -> Vec<HotkeyAction> {
        let mut out = vec![];
        while let Ok(ev) = GlobalHotKeyEvent::receiver().try_recv() {
            if ev.state == HotKeyState::Pressed
                && let Some((_, a, _)) = self.bindings.iter().find(|(id, _, _)| *id == ev.id)
            {
                out.push(*a);
            }
        }
        out
    }

    pub fn describe(&self) -> Vec<String> {
        self.bindings.iter().map(|(_, a, s)| format!("{a:?}: {s}")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_accelerators() {
        assert!(parse("CmdOrCtrl+Shift+K").is_ok());
        assert!(parse("Alt+F9").is_ok());
        assert!(parse("NotAKey+++").is_err());
    }
}
