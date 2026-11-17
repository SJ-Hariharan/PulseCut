//! Minimal official Twitch Helix client for stream discovery (free; no user login).
//!
//! Uses an *app access token* (OAuth client-credentials flow) from the user's own Twitch
//! application. Only two read-only calls are made:
//! * `GET /helix/streams?user_login=…` (batched, ≤100 logins) — live status + viewer counts
//! * `GET /helix/streams?first=N[&language][&game_id]` — top live streams ("trending")
//!
//! The token is cached until shortly before it expires and refreshed once on HTTP 401.
//! HTTP 429 is surfaced as [`HelixError::RateLimited`] with the server's reset hint so the
//! caller can back off.

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

pub const TOKEN_URL: &str = "https://id.twitch.tv/oauth2/token";
pub const API_BASE: &str = "https://api.twitch.tv/helix";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LiveStream {
    pub login: String,
    pub display_name: String,
    pub viewers: u32,
    pub game: String,
    pub title: String,
    pub language: String,
    pub started_at: String,
}

#[derive(Debug, thiserror::Error)]
pub enum HelixError {
    #[error("Twitch credentials rejected: {0}")]
    Auth(String),
    #[error("Twitch rate limit reached; retry in {0:?}")]
    RateLimited(Duration),
    #[error("Twitch API error: {0}")]
    Http(String),
}

#[derive(Deserialize)]
struct TokenResp {
    access_token: String,
    #[serde(default)]
    expires_in: u64,
}

#[derive(Deserialize)]
struct StreamsResp {
    #[serde(default)]
    data: Vec<StreamItem>,
}

#[derive(Deserialize)]
struct StreamItem {
    #[serde(default)]
    user_login: String,
    #[serde(default)]
    user_name: String,
    #[serde(default)]
    viewer_count: u32,
    #[serde(default)]
    game_name: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    language: String,
    #[serde(default)]
    started_at: String,
    #[serde(default, rename = "type")]
    kind: String,
}

/// Parse a `/helix/streams` response body (only `type == "live"` entries).
pub fn parse_streams(body: &str) -> Result<Vec<LiveStream>, HelixError> {
    let r: StreamsResp = serde_json::from_str(body).map_err(|e| HelixError::Http(format!("bad JSON: {e}")))?;
    Ok(r.data
        .into_iter()
        .filter(|s| s.kind.is_empty() || s.kind == "live")
        .map(|s| LiveStream {
            login: s.user_login.to_ascii_lowercase(),
            display_name: s.user_name,
            viewers: s.viewer_count,
            game: s.game_name,
            title: s.title,
            language: s.language,
            started_at: s.started_at,
        })
        .collect())
}

pub struct HelixClient {
    http: reqwest::Client,
    client_id: String,
    client_secret: String,
    token_url: String,
    api_base: String,
    token: Option<(String, Instant)>,
}

impl HelixClient {
    pub fn new(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        Self::with_urls(client_id, client_secret, TOKEN_URL, API_BASE)
    }

    /// For tests (mock server) — production code uses [`HelixClient::new`].
    pub fn with_urls(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        token_url: &str,
        api_base: &str,
    ) -> Self {
        HelixClient {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            token_url: token_url.into(),
            api_base: api_base.trim_end_matches('/').into(),
            token: None,
        }
    }

    async fn token(&mut self) -> Result<String, HelixError> {
        if let Some((t, exp)) = &self.token
            && Instant::now() < *exp
        {
            return Ok(t.clone());
        }
        let r = self
            .http
            .post(&self.token_url)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("grant_type", "client_credentials"),
            ])
            .send()
            .await
            .map_err(|e| HelixError::Http(e.to_string()))?;
        let status = r.status();
        let body = r.text().await.unwrap_or_default();
        if status.as_u16() == 400 || status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(HelixError::Auth(short(&body)));
        }
        if !status.is_success() {
            return Err(HelixError::Http(format!("token HTTP {status}: {}", short(&body))));
        }
        let t: TokenResp = serde_json::from_str(&body).map_err(|e| HelixError::Http(format!("token JSON: {e}")))?;
        // Refresh a minute early; default to 1 h when the server omits expires_in.
        let life = Duration::from_secs(t.expires_in.max(120).saturating_sub(60));
        let life = if t.expires_in == 0 {
            Duration::from_secs(3600)
        } else {
            life
        };
        self.token = Some((t.access_token.clone(), Instant::now() + life));
        Ok(t.access_token)
    }

    async fn get_streams(&mut self, query: &[(String, String)]) -> Result<Vec<LiveStream>, HelixError> {
        for attempt in 0..2 {
            let tok = self.token().await?;
            let r = self
                .http
                .get(format!("{}/streams", self.api_base))
                .query(query)
                .header("Client-Id", &self.client_id)
                .bearer_auth(&tok)
                .send()
                .await
                .map_err(|e| HelixError::Http(e.to_string()))?;
            let status = r.status().as_u16();
            if status == 401 && attempt == 0 {
                self.token = None; // expired/revoked: refresh once
                continue;
            }
            if status == 429 {
                let reset = r
                    .headers()
                    .get("Ratelimit-Reset")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .map(|epoch| {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        Duration::from_secs(epoch.saturating_sub(now).clamp(1, 120))
                    })
                    .unwrap_or(Duration::from_secs(60));
                return Err(HelixError::RateLimited(reset));
            }
            let body = r.text().await.unwrap_or_default();
            if status == 401 || status == 403 {
                return Err(HelixError::Auth(short(&body)));
            }
            if !(200..300).contains(&status) {
                return Err(HelixError::Http(format!("HTTP {status}: {}", short(&body))));
            }
            return parse_streams(&body);
        }
        Err(HelixError::Auth("token refresh failed".into()))
    }

    /// Live streams among `logins` (batched in groups of 100). Missing logins are offline.
    pub async fn live_status(&mut self, logins: &[String]) -> Result<Vec<LiveStream>, HelixError> {
        let mut out = vec![];
        for chunk in logins.chunks(100) {
            let q: Vec<(String, String)> = chunk.iter().map(|l| ("user_login".to_string(), l.clone())).collect();
            out.extend(self.get_streams(&q).await?);
        }
        Ok(out)
    }

    /// Top live streams by viewers, optionally filtered.
    pub async fn top_streams(
        &mut self,
        first: u32,
        language: &str,
        game_id: &str,
    ) -> Result<Vec<LiveStream>, HelixError> {
        let mut q = vec![("first".to_string(), first.clamp(1, 100).to_string())];
        if !language.is_empty() {
            q.push(("language".into(), language.into()));
        }
        if !game_id.is_empty() {
            q.push(("game_id".into(), game_id.into()));
        }
        self.get_streams(&q).await
    }
}

fn short(s: &str) -> String {
    s.chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const STREAMS: &str = r#"{"data":[
        {"user_login":"TenZ","user_name":"TenZ","viewer_count":12345,"game_name":"VALORANT","title":"ranked","language":"en","started_at":"2026-01-01T00:00:00Z","type":"live"},
        {"user_login":"rerun","user_name":"Rerun","viewer_count":5,"type":""},
        {"user_login":"weird","viewer_count":1,"type":"vodcast"}
    ],"pagination":{}}"#;

    #[test]
    fn parses_live_streams_only() {
        let v = parse_streams(STREAMS).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].login, "tenz");
        assert_eq!(v[0].viewers, 12345);
        assert!(parse_streams("not json").is_err());
    }

    /// Tiny HTTP mock: answers token + streams; the first streams call returns 401 to
    /// exercise the refresh path; a `first=99` query returns 429.
    async fn mock() -> (String, Arc<AtomicU32>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let tokens = Arc::new(AtomicU32::new(0));
        let t2 = Arc::clone(&tokens);
        let streams_calls = Arc::new(AtomicU32::new(0));
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else { break };
                let tokens = Arc::clone(&t2);
                let calls = Arc::clone(&streams_calls);
                tokio::spawn(async move {
                    // Read headers + body (the form body may arrive in a later segment).
                    let mut data = vec![];
                    let mut buf = vec![0u8; 8192];
                    loop {
                        let n = s.read(&mut buf).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        data.extend_from_slice(&buf[..n]);
                        let txt = String::from_utf8_lossy(&data).to_string();
                        if let Some(h) = txt.find("\r\n\r\n") {
                            let len = txt[..h]
                                .lines()
                                .find_map(|l| {
                                    l.to_ascii_lowercase()
                                        .strip_prefix("content-length:")
                                        .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                                })
                                .unwrap_or(0);
                            if data.len() >= h + 4 + len {
                                break;
                            }
                        }
                    }
                    let req = String::from_utf8_lossy(&data).to_string();
                    let line = req.lines().next().unwrap_or("").to_string();
                    let (status, extra, body) = if line.starts_with("POST /token") {
                        if req.contains("client_secret=bad") {
                            (
                                "400 Bad Request",
                                "",
                                r#"{"message":"invalid client secret"}"#.to_string(),
                            )
                        } else {
                            let n = tokens.fetch_add(1, Ordering::SeqCst);
                            (
                                "200 OK",
                                "",
                                format!(r#"{{"access_token":"tok{n}","expires_in":3600}}"#),
                            )
                        }
                    } else if line.contains("first=99") {
                        ("429 Too Many Requests", "Ratelimit-Reset: 0\r\n", "{}".to_string())
                    } else if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        ("401 Unauthorized", "", "{}".to_string())
                    } else {
                        assert!(req.contains("Client-Id: cid") || req.contains("client-id: cid"));
                        ("200 OK", "", STREAMS.to_string())
                    };
                    let resp = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = s.write_all(resp.as_bytes()).await;
                });
            }
        });
        (format!("http://{addr}"), tokens)
    }

    #[tokio::test]
    async fn token_refresh_on_401_then_live_status() {
        let (base, tokens) = mock().await;
        let mut c = HelixClient::with_urls("cid", "secret", &format!("{base}/token"), &base);
        let v = c.live_status(&["tenz".into(), "offline_one".into()]).await.unwrap();
        assert!(v.iter().any(|s| s.login == "tenz" && s.viewers == 12345));
        assert_eq!(tokens.load(Ordering::SeqCst), 2, "401 forces exactly one token refresh");
        // Cached token reused.
        c.top_streams(20, "en", "").await.unwrap();
        assert_eq!(tokens.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn rate_limit_and_bad_credentials_are_reported() {
        let (base, _) = mock().await;
        let mut c = HelixClient::with_urls("cid", "secret", &format!("{base}/token"), &base);
        match c.top_streams(99, "", "").await {
            Err(HelixError::RateLimited(d)) => assert!(d >= Duration::from_secs(1)),
            other => panic!("expected rate limit, got {other:?}"),
        }
        let mut bad = HelixClient::with_urls("cid", "bad", &format!("{base}/token"), &base);
        assert!(matches!(bad.top_streams(5, "", "").await, Err(HelixError::Auth(_))));
    }
}
