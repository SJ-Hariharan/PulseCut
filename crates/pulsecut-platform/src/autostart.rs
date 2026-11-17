//! Launch at login. macOS: a per-user LaunchAgent plist. Windows: HKCU Run value via `reg.exe`
//! (argument vector, no shell).

use pulsecut_core::{Error, Result};
use std::path::{Path, PathBuf};

pub const LABEL: &str = "app.pulsecut.PulseCut";

pub fn launch_agent_plist(exe: &Path, minimized: bool) -> String {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let mut args = format!("        <string>{}</string>\n", esc(&exe.display().to_string()));
    if minimized {
        args.push_str("        <string>--minimized</string>\n");
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
{args}    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>ProcessType</key>
    <string>Interactive</string>
</dict>
</plist>
"#
    )
}

fn agent_path() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|b| b.home_dir().join("Library/LaunchAgents").join(format!("{LABEL}.plist")))
}

pub fn is_enabled() -> bool {
    if cfg!(target_os = "macos") {
        agent_path().is_some_and(|p| p.exists())
    } else if cfg!(windows) {
        std::process::Command::new("reg")
            .args([
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                "PulseCut",
            ])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    } else {
        false
    }
}

pub fn set_enabled(on: bool, minimized: bool) -> Result<()> {
    let exe = std::env::current_exe()?;
    if cfg!(target_os = "macos") {
        let p = agent_path().ok_or_else(|| Error::other("no home directory"))?;
        if on {
            std::fs::create_dir_all(p.parent().unwrap())?;
            std::fs::write(&p, launch_agent_plist(&exe, minimized))?;
        } else if p.exists() {
            std::fs::remove_file(&p)?;
        }
        Ok(())
    } else if cfg!(windows) {
        let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
        let status = if on {
            let mut cmd = format!("\"{}\"", exe.display());
            if minimized {
                cmd.push_str(" --minimized");
            }
            std::process::Command::new("reg")
                .args(["add", key, "/v", "PulseCut", "/t", "REG_SZ", "/d", &cmd, "/f"])
                .status()?
        } else {
            std::process::Command::new("reg")
                .args(["delete", key, "/v", "PulseCut", "/f"])
                .status()?
        };
        if status.success() || !on {
            Ok(())
        } else {
            Err(Error::other("reg.exe failed"))
        }
    } else {
        Err(Error::other("launch at login is not supported on this platform"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plist_is_well_formed() {
        let p = launch_agent_plist(
            Path::new("/Applications/PulseCut.app/Contents/MacOS/pulsecut-app"),
            true,
        );
        assert!(p.contains("<string>app.pulsecut.PulseCut</string>"));
        assert!(p.contains("--minimized"));
        assert!(p.contains("<key>RunAtLoad</key>"));
    }
}
