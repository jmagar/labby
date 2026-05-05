//! SQLite-backed ACP persistence.
//!
//! # Architecture
//!
//! - Single-connection `write_pool` (r2d2, `max_size=1`) — session upserts and
//!   state updates, serialized through the pool.
//! - Multi-connection `read_pool` (r2d2, `max_size=4`) — WAL-mode readers
//!   proceed in parallel; each pooled connection is opened with
//!   `query_only=true`.
//! - Dedicated writer-task connection — privately owned by the background task
//!   that drains the mpsc channel and batches `acp_session_events` INSERTs
//!   (up to 64 events or 10 ms, whichever comes first). Kept separate from the
//!   write_pool because the batcher is single-owner and a pool would add
//!   no concurrency there.
//!
//! All connection-level pragmas (busy_timeout, WAL, synchronous, mmap_size,
//! cache_size, wal_autocheckpoint) are applied per-connection via
//! `SqliteConnectionManager::with_init`, never inside a transaction.
//!
//! # Path security
//!
//! The `LAB_ACP_DB` path is validated to reject any component that is a
//! `ParentDir` (`..`). The soft-canonicalize crate is yanked; we do the
//! check manually with `std::path::Component`.
//!
//! # File permissions
//!
//! The database file is created with mode 0600 (owner read/write only) on
//! first open. Subsequent opens do not change permissions.
//!
//! # HMAC-signed permission outcomes
//!
//! `PermissionOutcome` events have their `granted` field signed with
//! HMAC-SHA256 before storage. The key is read from `LAB_ACP_HMAC_SECRET`
//! when set. If the env var is absent, Lab generates an ephemeral per-process
//! fallback key that is not persisted and therefore rotates on restart. Set
//! `LAB_ACP_HMAC_SECRET` in `~/.lab/.env` when permission-outcome signatures
//! must verify across process restarts. This prevents DB-write bypass attacks
//! where an attacker could flip a `false` grant to `true` in the raw SQLite
//! file for events signed by the active key.
//!
//! # Payload redaction
//!
//! Before any event payload is written, fields named `token`, `api_key`,
//! `password`, `secret`, or `authorization` are replaced with `"[REDACTED]"`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use hmac::{Hmac, KeyInit, Mac};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{Connection, OpenFlags, params};
use serde_json::{Value, json};
use sha2::Sha256;
use tokio::sync::mpsc;

use lab_apis::acp::error::{AcpError, PersistenceError};
use lab_apis::acp::persistence::AcpPersistence;
use lab_apis::acp::types::{AcpEvent, AcpSessionState, AcpSessionSummary};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum number of events to accumulate before flushing.
const BATCH_SIZE: usize = 64;

/// Maximum wait before flushing a partial batch.
const BATCH_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(10);

// ── Types ─────────────────────────────────────────────────────────────────────

type HmacSha256 = Hmac<Sha256>;

/// Commands sent from the public API to the writer task.
enum PersistCmd {
    /// Event with pre-redacted, pre-serialized payload. The typed `AcpEvent`
    /// is kept for envelope-field accessors (id/session_id/seq/kind/created_at);
    /// `payload` is the JSON string written to the DB.
    AppendEvent(Box<AcpEvent>, String),
    /// Flush the current batch immediately and notify via the oneshot sender.
    /// Used for testing and graceful shutdown.
    #[allow(dead_code)]
    Flush(tokio::sync::oneshot::Sender<Result<(), String>>),
}

// ── SqliteAcpPersistence ──────────────────────────────────────────────────────

/// SQLite-backed implementation of `AcpPersistence`.
///
/// Clone is cheap — all state is behind `Arc`.
#[derive(Clone)]
pub struct SqliteAcpPersistence {
    /// Single-connection write pool for session upserts and state updates.
    write_pool: Pool<SqliteConnectionManager>,
    /// Multi-connection read pool; WAL lets pooled readers run in parallel.
    read_pool: Pool<SqliteConnectionManager>,
    /// Channel to the background writer task (hot path for event appends).
    event_tx: mpsc::Sender<PersistCmd>,
    /// HMAC key for signing permission outcomes.
    hmac_key: Arc<Vec<u8>>,
}

fn acp_pragma_init(
    query_only: bool,
) -> impl Fn(&mut Connection) -> rusqlite::Result<()> + Send + Sync + 'static {
    move |conn| {
        conn.busy_timeout(std::time::Duration::from_millis(5_000))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "mmap_size", 134_217_728_i64)?;
        conn.pragma_update(None, "cache_size", -65_536_i64)?;
        conn.pragma_update(None, "wal_autocheckpoint", 1000_i64)?;
        if query_only {
            conn.pragma_update(None, "query_only", "true")?;
        }
        Ok(())
    }
}

impl SqliteAcpPersistence {
    /// Open (or create) the ACP database.
    ///
    /// `db_path` must not contain `..` components. The file is created with
    /// mode 0600 on first open.
    pub async fn open(db_path: PathBuf) -> Result<Self, AcpError> {
        // Reject `..` components specifically — absolute paths are valid here.
        if db_path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(AcpError::Internal(format!(
                "LAB_ACP_DB path must not contain `..` components: {}",
                db_path.display()
            )));
        }

        let hmac_key = Arc::new(load_or_generate_hmac_key());
        let path = db_path.clone();

        let (write_pool, read_pool, writer_task_conn) =
            tokio::task::spawn_blocking(move || -> Result<_, String> {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| format!("create_dir_all: {e}"))?;
                }

                // Create the file with 0600 perms if it doesn't exist.
                #[cfg(unix)]
                create_db_file_0600(&path);

                let write_manager =
                    SqliteConnectionManager::file(&path).with_init(acp_pragma_init(false));
                let write_pool = Pool::builder()
                    .max_size(1)
                    .connection_timeout(std::time::Duration::from_secs(5))
                    .build(write_manager)
                    .map_err(|e| format!("build write pool: {e}"))?;

                // Run migrations on a connection from the write pool.
                {
                    let conn = write_pool
                        .get()
                        .map_err(|e| format!("get write conn: {e}"))?;
                    migrate(&conn).map_err(|e| format!("migrate: {e}"))?;
                }

                let read_manager =
                    SqliteConnectionManager::file(&path).with_init(acp_pragma_init(true));
                let read_pool = Pool::builder()
                    .max_size(4)
                    .connection_timeout(std::time::Duration::from_secs(5))
                    .build(read_manager)
                    .map_err(|e| format!("build read pool: {e}"))?;

                // Writer task connection — dedicated single-owner connection
                // for the hot-path batch inserts. Not pooled because the
                // writer task is serial by design.
                let rw_flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
                let mut tc = Connection::open_with_flags(&path, rw_flags)
                    .map_err(|e| format!("open writer conn: {e}"))?;
                acp_pragma_init(false)(&mut tc).map_err(|e| format!("writer conn pragmas: {e}"))?;

                Ok((write_pool, read_pool, tc))
            })
            .await
            .map_err(|e| AcpError::Internal(format!("db open join: {e}")))?
            .map_err(|e| AcpError::Persistence(PersistenceError::Sqlite(e)))?;

        // Bounded channel — back-pressure if writer falls behind.
        let (event_tx, event_rx) = mpsc::channel::<PersistCmd>(4096);

        // Spawn the background writer task.
        let writer_conn = Arc::new(Mutex::new(writer_task_conn));
        tokio::spawn(writer_task(writer_conn, event_rx));

        Ok(Self {
            write_pool,
            read_pool,
            event_tx,
            hmac_key,
        })
    }

    /// Open using the path from `LAB_ACP_DB` (or a default under `~/.lab/`).
    pub async fn from_env() -> Result<Self, AcpError> {
        let path = resolve_db_path()?;
        Self::open(path).await
    }

    // ── Internal helpers ───────────────────────────────────────────────────────

    async fn blocking_write<T, F>(&self, label: &'static str, f: F) -> Result<T, AcpError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, rusqlite::Error> + Send + 'static,
    {
        let pool = self.write_pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool
                .get()
                .map_err(|e| AcpError::Internal(format!("{label} pool get: {e}")))?;
            f(&conn).map_err(|e| {
                AcpError::Persistence(PersistenceError::Sqlite(format!("{label}: {e}")))
            })
        })
        .await
        .map_err(|e| AcpError::Internal(format!("{label} join: {e}")))?
    }

    async fn blocking_read<T, F>(&self, label: &'static str, f: F) -> Result<T, AcpError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, rusqlite::Error> + Send + 'static,
    {
        let pool = self.read_pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool
                .get()
                .map_err(|e| AcpError::Internal(format!("{label} pool get: {e}")))?;
            f(&conn).map_err(|e| {
                AcpError::Persistence(PersistenceError::Sqlite(format!("{label}: {e}")))
            })
        })
        .await
        .map_err(|e| AcpError::Internal(format!("{label} join: {e}")))?
    }
}

// ── Non-trait restore helpers ─────────────────────────────────────────────────

impl SqliteAcpPersistence {
    /// Return the max persisted event seq for every session in one query.
    /// Sessions with no events map to 0; callers should seed `next_seq = max + 1`.
    pub async fn load_max_seqs(&self) -> Result<std::collections::HashMap<String, u64>, AcpError> {
        self.blocking_read("load_max_seqs", |c| {
            let mut stmt = c.prepare(
                "SELECT session_id, MAX(seq) AS max_seq \
                 FROM acp_session_events \
                 GROUP BY session_id",
            )?;
            let rows = stmt.query_map([], |row| {
                let session_id: String = row.get("session_id")?;
                let max_seq: i64 = row.get("max_seq")?;
                Ok((session_id, max_seq as u64))
            })?;
            rows.collect::<rusqlite::Result<std::collections::HashMap<_, _>>>()
        })
        .await
    }
}

// ── AcpPersistence impl ───────────────────────────────────────────────────────

impl AcpPersistence for SqliteAcpPersistence {
    async fn load_sessions(&self) -> Result<Vec<AcpSessionSummary>, AcpError> {
        self.blocking_read("load_sessions", |c| db_load_sessions(c))
            .await
    }

    async fn load_events(&self, session_id: &str) -> Result<Vec<AcpEvent>, AcpError> {
        let sid = session_id.to_owned();
        let hmac_key = Arc::clone(&self.hmac_key);
        self.blocking_read("load_events", move |c| {
            db_load_events(c, &sid, None, None, hmac_key.as_slice())
        })
        .await
    }

    async fn load_events_since(
        &self,
        session_id: &str,
        since_seq: u64,
    ) -> Result<Vec<AcpEvent>, AcpError> {
        let sid = session_id.to_owned();
        let hmac_key = Arc::clone(&self.hmac_key);
        self.blocking_read("load_events_since", move |c| {
            db_load_events(c, &sid, Some(since_seq), None, hmac_key.as_slice())
        })
        .await
    }

    async fn load_events_since_capped(
        &self,
        session_id: &str,
        since_seq: u64,
        limit: u64,
    ) -> Result<Vec<AcpEvent>, AcpError> {
        let sid = session_id.to_owned();
        let hmac_key = Arc::clone(&self.hmac_key);
        self.blocking_read("load_events_since_capped", move |c| {
            db_load_events(c, &sid, Some(since_seq), Some(limit), hmac_key.as_slice())
        })
        .await
    }

    async fn save_session(&self, summary: &AcpSessionSummary) -> Result<(), AcpError> {
        let summary = summary.clone();
        self.blocking_write("save_session", move |c| db_save_session(c, &summary))
            .await
    }

    async fn append_event(&self, event: &AcpEvent) -> Result<(), AcpError> {
        // Serialize once, redact the JSON tree in place; no from_value round-trip.
        let payload = redact_event_payload(event, &self.hmac_key)
            .map_err(|e| AcpError::Internal(format!("serialize event: {e}")))?;

        self.event_tx
            .send(PersistCmd::AppendEvent(Box::new(event.clone()), payload))
            .await
            .map_err(|_| AcpError::Internal("event writer task channel closed".to_string()))
    }

    async fn update_session_state(
        &self,
        session_id: &str,
        state: AcpSessionState,
    ) -> Result<(), AcpError> {
        let sid = session_id.to_owned();
        self.blocking_write("update_session_state", move |c| {
            db_update_session_state(c, &sid, &state)
        })
        .await
    }
}

// ── Background writer task ────────────────────────────────────────────────────

async fn writer_task(conn: Arc<Mutex<Connection>>, mut rx: mpsc::Receiver<PersistCmd>) {
    let mut batch: Vec<(AcpEvent, String)> = Vec::with_capacity(BATCH_SIZE);

    loop {
        // Collect up to BATCH_SIZE events within BATCH_TIMEOUT.
        let deadline = tokio::time::Instant::now() + BATCH_TIMEOUT;

        loop {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Some(PersistCmd::AppendEvent(ev, payload))) => {
                    batch.push((*ev, payload));
                    if batch.len() >= BATCH_SIZE {
                        break;
                    }
                }
                Ok(Some(PersistCmd::Flush(tx))) => {
                    // Flush whatever we have now, then ack.
                    if let Some(retry) = flush_batch(&conn, &mut batch).await {
                        batch = retry;
                    }
                    tx.send(Ok(())).ok();
                    break;
                }
                Ok(None) => {
                    // Channel closed — flush and exit task.
                    drop(flush_batch(&conn, &mut batch).await);
                    return;
                }
                Err(_) => {
                    // Timeout — flush partial batch.
                    break;
                }
            }
        }

        if !batch.is_empty() {
            if let Some(retry) = flush_batch(&conn, &mut batch).await {
                batch = retry;
                tokio::time::sleep(BATCH_TIMEOUT).await;
            }
        }
    }
}

async fn flush_batch(
    conn: &Arc<Mutex<Connection>>,
    batch: &mut Vec<(AcpEvent, String)>,
) -> Option<Vec<(AcpEvent, String)>> {
    if batch.is_empty() {
        return None;
    }
    let events = std::mem::take(batch);
    let count = events.len();
    let retry_events = events.clone();
    let conn = Arc::clone(conn);
    let result = tokio::task::spawn_blocking(move || {
        let c = conn
            .lock()
            .map_err(|_| "writer mutex poisoned".to_string())?;
        db_batch_insert_events(&c, &events).map_err(|e| format!("batch insert events: {e}"))
    })
    .await;
    match result {
        Ok(Ok(())) => return None,
        Ok(Err(error)) => tracing::error!(
            surface = "acp",
            service = "persistence",
            action = "flush_batch",
            kind = "internal_error",
            events = count,
            error,
            "acp event batch insert failed",
        ),
        Err(join_err) => tracing::error!(
            surface = "acp",
            service = "persistence",
            action = "flush_batch",
            kind = "internal_error",
            events = count,
            error = %join_err,
            "acp event flush task panicked",
        ),
    }
    Some(retry_events)
}

// ── Database helpers ──────────────────────────────────────────────────────────

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let version: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version < 1 {
        conn.execute_batch(SCHEMA_SQL)?;
        conn.pragma_update(None, "user_version", 1)?;
    }
    if version < 2 {
        add_column_if_missing(conn, "acp_sessions", "model_id", "TEXT")?;
        add_column_if_missing(conn, "acp_sessions", "model_name", "TEXT")?;
        add_column_if_missing(
            conn,
            "acp_sessions",
            "config_options_json",
            "TEXT NOT NULL DEFAULT '[]'",
        )?;
        conn.pragma_update(None, "user_version", 2)?;
    }
    Ok(())
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for existing in columns {
        if existing? == column {
            return Ok(());
        }
    }
    conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {definition};"))
}

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS acp_sessions (
    id                 TEXT PRIMARY KEY,
    provider           TEXT NOT NULL,
    title              TEXT NOT NULL,
    cwd                TEXT NOT NULL,
    state              TEXT NOT NULL,
    created_at         TEXT NOT NULL,
    updated_at         TEXT NOT NULL,
    principal          TEXT NOT NULL DEFAULT '',
    agent_name         TEXT,
    agent_version      TEXT,
    provider_session_id TEXT,
    model_id           TEXT,
    model_name         TEXT,
    config_options_json TEXT NOT NULL DEFAULT '[]'
);

CREATE TABLE IF NOT EXISTS acp_session_events (
    id         TEXT PRIMARY KEY,
    session_id TEXT    NOT NULL REFERENCES acp_sessions(id),
    seq        INTEGER NOT NULL,
    kind       TEXT    NOT NULL,
    created_at TEXT    NOT NULL,
    payload    TEXT    NOT NULL,
    UNIQUE(session_id, seq)
);
CREATE INDEX IF NOT EXISTS idx_events_session_seq
    ON acp_session_events(session_id, seq);

CREATE TABLE IF NOT EXISTS acp_permission_requests (
    id             TEXT PRIMARY KEY,
    session_id     TEXT NOT NULL REFERENCES acp_sessions(id),
    action_summary TEXT NOT NULL,
    options        TEXT NOT NULL,
    outcome        TEXT,
    created_at     TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_perm_session
    ON acp_permission_requests(session_id);
";

// ── Session CRUD ──────────────────────────────────────────────────────────────

fn db_load_sessions(conn: &Connection) -> rusqlite::Result<Vec<AcpSessionSummary>> {
    let mut stmt = conn.prepare(
        "SELECT id, provider, title, cwd, state, created_at, updated_at,
                principal, agent_name, agent_version, provider_session_id,
                model_id, model_name, config_options_json
         FROM acp_sessions
         ORDER BY updated_at DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        let state_str: String = row.get("state")?;
        let state = parse_session_state(&state_str);
        let principal: String = row.get("principal")?;
        let config_options_json: String = row
            .get("config_options_json")
            .unwrap_or_else(|_| "[]".to_string());
        let config_options = serde_json::from_str(&config_options_json).unwrap_or_else(|error| {
            tracing::warn!(
                surface = "acp",
                service = "persistence",
                action = "session.config_options.decode",
                kind = "decode_error",
                error = %error,
                "failed to decode ACP session config options; returning empty config option list",
            );
            Vec::new()
        });
        Ok(AcpSessionSummary {
            id: row.get("id")?,
            provider: row.get("provider")?,
            title: row.get("title")?,
            cwd: row.get("cwd")?,
            state,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
            principal: if principal.is_empty() {
                None
            } else {
                Some(principal)
            },
            agent_name: row.get("agent_name")?,
            agent_version: row.get("agent_version")?,
            provider_session_id: row.get("provider_session_id")?,
            model_id: row.get("model_id")?,
            model_name: row.get("model_name")?,
            config_options,
        })
    })?;
    rows.collect()
}

fn db_save_session(conn: &Connection, s: &AcpSessionSummary) -> rusqlite::Result<()> {
    let config_options_json = serde_json::to_string(&s.config_options).map_err(|error| {
        rusqlite::Error::ToSqlConversionFailure(Box::new(error))
    })?;
    conn.execute(
        "INSERT INTO acp_sessions
             (id, provider, title, cwd, state, created_at, updated_at,
              principal, agent_name, agent_version, provider_session_id,
              model_id, model_name, config_options_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
         ON CONFLICT(id) DO UPDATE SET
             provider           = excluded.provider,
             title              = excluded.title,
             cwd                = excluded.cwd,
             state              = excluded.state,
             updated_at         = excluded.updated_at,
             principal          = excluded.principal,
             agent_name         = excluded.agent_name,
             agent_version      = excluded.agent_version,
             provider_session_id = excluded.provider_session_id,
             model_id           = excluded.model_id,
             model_name         = excluded.model_name,
             config_options_json = excluded.config_options_json",
        params![
            s.id,
            s.provider,
            s.title,
            s.cwd,
            session_state_str(&s.state),
            s.created_at,
            s.updated_at,
            s.principal.as_deref().unwrap_or(""),
            s.agent_name,
            s.agent_version,
            s.provider_session_id,
            s.model_id,
            s.model_name,
            config_options_json,
        ],
    )?;
    Ok(())
}

fn db_update_session_state(
    conn: &Connection,
    session_id: &str,
    state: &AcpSessionState,
) -> rusqlite::Result<()> {
    let now = jiff::Timestamp::now().to_string();
    conn.execute(
        "UPDATE acp_sessions SET state = ?1, updated_at = ?2 WHERE id = ?3",
        params![session_state_str(state), now, session_id],
    )?;
    Ok(())
}

// ── Event CRUD ────────────────────────────────────────────────────────────────

fn db_load_events(
    conn: &Connection,
    session_id: &str,
    since_seq: Option<u64>,
    limit: Option<u64>,
    hmac_key: &[u8],
) -> rusqlite::Result<Vec<AcpEvent>> {
    // When a limit is set we want the most recent `limit` events (preserving
    // the "last N" backfill contract), so we order DESC + LIMIT inside a
    // subquery and re-sort ASC outside. Without a limit we return everything
    // ordered ASC directly.
    let (sql, args): (&str, Vec<rusqlite::types::Value>) = match (since_seq, limit) {
        (None, None) => (
            "SELECT id, seq, kind, created_at, payload
             FROM acp_session_events
             WHERE session_id = ?1
             ORDER BY seq ASC",
            vec![session_id.to_owned().into()],
        ),
        (None, Some(n)) => (
            "SELECT id, seq, kind, created_at, payload FROM (
                 SELECT id, seq, kind, created_at, payload
                 FROM acp_session_events
                 WHERE session_id = ?1
                 ORDER BY seq DESC
                 LIMIT ?2
             ) ORDER BY seq ASC",
            vec![session_id.to_owned().into(), (n as i64).into()],
        ),
        (Some(since), None) => (
            "SELECT id, seq, kind, created_at, payload
             FROM acp_session_events
             WHERE session_id = ?1 AND seq > ?2
             ORDER BY seq ASC",
            vec![session_id.to_owned().into(), (since as i64).into()],
        ),
        (Some(since), Some(n)) => (
            "SELECT id, seq, kind, created_at, payload FROM (
                 SELECT id, seq, kind, created_at, payload
                 FROM acp_session_events
                 WHERE session_id = ?1 AND seq > ?2
                 ORDER BY seq DESC
                 LIMIT ?3
             ) ORDER BY seq ASC",
            vec![
                session_id.to_owned().into(),
                (since as i64).into(),
                (n as i64).into(),
            ],
        ),
    };

    let mut stmt = conn.prepare(sql)?;
    let session_id_owned = session_id.to_string();
    let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), |row| {
        Ok((
            row.get::<_, String>("id")?,
            row.get::<_, i64>("seq")? as u64,
            row.get::<_, String>("kind")?,
            row.get::<_, String>("created_at")?,
            row.get::<_, String>("payload")?,
        ))
    })?;

    let mut events = Vec::new();
    for row in rows {
        let (id, seq, kind, created_at, payload) = row?;
        let value: Value = match serde_json::from_str(&payload) {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(
                    surface = "acp",
                    service = "persistence",
                    action = "load_events",
                    kind = "decode_error",
                    error = %error,
                    "failed to parse persisted acp event payload",
                );
                events.push(corrupt_persisted_event(
                    &id,
                    &session_id_owned,
                    seq,
                    &created_at,
                    "invalid_json_payload",
                    &kind,
                    Value::String(payload),
                    error.to_string(),
                ));
                continue;
            }
        };
        if let Err(error) = verify_permission_outcome_payload(&value, hmac_key) {
            tracing::warn!(
                surface = "acp",
                service = "persistence",
                action = "load_events",
                kind = "validation_failed",
                error,
                "persisted permission outcome failed hmac verification",
            );
            events.push(corrupt_persisted_event(
                &id,
                &session_id_owned,
                seq,
                &created_at,
                "permission_outcome_validation_failed",
                &kind,
                value,
                error,
            ));
            continue;
        }
        match serde_json::from_value::<AcpEvent>(value.clone()) {
            Ok(event) => events.push(event),
            Err(error) => {
                tracing::warn!(
                    surface = "acp",
                    service = "persistence",
                    action = "load_events",
                    kind = "decode_error",
                    error = %error,
                    "failed to deserialize persisted acp event",
                );
                events.push(corrupt_persisted_event(
                    &id,
                    &session_id_owned,
                    seq,
                    &created_at,
                    "typed_event_deserialize_failed",
                    &kind,
                    value,
                    error.to_string(),
                ));
            }
        }
    }
    Ok(events)
}

fn corrupt_persisted_event(
    id: &str,
    session_id: &str,
    seq: u64,
    created_at: &str,
    error_kind: &str,
    stored_kind: &str,
    payload: Value,
    error: String,
) -> AcpEvent {
    AcpEvent::ProviderInfo {
        id: id.to_string(),
        created_at: created_at.to_string(),
        session_id: session_id.to_string(),
        seq,
        provider: "persistence".to_string(),
        raw: json!({
            "type": "persisted_event_error",
            "error_kind": error_kind,
            "stored_kind": stored_kind,
            "error": error,
            "payload": payload,
        }),
    }
}

fn db_batch_insert_events(conn: &Connection, events: &[(AcpEvent, String)]) -> Result<(), String> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("begin transaction: {e}"))?;

    for (event, payload) in events {
        let id = event_id(event);
        let session_id = event.session_id();
        let seq = event.seq() as i64;
        let kind = event_kind_str(event);
        let created_at = event_created_at(event);

        tx.execute(
            "INSERT OR IGNORE INTO acp_session_events
                 (id, session_id, seq, kind, created_at, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, session_id, seq, kind, created_at, payload],
        )
        .map_err(|e| format!("insert event: {e}"))?;
    }

    tx.commit().map_err(|e| format!("commit: {e}"))?;
    Ok(())
}

// ── Event field accessors ─────────────────────────────────────────────────────

fn event_id(event: &AcpEvent) -> &str {
    match event {
        AcpEvent::MessageChunk { id, .. }
        | AcpEvent::ReasoningChunk { id, .. }
        | AcpEvent::ToolCallStart { id, .. }
        | AcpEvent::ToolCallUpdate { id, .. }
        | AcpEvent::PermissionRequest { id, .. }
        | AcpEvent::PermissionOutcome { id, .. }
        | AcpEvent::UsageUpdate { id, .. }
        | AcpEvent::ContentBlocks { id, .. }
        | AcpEvent::SessionUpdate { id, .. }
        | AcpEvent::ProviderInfo { id, .. }
        | AcpEvent::Unknown { id, .. } => id,
    }
}

fn event_kind_str(event: &AcpEvent) -> &'static str {
    match event {
        AcpEvent::MessageChunk { .. } => "message_chunk",
        AcpEvent::ReasoningChunk { .. } => "reasoning_chunk",
        AcpEvent::ToolCallStart { .. } => "tool_call_start",
        AcpEvent::ToolCallUpdate { .. } => "tool_call_update",
        AcpEvent::PermissionRequest { .. } => "permission_request",
        AcpEvent::PermissionOutcome { .. } => "permission_outcome",
        AcpEvent::UsageUpdate { .. } => "usage_update",
        AcpEvent::ContentBlocks { .. } => "content_blocks",
        AcpEvent::SessionUpdate { .. } => "session_update",
        AcpEvent::ProviderInfo { .. } => "provider_info",
        AcpEvent::Unknown { .. } => "unknown",
    }
}

fn event_created_at(event: &AcpEvent) -> &str {
    match event {
        AcpEvent::MessageChunk { created_at, .. }
        | AcpEvent::ReasoningChunk { created_at, .. }
        | AcpEvent::ToolCallStart { created_at, .. }
        | AcpEvent::ToolCallUpdate { created_at, .. }
        | AcpEvent::PermissionRequest { created_at, .. }
        | AcpEvent::PermissionOutcome { created_at, .. }
        | AcpEvent::UsageUpdate { created_at, .. }
        | AcpEvent::ContentBlocks { created_at, .. }
        | AcpEvent::SessionUpdate { created_at, .. }
        | AcpEvent::ProviderInfo { created_at, .. }
        | AcpEvent::Unknown { created_at, .. } => created_at,
    }
}

// ── Session state helpers ─────────────────────────────────────────────────────

fn session_state_str(state: &AcpSessionState) -> &'static str {
    match state {
        AcpSessionState::Creating => "creating",
        AcpSessionState::Idle => "idle",
        AcpSessionState::Running => "running",
        AcpSessionState::WaitingForPermission => "waiting_for_permission",
        AcpSessionState::Completed => "completed",
        AcpSessionState::Cancelled => "cancelled",
        AcpSessionState::Failed => "failed",
        AcpSessionState::Closed => "closed",
    }
}

fn parse_session_state(s: &str) -> AcpSessionState {
    match s {
        "creating" => AcpSessionState::Creating,
        "idle" => AcpSessionState::Idle,
        "running" => AcpSessionState::Running,
        "waiting_for_permission" => AcpSessionState::WaitingForPermission,
        "completed" => AcpSessionState::Completed,
        "cancelled" => AcpSessionState::Cancelled,
        "failed" => AcpSessionState::Failed,
        "closed" => AcpSessionState::Closed,
        _ => AcpSessionState::Failed,
    }
}

// ── HMAC permission outcome signing ──────────────────────────────────────────

fn permission_outcome_message(value: &Value) -> Option<String> {
    let id = value.get("id")?.as_str()?;
    let request_id = value.get("request_id")?.as_str()?;
    let granted = value.get("granted")?.as_bool()?;
    let seq = value.get("seq")?.as_u64()?;
    Some(format!("{id}:{request_id}:{granted}:{seq}"))
}

fn verify_permission_outcome_payload(value: &Value, key: &[u8]) -> Result<(), String> {
    if value.get("kind").and_then(Value::as_str) != Some("permission_outcome") {
        return Ok(());
    }

    let expected = value
        .get("hmac")
        .and_then(Value::as_str)
        .ok_or_else(|| "permission outcome payload missing hmac".to_string())?;
    let message = permission_outcome_message(value)
        .ok_or_else(|| "permission outcome payload missing required fields".to_string())?;
    let actual = hmac_tag(key, &message);
    if actual == expected {
        Ok(())
    } else {
        Err("permission outcome hmac mismatch".to_string())
    }
}

fn hmac_tag(key: &[u8], message: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(message.as_bytes());
    let result = mac.finalize();
    hex::encode(result.into_bytes())
}

/// Load the HMAC secret from `LAB_ACP_HMAC_SECRET` or generate an ephemeral
/// key from process ID + startup timestamp hashed through SHA-256.
///
/// The generated key is NOT cryptographically random — it is only suitable
/// for protecting against naive DB-write bypass within a single process
/// lifetime. For cross-restart signature verification, set
/// `LAB_ACP_HMAC_SECRET` in `~/.lab/.env`.
fn load_or_generate_hmac_key() -> Vec<u8> {
    if let Ok(secret) = std::env::var("LAB_ACP_HMAC_SECRET") {
        if !secret.is_empty() {
            return secret.into_bytes();
        }
    }
    tracing::warn!(
        surface = "acp",
        service = "persistence",
        action = "hmac_key_init",
        kind = "ephemeral_key",
        "LAB_ACP_HMAC_SECRET is not set; using an ephemeral non-random HMAC key. \
         Set LAB_ACP_HMAC_SECRET in ~/.lab/.env for persistent, \
         cryptographically-random protection."
    );
    let pid = std::process::id();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    generate_ephemeral_hmac_key(pid, now)
}

fn generate_ephemeral_hmac_key(pid: u32, now_nanos: u128) -> Vec<u8> {
    // Derive a 32-byte key from process ID + timestamp via SHA-256.
    // NOT cryptographically random — unique per process instance only.
    use sha2::Digest;
    let input = format!("lab-acp-hmac-ephemeral:{pid}:{now_nanos}");
    Sha256::digest(input.as_bytes()).to_vec()
}

#[cfg(test)]
mod db_load_events_tests {
    use super::*;

    fn fresh_in_memory_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory");
        migrate(&conn).expect("migrate");
        conn.execute(
            "INSERT INTO acp_sessions
             (id, provider, title, cwd, state, created_at, updated_at, principal)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                "sess-cap",
                "codex",
                "test",
                "/tmp",
                "active",
                "2026-04-30T00:00:00Z",
                "2026-04-30T00:00:00Z",
                "test-principal",
            ],
        )
        .expect("insert session");
        conn
    }

    fn insert_message_chunks(conn: &Connection, session_id: &str, count: u64) {
        for n in 1..=count {
            let event = AcpEvent::MessageChunk {
                id: format!("evt-{n}"),
                session_id: session_id.to_string(),
                seq: n,
                created_at: "2026-04-30T00:00:00Z".to_string(),
                role: "assistant".to_string(),
                text: format!("chunk-{n}"),
                message_id: "msg-1".to_string(),
            };
            let payload = serde_json::to_string(&event).expect("serialize");
            conn.execute(
                "INSERT INTO acp_session_events
                 (id, session_id, seq, kind, created_at, payload)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    format!("evt-{n}"),
                    session_id,
                    n as i64,
                    "message_chunk",
                    "2026-04-30T00:00:00Z",
                    payload,
                ],
            )
            .expect("insert event");
        }
    }

    fn event_seq(event: &AcpEvent) -> u64 {
        match event {
            AcpEvent::MessageChunk { seq, .. } => *seq,
            _ => panic!("expected MessageChunk in test fixture"),
        }
    }

    #[test]
    fn capped_load_returns_last_n_events_in_ascending_order() {
        let conn = fresh_in_memory_conn();
        insert_message_chunks(&conn, "sess-cap", 25);

        let events =
            db_load_events(&conn, "sess-cap", Some(0), Some(10), &[0u8; 32]).expect("load events");

        assert_eq!(events.len(), 10, "cap of 10 must be applied at SQL layer");
        let seqs: Vec<u64> = events.iter().map(event_seq).collect();
        assert_eq!(
            seqs,
            (16..=25).collect::<Vec<u64>>(),
            "must return the last 10 events ordered ascending — the existing SSE backfill contract",
        );
    }

    #[test]
    fn capped_load_respects_since_seq_cursor() {
        let conn = fresh_in_memory_conn();
        insert_message_chunks(&conn, "sess-cap", 25);

        // since_seq = 5 narrows to seqs 6..=25 (20 events); cap of 5 keeps the
        // last 5 of those (seqs 21..=25).
        let events =
            db_load_events(&conn, "sess-cap", Some(5), Some(5), &[0u8; 32]).expect("load events");

        let seqs: Vec<u64> = events.iter().map(event_seq).collect();
        assert_eq!(seqs, vec![21, 22, 23, 24, 25]);
    }

    #[test]
    fn capped_load_returns_all_when_under_cap() {
        let conn = fresh_in_memory_conn();
        insert_message_chunks(&conn, "sess-cap", 7);

        let events =
            db_load_events(&conn, "sess-cap", Some(0), Some(100), &[0u8; 32]).expect("load events");

        assert_eq!(events.len(), 7);
        let seqs: Vec<u64> = events.iter().map(event_seq).collect();
        assert_eq!(seqs, (1..=7).collect::<Vec<u64>>());
    }

    #[test]
    fn uncapped_load_still_works_with_no_limit() {
        let conn = fresh_in_memory_conn();
        insert_message_chunks(&conn, "sess-cap", 12);

        let events =
            db_load_events(&conn, "sess-cap", Some(0), None, &[0u8; 32]).expect("load events");

        assert_eq!(events.len(), 12, "passing None for limit must not cap");
    }
}

#[cfg(test)]
mod hmac_tests {
    use super::*;

    #[test]
    fn generated_fallback_hmac_key_is_ephemeral_seeded_and_32_bytes() {
        let first = generate_ephemeral_hmac_key(42, 100);
        let same_seed = generate_ephemeral_hmac_key(42, 100);
        let later_start = generate_ephemeral_hmac_key(42, 101);

        assert_eq!(first.len(), 32);
        assert_eq!(first, same_seed);
        assert_ne!(
            first, later_start,
            "fallback HMAC key intentionally rotates when the process-start seed changes"
        );
    }

    #[test]
    fn permission_outcome_hmac_rejects_wrong_key_after_restart() {
        let event = AcpEvent::PermissionOutcome {
            id: "evt-1".to_string(),
            session_id: "sess-1".to_string(),
            seq: 7,
            created_at: "2026-04-30T00:00:00Z".to_string(),
            request_id: "perm-1".to_string(),
            granted: true,
        };

        let first_key = generate_ephemeral_hmac_key(42, 100);
        let restarted_key = generate_ephemeral_hmac_key(42, 101);
        let payload = redact_event_payload(&event, &first_key).expect("serialize payload");
        let value: Value = serde_json::from_str(&payload).expect("payload json");

        verify_permission_outcome_payload(&value, &first_key).expect("first key verifies");
        assert_eq!(
            verify_permission_outcome_payload(&value, &restarted_key),
            Err("permission outcome hmac mismatch".to_string()),
            "ephemeral fallback signatures do not verify after fallback key rotation"
        );
    }
}

// ── Payload redaction ─────────────────────────────────────────────────────────

/// Serialize an event and redact sensitive fields in the JSON tree.
/// Returns the final payload string used for the DB `payload` column,
/// without a `from_value` round-trip back to the typed event.
fn redact_event_payload(event: &AcpEvent, hmac_key: &[u8]) -> serde_json::Result<String> {
    let mut value = serde_json::to_value(event)?;
    if value.get("kind").and_then(Value::as_str) == Some("permission_outcome")
        && let Some(message) = permission_outcome_message(&value)
        && let Value::Object(map) = &mut value
    {
        map.insert(
            "hmac".to_string(),
            Value::String(hmac_tag(hmac_key, &message)),
        );
    }
    redact_value(&mut value);
    serde_json::to_string(&value)
}

fn redact_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, val) in map.iter_mut() {
                if crate::dispatch::redact::is_sensitive_key(key) {
                    *val = Value::String("[REDACTED]".to_string());
                } else {
                    redact_value(val);
                }
            }
        }
        Value::Array(arr) => {
            for item in arr.iter_mut() {
                redact_value(item);
            }
        }
        _ => {}
    }
}

// ── Path resolution ───────────────────────────────────────────────────────────

fn resolve_db_path() -> Result<PathBuf, AcpError> {
    if let Ok(path) = std::env::var("LAB_ACP_DB") {
        crate::dispatch::helpers::reject_path_traversal(&path)
            .map_err(|_| AcpError::Internal(format!("LAB_ACP_DB must not contain `..`: {path}")))?;
        return Ok(PathBuf::from(path));
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    Ok(PathBuf::from(home).join(".lab").join("acp.db"))
}

// ── Unix file creation with 0600 perms ────────────────────────────────────────

#[cfg(unix)]
fn create_db_file_0600(path: &PathBuf) {
    use std::os::unix::fs::OpenOptionsExt;
    // Only set mode on creation; if the file already exists, leave perms alone.
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true) // fails if file exists — that's fine
        .mode(0o600)
        .open(path)
        .ok();
}
