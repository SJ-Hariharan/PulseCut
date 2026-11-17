//! Open files/URLs, reveal in the file manager, preview sounds. Always an argument vector,
//! never a shell string.

use std::path::Path;
use std::process::Command;

pub fn open(target: &str) -> bool {
    let r = if cfg!(target_os = "macos") {
        Command::new("open").arg(target).spawn()
    } else if cfg!(windows) {
        Command::new("explorer").arg(target).spawn()
    } else {
        Command::new("xdg-open").arg(target).spawn()
    };
    r.is_ok()
}

pub fn reveal(path: &Path) -> bool {
    let r = if cfg!(target_os = "macos") {
        Command::new("open").arg("-R").arg(path).spawn()
    } else if cfg!(windows) {
        Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn()
    } else {
        Command::new("xdg-open").arg(path.parent().unwrap_or(path)).spawn()
    };
    r.is_ok()
}

/// Play a sound file asynchronously (ffplay if installed, else the OS player). `volume` 0..2.
pub fn play_sound(ffplay: Option<&Path>, path: &Path, volume: f32) -> bool {
    if let Some(fp) = ffplay {
        let vol = ((volume.clamp(0.0, 2.0)) * 100.0) as i32;
        return Command::new(fp)
            .args([
                "-nodisp",
                "-autoexit",
                "-loglevel",
                "quiet",
                "-volume",
                &vol.min(100).to_string(),
            ])
            .arg(path)
            .spawn()
            .is_ok();
    }
    if cfg!(target_os = "macos") {
        return Command::new("afplay")
            .arg("-v")
            .arg(format!("{:.2}", volume))
            .arg(path)
            .spawn()
            .is_ok();
    }
    open(&path.display().to_string())
}
