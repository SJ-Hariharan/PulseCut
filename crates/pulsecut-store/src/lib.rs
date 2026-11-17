//! SQLite persistence (WAL mode). Schema migrations are versioned with `PRAGMA user_version`.
//!
//! * `clips`   – the clip library (status, paths, metadata, the full event/plan as JSON)
//! * `jobs`    – durable work queue; `recover()` re-queues jobs that were running at a crash
//! * `uploads` – publishing queue/history, unique per (clip, platform, account) → no duplicates
//! * `events`  – detection log for the Hype Map history

use pulsecut_core::{ClipMetadata, ClipStatus, Coverage, EditPlan, HighlightEvent, TsMs, now_ms};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow};
use sqlx::{Row, SqlitePool};
use std::path::Path;
use uuid::Uuid;

pub type Result<T> = std::result::Result<T, sqlx::Error>;

const MIGRATIONS: &[&str] = &[
    // v1
    r#"
    CREATE TABLE IF NOT EXISTS clips (
        id TEXT PRIMARY KEY,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        status TEXT NOT NULL,
        stream_id TEXT NOT NULL,
        trigger TEXT NOT NULL,
        kind TEXT NOT NULL,
        confidence REAL NOT NULL,
        score REAL NOT NULL,
        moment_ts INTEGER NOT NULL,
        clip_start INTEGER NOT NULL,
        clip_end INTEGER NOT NULL,
        duration_ms INTEGER NOT NULL DEFAULT 0,
        raw_path TEXT,
        video_path TEXT,
        landscape_path TEXT,
        thumb_path TEXT,
        thumb_vertical_path TEXT,
        title TEXT NOT NULL DEFAULT '',
        meta_json TEXT,
        event_json TEXT NOT NULL,
        plan_json TEXT,
        coverage_json TEXT,
        transcript TEXT NOT NULL DEFAULT '',
        words_json TEXT,
        error TEXT
    );
    CREATE INDEX IF NOT EXISTS clips_created ON clips(created_at DESC);
    CREATE INDEX IF NOT EXISTS clips_status ON clips(status);
    CREATE TABLE IF NOT EXISTS jobs (
        id TEXT PRIMARY KEY,
        kind TEXT NOT NULL,
        clip_id TEXT,
        state TEXT NOT NULL,
        attempts INTEGER NOT NULL DEFAULT 0,
        payload_json TEXT NOT NULL,
        error TEXT,
        run_after INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS jobs_state ON jobs(state, run_after);
    CREATE TABLE IF NOT EXISTS uploads (
        id TEXT PRIMARY KEY,
        clip_id TEXT NOT NULL,
        platform TEXT NOT NULL,
        account TEXT NOT NULL DEFAULT '',
        state TEXT NOT NULL,
        attempts INTEGER NOT NULL DEFAULT 0,
        privacy TEXT NOT NULL DEFAULT 'unlisted',
        title TEXT NOT NULL DEFAULT '',
        description TEXT NOT NULL DEFAULT '',
        scheduled_at INTEGER,
        remote_id TEXT,
        remote_url TEXT,
        session_url TEXT,
        bytes_sent INTEGER NOT NULL DEFAULT 0,
        error TEXT,
        next_attempt_at INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        UNIQUE(clip_id, platform, account)
    );
    CREATE TABLE IF NOT EXISTS events (
        id TEXT PRIMARY KEY,
        stream_id TEXT NOT NULL,
        ts INTEGER NOT NULL,
        json TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS events_ts ON events(ts DESC);
    "#,
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipRecord {
    pub id: Uuid,
    pub created_at: TsMs,
    pub updated_at: TsMs,
    pub status: ClipStatus,
    pub stream_id: String,
    pub event: HighlightEvent,
    pub clip_start: TsMs,
    pub clip_end: TsMs,
    pub duration_ms: i64,
    pub raw_path: Option<String>,
    pub video_path: Option<String>,
    pub landscape_path: Option<String>,
    pub thumb_path: Option<String>,
    pub thumb_vertical_path: Option<String>,
    pub meta: Option<ClipMetadata>,
    pub plan: Option<EditPlan>,
    pub coverage: Option<Coverage>,
    pub transcript: String,
    pub words_json: Option<String>,
    pub error: Option<String>,
}

impl ClipRecord {
    pub fn new(event: HighlightEvent, clip_start: TsMs, clip_end: TsMs) -> Self {
        let t = now_ms();
        ClipRecord {
            id: Uuid::new_v4(),
            created_at: t,
            updated_at: t,
            status: ClipStatus::Capturing,
            stream_id: event.stream_id.clone(),
            event,
            clip_start,
            clip_end,
            duration_ms: 0,
            raw_path: None,
            video_path: None,
            landscape_path: None,
            thumb_path: None,
            thumb_vertical_path: None,
            meta: None,
            plan: None,
            coverage: None,
            transcript: String::new(),
            words_json: None,
            error: None,
        }
    }
    pub fn title(&self) -> &str {
        self.meta.as_ref().map(|m| m.title.as_str()).unwrap_or("")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadState {
    Queued,
    Uploading,
    Processing,
    Published,
    Failed,
    Cancelled,
}

impl UploadState {
    pub fn as_str(self) -> &'static str {
        match self {
            UploadState::Queued => "queued",
            UploadState::Uploading => "uploading",
            UploadState::Processing => "processing",
            UploadState::Published => "published",
            UploadState::Failed => "failed",
            UploadState::Cancelled => "cancelled",
        }
    }
    pub fn parse(s: &str) -> UploadState {
        match s {
            "uploading" => UploadState::Uploading,
            "processing" => UploadState::Processing,
            "published" => UploadState::Published,
            "failed" => UploadState::Failed,
            "cancelled" => UploadState::Cancelled,
            _ => UploadState::Queued,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadRecord {
    pub id: Uuid,
    pub clip_id: Uuid,
    pub platform: String,
    pub account: String,
    pub state: UploadState,
    pub attempts: i64,
    pub privacy: String,
    pub title: String,
    pub description: String,
    pub scheduled_at: Option<TsMs>,
    pub remote_id: Option<String>,
    pub remote_url: Option<String>,
    pub session_url: Option<String>,
    pub bytes_sent: i64,
    pub error: Option<String>,
    pub next_attempt_at: TsMs,
    pub created_at: TsMs,
    pub updated_at: TsMs,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: Uuid,
    pub kind: String,
    pub clip_id: Option<Uuid>,
    pub state: String,
    pub attempts: i64,
    pub payload: serde_json::Value,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ClipFilter {
    pub status: Option<ClipStatus>,
    pub search: Option<String>,
    pub limit: i64,
}

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

fn uuid_of(row: &SqliteRow, col: &str) -> Uuid {
    row.try_get::<String, _>(col)
        .ok()
        .and_then(|s| Uuid::parse_str(&s).ok())
        .unwrap_or_default()
}

fn json_opt<T: serde::de::DeserializeOwned>(row: &SqliteRow, col: &str) -> Option<T> {
    row.try_get::<Option<String>, _>(col)
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
}

fn to_json<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "null".into())
}

impl Store {
    pub async fn open(path: &Path) -> Result<Store> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(std::time::Duration::from_secs(5));
        let pool = SqlitePoolOptions::new().max_connections(4).connect_with(opts).await?;
        let s = Store { pool };
        s.migrate().await?;
        Ok(s)
    }

    pub async fn open_memory() -> Result<Store> {
        let opts = SqliteConnectOptions::new().in_memory(true);
        let pool = SqlitePoolOptions::new().max_connections(1).connect_with(opts).await?;
        let s = Store { pool };
        s.migrate().await?;
        Ok(s)
    }

    async fn migrate(&self) -> Result<()> {
        let v: i64 = sqlx::query("PRAGMA user_version").fetch_one(&self.pool).await?.get(0);
        for (i, m) in MIGRATIONS.iter().enumerate().skip(v as usize) {
            let mut tx = self.pool.begin().await?;
            for stmt in m.split(';').map(str::trim).filter(|s| !s.is_empty()) {
                sqlx::query(stmt).execute(&mut *tx).await?;
            }
            sqlx::query(sqlx::AssertSqlSafe(format!("PRAGMA user_version = {}", i + 1)))
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            tracing::info!("database migrated to v{}", i + 1);
        }
        Ok(())
    }

    // ---------- clips ----------

    pub async fn upsert_clip(&self, c: &ClipRecord) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO clips (id, created_at, updated_at, status, stream_id, trigger, kind, confidence, score,
                moment_ts, clip_start, clip_end, duration_ms, raw_path, video_path, landscape_path, thumb_path,
                thumb_vertical_path, title, meta_json, event_json, plan_json, coverage_json, transcript, words_json, error)
               VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
               ON CONFLICT(id) DO UPDATE SET updated_at=excluded.updated_at, status=excluded.status,
                kind=excluded.kind, confidence=excluded.confidence, score=excluded.score,
                clip_start=excluded.clip_start, clip_end=excluded.clip_end, duration_ms=excluded.duration_ms,
                raw_path=excluded.raw_path, video_path=excluded.video_path, landscape_path=excluded.landscape_path,
                thumb_path=excluded.thumb_path, thumb_vertical_path=excluded.thumb_vertical_path, title=excluded.title,
                meta_json=excluded.meta_json, event_json=excluded.event_json, plan_json=excluded.plan_json,
                coverage_json=excluded.coverage_json, transcript=excluded.transcript, words_json=excluded.words_json,
                error=excluded.error"#,
        )
        .bind(c.id.to_string())
        .bind(c.created_at)
        .bind(now_ms())
        .bind(c.status.as_str())
        .bind(&c.stream_id)
        .bind(c.event.trigger.label())
        .bind(c.event.kind.label())
        .bind(c.event.confidence as f64)
        .bind(c.event.score as f64)
        .bind(c.event.moment_ts)
        .bind(c.clip_start)
        .bind(c.clip_end)
        .bind(c.duration_ms)
        .bind(&c.raw_path)
        .bind(&c.video_path)
        .bind(&c.landscape_path)
        .bind(&c.thumb_path)
        .bind(&c.thumb_vertical_path)
        .bind(c.title())
        .bind(c.meta.as_ref().map(to_json))
        .bind(to_json(&c.event))
        .bind(c.plan.as_ref().map(to_json))
        .bind(c.coverage.as_ref().map(to_json))
        .bind(&c.transcript)
        .bind(&c.words_json)
        .bind(&c.error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    fn row_to_clip(r: &SqliteRow) -> Option<ClipRecord> {
        Some(ClipRecord {
            id: uuid_of(r, "id"),
            created_at: r.try_get("created_at").ok()?,
            updated_at: r.try_get("updated_at").ok()?,
            status: ClipStatus::parse(&r.try_get::<String, _>("status").ok()?).unwrap_or(ClipStatus::Failed),
            stream_id: r.try_get("stream_id").ok()?,
            event: json_opt(r, "event_json")?,
            clip_start: r.try_get("clip_start").ok()?,
            clip_end: r.try_get("clip_end").ok()?,
            duration_ms: r.try_get("duration_ms").ok()?,
            raw_path: r.try_get("raw_path").ok()?,
            video_path: r.try_get("video_path").ok()?,
            landscape_path: r.try_get("landscape_path").ok()?,
            thumb_path: r.try_get("thumb_path").ok()?,
            thumb_vertical_path: r.try_get("thumb_vertical_path").ok()?,
            meta: json_opt(r, "meta_json"),
            plan: json_opt(r, "plan_json"),
            coverage: json_opt(r, "coverage_json"),
            transcript: r.try_get("transcript").ok()?,
            words_json: r.try_get("words_json").ok()?,
            error: r.try_get("error").ok()?,
        })
    }

    pub async fn get_clip(&self, id: Uuid) -> Result<Option<ClipRecord>> {
        let r = sqlx::query("SELECT * FROM clips WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        Ok(r.as_ref().and_then(Store::row_to_clip))
    }

    pub async fn list_clips(&self, f: &ClipFilter) -> Result<Vec<ClipRecord>> {
        let limit = if f.limit <= 0 { 500 } else { f.limit };
        let like = f.search.as_ref().map(|s| format!("%{}%", s.replace(['%', '_'], "")));
        let rows = sqlx::query(
            "SELECT * FROM clips WHERE (?1 IS NULL OR status = ?1)
               AND (?2 IS NULL OR title LIKE ?2 OR transcript LIKE ?2 OR stream_id LIKE ?2 OR kind LIKE ?2)
             ORDER BY created_at DESC LIMIT ?3",
        )
        .bind(f.status.map(|s| s.as_str()))
        .bind(like)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().filter_map(Store::row_to_clip).collect())
    }

    pub async fn set_clip_status(&self, id: Uuid, status: ClipStatus, error: Option<&str>) -> Result<()> {
        sqlx::query("UPDATE clips SET status = ?, error = ?, updated_at = ? WHERE id = ?")
            .bind(status.as_str())
            .bind(error)
            .bind(now_ms())
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn update_meta(&self, id: Uuid, meta: &ClipMetadata) -> Result<()> {
        sqlx::query("UPDATE clips SET meta_json = ?, title = ?, updated_at = ? WHERE id = ?")
            .bind(to_json(meta))
            .bind(&meta.title)
            .bind(now_ms())
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn delete_clip(&self, id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM clips WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Clips older than `before` that are not awaiting review/publishing (for retention).
    pub async fn expired_clips(&self, before: TsMs) -> Result<Vec<ClipRecord>> {
        let rows =
            sqlx::query("SELECT * FROM clips WHERE created_at < ? AND status IN ('published','rejected','failed')")
                .bind(before)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.iter().filter_map(Store::row_to_clip).collect())
    }

    pub async fn count_by_status(&self) -> Result<Vec<(String, i64)>> {
        let rows = sqlx::query("SELECT status, COUNT(*) AS n FROM clips GROUP BY status")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get::<String, _>("status"), r.get::<i64, _>("n")))
            .collect())
    }

    // ---------- jobs ----------

    pub async fn enqueue_job(&self, kind: &str, clip_id: Option<Uuid>, payload: &serde_json::Value) -> Result<Uuid> {
        let id = Uuid::new_v4();
        let t = now_ms();
        sqlx::query(
            "INSERT INTO jobs (id, kind, clip_id, state, attempts, payload_json, created_at, updated_at, run_after)
             VALUES (?, ?, ?, 'queued', 0, ?, ?, ?, 0)",
        )
        .bind(id.to_string())
        .bind(kind)
        .bind(clip_id.map(|c| c.to_string()))
        .bind(payload.to_string())
        .bind(t)
        .bind(t)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    /// Atomically claim the oldest runnable job of `kind`.
    pub async fn claim_job(&self, kind: &str) -> Result<Option<Job>> {
        let t = now_ms();
        let row = sqlx::query(
            "UPDATE jobs SET state = 'running', attempts = attempts + 1, updated_at = ?1
             WHERE id = (SELECT id FROM jobs WHERE kind = ?2 AND state = 'queued' AND run_after <= ?1
                         ORDER BY created_at LIMIT 1)
             RETURNING *",
        )
        .bind(t)
        .bind(kind)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| Job {
            id: uuid_of(&r, "id"),
            kind: r.get("kind"),
            clip_id: r
                .try_get::<Option<String>, _>("clip_id")
                .ok()
                .flatten()
                .and_then(|s| Uuid::parse_str(&s).ok()),
            state: r.get("state"),
            attempts: r.get("attempts"),
            payload: serde_json::from_str(&r.get::<String, _>("payload_json")).unwrap_or_default(),
            error: r.try_get("error").ok().flatten(),
        }))
    }

    pub async fn finish_job(&self, id: Uuid, ok: bool, error: Option<&str>) -> Result<()> {
        sqlx::query("UPDATE jobs SET state = ?, error = ?, updated_at = ? WHERE id = ?")
            .bind(if ok { "done" } else { "failed" })
            .bind(error)
            .bind(now_ms())
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn retry_job_later(&self, id: Uuid, delay_ms: i64, error: &str) -> Result<()> {
        sqlx::query("UPDATE jobs SET state = 'queued', error = ?, run_after = ?, updated_at = ? WHERE id = ?")
            .bind(error)
            .bind(now_ms() + delay_ms)
            .bind(now_ms())
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Crash recovery: anything left `running` is re-queued; clips stuck mid-render are marked
    /// for re-render. Returns (jobs requeued, uploads reset).
    pub async fn recover(&self) -> Result<(u64, u64)> {
        let j = sqlx::query("UPDATE jobs SET state = 'queued', updated_at = ? WHERE state = 'running'")
            .bind(now_ms())
            .execute(&self.pool)
            .await?
            .rows_affected();
        let u = sqlx::query("UPDATE uploads SET state = 'queued', updated_at = ? WHERE state = 'uploading'")
            .bind(now_ms())
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok((j, u))
    }

    pub async fn pending_jobs(&self) -> Result<i64> {
        Ok(
            sqlx::query("SELECT COUNT(*) FROM jobs WHERE state IN ('queued','running')")
                .fetch_one(&self.pool)
                .await?
                .get(0),
        )
    }

    // ---------- uploads ----------

    /// Create an upload unless one already exists for (clip, platform, account). Returns the
    /// existing record in that case — duplicate prevention.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_upload(
        &self,
        clip_id: Uuid,
        platform: &str,
        account: &str,
        title: &str,
        description: &str,
        privacy: &str,
        scheduled_at: Option<TsMs>,
    ) -> Result<(UploadRecord, bool)> {
        if let Some(u) = self.find_upload(clip_id, platform, account).await? {
            return Ok((u, false));
        }
        let t = now_ms();
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO uploads (id, clip_id, platform, account, state, attempts, privacy, title, description,
               scheduled_at, created_at, updated_at) VALUES (?, ?, ?, ?, 'queued', 0, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(clip_id.to_string())
        .bind(platform)
        .bind(account)
        .bind(privacy)
        .bind(title)
        .bind(description)
        .bind(scheduled_at)
        .bind(t)
        .bind(t)
        .execute(&self.pool)
        .await?;
        Ok((self.get_upload(id).await?.expect("just inserted"), true))
    }

    fn row_to_upload(r: &SqliteRow) -> UploadRecord {
        UploadRecord {
            id: uuid_of(r, "id"),
            clip_id: uuid_of(r, "clip_id"),
            platform: r.get("platform"),
            account: r.get("account"),
            state: UploadState::parse(&r.get::<String, _>("state")),
            attempts: r.get("attempts"),
            privacy: r.get("privacy"),
            title: r.get("title"),
            description: r.get("description"),
            scheduled_at: r.try_get("scheduled_at").ok().flatten(),
            remote_id: r.try_get("remote_id").ok().flatten(),
            remote_url: r.try_get("remote_url").ok().flatten(),
            session_url: r.try_get("session_url").ok().flatten(),
            bytes_sent: r.get("bytes_sent"),
            error: r.try_get("error").ok().flatten(),
            next_attempt_at: r.get("next_attempt_at"),
            created_at: r.get("created_at"),
            updated_at: r.get("updated_at"),
        }
    }

    pub async fn get_upload(&self, id: Uuid) -> Result<Option<UploadRecord>> {
        let r = sqlx::query("SELECT * FROM uploads WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        Ok(r.as_ref().map(Store::row_to_upload))
    }

    pub async fn find_upload(&self, clip_id: Uuid, platform: &str, account: &str) -> Result<Option<UploadRecord>> {
        let r = sqlx::query("SELECT * FROM uploads WHERE clip_id = ? AND platform = ? AND account = ?")
            .bind(clip_id.to_string())
            .bind(platform)
            .bind(account)
            .fetch_optional(&self.pool)
            .await?;
        Ok(r.as_ref().map(Store::row_to_upload))
    }

    pub async fn list_uploads(&self, limit: i64) -> Result<Vec<UploadRecord>> {
        let rows = sqlx::query("SELECT * FROM uploads ORDER BY created_at DESC LIMIT ?")
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(Store::row_to_upload).collect())
    }

    /// Next upload due for an attempt.
    pub async fn next_due_upload(&self) -> Result<Option<UploadRecord>> {
        let r = sqlx::query(
            "SELECT * FROM uploads WHERE state = 'queued' AND next_attempt_at <= ? ORDER BY created_at LIMIT 1",
        )
        .bind(now_ms())
        .fetch_optional(&self.pool)
        .await?;
        Ok(r.as_ref().map(Store::row_to_upload))
    }

    pub async fn save_upload(&self, u: &UploadRecord) -> Result<()> {
        sqlx::query(
            "UPDATE uploads SET state = ?, attempts = ?, title = ?, description = ?, privacy = ?, scheduled_at = ?,
               remote_id = ?, remote_url = ?, session_url = ?, bytes_sent = ?, error = ?, next_attempt_at = ?,
               updated_at = ? WHERE id = ?",
        )
        .bind(u.state.as_str())
        .bind(u.attempts)
        .bind(&u.title)
        .bind(&u.description)
        .bind(&u.privacy)
        .bind(u.scheduled_at)
        .bind(&u.remote_id)
        .bind(&u.remote_url)
        .bind(&u.session_url)
        .bind(u.bytes_sent)
        .bind(&u.error)
        .bind(u.next_attempt_at)
        .bind(now_ms())
        .bind(u.id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn uploads_since(&self, platform: &str, since: TsMs) -> Result<i64> {
        Ok(
            sqlx::query("SELECT COUNT(*) FROM uploads WHERE platform = ? AND created_at >= ? AND state != 'cancelled'")
                .bind(platform)
                .bind(since)
                .fetch_one(&self.pool)
                .await?
                .get(0),
        )
    }

    // ---------- events ----------

    pub async fn log_event(&self, ev: &HighlightEvent) -> Result<()> {
        sqlx::query("INSERT OR REPLACE INTO events (id, stream_id, ts, json) VALUES (?, ?, ?, ?)")
            .bind(ev.id.to_string())
            .bind(&ev.stream_id)
            .bind(ev.moment_ts)
            .bind(to_json(ev))
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn recent_events(&self, limit: i64) -> Result<Vec<HighlightEvent>> {
        let rows = sqlx::query("SELECT json FROM events ORDER BY ts DESC LIMIT ?")
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .iter()
            .filter_map(|r| serde_json::from_str(&r.get::<String, _>("json")).ok())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsecut_core::TriggerSource;

    fn clip() -> ClipRecord {
        let mut ev = HighlightEvent::manual("twitch:test", TriggerSource::Chat, 1_000_000);
        ev.top_phrases = vec!["no way".into()];
        ClipRecord::new(ev, 980_000, 1_010_000)
    }

    #[tokio::test]
    async fn clip_roundtrip_filter_and_status() {
        let s = Store::open_memory().await.unwrap();
        let mut c = clip();
        c.meta = Some(ClipMetadata {
            title: "No way he did that".into(),
            ..Default::default()
        });
        s.upsert_clip(&c).await.unwrap();
        let back = s.get_clip(c.id).await.unwrap().unwrap();
        assert_eq!(back.event, c.event);
        assert_eq!(back.title(), "No way he did that");
        s.set_clip_status(c.id, ClipStatus::Review, None).await.unwrap();
        let review = s
            .list_clips(&ClipFilter {
                status: Some(ClipStatus::Review),
                search: Some("way".into()),
                limit: 10,
            })
            .await
            .unwrap();
        assert_eq!(review.len(), 1);
        let none = s
            .list_clips(&ClipFilter {
                search: Some("zebra".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(none.is_empty());
    }

    #[tokio::test]
    async fn jobs_claim_and_crash_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        let id = {
            let s = Store::open(&path).await.unwrap();
            let id = s
                .enqueue_job("render", None, &serde_json::json!({"x":1}))
                .await
                .unwrap();
            let j = s.claim_job("render").await.unwrap().unwrap();
            assert_eq!(j.id, id);
            assert!(s.claim_job("render").await.unwrap().is_none(), "no double claim");
            id
            // "crash": drop the store while the job is running
        };
        let s = Store::open(&path).await.unwrap();
        let (requeued, _) = s.recover().await.unwrap();
        assert_eq!(requeued, 1);
        let j = s.claim_job("render").await.unwrap().unwrap();
        assert_eq!(j.id, id);
        assert_eq!(j.attempts, 2);
        s.finish_job(j.id, true, None).await.unwrap();
        assert_eq!(s.pending_jobs().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn uploads_are_deduplicated() {
        let s = Store::open_memory().await.unwrap();
        let c = clip();
        s.upsert_clip(&c).await.unwrap();
        let (u1, new1) = s
            .create_upload(c.id, "youtube", "me", "t", "d", "unlisted", None)
            .await
            .unwrap();
        let (u2, new2) = s
            .create_upload(c.id, "youtube", "me", "t", "d", "unlisted", None)
            .await
            .unwrap();
        assert!(new1 && !new2);
        assert_eq!(u1.id, u2.id);
        let mut u = u1;
        u.state = UploadState::Uploading;
        s.save_upload(&u).await.unwrap();
        assert_eq!(s.recover().await.unwrap().1, 1);
        assert_eq!(s.next_due_upload().await.unwrap().unwrap().id, u.id);
    }
}
