//! YouTube Live chat via the Data API v3 with an API key (public streams).
//!
//! Quota: `liveChatMessages.list` costs 5 units; the default project quota is 10,000 units/day.
//! PulseCut therefore polls no faster than `max(pollingIntervalMillis, min_interval)` (default
//! 10 s ≈ 5.5 h of chat per day per key) and reports quota exhaustion as a source failure.

use crate::{SourceState, StreamChat};
use pulsecut_detect::ChatMessage;
use serde::Deserialize;
use std::time::Duration;
use tokio::sync::{mpsc, watch};

pub const API: &str = "https://www.googleapis.com/youtube/v3";

#[derive(Debug, Deserialize)]
struct VideoList {
    items: Vec<VideoItem>,
}
#[derive(Debug, Deserialize)]
struct VideoItem {
    #[serde(rename = "liveStreamingDetails")]
    live: Option<LiveDetails>,
}
#[derive(Debug, Deserialize)]
struct LiveDetails {
    #[serde(rename = "activeLiveChatId")]
    chat_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ChatPage {
    #[serde(rename = "nextPageToken")]
    pub next: Option<String>,
    #[serde(rename = "pollingIntervalMillis", default)]
    pub interval_ms: u64,
    #[serde(default)]
    pub items: Vec<ChatItem>,
}

#[derive(Debug, Deserialize)]
pub struct ChatItem {
    pub snippet: Snippet,
    #[serde(rename = "authorDetails")]
    pub author: Author,
}

#[derive(Debug, Deserialize)]
pub struct Snippet {
    #[serde(rename = "publishedAt")]
    pub published_at: String,
    #[serde(rename = "displayMessage", default)]
    pub display_message: String,
    #[serde(rename = "type", default)]
    pub kind: String,
}

#[derive(Debug, Deserialize)]
pub struct Author {
    #[serde(rename = "displayName")]
    pub name: String,
    #[serde(rename = "isChatModerator", default)]
    pub is_mod: bool,
    #[serde(rename = "isChatSponsor", default)]
    pub is_member: bool,
    #[serde(rename = "isChatOwner", default)]
    pub is_owner: bool,
}

pub fn to_messages(page: &ChatPage) -> Vec<ChatMessage> {
    page.items
        .iter()
        .filter(|i| {
            i.snippet.kind.is_empty() || i.snippet.kind == "textMessageEvent" || i.snippet.kind == "superChatEvent"
        })
        .map(|i| ChatMessage {
            ts: chrono::DateTime::parse_from_rfc3339(&i.snippet.published_at)
                .map(|d| d.timestamp_millis())
                .unwrap_or_else(|_| pulsecut_core::now_ms()),
            user: i.author.name.clone(),
            text: i.snippet.display_message.clone(),
            emotes: vec![],
            is_mod: i.author.is_mod || i.author.is_owner,
            is_subscriber: i.author.is_member,
        })
        .collect()
}

pub struct YouTubeChat {
    pub state: watch::Receiver<SourceState>,
    stop: watch::Sender<bool>,
}

impl YouTubeChat {
    pub fn stop(&self) {
        let _ = self.stop.send(true);
    }
}

/// Poll chat for `video_id` (the live broadcast's video id).
pub fn spawn(
    video_id: String,
    api_key: String,
    out: mpsc::Sender<StreamChat>,
    min_interval: Duration,
    base: Option<String>,
) -> YouTubeChat {
    let (state_tx, state) = watch::channel(SourceState::Connecting);
    let (stop, mut stop_rx) = watch::channel(false);
    let base = base.unwrap_or_else(|| API.to_string());
    tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .expect("client");
        let stream_id = format!("youtube:{video_id}");
        let chat_id = loop {
            let r = client
                .get(format!("{base}/videos"))
                .query(&[
                    ("part", "liveStreamingDetails"),
                    ("id", video_id.as_str()),
                    ("key", api_key.as_str()),
                ])
                .send()
                .await;
            match r {
                Ok(r) if r.status().is_success() => match r.json::<VideoList>().await {
                    Ok(v) => match v.items.into_iter().find_map(|i| i.live.and_then(|l| l.chat_id)) {
                        Some(id) => break id,
                        None => {
                            let _ = state_tx.send(SourceState::Failed("video is not live or chat is disabled".into()));
                        }
                    },
                    Err(e) => {
                        let _ = state_tx.send(SourceState::Failed(format!("bad response: {e}")));
                    }
                },
                Ok(r) => {
                    let _ = state_tx.send(SourceState::Failed(format!("YouTube API {}", r.status())));
                }
                Err(e) => {
                    let _ = state_tx.send(SourceState::Failed(e.to_string()));
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(60)) => {}
                _ = stop_rx.changed() => return,
            }
        };
        let _ = state_tx.send(SourceState::Connected);
        let mut page_token: Option<String> = None;
        let mut first = true;
        loop {
            let mut q = vec![
                ("liveChatId", chat_id.clone()),
                ("part", "snippet,authorDetails".into()),
                ("key", api_key.clone()),
                ("maxResults", "2000".into()),
            ];
            if let Some(t) = &page_token {
                q.push(("pageToken", t.clone()));
            }
            let mut wait = min_interval;
            match client.get(format!("{base}/liveChat/messages")).query(&q).send().await {
                Ok(r) if r.status().is_success() => {
                    if let Ok(page) = r.json::<ChatPage>().await {
                        // Skip the backlog on the first page: only live reactions matter.
                        if !first {
                            for m in to_messages(&page) {
                                if out
                                    .try_send(StreamChat {
                                        stream_id: stream_id.clone(),
                                        msg: m,
                                    })
                                    .is_err()
                                    && out.is_closed()
                                {
                                    return;
                                }
                            }
                        }
                        first = false;
                        page_token = page.next;
                        wait = wait.max(Duration::from_millis(page.interval_ms));
                        let _ = state_tx.send(SourceState::Connected);
                    }
                }
                Ok(r) if r.status().as_u16() == 403 => {
                    let _ = state_tx.send(SourceState::Failed("quota exceeded or key not permitted (403)".into()));
                    wait = Duration::from_secs(900);
                }
                Ok(r) => {
                    let _ = state_tx.send(SourceState::Reconnecting {
                        attempt: 1,
                        reason: format!("HTTP {}", r.status()),
                    });
                    wait = Duration::from_secs(30);
                }
                Err(e) => {
                    let _ = state_tx.send(SourceState::Reconnecting {
                        attempt: 1,
                        reason: e.to_string(),
                    });
                    wait = Duration::from_secs(15);
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = stop_rx.changed() => break,
            }
        }
        let _ = state_tx.send(SourceState::Stopped);
    });
    YouTubeChat { state, stop }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_page() {
        let j = r#"{"nextPageToken":"abc","pollingIntervalMillis":4000,"items":[
          {"snippet":{"publishedAt":"2026-11-17T10:00:00.250Z","displayMessage":"LETS GO","type":"textMessageEvent"},
           "authorDetails":{"displayName":"Ann","isChatModerator":false,"isChatSponsor":true}},
          {"snippet":{"publishedAt":"2026-11-17T10:00:01Z","displayMessage":"","type":"tombstone"},
           "authorDetails":{"displayName":"X"}}]}"#;
        let p: ChatPage = serde_json::from_str(j).unwrap();
        let m = to_messages(&p);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].text, "LETS GO");
        assert!(m[0].is_subscriber);
        assert_eq!(m[0].ts % 1000, 250);
        assert_eq!(p.interval_ms, 4000);
    }
}
