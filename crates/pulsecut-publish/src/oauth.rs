//! OAuth 2.0 for installed apps: authorization-code flow with PKCE (S256) and a one-shot
//! loopback redirect server on 127.0.0.1 (RFC 8252). Tokens are serialized as JSON for the
//! OS keychain by the caller.

use base64::Engine;
use pulsecut_core::now_ms;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::sync::oneshot;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthClient {
    pub auth_url: String,
    pub token_url: String,
    pub client_id: String,
    /// Google "Desktop app" clients have a (non-confidential) secret; others may not.
    pub client_secret: Option<String>,
    pub scopes: Vec<String>,
    /// Extra auth params (e.g. access_type=offline, prompt=consent for Google).
    pub extra: Vec<(String, String)>,
    /// TikTok calls the client id `client_key`.
    pub client_id_param: String,
}

impl OAuthClient {
    pub fn google(client_id: &str, client_secret: Option<String>) -> Self {
        OAuthClient {
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token_url: "https://oauth2.googleapis.com/token".into(),
            client_id: client_id.into(),
            client_secret,
            scopes: vec!["https://www.googleapis.com/auth/youtube.upload".into()],
            extra: vec![
                ("access_type".into(), "offline".into()),
                ("prompt".into(), "consent".into()),
            ],
            client_id_param: "client_id".into(),
        }
    }

    pub fn tiktok(client_key: &str, client_secret: Option<String>) -> Self {
        OAuthClient {
            auth_url: "https://www.tiktok.com/v2/auth/authorize/".into(),
            token_url: "https://open.tiktokapis.com/v2/oauth/token/".into(),
            client_id: client_key.into(),
            client_secret,
            scopes: vec!["video.upload".into()],
            extra: vec![],
            client_id_param: "client_key".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Epoch ms.
    pub expires_at: i64,
    #[serde(default)]
    pub account: String,
}

impl Tokens {
    pub fn needs_refresh(&self) -> bool {
        now_ms() > self.expires_at - 120_000
    }
}

#[derive(Deserialize)]
struct TokenResp {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
}

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

fn random_token(bytes: usize) -> String {
    // uuid v4 uses the OS RNG; two of them give 244 bits of entropy.
    let mut raw = Vec::with_capacity(bytes);
    while raw.len() < bytes {
        raw.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    }
    raw.truncate(bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)
}

pub fn pkce() -> Pkce {
    let verifier = random_token(48);
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    Pkce {
        verifier,
        challenge,
        state: random_token(16),
    }
}

pub fn authorize_url(c: &OAuthClient, redirect: &str, p: &Pkce) -> String {
    let mut u = url::Url::parse(&c.auth_url).expect("valid auth url");
    {
        let mut q = u.query_pairs_mut();
        q.append_pair(&c.client_id_param, &c.client_id)
            .append_pair("redirect_uri", redirect)
            .append_pair("response_type", "code")
            .append_pair(
                "scope",
                &c.scopes.join(if c.client_id_param == "client_key" { "," } else { " " }),
            )
            .append_pair("code_challenge", &p.challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &p.state);
        for (k, v) in &c.extra {
            q.append_pair(k, v);
        }
    }
    u.to_string()
}

/// Run the full flow: start loopback server, hand the URL to `open_browser`, wait for the
/// redirect (≤ 5 min), exchange the code.
pub async fn authorize(c: &OAuthClient, open_browser: impl FnOnce(&str)) -> Result<Tokens, String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect = format!("http://127.0.0.1:{port}/callback");
    let p = pkce();
    let (tx, rx) = oneshot::channel::<Result<String, String>>();
    let tx = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));
    let state = p.state.clone();
    let app = axum::Router::new().route(
        "/callback",
        axum::routing::get(
            move |q: axum::extract::Query<std::collections::HashMap<String, String>>| {
                let tx = std::sync::Arc::clone(&tx);
                let state = state.clone();
                async move {
                    let res = if q.get("state") != Some(&state) {
                        Err("state mismatch (possible CSRF); try again".to_string())
                    } else if let Some(err) = q.get("error") {
                        Err(format!("authorization denied: {err}"))
                    } else if let Some(code) = q.get("code") {
                        Ok(code.clone())
                    } else {
                        Err("no code in redirect".into())
                    };
                    let ok = res.is_ok();
                    if let Some(t) = tx.lock().unwrap().take() {
                        let _ = t.send(res);
                    }
                    axum::response::Html(if ok {
                        "<h2>PulseCut is connected.</h2><p>You can close this tab.</p>"
                    } else {
                        "<h2>PulseCut could not connect.</h2><p>Return to the app for details.</p>"
                    })
                }
            },
        ),
    );
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    open_browser(&authorize_url(c, &redirect, &p));
    let code = tokio::time::timeout(Duration::from_secs(300), rx)
        .await
        .map_err(|_| "timed out waiting for browser authorization".to_string())?
        .map_err(|_| "authorization cancelled".to_string())??;
    server.abort();
    exchange(c, &code, &redirect, &p.verifier).await
}

pub async fn exchange(c: &OAuthClient, code: &str, redirect: &str, verifier: &str) -> Result<Tokens, String> {
    let mut form = vec![
        (c.client_id_param.as_str(), c.client_id.as_str()),
        ("code", code),
        ("code_verifier", verifier),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect),
    ];
    if let Some(s) = &c.client_secret {
        form.push(("client_secret", s));
    }
    token_request(c, &form, None).await
}

pub async fn refresh(c: &OAuthClient, t: &Tokens) -> Result<Tokens, String> {
    let rt = t
        .refresh_token
        .clone()
        .ok_or("no refresh token; reconnect the account")?;
    let mut form = vec![
        (c.client_id_param.as_str(), c.client_id.as_str()),
        ("refresh_token", rt.as_str()),
        ("grant_type", "refresh_token"),
    ];
    if let Some(s) = &c.client_secret {
        form.push(("client_secret", s));
    }
    let mut nt = token_request(c, &form, Some(rt.clone())).await?;
    nt.account = t.account.clone();
    Ok(nt)
}

async fn token_request(c: &OAuthClient, form: &[(&str, &str)], keep_refresh: Option<String>) -> Result<Tokens, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let r = client
        .post(&c.token_url)
        .form(form)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = r.status();
    let body = r.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "token endpoint {status}: {}",
            body.chars().take(300).collect::<String>()
        ));
    }
    let tr: TokenResp = serde_json::from_str(&body).map_err(|e| format!("token response: {e}"))?;
    Ok(Tokens {
        access_token: tr.access_token,
        refresh_token: tr.refresh_token.or(keep_refresh),
        expires_at: now_ms() + tr.expires_in.unwrap_or(3600) * 1000,
        account: String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_is_s256_of_verifier() {
        let p = pkce();
        assert!(p.verifier.len() >= 43 && p.verifier.len() <= 128);
        let expect = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(p.verifier.as_bytes()));
        assert_eq!(p.challenge, expect);
        assert_ne!(pkce().verifier, p.verifier);
    }

    #[test]
    fn auth_url_has_required_params() {
        let c = OAuthClient::google("cid.apps.googleusercontent.com", None);
        let p = pkce();
        let u = authorize_url(&c, "http://127.0.0.1:5555/callback", &p);
        for needle in [
            "code_challenge_method=S256",
            "access_type=offline",
            "youtube.upload",
            "redirect_uri=http%3A%2F%2F127.0.0.1%3A5555",
        ] {
            assert!(u.contains(needle), "{needle} missing from {u}");
        }
    }
}
