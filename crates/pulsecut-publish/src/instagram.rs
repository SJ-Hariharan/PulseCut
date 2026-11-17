//! Instagram Reels via the Instagram Graph API (professional accounts only).
//!
//! Flow: create a REELS container from a **public** `video_url` → poll `status_code` until
//! FINISHED → `media_publish`. Instagram fetches the video itself, so PulseCut needs a public
//! URL. The `Hosting` adapter copies the clip into a directory the user serves publicly (their
//! own web server, a synced bucket, a tunnel) under an unguessable name and removes it after
//! publishing. Local clipping never depends on this; it's opt-in infrastructure that may cost money.

use crate::PublishError;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const GRAPH: &str = "https://graph.instagram.com/v21.0";

#[derive(Clone)]
pub struct Hosting {
    pub dir: PathBuf,
    pub public_base: String,
}

impl Hosting {
    pub fn is_configured(&self) -> bool {
        !self.public_base.trim().is_empty() && !self.dir.as_os_str().is_empty()
    }

    /// Copy `file` into the hosted dir; returns (public URL, local path to delete later).
    pub fn publish(&self, file: &Path) -> Result<(String, PathBuf), PublishError> {
        if !self.public_base.starts_with("https://") {
            return Err(PublishError::Permanent(
                "Instagram requires an https:// public base URL".into(),
            ));
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| PublishError::Permanent(e.to_string()))?;
        let name = format!("{}{}.mp4", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
        let dest = self.dir.join(&name);
        std::fs::copy(file, &dest).map_err(|e| PublishError::Permanent(e.to_string()))?;
        Ok((format!("{}/{}", self.public_base.trim_end_matches('/'), name), dest))
    }
}

#[derive(Clone)]
pub struct Instagram {
    pub http: reqwest::Client,
    pub base: String,
}

impl Default for Instagram {
    fn default() -> Self {
        Instagram {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .expect("client"),
            base: GRAPH.into(),
        }
    }
}

async fn json_or_err(r: reqwest::Response) -> Result<serde_json::Value, PublishError> {
    let status = r.status().as_u16();
    let body = r.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        // Graph API rate-limit errors: code 4 / 17 / 32 / 613; "application request limit".
        if body.contains("\"code\":4") || body.contains("\"code\":17") || body.contains("limit") {
            return Err(PublishError::Quota {
                message: body.chars().take(300).collect(),
                retry_after_secs: 3600,
            });
        }
        return Err(crate::classify(status, &body));
    }
    serde_json::from_str(&body).map_err(|e| PublishError::Transient(e.to_string()))
}

impl Instagram {
    pub async fn publish_reel(
        &self,
        user_id: &str,
        token: &str,
        video_url: &str,
        caption: &str,
    ) -> Result<String, PublishError> {
        let v = json_or_err(
            self.http
                .post(format!("{}/{user_id}/media", self.base))
                .form(&[
                    ("media_type", "REELS"),
                    ("video_url", video_url),
                    ("caption", caption),
                    ("share_to_feed", "true"),
                    ("access_token", token),
                ])
                .send()
                .await?,
        )
        .await?;
        let container = v["id"]
            .as_str()
            .ok_or_else(|| PublishError::Transient("no container id".into()))?
            .to_string();
        // Processing usually takes 10 s – a few minutes.
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let s = json_or_err(
                self.http
                    .get(format!("{}/{container}", self.base))
                    .query(&[("fields", "status_code,status"), ("access_token", token)])
                    .send()
                    .await?,
            )
            .await?;
            match s["status_code"].as_str() {
                Some("FINISHED") => {
                    let p = json_or_err(
                        self.http
                            .post(format!("{}/{user_id}/media_publish", self.base))
                            .form(&[("creation_id", container.as_str()), ("access_token", token)])
                            .send()
                            .await?,
                    )
                    .await?;
                    return p["id"]
                        .as_str()
                        .map(str::to_string)
                        .ok_or_else(|| PublishError::Transient("publish returned no id".into()));
                }
                Some("ERROR") | Some("EXPIRED") => {
                    return Err(PublishError::Permanent(format!(
                        "Instagram rejected the video: {}",
                        s["status"]
                    )));
                }
                _ => {}
            }
        }
        Err(PublishError::Transient("Instagram processing timed out".into()))
    }
}
