//! End-to-end upload worker against a mock YouTube API (token refresh, resumable session,
//! mid-upload 503, resume via Range, duplicate prevention).

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{post, put};
use pulsecut_core::{ClipStatus, HighlightEvent, TriggerSource};
use pulsecut_publish::oauth::{OAuthClient, Tokens};
use pulsecut_publish::worker::{TokenStore, Worker, WorkerConfig};
use pulsecut_publish::youtube::YouTube;
use pulsecut_store::{ClipRecord, Store, UploadState};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Mock {
    received: Vec<u8>,
    puts: u32,
    fail_on_put: Option<u32>,
    sessions: u32,
    refreshes: u32,
    last_token: String,
}
type S = Arc<Mutex<Mock>>;

async fn token(State(s): State<S>) -> impl IntoResponse {
    s.lock().unwrap().refreshes += 1;
    axum::Json(serde_json::json!({"access_token": "fresh-token", "expires_in": 3600}))
}

async fn init(State(s): State<S>, h: HeaderMap) -> impl IntoResponse {
    let mut m = s.lock().unwrap();
    m.sessions += 1;
    m.last_token = h
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let port = h.get("host").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    (StatusCode::OK, [("Location", format!("http://{port}/session/1"))], "")
}

async fn chunk(State(s): State<S>, h: HeaderMap, body: Bytes) -> axum::response::Response {
    let mut m = s.lock().unwrap();
    let range = h
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let total: usize = range.rsplit('/').next().unwrap().parse().unwrap();
    if range.starts_with("bytes */") {
        // status query
        if m.received.is_empty() {
            return (StatusCode::PERMANENT_REDIRECT, "").into_response();
        }
        return (
            StatusCode::PERMANENT_REDIRECT,
            [("Range", format!("bytes=0-{}", m.received.len() - 1))],
            "",
        )
            .into_response();
    }
    m.puts += 1;
    if m.fail_on_put == Some(m.puts) {
        return (StatusCode::SERVICE_UNAVAILABLE, "backend error").into_response();
    }
    let start: usize = range
        .trim_start_matches("bytes ")
        .split('-')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(start, m.received.len(), "chunks must be contiguous");
    m.received.extend_from_slice(&body);
    if m.received.len() == total {
        (StatusCode::OK, axum::Json(serde_json::json!({"id": "vid123"}))).into_response()
    } else {
        (
            StatusCode::PERMANENT_REDIRECT,
            [("Range", format!("bytes=0-{}", m.received.len() - 1))],
            "",
        )
            .into_response()
    }
}

struct Tok(Mutex<HashMap<String, String>>);
impl TokenStore for Tok {
    fn get(&self, n: &str) -> Option<String> {
        self.0.lock().unwrap().get(n).cloned()
    }
    fn set(&self, n: &str, v: &str) {
        self.0.lock().unwrap().insert(n.into(), v.into());
    }
}

#[tokio::test]
async fn resumable_upload_survives_failure() {
    let state: S = Arc::new(Mutex::new(Mock {
        fail_on_put: Some(2),
        ..Default::default()
    }));
    let app = axum::Router::new()
        .route("/token", post(token))
        .route("/upload/videos", post(init))
        .route("/session/1", put(chunk))
        .layer(axum::extract::DefaultBodyLimit::disable())
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let dir = tempfile::tempdir().unwrap();
    let video = dir.path().join("clip.mp4");
    let data: Vec<u8> = (0..(20 * 1024 * 1024 + 12345)).map(|i| (i % 251) as u8).collect();
    std::fs::write(&video, &data).unwrap();

    let store = Store::open_memory().await.unwrap();
    let mut clip = ClipRecord::new(HighlightEvent::manual("t", TriggerSource::Hotkey, 0), 0, 1);
    clip.video_path = Some(video.display().to_string());
    clip.status = ClipStatus::Approved;
    store.upsert_clip(&clip).await.unwrap();
    let (up, created) = store
        .create_upload(clip.id, "youtube", "me", "Title", "Desc", "unlisted", None)
        .await
        .unwrap();
    assert!(created);
    let (_, again) = store
        .create_upload(clip.id, "youtube", "me", "Title", "Desc", "unlisted", None)
        .await
        .unwrap();
    assert!(!again, "duplicate upload prevented");

    let tokens = Arc::new(Tok(Mutex::new(HashMap::new())));
    let expired = Tokens {
        access_token: "old".into(),
        refresh_token: Some("r".into()),
        expires_at: 0,
        account: "me".into(),
    };
    tokens.set("youtube-oauth", &serde_json::to_string(&expired).unwrap());
    let mut client = OAuthClient::google("cid", None);
    client.token_url = format!("{base}/token");
    let worker = Worker {
        store: store.clone(),
        tokens: tokens.clone(),
        cfg: Arc::new(Mutex::new(WorkerConfig {
            youtube_client: Some(client),
            youtube_category: "20".into(),
            instagram_user_id: String::new(),
            hosting: None,
            tiktok_client: None,
            max_attempts: 5,
            daily_caps: HashMap::new(),
        })),
        youtube: YouTube {
            http: reqwest::Client::new(),
            upload_base: format!("{base}/upload"),
        },
        instagram: Default::default(),
        tiktok: Default::default(),
        progress: Default::default(),
    };

    // Attempt 1: second chunk fails with 503 -> queued for retry, session URL persisted.
    worker.process(store.get_upload(up.id).await.unwrap().unwrap()).await;
    let u = store.get_upload(up.id).await.unwrap().unwrap();
    assert_eq!(u.state, UploadState::Queued, "{:?}", u.error);
    assert!(u.session_url.is_some());
    assert!(u.next_attempt_at > pulsecut_core::now_ms());
    assert_eq!(state.lock().unwrap().refreshes, 1, "expired token refreshed");
    assert_eq!(state.lock().unwrap().last_token, "Bearer fresh-token");

    // Attempt 2 (simulating an app restart: process from the stored record): resumes.
    let mut u2 = u.clone();
    u2.next_attempt_at = 0;
    store.save_upload(&u2).await.unwrap();
    worker.process(store.get_upload(up.id).await.unwrap().unwrap()).await;
    let u = store.get_upload(up.id).await.unwrap().unwrap();
    assert_eq!(u.state, UploadState::Published, "{:?}", u.error);
    assert_eq!(u.remote_id.as_deref(), Some("vid123"));
    {
        let m = state.lock().unwrap();
        assert_eq!(m.sessions, 1, "resumed the same session instead of starting over");
        assert_eq!(m.received, data, "server got every byte exactly once, in order");
    }
    assert_eq!(
        store.get_clip(clip.id).await.unwrap().unwrap().status,
        ClipStatus::Published
    );
}
