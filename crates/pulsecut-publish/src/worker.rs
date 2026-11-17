//! Upload queue worker: picks due uploads from SQLite, dispatches to the platform, persists
//! progress (YouTube session URL) after every step so crashes resume, retries with backoff, and
//! respects per-platform daily caps.

use crate::instagram::{Hosting, Instagram};
use crate::oauth::{OAuthClient, Tokens};
use crate::tiktok::TikTok;
use crate::youtube::{Step, YouTube, watch_url};
use crate::{Progress, PublishError, UploadMeta};
use pulsecut_core::{ClipStatus, now_ms};
use pulsecut_store::{Store, UploadRecord, UploadState};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

/// Where OAuth tokens and keys come from (the engine adapts the OS keychain to this).
pub trait TokenStore: Send + Sync {
    fn get(&self, name: &str) -> Option<String>;
    fn set(&self, name: &str, value: &str);
}

#[derive(Clone)]
pub struct WorkerConfig {
    pub youtube_client: Option<OAuthClient>,
    pub youtube_category: String,
    pub instagram_user_id: String,
    pub hosting: Option<Hosting>,
    pub tiktok_client: Option<OAuthClient>,
    pub max_attempts: i64,
    /// Uploads per rolling 24 h per platform.
    pub daily_caps: HashMap<String, i64>,
}

pub type ProgressMap = Arc<Mutex<HashMap<Uuid, (u64, u64)>>>;

pub struct Worker {
    pub store: Store,
    pub tokens: Arc<dyn TokenStore>,
    pub cfg: Arc<Mutex<WorkerConfig>>,
    pub youtube: YouTube,
    pub instagram: Instagram,
    pub tiktok: TikTok,
    pub progress: ProgressMap,
}

pub fn backoff_ms(attempts: i64) -> i64 {
    (30_000i64 * 4i64.pow(attempts.clamp(0, 5) as u32)).min(2 * 3_600_000)
}

impl Worker {
    pub fn spawn(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                match self.store.next_due_upload().await {
                    Ok(Some(u)) => self.process(u).await,
                    Ok(None) => tokio::time::sleep(Duration::from_secs(2)).await,
                    Err(e) => {
                        tracing::error!("upload queue: {e}");
                        tokio::time::sleep(Duration::from_secs(10)).await;
                    }
                }
            }
        })
    }

    async fn fresh_tokens(&self, key: &str, client: &OAuthClient) -> Result<Tokens, PublishError> {
        let raw = self.tokens.get(key).ok_or_else(|| {
            PublishError::Permanent(format!("account not connected ({key}); connect it in Publishing"))
        })?;
        let mut t: Tokens =
            serde_json::from_str(&raw).map_err(|e| PublishError::Permanent(format!("stored token: {e}")))?;
        if t.needs_refresh() {
            t = crate::oauth::refresh(client, &t)
                .await
                .map_err(PublishError::Unauthorized)?;
            self.tokens.set(key, &serde_json::to_string(&t).unwrap_or_default());
        }
        Ok(t)
    }

    /// Process one upload attempt and persist the outcome.
    pub async fn process(&self, mut u: UploadRecord) {
        let cfg = self.cfg.lock().unwrap().clone();
        let cap = cfg.daily_caps.get(&u.platform).copied().unwrap_or(i64::MAX);
        let used = self
            .store
            .uploads_since(&u.platform, now_ms() - 86_400_000)
            .await
            .unwrap_or(0);
        if used > cap {
            u.next_attempt_at = now_ms() + 3_600_000;
            u.error = Some(format!("daily {} cap of {cap} reached; will retry later", u.platform));
            let _ = self.store.save_upload(&u).await;
            return;
        }
        u.attempts += 1;
        u.state = UploadState::Uploading;
        u.error = None;
        let _ = self.store.save_upload(&u).await;
        let _ = self
            .store
            .set_clip_status(u.clip_id, ClipStatus::Publishing, None)
            .await;

        let clip = self.store.get_clip(u.clip_id).await.ok().flatten();
        let Some(path) = clip.as_ref().and_then(|c| c.video_path.clone()).map(PathBuf::from) else {
            self.fail(&mut u, PublishError::Permanent("clip video file is missing".into()))
                .await;
            return;
        };
        let tags = clip
            .as_ref()
            .and_then(|c| c.meta.as_ref())
            .map(|m| m.hashtags.clone())
            .unwrap_or_default();
        let progress: Progress = {
            let map = Arc::clone(&self.progress);
            let id = u.id;
            Arc::new(move |s, t| {
                map.lock().unwrap().insert(id, (s, t));
            })
        };
        let result = match u.platform.as_str() {
            "youtube" => self.youtube_upload(&mut u, &cfg, &path, tags, progress).await,
            "instagram" => self.instagram_upload(&u, &cfg, &path).await,
            "tiktok" => self.tiktok_upload(&cfg, &path, progress).await,
            other => Err(PublishError::Permanent(format!("unknown platform {other}"))),
        };
        self.progress.lock().unwrap().remove(&u.id);
        match result {
            Ok((id, url)) => {
                u.state = UploadState::Published;
                u.remote_id = Some(id);
                u.remote_url = Some(url);
                u.session_url = None;
                let _ = self.store.save_upload(&u).await;
                let _ = self.store.set_clip_status(u.clip_id, ClipStatus::Published, None).await;
                tracing::info!("published clip {} to {}", u.clip_id, u.platform);
            }
            Err(e) => self.fail(&mut u, e).await,
        }
    }

    async fn fail(&self, u: &mut UploadRecord, e: PublishError) {
        let msg = e.to_string();
        match e {
            PublishError::Permanent(_) => u.state = UploadState::Failed,
            PublishError::Quota { retry_after_secs, .. } => {
                u.state = UploadState::Queued;
                u.next_attempt_at = now_ms() + retry_after_secs as i64 * 1000;
                u.attempts -= 1; // quota waits don't consume attempts
            }
            PublishError::Transient(_) | PublishError::Unauthorized(_) => {
                if u.attempts >= self.cfg.lock().unwrap().max_attempts {
                    u.state = UploadState::Failed;
                } else {
                    u.state = UploadState::Queued;
                    u.next_attempt_at = now_ms() + backoff_ms(u.attempts);
                }
            }
        }
        u.error = Some(msg.clone());
        let _ = self.store.save_upload(u).await;
        if u.state == UploadState::Failed {
            let _ = self
                .store
                .set_clip_status(u.clip_id, ClipStatus::Approved, Some(&format!("upload failed: {msg}")))
                .await;
        }
        tracing::warn!("upload {} ({}) attempt {}: {msg}", u.id, u.platform, u.attempts);
    }

    async fn youtube_upload(
        &self,
        u: &mut UploadRecord,
        cfg: &WorkerConfig,
        path: &std::path::Path,
        tags: Vec<String>,
        progress: Progress,
    ) -> Result<(String, String), PublishError> {
        let client = cfg
            .youtube_client
            .as_ref()
            .ok_or_else(|| PublishError::Permanent("YouTube client id not configured (Settings → Platforms)".into()))?;
        let mut t = self.fresh_tokens(pulsecut_core_keys::YOUTUBE_OAUTH, client).await?;
        let size = std::fs::metadata(path)
            .map_err(|e| PublishError::Permanent(e.to_string()))?
            .len();
        let meta = UploadMeta {
            title: u.title.clone(),
            description: u.description.clone(),
            tags,
            privacy: u.privacy.clone(),
            publish_at: u.scheduled_at.map(chrono_rfc3339),
            category_id: cfg.youtube_category.clone(),
        };
        for _ in 0..2 {
            // Resume an existing session if we have one.
            let mut sent = 0;
            let session = match u.session_url.clone() {
                Some(s) => match self.youtube.query_offset(&t.access_token, &s, size).await {
                    Ok(Step::Done { video_id }) => return Ok((video_id.clone(), watch_url(&video_id))),
                    Ok(Step::Partial { sent: s2 }) => {
                        sent = s2;
                        s
                    }
                    Err(PublishError::Transient(_)) => {
                        u.session_url = None;
                        self.youtube.start_session(&t.access_token, &meta, size).await?
                    }
                    Err(e) => return Err(e),
                },
                None => self.youtube.start_session(&t.access_token, &meta, size).await?,
            };
            u.session_url = Some(session.clone());
            u.bytes_sent = sent as i64;
            let _ = self.store.save_upload(u).await;
            match self
                .youtube
                .upload(&t.access_token, &session, path, sent, Some(Arc::clone(&progress)))
                .await
            {
                Ok(id) => return Ok((id.clone(), watch_url(&id))),
                Err(PublishError::Unauthorized(m)) => {
                    t = crate::oauth::refresh(client, &t)
                        .await
                        .map_err(|_| PublishError::Unauthorized(m))?;
                    self.tokens.set(
                        pulsecut_core_keys::YOUTUBE_OAUTH,
                        &serde_json::to_string(&t).unwrap_or_default(),
                    );
                }
                Err(e) => return Err(e),
            }
        }
        Err(PublishError::Unauthorized("token rejected after refresh".into()))
    }

    async fn instagram_upload(
        &self,
        u: &UploadRecord,
        cfg: &WorkerConfig,
        path: &std::path::Path,
    ) -> Result<(String, String), PublishError> {
        let token = self
            .tokens
            .get(pulsecut_core_keys::INSTAGRAM_TOKEN)
            .ok_or_else(|| PublishError::Permanent("Instagram account not connected".into()))?;
        let hosting = cfg.hosting.as_ref().filter(|h| h.is_configured()).ok_or_else(|| {
            PublishError::Permanent("Instagram needs a public media host (Settings → Platforms)".into())
        })?;
        let (url, local) = hosting.publish(path)?;
        let caption = format!("{}\n\n{}", u.title, u.description);
        let r = self
            .instagram
            .publish_reel(&cfg.instagram_user_id, &token, &url, &caption)
            .await;
        let _ = std::fs::remove_file(local);
        let id = r?;
        Ok((id.clone(), format!("https://www.instagram.com/reel/{id}/")))
    }

    async fn tiktok_upload(
        &self,
        cfg: &WorkerConfig,
        path: &std::path::Path,
        progress: Progress,
    ) -> Result<(String, String), PublishError> {
        let client = cfg
            .tiktok_client
            .as_ref()
            .ok_or_else(|| PublishError::Permanent("TikTok client key not configured".into()))?;
        let t = self.fresh_tokens(pulsecut_core_keys::TIKTOK_OAUTH, client).await?;
        let id = self
            .tiktok
            .upload_to_inbox(&t.access_token, path, Some(progress))
            .await?;
        Ok((id, "tiktok://inbox (finish posting in the TikTok app)".into()))
    }
}

/// Secret names (mirrors `pulsecut_platform::secrets::keys` without the dependency).
pub mod pulsecut_core_keys {
    pub const YOUTUBE_OAUTH: &str = "youtube-oauth";
    pub const INSTAGRAM_TOKEN: &str = "instagram-token";
    pub const TIKTOK_OAUTH: &str = "tiktok-oauth";
}

fn chrono_rfc3339(ms: i64) -> String {
    // Minimal RFC 3339 formatter (UTC) to avoid pulling chrono into this crate.
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rfc3339() {
        assert_eq!(chrono_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(chrono_rfc3339(1_764_000_000_000), "2025-11-24T16:00:00Z");
    }
    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff_ms(1), 120_000);
        assert!(backoff_ms(3) > backoff_ms(2));
        assert_eq!(backoff_ms(10), 2 * 3_600_000);
    }
}
