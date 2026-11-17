//! Desktop notifications. macOS uses `osascript` (argument vector; the message is passed as a
//! script argument, never interpolated into script source). Elsewhere the app shows in-app toasts.

pub fn desktop(title: &str, body: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        let script = "on run argv\n display notification (item 2 of argv) with title (item 1 of argv)\nend run";
        return std::process::Command::new("osascript")
            .args(["-e", script, title, body])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    }
    #[allow(unreachable_code)]
    {
        let _ = (title, body);
        false
    }
}
