//! Publishing through official APIs only. Nothing is uploaded unless the user approves a clip,
//! or explicitly enables auto-publish (with confidence + daily caps).
//!
//! * `oauth`     – installed-app OAuth 2.0: loopback redirect + PKCE, token refresh
//! * `youtube`   – Data API v3 resumable upload (resume after failure), privacy, scheduling
//! * `instagram` – Graph API Reels container → status poll → publish, via a hosting adapter
//! * `tiktok`    – Content Posting API inbox upload (FILE_UPLOAD) — experimental
//! * `worker`    – the upload queue: retries with backoff, quota/rate limits, duplicate guard

pub mod instagram;
pub mod oauth;
pub mod tiktok;
pub mod worker;
pub mod youtube;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UploadMeta {
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub privacy: String,
    /// RFC 3339 publish time (YouTube requires privacy=private when scheduling).
    pub publish_at: Option<String>,
    pub category_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PublishError {
    /// Retry later (network, 5xx, 429).
    Transient(String),
    /// Access token expired/invalid; refresh then retry.
    Unauthorized(String),
    /// Daily quota or rate limit hit; retry after the given seconds.
    Quota { message: String, retry_after_secs: u64 },
    /// Won't succeed without user action (bad config, rejected media, missing permission).
    Permanent(String),
}

impl std::fmt::Display for PublishError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PublishError::Transient(m) => write!(f, "temporary failure: {m}"),
            PublishError::Unauthorized(m) => write!(f, "authorization failed: {m}"),
            PublishError::Quota {
                message,
                retry_after_secs,
            } => {
                write!(f, "quota/rate limit: {message} (retry in {retry_after_secs}s)")
            }
            PublishError::Permanent(m) => write!(f, "{m}"),
        }
    }
}

impl From<reqwest::Error> for PublishError {
    fn from(e: reqwest::Error) -> Self {
        PublishError::Transient(e.to_string())
    }
}

/// Map an HTTP failure to a retry class.
pub fn classify(status: u16, body: &str) -> PublishError {
    let short: String = body.chars().take(400).collect();
    match status {
        401 => PublishError::Unauthorized(short),
        403 if body.contains("quotaExceeded")
            || body.contains("dailyLimitExceeded")
            || body.contains("uploadLimitExceeded") =>
        {
            PublishError::Quota {
                message: short,
                retry_after_secs: 6 * 3600,
            }
        }
        429 => PublishError::Quota {
            message: short,
            retry_after_secs: 600,
        },
        500..=599 | 408 => PublishError::Transient(format!("HTTP {status}: {short}")),
        _ => PublishError::Permanent(format!("HTTP {status}: {short}")),
    }
}

/// Progress callback: (bytes sent, total).
pub type Progress = std::sync::Arc<dyn Fn(u64, u64) + Send + Sync>;
