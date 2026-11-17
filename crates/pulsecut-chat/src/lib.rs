//! Chat sources. Each produces `(stream_id, ChatMessage)` on a bounded channel.
//!
//! * `twitch`  – anonymous read-only IRC over WebSocket (no account or token needed)
//! * `youtube` – Data API v3 `liveChatMessages.list` polling with an API key (quota-aware)
//! * `kick`    – official webhook payload parsing + RSA-SHA256 signature verification
//!
//! Each source is independent so one platform failing never stops the others.

pub mod kick;
pub mod twitch;
pub mod youtube;

pub use pulsecut_detect::ChatMessage;

/// One chat message tagged with its stream (`twitch:<channel>`, `youtube:<video>`, `kick:<slug>`).
#[derive(Debug, Clone)]
pub struct StreamChat {
    pub stream_id: String,
    pub msg: ChatMessage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceState {
    Connecting,
    Connected,
    Reconnecting { attempt: u32, reason: String },
    Failed(String),
    Stopped,
}
