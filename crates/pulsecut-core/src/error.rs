use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("external tool `{tool}` not found; install it or set its path in Settings")]
    ToolMissing { tool: String },
    #[error("external tool `{tool}` failed (exit {code:?}): {stderr}")]
    ToolFailed {
        tool: String,
        code: Option<i32>,
        stderr: String,
    },
    #[error("external tool `{tool}` timed out after {secs}s")]
    ToolTimeout { tool: String, secs: u64 },
    #[error("invalid settings: {0}")]
    Settings(String),
    #[error("capture window not covered: wanted [{want_start}, {want_end}] ms, have [{have_start}, {have_end}] ms")]
    CaptureGap {
        want_start: i64,
        want_end: i64,
        have_start: i64,
        have_end: i64,
    },
    #[error("unsafe path rejected: {0}")]
    UnsafePath(PathBuf),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn other(msg: impl Into<String>) -> Self {
        Error::Other(msg.into())
    }
}
