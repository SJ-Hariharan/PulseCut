//! YouTube Data API v3 resumable uploads.
//!
//! Protocol: POST metadata to `/upload/youtube/v3/videos?uploadType=resumable` → session URL in
//! `Location`; PUT the file in 8 MiB chunks with `Content-Range`; `308` means "continue from the
//! `Range` header"; `200/201` returns the video resource. The session URL is persisted so a
//! crash or network failure resumes (PUT `bytes */total` asks the server how much it has).
//! Shorts: there is no API flag; vertical ≤ 3 min videos are treated as Shorts by YouTube.

use crate::{Progress, PublishError, UploadMeta, classify};
use serde_json::json;
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

pub const UPLOAD_BASE: &str = "https://www.googleapis.com/upload/youtube/v3";
const CHUNK: u64 = 8 * 1024 * 1024; // multiple of 256 KiB

#[derive(Clone)]
pub struct YouTube {
    pub http: reqwest::Client,
    pub upload_base: String,
}

impl Default for YouTube {
    fn default() -> Self {
        YouTube {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(300))
                .build()
                .expect("http client"),
            upload_base: UPLOAD_BASE.into(),
        }
    }
}

pub fn video_resource(m: &UploadMeta) -> serde_json::Value {
    let title: String = m.title.chars().filter(|c| *c != '<' && *c != '>').take(100).collect();
    let description: String = m
        .description
        .chars()
        .filter(|c| *c != '<' && *c != '>')
        .take(4900)
        .collect();
    let mut tags: Vec<String> = m.tags.iter().map(|t| t.trim_start_matches('#').to_string()).collect();
    tags.retain(|t| !t.is_empty());
    let mut status = json!({
        "privacyStatus": if m.publish_at.is_some() { "private" } else { m.privacy.as_str() },
        "selfDeclaredMadeForKids": false,
    });
    if let Some(at) = &m.publish_at {
        status["publishAt"] = json!(at);
    }
    json!({
        "snippet": {"title": title, "description": description, "tags": tags, "categoryId": m.category_id},
        "status": status,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Done { video_id: String },
    Partial { sent: u64 },
}

impl YouTube {
    /// Start a resumable session; returns the session URL.
    pub async fn start_session(&self, token: &str, meta: &UploadMeta, size: u64) -> Result<String, PublishError> {
        let r = self
            .http
            .post(format!("{}/videos", self.upload_base))
            .query(&[("uploadType", "resumable"), ("part", "snippet,status")])
            .bearer_auth(token)
            .header("X-Upload-Content-Length", size.to_string())
            .header("X-Upload-Content-Type", "video/mp4")
            .json(&video_resource(meta))
            .send()
            .await?;
        let status = r.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(classify(status, &r.text().await.unwrap_or_default()));
        }
        r.headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
            .ok_or_else(|| PublishError::Transient("no session URL returned".into()))
    }

    /// Ask the server how many bytes it has for a session.
    pub async fn query_offset(&self, token: &str, session: &str, size: u64) -> Result<Step, PublishError> {
        let r = self
            .http
            .put(session)
            .bearer_auth(token)
            .header("Content-Range", format!("bytes */{size}"))
            .header("Content-Length", "0")
            .send()
            .await?;
        self.interpret(r).await
    }

    async fn interpret(&self, r: reqwest::Response) -> Result<Step, PublishError> {
        let status = r.status().as_u16();
        match status {
            200 | 201 => {
                let v: serde_json::Value = r.json().await.map_err(|e| PublishError::Transient(e.to_string()))?;
                let id = v["id"]
                    .as_str()
                    .ok_or_else(|| PublishError::Transient("upload finished without an id".into()))?;
                Ok(Step::Done {
                    video_id: id.to_string(),
                })
            }
            308 => {
                let sent = r
                    .headers()
                    .get("Range")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.rsplit('-').next())
                    .and_then(|n| n.parse::<u64>().ok())
                    .map(|last| last + 1)
                    .unwrap_or(0);
                Ok(Step::Partial { sent })
            }
            404 | 410 => Err(PublishError::Transient("upload session expired; restarting".into())),
            _ => Err(classify(status, &r.text().await.unwrap_or_default())),
        }
    }

    /// Upload (or resume) the whole file through `session`.
    pub async fn upload(
        &self,
        token: &str,
        session: &str,
        path: &Path,
        mut sent: u64,
        progress: Option<Progress>,
    ) -> Result<String, PublishError> {
        let mut f = tokio::fs::File::open(path)
            .await
            .map_err(|e| PublishError::Permanent(e.to_string()))?;
        let size = f
            .metadata()
            .await
            .map_err(|e| PublishError::Permanent(e.to_string()))?
            .len();
        loop {
            if let Some(p) = &progress {
                p(sent, size);
            }
            let end = (sent + CHUNK).min(size);
            let mut buf = vec![0u8; (end - sent) as usize];
            f.seek(std::io::SeekFrom::Start(sent))
                .await
                .map_err(|e| PublishError::Permanent(e.to_string()))?;
            f.read_exact(&mut buf)
                .await
                .map_err(|e| PublishError::Permanent(e.to_string()))?;
            let r = self
                .http
                .put(session)
                .bearer_auth(token)
                .header("Content-Range", format!("bytes {}-{}/{}", sent, end - 1, size))
                .body(buf)
                .send()
                .await?;
            match self.interpret(r).await? {
                Step::Done { video_id } => {
                    if let Some(p) = &progress {
                        p(size, size);
                    }
                    return Ok(video_id);
                }
                Step::Partial { sent: s } => {
                    if s <= sent && end != size {
                        // No progress: avoid a hot loop.
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                    sent = s;
                }
            }
        }
    }
}

pub fn watch_url(id: &str) -> String {
    format!("https://youtube.com/shorts/{id}")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resource_respects_limits_and_scheduling() {
        let m = UploadMeta {
            title: "x".repeat(150) + "<b>",
            description: "d".into(),
            tags: vec!["#shorts".into(), "#".into()],
            privacy: "unlisted".into(),
            publish_at: Some("2026-12-01T10:00:00Z".into()),
            category_id: "20".into(),
        };
        let v = video_resource(&m);
        assert_eq!(v["snippet"]["title"].as_str().unwrap().len(), 100);
        assert_eq!(v["status"]["privacyStatus"], "private");
        assert_eq!(v["status"]["publishAt"], "2026-12-01T10:00:00Z");
        assert_eq!(v["snippet"]["tags"], json!(["shorts"]));
    }
}
