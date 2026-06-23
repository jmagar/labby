//! SQLite-backed persistence for captured log events.
//!
//! Single writer (the async writer task in `ingest`), many readers. WAL mode
//! + split `write_conn`/`read_conn` mutexes inside `spawn_blocking` keeps the
//! API async without dragging in `sqlx`. WAL allows concurrent readers, so
//! separating the two mutexes lets reads proceed independently of writes.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OpenFlags, params, params_from_iter};

use super::types::{
    LogEvent, LogLevel, LogQuery, LogRetention, LogSearchResult, LogStoreStats, LogTailRequest,
    LogTailResult, Subsystem, Surface,
};
use crate::dispatch::error::ToolError;

/// Column list shared by search + tail. Kept in sync with
/// `row_to_event`'s `row.get(...)` names.
const SELECT_COLS: &str = "event_id, ts, level, subsystem, surface, action, message,
     request_id, session_id, correlation_id, trace_id, span_id,
     instance, auth_flow, outcome_kind, fields_json,
     source_kind, source_node_id, source_device_id, actor_key, ingest_path, upstream_event_id";

pub struct LogStore {
    /// Exclusive connection for writes (INSERT, DELETE, VACUUM).
    write_conn: Arc<Mutex<Connection>>,
    /// Separate connection for reads (SELECT). WAL mode allows this to proceed
    /// concurrently with writes without contending on `write_conn`.
    read_conn: Arc<Mutex<Connection>>,
    retention: LogRetention,
}

impl LogStore {
    pub async fn open(path: PathBuf, retention: LogRetention) -> Result<Self, ToolError> {
        let path_display = path.display().to_string();
        tracing::info!(
            target: "labby::dispatch::logs",
            surface = "logs",
            service = "store",
            action = "sqlite.open.start",
            path = %path_display,
            max_age_days = retention.max_age_days,
            max_bytes = retention.max_bytes,
            "opening log store SQLite database",
        );
        let (write_conn, read_conn) = tokio::task::spawn_blocking(
            move || -> Result<(Connection, Connection), rusqlite::Error> {
                if let Some(parent) = path.parent() {
                    if std::fs::create_dir_all(parent).is_ok() {
                        tracing::debug!(
                            target: "labby::dispatch::logs",
                            surface = "logs",
                            service = "store",
                            action = "sqlite.open.parent_ready",
                            parent = %parent.display(),
                            "log store parent directory ready",
                        );
                    }
                }
                let rw_flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;

                // Write connection — owns schema init and WAL configuration.
                let wc = Connection::open_with_flags(&path, rw_flags)?;
                wc.busy_timeout(std::time::Duration::from_millis(5_000))?;
                wc.pragma_update(None, "journal_mode", "WAL")?;
                wc.pragma_update(None, "synchronous", "NORMAL")?;
                wc.pragma_update(None, "temp_store", "MEMORY")?;
                wc.pragma_update(None, "mmap_size", 134_217_728_i64)?;
                tracing::debug!(
                    target: "labby::dispatch::logs",
                    surface = "logs",
                    service = "store",
                    action = "sqlite.pragmas",
                    connection = "write",
                    journal_mode = "WAL",
                    synchronous = "NORMAL",
                    temp_store = "MEMORY",
                    mmap_size = 134_217_728_i64,
                    "log store SQLite write pragmas applied",
                );
                migrate(&wc)?;

                // Read connection — opened after schema is applied.
                let rc = Connection::open_with_flags(&path, rw_flags)?;
                rc.busy_timeout(std::time::Duration::from_millis(5_000))?;
                rc.pragma_update(None, "journal_mode", "WAL")?;
                rc.pragma_update(None, "temp_store", "MEMORY")?;
                rc.pragma_update(None, "mmap_size", 134_217_728_i64)?;
                rc.pragma_update(None, "query_only", "true")?;
                tracing::debug!(
                    target: "labby::dispatch::logs",
                    surface = "logs",
                    service = "store",
                    action = "sqlite.pragmas",
                    connection = "read",
                    journal_mode = "WAL",
                    temp_store = "MEMORY",
                    mmap_size = 134_217_728_i64,
                    query_only = true,
                    "log store SQLite read pragmas applied",
                );

                Ok((wc, rc))
            },
        )
        .await
        .map_err(|e| ToolError::internal_message(format!("log store open join: {e}")))?
        .map_err(|e| ToolError::internal_message(format!("log store open: {e}")))?;

        let store = Self {
            write_conn: Arc::new(Mutex::new(write_conn)),
            read_conn: Arc::new(Mutex::new(read_conn)),
            retention,
        };
        tracing::info!(
            target: "labby::dispatch::logs",
            surface = "logs", service = "store", action = "open",
            path = %path_display,
            "log store SQLite opened",
        );
        Ok(store)
    }

    pub async fn insert(&self, event: &LogEvent) -> Result<(), ToolError> {
        let event = event.clone();
        self.blocking_write("insert", move |c| insert_event(c, &event))
            .await
    }

    pub async fn search(&self, query: LogQuery) -> Result<LogSearchResult, ToolError> {
        self.blocking_read("search", move |c| run_search(c, &query))
            .await
    }

    pub async fn completion_events(
        &self,
        after_ts: Option<i64>,
        before_ts: Option<i64>,
    ) -> Result<Vec<LogEvent>, ToolError> {
        self.blocking_read("completion_events", move |c| {
            run_completion_events(c, after_ts, before_ts)
        })
        .await
    }

    pub async fn previous_completion_actor_ids(
        &self,
        before_ts: i64,
    ) -> Result<std::collections::BTreeSet<String>, ToolError> {
        self.blocking_read("previous_completion_actor_ids", move |c| {
            run_previous_completion_actor_ids(c, before_ts)
        })
        .await
    }

    pub async fn tail(&self, req: LogTailRequest) -> Result<LogTailResult, ToolError> {
        self.blocking_read("tail", move |c| run_tail(c, &req)).await
    }

    pub async fn stats(&self) -> Result<LogStoreStats, ToolError> {
        let retention = self.retention;
        self.blocking_read("stats", move |c| run_stats(c, retention))
            .await
    }

    #[doc(hidden)]
    #[allow(dead_code)]
    pub async fn run_maintenance(&self) -> Result<(), ToolError> {
        let retention = self.retention;
        self.blocking_write("maintenance", move |c| run_maintenance(c, retention))
            .await
    }

    /// Run a write closure on the blocking pool using the dedicated write connection.
    async fn blocking_write<T, F>(&self, label: &'static str, f: F) -> Result<T, ToolError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, rusqlite::Error> + Send + 'static,
    {
        let conn = Arc::clone(&self.write_conn);
        tokio::task::spawn_blocking(move || {
            let c = conn
                .lock()
                .map_err(|_| ToolError::internal_message("log store write mutex poisoned"))?;
            f(&c).map_err(|e| ToolError::internal_message(format!("log store {label}: {e}")))
        })
        .await
        .map_err(|e| ToolError::internal_message(format!("log store {label} join: {e}")))?
    }

    /// Run a read closure on the blocking pool using the dedicated read connection.
    /// The read connection is opened with `query_only=true` and does not contend
    /// with `write_conn`, allowing WAL-mode concurrency.
    async fn blocking_read<T, F>(&self, label: &'static str, f: F) -> Result<T, ToolError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, rusqlite::Error> + Send + 'static,
    {
        let conn = Arc::clone(&self.read_conn);
        tokio::task::spawn_blocking(move || {
            let c = conn
                .lock()
                .map_err(|_| ToolError::internal_message("log store read mutex poisoned"))?;
            f(&c).map_err(|e| ToolError::internal_message(format!("log store {label}: {e}")))
        })
        .await
        .map_err(|e| ToolError::internal_message(format!("log store {label} join: {e}")))?
    }
}

#[doc(hidden)]
#[allow(dead_code)]
pub async fn open_store_for_test(retention: LogRetention) -> Result<LogStore, ToolError> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path =
        std::env::temp_dir().join(format!("lab-logs-test-{}-{unique}.db", std::process::id()));
    LogStore::open(path, retention).await
}

// ── Schema migration ──────────────────────────────────────────────────────────

/// Apply pending schema migrations using PRAGMA user_version as the version
/// counter. Each `if version < N` block is a single, idempotent migration step.
///
/// Rules:
/// - Only bump `user_version` **after** the DDL succeeds.
/// - Keep version numbers consecutive and never reuse them.
/// - Historical rows remain nullable unless a migration explicitly backfills.
fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let mut version: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    tracing::debug!(
        target: "labby::dispatch::logs",
        surface = "logs",
        service = "store",
        action = "sqlite.migrate.start",
        from_version = version,
        "checking log store SQLite migrations",
    );
    if version < 1 {
        conn.execute_batch(include_str!("store_schema.sql"))?;
        conn.pragma_update(None, "user_version", 1)?;
        tracing::info!(
            target: "labby::dispatch::logs",
            surface = "logs",
            service = "store",
            action = "sqlite.migrate.apply",
            from_version = version,
            to_version = 1,
            "applied log store SQLite migration",
        );
        version = 1;
    }
    if version < 2 {
        migrate_actor_key(conn)?;
        conn.pragma_update(None, "user_version", 2)?;
        tracing::info!(
            target: "labby::dispatch::logs",
            surface = "logs",
            service = "store",
            action = "sqlite.migrate.apply",
            from_version = version,
            to_version = 2,
            "applied log store SQLite migration",
        );
        version = 2;
    }
    if version < 3 {
        migrate_completion_kind(conn)?;
        conn.pragma_update(None, "user_version", 3)?;
        tracing::info!(
            target: "labby::dispatch::logs",
            surface = "logs",
            service = "store",
            action = "sqlite.migrate.apply",
            from_version = version,
            to_version = 3,
            "applied log store SQLite migration",
        );
    } else {
        tracing::debug!(
            target: "labby::dispatch::logs",
            surface = "logs",
            service = "store",
            action = "sqlite.migrate.skip",
            version,
            "log store SQLite schema already current",
        );
    }
    Ok(())
}

fn migrate_actor_key(conn: &Connection) -> rusqlite::Result<()> {
    if !column_exists(conn, "log_events", "actor_key")? {
        // Historical rows intentionally keep actor_key NULL. Actor identity is
        // only populated for rows inserted after upstream plumbing supplies it.
        conn.execute_batch("ALTER TABLE log_events ADD COLUMN actor_key TEXT;")?;
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_log_events_actor_key_ts
            ON log_events(actor_key, ts DESC)
            WHERE actor_key IS NOT NULL;",
    )?;
    Ok(())
}

fn migrate_completion_kind(conn: &Connection) -> rusqlite::Result<()> {
    if !column_exists(conn, "log_events", "completion_kind")? {
        conn.execute_batch(
            "ALTER TABLE log_events ADD COLUMN completion_kind INTEGER NOT NULL DEFAULT 0;",
        )?;
    }
    conn.execute(
        "UPDATE log_events
         SET completion_kind = 1
         WHERE completion_kind = 0
           AND fields_json LIKE '%\"input_tokens\"%'
           AND fields_json LIKE '%\"output_tokens\"%'",
        [],
    )?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_log_events_completion_ts
            ON log_events(completion_kind, ts DESC, event_id DESC)
            WHERE completion_kind = 1;
         CREATE INDEX IF NOT EXISTS idx_log_events_completion_actor_ts
            ON log_events(actor_key, ts DESC)
            WHERE completion_kind = 1 AND actor_key IS NOT NULL;",
    )?;
    Ok(())
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for name in columns {
        if name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

// ── Insert ────────────────────────────────────────────────────────────────────

fn insert_event(conn: &Connection, event: &LogEvent) -> Result<(), rusqlite::Error> {
    let completion_kind = i64::from(is_completion_event(&event.fields_json));
    conn.execute(
        "INSERT OR IGNORE INTO log_events (
            event_id, ts, level, subsystem, surface, action, message,
            request_id, session_id, correlation_id, trace_id, span_id,
            instance, auth_flow, outcome_kind, fields_json,
            completion_kind, source_kind, source_node_id, source_device_id,
            actor_key, ingest_path, upstream_event_id
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
        params![
            event.event_id,
            event.ts,
            event.level.as_str(),
            event.subsystem.as_str(),
            event.surface.as_str(),
            event.action,
            event.message,
            event.request_id,
            event.session_id,
            event.correlation_id,
            event.trace_id,
            event.span_id,
            event.instance,
            event.auth_flow,
            event.outcome_kind,
            event.fields_json.to_string(),
            completion_kind,
            event.source_kind,
            event.source_node_id,
            event.source_device_id,
            event.actor_key,
            event.ingest_path,
            event.upstream_event_id,
        ],
    )?;
    Ok(())
}

fn is_completion_event(fields_json: &serde_json::Value) -> bool {
    fields_json.get("input_tokens").is_some() && fields_json.get("output_tokens").is_some()
}

// ── Search ────────────────────────────────────────────────────────────────────

fn run_search(conn: &Connection, q: &LogQuery) -> Result<LogSearchResult, rusqlite::Error> {
    let mut sql = format!("SELECT {SELECT_COLS} FROM log_events WHERE 1=1");
    let mut args: Vec<rusqlite::types::Value> = Vec::new();

    if let Some(after) = q.after_ts {
        sql.push_str(" AND ts > ?");
        args.push(after.into());
    }
    if let Some(before) = q.before_ts {
        sql.push_str(" AND ts < ?");
        args.push(before.into());
    }
    append_in_clause(
        &mut sql,
        &mut args,
        "level",
        q.levels.iter().map(|l| l.as_str().to_string()),
    );
    append_in_clause(
        &mut sql,
        &mut args,
        "subsystem",
        q.subsystems.iter().map(|s| s.as_str().to_string()),
    );
    append_in_clause(
        &mut sql,
        &mut args,
        "surface",
        q.surfaces.iter().map(|s| s.as_str().to_string()),
    );
    if let Some(action) = &q.action {
        sql.push_str(" AND action = ?");
        args.push(action.clone().into());
    }
    if let Some(request_id) = &q.request_id {
        sql.push_str(" AND request_id = ?");
        args.push(request_id.clone().into());
    }
    if let Some(session_id) = &q.session_id {
        sql.push_str(" AND session_id = ?");
        args.push(session_id.clone().into());
    }
    if let Some(corr) = &q.correlation_id {
        sql.push_str(" AND correlation_id = ?");
        args.push(corr.clone().into());
    }
    append_in_clause(
        &mut sql,
        &mut args,
        "source_node_id",
        q.source_node_ids.iter().cloned(),
    );
    append_in_clause(
        &mut sql,
        &mut args,
        "source_kind",
        q.source_kinds.iter().cloned(),
    );
    if let Some(actor_key) = &q.actor_key {
        sql.push_str(" AND actor_key = ?");
        args.push(actor_key.clone().into());
    }
    if let Some(text) = &q.text {
        sql.push_str(
            " AND (message LIKE ? ESCAPE '\\' OR IFNULL(request_id,'') LIKE ? ESCAPE '\\' OR IFNULL(session_id,'') LIKE ? ESCAPE '\\' OR IFNULL(correlation_id,'') LIKE ? ESCAPE '\\')",
        );
        let like = format!("%{}%", escape_like(text));
        args.push(like.clone().into());
        args.push(like.clone().into());
        args.push(like.clone().into());
        args.push(like.into());
    }
    sql.push_str(" ORDER BY ts DESC, event_id DESC");
    let limit = q.limit.unwrap_or(500).min(10_000);
    sql.push_str(&format!(" LIMIT {limit}"));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(args.iter()), row_to_event)?;
    let events: Vec<LogEvent> = rows.collect::<Result<_, _>>()?;
    let next_cursor = events.last().map(|e| e.event_id.clone());
    Ok(LogSearchResult {
        events,
        next_cursor,
    })
}

fn run_completion_events(
    conn: &Connection,
    after_ts: Option<i64>,
    before_ts: Option<i64>,
) -> Result<Vec<LogEvent>, rusqlite::Error> {
    let mut sql = format!("SELECT {SELECT_COLS} FROM log_events WHERE completion_kind = 1");
    let mut args: Vec<rusqlite::types::Value> = Vec::new();
    if let Some(after) = after_ts {
        sql.push_str(" AND ts > ?");
        args.push(after.into());
    }
    if let Some(before) = before_ts {
        sql.push_str(" AND ts <= ?");
        args.push(before.into());
    }
    sql.push_str(" ORDER BY ts DESC, event_id DESC");

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(args.iter()), row_to_event)?;
    rows.collect::<Result<_, _>>()
}

fn run_previous_completion_actor_ids(
    conn: &Connection,
    before_ts: i64,
) -> Result<std::collections::BTreeSet<String>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT actor_key
         FROM log_events
         WHERE completion_kind = 1
           AND actor_key IS NOT NULL
           AND ts <= ?1",
    )?;
    let rows = stmt.query_map(params![before_ts], |row| row.get::<_, String>(0))?;
    rows.collect::<Result<_, _>>()
}

/// Escape `%`, `_`, and `\` in a user-supplied string so they are treated as
/// literals by a SQLite LIKE expression that uses `ESCAPE '\'`.
fn escape_like(s: &str) -> String {
    s.replace('\\', r"\\")
        .replace('%', r"\%")
        .replace('_', r"\_")
}

fn append_in_clause<I>(
    sql: &mut String,
    args: &mut Vec<rusqlite::types::Value>,
    column: &str,
    values: I,
) where
    I: IntoIterator<Item = String>,
{
    let values: Vec<String> = values.into_iter().collect();
    if values.is_empty() {
        return;
    }
    sql.push_str(&format!(" AND {column} IN ("));
    for (i, v) in values.into_iter().enumerate() {
        if i > 0 {
            sql.push(',');
        }
        sql.push('?');
        args.push(v.into());
    }
    sql.push(')');
}

fn row_to_event(row: &rusqlite::Row<'_>) -> Result<LogEvent, rusqlite::Error> {
    let level: String = row.get("level")?;
    let subsystem: String = row.get("subsystem")?;
    let surface: String = row.get("surface")?;
    let fields_json: String = row.get("fields_json")?;
    Ok(LogEvent {
        event_id: row.get("event_id")?,
        ts: row.get("ts")?,
        level: LogLevel::parse(&level).unwrap_or(LogLevel::Info),
        subsystem: Subsystem::parse(&subsystem).unwrap_or(Subsystem::CoreRuntime),
        surface: Surface::parse(&surface).unwrap_or(Surface::CoreRuntime),
        action: row.get("action")?,
        message: row.get("message")?,
        request_id: row.get("request_id")?,
        session_id: row.get("session_id")?,
        correlation_id: row.get("correlation_id")?,
        trace_id: row.get("trace_id")?,
        span_id: row.get("span_id")?,
        instance: row.get("instance")?,
        auth_flow: row.get("auth_flow")?,
        outcome_kind: row.get("outcome_kind")?,
        fields_json: serde_json::from_str(&fields_json).unwrap_or(serde_json::Value::Null),
        source_kind: row.get("source_kind")?,
        source_node_id: row.get("source_node_id")?,
        source_device_id: row.get("source_device_id")?,
        actor_key: row.get("actor_key")?,
        ingest_path: row.get("ingest_path")?,
        upstream_event_id: row.get("upstream_event_id")?,
    })
}

// ── Tail ──────────────────────────────────────────────────────────────────────

fn run_tail(conn: &Connection, req: &LogTailRequest) -> Result<LogTailResult, rusqlite::Error> {
    let mut cursor_ts: Option<i64> = req.after_ts;
    let mut cursor_event_id: Option<String> = None;
    if let Some(ev_id) = &req.since_event_id {
        let ts: Option<i64> = conn
            .query_row(
                "SELECT ts FROM log_events WHERE event_id = ?1",
                params![ev_id],
                |r| r.get(0),
            )
            .ok();
        if let Some(t) = ts {
            cursor_ts = Some(t);
            cursor_event_id = Some(ev_id.clone());
        }
    }
    let limit = req.limit.unwrap_or(500).min(10_000) as i64;

    // NOTE: the `ts > ?1 OR (ts = ?1 AND event_id > ?2)` tiebreaker is what
    // makes the `since_event_id` cursor ordering stable. Do not collapse.
    let (where_clause, args): (&str, Vec<rusqlite::types::Value>) =
        match (cursor_ts, cursor_event_id) {
            (Some(ts), Some(ev_id)) => (
                "WHERE ts > ?1 OR (ts = ?1 AND event_id > ?2)",
                vec![ts.into(), ev_id.into(), limit.into()],
            ),
            (Some(ts), None) => ("WHERE ts > ?1", vec![ts.into(), limit.into()]),
            (None, _) => ("", vec![limit.into()]),
        };

    let limit_placeholder = args.len();
    let sql = format!(
        "SELECT {SELECT_COLS} FROM log_events {where_clause} \
         ORDER BY ts ASC, event_id ASC LIMIT ?{limit_placeholder}"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(args.iter()), row_to_event)?;
    let events: Vec<LogEvent> = rows.collect::<Result<_, _>>()?;
    let next_cursor = events.last().map(|e| e.event_id.clone());
    Ok(LogTailResult {
        events,
        next_cursor,
    })
}

// ── Stats ─────────────────────────────────────────────────────────────────────

fn run_stats(conn: &Connection, retention: LogRetention) -> Result<LogStoreStats, rusqlite::Error> {
    let (total_event_count_i64, oldest_retained_ts, newest_retained_ts): (
        i64,
        Option<i64>,
        Option<i64>,
    ) = conn.query_row(
        "SELECT COUNT(*), MIN(ts), MAX(ts) FROM log_events",
        [],
        |row| Ok((row.get::<_, i64>(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let total_event_count = total_event_count_i64.max(0) as u64;

    let on_disk_bytes = content_bytes(conn)?;

    Ok(LogStoreStats {
        on_disk_bytes,
        oldest_retained_ts,
        newest_retained_ts,
        total_event_count,
        dropped_event_count: 0,
        retention,
    })
}

/// Sum of the logical content bytes retained (message + fields_json + small
/// per-row overhead). This is what retention policies act on — NOT the
/// physical SQLite file size, which has fixed overhead + WAL sidecar weight
/// that can't be shrunk below a few KB regardless of content.
fn content_bytes(conn: &Connection) -> Result<u64, rusqlite::Error> {
    conn.query_row(
        "SELECT COALESCE(SUM(LENGTH(message) + LENGTH(fields_json)), 0) FROM log_events",
        [],
        |row| row.get::<_, i64>(0).map(|n| n.max(0) as u64),
    )
}

// ── Maintenance ───────────────────────────────────────────────────────────────

#[allow(dead_code)]
fn run_maintenance(conn: &Connection, retention: LogRetention) -> Result<(), rusqlite::Error> {
    let now_ms = super::ingest::now_ms();
    let age_ms = i64::try_from(retention.max_age_days)
        .ok()
        .and_then(|d| d.checked_mul(86_400_000))
        .unwrap_or(i64::MAX);
    let cutoff = now_ms.saturating_sub(age_ms);
    conn.execute("DELETE FROM log_events WHERE ts < ?1", params![cutoff])?;

    for _ in 0..64 {
        if content_bytes(conn)? <= retention.max_bytes {
            break;
        }
        let affected = conn.execute(
            "DELETE FROM log_events WHERE rowid IN
               (SELECT rowid FROM log_events ORDER BY ts ASC LIMIT 256)",
            [],
        )?;
        if affected == 0 {
            break;
        }
    }
    // Checkpoint the WAL to reclaim pages and keep the WAL file small.
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column_names(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("PRAGMA table_info(log_events)")
            .expect("prepare table_info");
        stmt.query_map([], |row| row.get::<_, String>(1))
            .expect("query table_info")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect columns")
    }

    fn index_sql(conn: &Connection, index_name: &str) -> String {
        conn.query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = ?1",
            params![index_name],
            |row| row.get(0),
        )
        .expect("index sql")
    }

    #[test]
    fn fresh_schema_includes_actor_key_completion_kind_and_partial_indexes() {
        let conn = Connection::open_in_memory().expect("open db");

        migrate(&conn).expect("migrate");

        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("user_version");
        assert_eq!(version, 3);
        assert!(column_names(&conn).iter().any(|name| name == "actor_key"));
        assert!(
            column_names(&conn)
                .iter()
                .any(|name| name == "completion_kind")
        );

        let sql = index_sql(&conn, "idx_log_events_actor_key_ts");
        assert!(sql.contains("actor_key, ts DESC"));
        assert!(sql.contains("WHERE actor_key IS NOT NULL"));
        let sql = index_sql(&conn, "idx_log_events_completion_ts");
        assert!(sql.contains("completion_kind, ts DESC, event_id DESC"));
        assert!(sql.contains("WHERE completion_kind = 1"));
        let sql = index_sql(&conn, "idx_log_events_completion_actor_ts");
        assert!(sql.contains("actor_key, ts DESC"));
        assert!(sql.contains("completion_kind = 1"));
    }

    #[test]
    fn v1_database_migrates_actor_key_and_completion_kind() {
        let conn = Connection::open_in_memory().expect("open db");
        conn.execute_batch(
            r#"
            CREATE TABLE log_events (
                event_id          TEXT PRIMARY KEY,
                ts                INTEGER NOT NULL,
                level             TEXT NOT NULL,
                subsystem         TEXT NOT NULL,
                surface           TEXT NOT NULL,
                action            TEXT,
                message           TEXT NOT NULL,
                request_id        TEXT,
                session_id        TEXT,
                correlation_id    TEXT,
                trace_id          TEXT,
                span_id           TEXT,
                instance          TEXT,
                auth_flow         TEXT,
                outcome_kind      TEXT,
                fields_json       TEXT NOT NULL DEFAULT '{}',
                source_kind       TEXT,
                source_node_id    TEXT,
                source_device_id  TEXT,
                ingest_path       TEXT,
                upstream_event_id TEXT
            );
            INSERT INTO log_events (
                event_id, ts, level, subsystem, surface, message, fields_json
            ) VALUES (
                'evt-history', 123, 'info', 'core_runtime', 'core_runtime', 'old row', '{}'
            ), (
                'evt-completion', 124, 'info', 'core_runtime', 'core_runtime', 'dispatch ok',
                '{"service":"radarr","input_tokens":1,"output_tokens":2}'
            );
            PRAGMA user_version = 1;
            "#,
        )
        .expect("create v1 db");

        migrate(&conn).expect("migrate");

        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("user_version");
        assert_eq!(version, 3);
        assert!(column_names(&conn).iter().any(|name| name == "actor_key"));
        assert!(
            column_names(&conn)
                .iter()
                .any(|name| name == "completion_kind")
        );

        let actor_key: Option<String> = conn
            .query_row(
                "SELECT actor_key FROM log_events WHERE event_id = 'evt-history'",
                [],
                |row| row.get(0),
            )
            .expect("historical actor_key");
        assert_eq!(actor_key, None);

        let completion_kind: i64 = conn
            .query_row(
                "SELECT completion_kind FROM log_events WHERE event_id = 'evt-completion'",
                [],
                |row| row.get(0),
            )
            .expect("historical completion kind");
        assert_eq!(completion_kind, 1);

        let sql = index_sql(&conn, "idx_log_events_actor_key_ts");
        assert!(sql.contains("WHERE actor_key IS NOT NULL"));
        let sql = index_sql(&conn, "idx_log_events_completion_ts");
        assert!(sql.contains("WHERE completion_kind = 1"));
    }

    #[test]
    fn v2_database_repairs_existing_completion_kind_column() {
        let conn = Connection::open_in_memory().expect("open db");
        conn.execute_batch(
            r#"
            CREATE TABLE log_events (
                event_id          TEXT PRIMARY KEY,
                ts                INTEGER NOT NULL,
                level             TEXT NOT NULL,
                subsystem         TEXT NOT NULL,
                surface           TEXT NOT NULL,
                action            TEXT,
                message           TEXT NOT NULL,
                request_id        TEXT,
                session_id        TEXT,
                correlation_id    TEXT,
                trace_id          TEXT,
                span_id           TEXT,
                instance          TEXT,
                auth_flow         TEXT,
                outcome_kind      TEXT,
                fields_json       TEXT NOT NULL DEFAULT '{}',
                completion_kind   INTEGER NOT NULL DEFAULT 0,
                source_kind       TEXT,
                source_node_id    TEXT,
                source_device_id  TEXT,
                actor_key         TEXT,
                ingest_path       TEXT,
                upstream_event_id TEXT
            );
            INSERT INTO log_events (
                event_id, ts, level, subsystem, surface, message, fields_json, actor_key
            ) VALUES (
                'evt-start', 123, 'info', 'core_runtime', 'core_runtime', 'dispatch start',
                '{"service":"radarr","input_tokens":1}', 'start-agent'
            ), (
                'evt-completion', 124, 'info', 'core_runtime', 'core_runtime', 'dispatch ok',
                '{"service":"radarr","input_tokens":1,"output_tokens":2}', 'completion-agent'
            );
            PRAGMA user_version = 2;
            "#,
        )
        .expect("create partial v2 db");

        migrate(&conn).expect("migrate");

        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("user_version");
        assert_eq!(version, 3);

        let start_kind: i64 = conn
            .query_row(
                "SELECT completion_kind FROM log_events WHERE event_id = 'evt-start'",
                [],
                |row| row.get(0),
            )
            .expect("start completion kind");
        let completion_kind: i64 = conn
            .query_row(
                "SELECT completion_kind FROM log_events WHERE event_id = 'evt-completion'",
                [],
                |row| row.get(0),
            )
            .expect("completion kind");
        assert_eq!(start_kind, 0);
        assert_eq!(completion_kind, 1);

        let sql = index_sql(&conn, "idx_log_events_completion_ts");
        assert!(sql.contains("WHERE completion_kind = 1"));
        let sql = index_sql(&conn, "idx_log_events_completion_actor_ts");
        assert!(sql.contains("completion_kind = 1"));
    }
}
