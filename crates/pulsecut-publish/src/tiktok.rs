//! TikTok Content Posting API — **experimental**. Uses the inbox ("upload as draft") flow with
//! `FILE_UPLOAD`; the creator finishes posting inside the TikTok app. Unaudited API clients can
//! only create private/draft content, which matches PulseCut's review-first default.
//! The official docs were unreachable during research (see RESEARCH.md); chunk rules below follow
//! the published 5–64 MB guidance and must be re-verified with a real developer account.

use crate::{Progress, PublishError};
use serde_json::json;
use std::path::Path;
use std::time::Duration;

pub const API: &str = "https://open.tiktokapis.com/v2";
const MIN_CHUNK: u64 = 5 * 1024 * 1024;
const MAX_CHUNK: u64 = 64 * 1024 * 1024;

/// (chunk_size, total_chunk_count). Files < 5 MB go in one chunk; the final chunk may be larger
/// than `chunk_size` (it absorbs the remainder) as the API requires.
pub fn chunk_plan(size: u64) -> (u64, u64) {
    let chunk = (10 * 1024 * 1024u64).clamp(MIN_CHUNK, MAX_CHUNK);
    if size <= chunk {
        return (size, 1);
    }
    let count = (size / chunk).max(1);
    (chunk, count)
}

#[derive(Clone)]
pub struct TikTok {
    pub http: reqwest::Client,
    pub base: String,
}

impl Default for TikTok {
    fn default() -> Self {
        TikTok {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(300))
                .build()
                .expect("client"),
            base: API.into(),
        }
    }
}

impl TikTok {
    pub async fn upload_to_inbox(
        &self,
        token: &str,
        path: &Path,
        progress: Option<Progress>,
    ) -> Result<String, PublishError> {
        let data = tokio::fs::read(path)
            .await
            .map_err(|e| PublishError::Permanent(e.to_string()))?;
        let size = data.len() as u64;
        let (chunk, count) = chunk_plan(size);
        let r = self
            .http
            .post(format!("{}/post/publish/inbox/video/init/", self.base))
            .bearer_auth(token)
            .json(&json!({"source_info": {"source": "FILE_UPLOAD", "video_size": size, "chunk_size": chunk, "total_chunk_count": count}}))
            .send()
            .await?;
        let status = r.status().as_u16();
        let v: serde_json::Value = r.json().await.unwrap_or_default();
        if !(200..300).contains(&status) || v["error"]["code"].as_str().is_some_and(|c| c != "ok") {
            return Err(crate::classify(status.max(400), &v.to_string()));
        }
        let upload_url = v["data"]["upload_url"]
            .as_str()
            .ok_or_else(|| PublishError::Transient("no upload_url".into()))?;
        let publish_id = v["data"]["publish_id"].as_str().unwrap_or_default().to_string();
        for i in 0..count {
            let start = i * chunk;
            let end = if i + 1 == count { size } else { start + chunk };
            let r = self
                .http
                .put(upload_url)
                .header("Content-Type", "video/mp4")
                .header("Content-Range", format!("bytes {}-{}/{}", start, end - 1, size))
                .body(data[start as usize..end as usize].to_vec())
                .send()
                .await?;
            let st = r.status().as_u16();
            if !(200..300).contains(&st) {
                return Err(crate::classify(st, &r.text().await.unwrap_or_default()));
            }
            if let Some(p) = &progress {
                p(end, size);
            }
        }
        Ok(publish_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chunking() {
        assert_eq!(chunk_plan(1_000_000), (1_000_000, 1));
        let (c, n) = chunk_plan(25 * 1024 * 1024);
        assert_eq!(c, 10 * 1024 * 1024);
        assert_eq!(n, 2); // last chunk carries 15 MiB
        let (c, n) = chunk_plan(7 * 1024 * 1024);
        assert_eq!((c, n), (7 * 1024 * 1024, 1));
    }
}
