//! Shell-free execution of the small set of external native tools PulseCut relies on
//! (ffmpeg, ffprobe, whisper-cli). Arguments are passed as a vector — never through a shell —
//! and every run has a timeout and is killed when dropped.

use crate::error::{Error, Result};
use crate::settings::ToolSettings;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Ffmpeg,
    Ffprobe,
    WhisperCli,
}

impl Tool {
    pub fn binary_name(self) -> &'static str {
        match self {
            Tool::Ffmpeg => "ffmpeg",
            Tool::Ffprobe => "ffprobe",
            Tool::WhisperCli => "whisper-cli",
        }
    }
}

/// Resolved tool locations.
#[derive(Debug, Clone, Default)]
pub struct Tools {
    pub ffmpeg: Option<PathBuf>,
    pub ffprobe: Option<PathBuf>,
    pub whisper_cli: Option<PathBuf>,
    pub whisper_model: Option<PathBuf>,
}

impl Tools {
    /// Resolve tools: explicit settings first, then PATH, then well-known install locations.
    pub fn discover(cfg: &ToolSettings, extra_model_dirs: &[PathBuf]) -> Tools {
        let find = |explicit: &str, tool: Tool| -> Option<PathBuf> {
            if !explicit.is_empty() {
                let p = PathBuf::from(explicit);
                return p.is_file().then_some(p);
            }
            if let Ok(p) = which::which(tool.binary_name()) {
                return Some(p);
            }
            let exe = if cfg!(windows) {
                format!("{}.exe", tool.binary_name())
            } else {
                tool.binary_name().to_string()
            };
            let candidates: &[&str] = if cfg!(target_os = "macos") {
                &["/opt/homebrew/bin", "/usr/local/bin"]
            } else if cfg!(windows) {
                &[
                    "C:\\ffmpeg\\bin",
                    "C:\\Program Files\\ffmpeg\\bin",
                    "C:\\Program Files\\whisper.cpp",
                ]
            } else {
                &["/usr/bin", "/usr/local/bin"]
            };
            candidates.iter().map(|d| Path::new(d).join(&exe)).find(|p| p.is_file())
        };
        let whisper_model = if !cfg.whisper_model.is_empty() {
            let p = PathBuf::from(&cfg.whisper_model);
            p.is_file().then_some(p)
        } else {
            let names = [
                "ggml-base.en.bin",
                "ggml-small.en.bin",
                "ggml-tiny.en.bin",
                "ggml-base.bin",
            ];
            extra_model_dirs
                .iter()
                .flat_map(|d| names.iter().map(move |n| d.join(n)))
                .find(|p| p.is_file())
        };
        Tools {
            ffmpeg: find(&cfg.ffmpeg, Tool::Ffmpeg),
            ffprobe: find(&cfg.ffprobe, Tool::Ffprobe),
            whisper_cli: find(&cfg.whisper_cli, Tool::WhisperCli),
            whisper_model,
        }
    }

    pub fn path(&self, tool: Tool) -> Result<&Path> {
        let p = match tool {
            Tool::Ffmpeg => &self.ffmpeg,
            Tool::Ffprobe => &self.ffprobe,
            Tool::WhisperCli => &self.whisper_cli,
        };
        p.as_deref().ok_or_else(|| Error::ToolMissing {
            tool: tool.binary_name().into(),
        })
    }

    pub fn ffmpeg(&self) -> Result<&Path> {
        self.path(Tool::Ffmpeg)
    }
    pub fn ffprobe(&self) -> Result<&Path> {
        self.path(Tool::Ffprobe)
    }
}

#[derive(Debug)]
pub struct Output {
    pub stdout: Vec<u8>,
    pub stderr: String,
}

/// Run a tool to completion with a timeout. stderr is captured (truncated to the last 4 KB
/// in errors so FFmpeg's verbose logs don't flood the UI).
pub async fn run(program: &Path, args: &[String], timeout: Duration) -> Result<Output> {
    let name = program
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    tracing::debug!(tool = %name, ?args, "run");
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    hide_console_window(&mut cmd);
    let child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::ToolMissing { tool: name.clone() }
        } else {
            Error::Io(e)
        }
    })?;
    let out = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(r) => r?,
        Err(_) => {
            return Err(Error::ToolTimeout {
                tool: name,
                secs: timeout.as_secs(),
            });
        }
    };
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if !out.status.success() {
        return Err(Error::ToolFailed {
            tool: name,
            code: out.status.code(),
            stderr: tail(&stderr, 4096),
        });
    }
    Ok(Output {
        stdout: out.stdout,
        stderr,
    })
}

pub fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    s[start..].to_string()
}

/// On Windows, prevent a console window from flashing for every FFmpeg call.
#[allow(unused_variables)]
pub fn hide_console_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_respects_char_boundaries() {
        let s = "ééééé";
        let t = tail(s, 3);
        assert!(t.len() <= 4);
        assert!(s.ends_with(&t));
    }

    #[tokio::test]
    async fn missing_tool_reports_cleanly() {
        let r = run(Path::new("/definitely/not/here/ffmpeg"), &[], Duration::from_secs(1)).await;
        assert!(matches!(r, Err(Error::ToolMissing { .. }) | Err(Error::Io(_))));
    }
}
