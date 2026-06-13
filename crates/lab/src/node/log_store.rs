#![allow(dead_code)]

//! SQLite-backed node log persistence.
//!
//! # Architecture
//!
//! - Single-connection `write_pool` (r2d2, `max_size=1`) — used for one-off writes.
//! - Multi-connection `read_pool` (r2d2, `max_size=4`) — WAL-mode readers proceed
//!   in parallel; each pooled connection is opened with `query_only=true`.
//! - Dedicated background writer task — drains the mpsc channel and batches
//!   `node_logs` INSERTs (up to 128 events or 25 ms, whichever comes first).
//!   Retention TTL sweeps also run inside the same task every 5 minutes.
//!
//! # Single writer pattern
//!
//! DECISION: One writer task handles both batch ingest and retention. Using two
//! tasks against the same WAL writer would require `busy_timeout` coordination
//! and introduce retry noise. Merging them in a `tokio::select!` keeps the writer
//! serial and allows the retention sweep to borrow the dedicated connection.
//!
//! # Ingest fields cap
//!
//! DECISION: Fields exceeding 4 KB are rejected at ingest (WARN log, event dropped).
//! Truncating a JSON object mid-serialization produces invalid JSON, which is
//! worse than dropping. If a caller sends oversized fields intentionally, they
//! will see the WARN and must shrink the payload client-side.
//!
//! # LIKE escaping
//!
//! User-supplied query strings are escaped so that `%` and `_` are treated as
//! literals. The escape character is `\`. Replacement order: `\` → `\\` first,
//! then `%` → `\%`, then `_` → `\_`. Wrong order would double-escape.
//!
//! # Path security
//!
//! The `LAB_NODE_LOG_DB` path is validated to reject any `..` component.
//!
//! # File permissions
//!
//! The database file is created with mode 0600 (owner read/write only) on Unix
//! first open. Subsequent opens do not change permissions.

use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{Connection, OpenFlags, params};
use tokio::sync::mpsc;

use crate::node::log_event::NodeLogEvent;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum events to accumulate before flushing to SQLite.
const BATCH_SIZE: usize = 128;

/// Maximum wait before flushing a partial batch.
const BATCH_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(25);

/// How often the writer task runs the TTL retention sweep.
const RETENTION_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// Max rows deleted per retention iteration (convergence-loop guard).
const RETENTION_BATCH_SIZE: i64 = 5_000;

/// Maximum serialized size (bytes) for the `fields` JSON column.
/// Events exceeding this limit are dropped with a WARN log at ingest.
const FIELDS_MAX_BYTES: usize = 4 * 1024; // 4 KB

/// Default TTL in days when not set in config.
pub const DEFAULT_RETENTION_DAYS: u32 = 30;

fn should_log_counter(count: u64) -> bool {
    count <= 10 || count.is_power_of_two()
}

// ── SqliteNodeLogStore ────────────────────────────────────────────────────────

/// Durable SQLite-backed store for node log events.
///
/// Clone is cheap — all state is behind `Arc`.
#[derive(Clone)]
pub struct SqliteNodeLogStore {
    /// Single-connection write pool (used for one-off writes, not batch ingest).
    #[allow(dead_code)]
    write_pool: Pool<SqliteConnectionManager>,
    /// Multi-connection read pool; WAL lets pooled readers run in parallel.
    read_pool: Pool<SqliteConnectionManager>,
    /// Channel to the background writer task.
    event_tx: mpsc::Sender<NodeLogEvent>,
    /// Best-effort visibility for events dropped before SQLite persistence.
    dropped_events: Arc<AtomicU64>,
    /// Count of ingest sends that observed a saturated or near-saturated channel.
    saturation_events: Arc<AtomicU64>,
    /// Count of events successfully flushed by the background writer.
    #[cfg_attr(not(test), allow(dead_code))]
    flushed_events: Arc<AtomicU64>,
    /// Dedicated writer connection shared with the background task (Arc for test access).
    #[cfg_attr(not(test), allow(dead_code))]
    writer_conn: Arc<Mutex<Connection>>,
    /// Retention TTL in days (stored for test helper access).
    #[cfg_attr(not(test), allow(dead_code))]
    retention_days: u32,
}

impl std::fmt::Debug for SqliteNodeLogStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteNodeLogStore")
            .field("event_tx_capacity", &self.event_tx.capacity())
            .field("event_tx_max_capacity", &self.event_tx.max_capacity())
            .finish_non_exhaustive()
    }
}

fn node_log_pragma_init(
    query_only: bool,
) -> impl Fn(&mut Connection) -> rusqlite::Result<()> + Send + Sync + 'static {
    // FACT: WAL journal_mode must be applied per-connection via pragma_update
    // (never inside a migration transaction — silent no-op there; prior incident lab-fstf.4).
    move |conn| {
        conn.busy_timeout(std::time::Duration::from_millis(5_000))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "mmap_size", 134_217_728_i64)?;
        conn.pragma_update(None, "cache_size", -32_768_i64)?;
        conn.pragma_update(None, "wal_autocheckpoint", 1000_i64)?;
        if query_only {
            conn.pragma_update(None, "query_only", "true")?;
        }
        Ok(())
    }
}

impl SqliteNodeLogStore {
    /// Open (or create) the node log database at the given path.
    ///
    /// The database file is created with 0600 permissions on Unix.
    /// `retention_days` controls the TTL for the background retention sweep.
    pub async fn open(db_path: PathBuf, retention_days: u32) -> Result<Self, String> {
        reject_db_path_traversal(&db_path)?;

        let path = db_path.clone();
        let path_display = db_path.display().to_string();
        tracing::info!(
            surface = "node",
            service = "log_store",
            action = "sqlite.open.start",
            path = %path_display,
            retention_days,
            "opening node log SQLite database",
        );

        let (write_pool, read_pool, writer_task_conn) =
            tokio::task::spawn_blocking(move || -> Result<_, String> {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| format!("create_dir_all: {e}"))?;
                    tracing::debug!(
                        surface = "node",
                        service = "log_store",
                        action = "sqlite.open.parent_ready",
                        parent = %parent.display(),
                        "node log store parent directory ready",
                    );
                }

                // Create the file with 0600 perms on first open (Unix only).
                #[cfg(unix)]
                crate::dispatch::helpers::create_db_file_0600(&path);

                // CRITICAL: Run migration on a bare connection (no WAL, no pragmas) so
                // that `auto_vacuum=INCREMENTAL` and `page_size=8192` can be set before
                // SQLite commits the first page. Switching to WAL mode writes to the DB
                // header and locks in the vacuum setting; we must migrate first.
                {
                    let migration_conn =
                        Connection::open(&path).map_err(|e| format!("open migration conn: {e}"))?;
                    migrate(&migration_conn).map_err(|e| format!("migrate: {e}"))?;
                }

                let write_manager =
                    SqliteConnectionManager::file(&path).with_init(node_log_pragma_init(false));
                let write_pool = Pool::builder()
                    .max_size(1)
                    .connection_timeout(std::time::Duration::from_secs(5))
                    .build(write_manager)
                    .map_err(|e| format!("build write pool: {e}"))?;
                tracing::debug!(
                    surface = "node",
                    service = "log_store",
                    action = "sqlite.pool.open",
                    pool = "write",
                    max_size = 1_u32,
                    journal_mode = "WAL",
                    "node log SQLite write pool opened",
                );

                let read_manager =
                    SqliteConnectionManager::file(&path).with_init(node_log_pragma_init(true));
                let read_pool = Pool::builder()
                    .max_size(4)
                    .connection_timeout(std::time::Duration::from_secs(5))
                    .build(read_manager)
                    .map_err(|e| format!("build read pool: {e}"))?;
                tracing::debug!(
                    surface = "node",
                    service = "log_store",
                    action = "sqlite.pool.open",
                    pool = "read",
                    max_size = 4_u32,
                    journal_mode = "WAL",
                    query_only = true,
                    "node log SQLite read pool opened",
                );

                // Dedicated single-owner writer connection for the hot-path batch inserts.
                let rw_flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
                let mut tc = Connection::open_with_flags(&path, rw_flags)
                    .map_err(|e| format!("open writer conn: {e}"))?;
                node_log_pragma_init(false)(&mut tc)
                    .map_err(|e| format!("writer conn pragmas: {e}"))?;
                tracing::debug!(
                    surface = "node",
                    service = "log_store",
                    action = "sqlite.writer.open",
                    journal_mode = "WAL",
                    "node log SQLite writer connection opened",
                );

                Ok((write_pool, read_pool, tc))
            })
            .await
            .map_err(|e| format!("db open join: {e}"))??;

        // Bounded channel — back-pressure if writer falls behind.
        let (event_tx, event_rx) = mpsc::channel::<NodeLogEvent>(4096);

        let writer_conn = Arc::new(Mutex::new(writer_task_conn));
        let flushed_events = Arc::new(AtomicU64::new(0));
        tokio::spawn(writer_task(
            Arc::clone(&writer_conn),
            event_rx,
            retention_days,
            Arc::clone(&flushed_events),
        ));

        tracing::info!(
            surface = "node",
            service = "log_store",
            action = "sqlite.open",
            path = %path_display,
            channel_capacity = event_tx.max_capacity(),
            retention_days,
            "node log SQLite database opened",
        );

        Ok(Self {
            write_pool,
            read_pool,
            event_tx,
            dropped_events: Arc::new(AtomicU64::new(0)),
            saturation_events: Arc::new(AtomicU64::new(0)),
            flushed_events,
            writer_conn,
            retention_days,
        })
    }

    /// Open from environment or default path.
    pub async fn from_env(retention_days: u32) -> Result<Self, String> {
        let path = resolve_db_path()?;
        Self::open(path, retention_days).await
    }

    /// Send a log event to the writer task.
    ///
    /// If the `fields` column would exceed 4 KB when serialized, the event is
    /// dropped with a WARN log and `Ok(())` is returned (best-effort ingest).
    pub async fn ingest(&self, event: NodeLogEvent) -> Result<(), String> {
        // Enforce 4 KB fields cap at ingest.
        let fields_str =
            serde_json::to_string(&event.fields).map_err(|e| format!("serialize fields: {e}"))?;
        if fields_str.len() > FIELDS_MAX_BYTES {
            let total_dropped = self.dropped_events.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::warn!(
                surface = "node",
                service = "log_store",
                action = "ingest",
                kind = "payload_too_large",
                node_id = %event.node_id,
                fields_bytes = fields_str.len(),
                limit_bytes = FIELDS_MAX_BYTES,
                total_dropped,
                "node log event fields exceed 4 KB limit; event dropped",
            );
            return Ok(());
        }

        let remaining = self.event_tx.capacity();
        let max_capacity = self.event_tx.max_capacity();
        let saturated = remaining == 0;
        let near_saturation = remaining <= (max_capacity / 16).max(1);
        if saturated || near_saturation {
            let saturation_events = self.saturation_events.fetch_add(1, Ordering::Relaxed) + 1;
            if should_log_counter(saturation_events) {
                tracing::warn!(
                    surface = "node",
                    service = "log_store",
                    action = "ingest.channel_saturation",
                    node_id = %event.node_id,
                    queue_capacity = max_capacity,
                    queue_remaining = remaining,
                    send_will_await_capacity = saturated,
                    saturation_events,
                    "node log writer channel saturated; ingest send may block",
                );
            }
        }

        self.event_tx.send(event).await.map_err(|_| {
            let total_dropped = self.dropped_events.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::warn!(
                surface = "node",
                service = "log_store",
                action = "ingest.drop",
                kind = "internal_error",
                total_dropped,
                "node log writer task channel closed; event dropped",
            );
            "node log writer task channel closed".to_string()
        })
    }

    /// Search log events for a node.
    ///
    /// `node_id` is a mandatory predicate (uses `idx_node_logs_node_ts`).
    /// `needle` is matched case-insensitively (SQLite ASCII default); LIKE
    /// special characters `%` and `_` in `needle` are treated as literals.
    /// Optional `since_ms` / `until_ms` narrow the timestamp range.
    pub async fn search(
        &self,
        node_id: String,
        needle: String,
        since_ms: Option<i64>,
        until_ms: Option<i64>,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<NodeLogEvent>, String> {
        let read_pool = self.read_pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = read_pool
                .get()
                .map_err(|e| format!("search pool get: {e}"))?;
            db_search(&conn, &node_id, &needle, since_ms, until_ms, offset, limit)
                .map_err(|e| format!("search query: {e}"))
        })
        .await
        .map_err(|e| format!("search join: {e}"))?
    }

    /// Run a retention sweep synchronously (test helper only).
    ///
    /// Calls `run_retention` directly on the writer connection. This bypasses the
    /// background task channel so tests can trigger retention deterministically
    /// without waiting for the 5-minute interval.
    #[cfg(test)]
    pub(crate) async fn run_retention_for_test(&self) {
        run_retention(&self.writer_conn, self.retention_days).await;
    }

    /// Wait until the background writer has persisted at least `expected` events.
    #[cfg(test)]
    pub(crate) async fn wait_for_flushed_for_test(&self, expected: u64) -> Result<(), String> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let flushed = self.flushed_events.load(Ordering::Acquire);
            if flushed >= expected {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!(
                    "timed out waiting for node log flush: flushed {flushed}, expected {expected}"
                ));
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}

// ── Background writer task ─────────────────────────────────────────────────────

async fn writer_task(
    conn: Arc<Mutex<Connection>>,
    mut rx: mpsc::Receiver<NodeLogEvent>,
    retention_days: u32,
    flushed_events: Arc<AtomicU64>,
) {
    let mut batch: Vec<NodeLogEvent> = Vec::with_capacity(BATCH_SIZE);
    let mut retention_ticker = tokio::time::interval(RETENTION_INTERVAL);
    // Skip the first tick (fires immediately on creation).
    retention_ticker.tick().await;

    loop {
        let deadline = tokio::time::Instant::now() + BATCH_TIMEOUT;

        // Collect up to BATCH_SIZE events within BATCH_TIMEOUT, also watching
        // for the retention ticker. This is the single writer task pattern.
        'collect: loop {
            // NOTE: do not mark this select `biased`. Under sustained ingest the
            // `rx.recv()` branch can fire on every poll, and a biased select would
            // never give the retention ticker a turn — TTL sweeps would silently
            // stop running. Random poll order ensures both branches make progress.
            tokio::select! {
                recv_result = tokio::time::timeout_at(deadline, rx.recv()) => {
                    match recv_result {
                        Ok(Some(event)) => {
                            batch.push(event);
                            if batch.len() >= BATCH_SIZE {
                                break 'collect;
                            }
                        }
                        Ok(None) => {
                            // Channel closed — flush remaining and exit.
                            let flushed = flush_batch(&conn, &mut batch).await as u64;
                            if flushed > 0 {
                                flushed_events.fetch_add(flushed, Ordering::Release);
                            }
                            tracing::warn!(
                                surface = "node",
                                service = "log_store",
                                action = "writer.exit",
                                flushed_events = flushed_events.load(Ordering::Acquire),
                                "node log writer task exited; all senders dropped",
                            );
                            return;
                        }
                        Err(_timeout) => {
                            // Deadline reached — flush partial batch.
                            break 'collect;
                        }
                    }
                }
                _ = retention_ticker.tick() => {
                    // Run retention inline, using the same dedicated connection.
                    // Flush pending batch first so retention sees all recent events.
                    let flushed = flush_batch(&conn, &mut batch).await as u64;
                    if flushed > 0 {
                        flushed_events.fetch_add(flushed, Ordering::Release);
                    }
                    run_retention(&conn, retention_days).await;
                    break 'collect;
                }
            }
        }

        if !batch.is_empty() {
            let flushed = flush_batch(&conn, &mut batch).await as u64;
            if flushed > 0 {
                flushed_events.fetch_add(flushed, Ordering::Release);
            }
        }
    }
}

async fn flush_batch(conn: &Arc<Mutex<Connection>>, batch: &mut Vec<NodeLogEvent>) -> usize {
    if batch.is_empty() {
        return 0;
    }
    let events = std::mem::take(batch);
    let count = events.len();
    let conn = Arc::clone(conn);
    let result = tokio::task::spawn_blocking(move || {
        let c = conn
            .lock()
            .map_err(|_| "writer mutex poisoned".to_string())?;
        db_batch_insert(&c, &events).map_err(|e| format!("batch insert: {e}"))
    })
    .await;
    match result {
        Ok(Ok(())) => count,
        Ok(Err(error)) => {
            tracing::error!(
                surface = "node",
                service = "log_store",
                action = "flush_batch",
                kind = "internal_error",
                events = count,
                error,
                "node log batch insert failed",
            );
            0
        }
        Err(join_err) => {
            tracing::error!(
                surface = "node",
                service = "log_store",
                action = "flush_batch",
                kind = "internal_error",
                events = count,
                error = %join_err,
                "node log flush task panicked",
            );
            0
        }
    }
}

async fn run_retention(conn: &Arc<Mutex<Connection>>, retention_days: u32) {
    let cutoff_ms = {
        let days = i64::from(retention_days);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        now_ms - days * 86_400 * 1_000
    };

    let conn = Arc::clone(conn);
    // Convergence guard: loop until fewer than RETENTION_BATCH_SIZE rows deleted.
    let result = tokio::task::spawn_blocking(move || {
        let c = conn
            .lock()
            .map_err(|_| "writer mutex poisoned".to_string())?;
        loop {
            let affected = c
                .execute(
                    "DELETE FROM node_logs WHERE id IN \
                     (SELECT id FROM node_logs WHERE timestamp_unix_ms < ?1 \
                      ORDER BY timestamp_unix_ms ASC LIMIT ?2)",
                    params![cutoff_ms, RETENTION_BATCH_SIZE],
                )
                .map_err(|e| format!("retention delete: {e}"))?;

            tracing::debug!(
                surface = "node",
                service = "log_store",
                action = "retention",
                rows_deleted = affected,
                cutoff_ms,
                "node log retention sweep iteration",
            );

            if affected < RETENTION_BATCH_SIZE as usize {
                break;
            }
            // Affected the full batch — loop immediately (convergence guard).
        }
        Ok::<_, String>(())
    })
    .await;

    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::error!(
            surface = "node",
            service = "log_store",
            action = "retention",
            kind = "internal_error",
            error,
            "node log retention sweep failed",
        ),
        Err(join_err) => tracing::error!(
            surface = "node",
            service = "log_store",
            action = "retention",
            kind = "internal_error",
            error = %join_err,
            "node log retention task panicked",
        ),
    }
}

// ── Database helpers ──────────────────────────────────────────────────────────

/// Run schema migrations on a fresh connection.
///
/// CRITICAL: `auto_vacuum` and `page_size` must be set inside the `version < 1`
/// branch, before `execute_batch`. Setting them after the first write is a
/// silent no-op in SQLite.
fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    // Detect existing DB created without auto_vacuum=INCREMENTAL.
    let av: i64 = conn.pragma_query_value(None, "auto_vacuum", |r| r.get(0))?;
    if av == 0 {
        tracing::warn!(
            surface = "node",
            service = "log_store",
            action = "migrate",
            "node-logs.db was created without auto_vacuum=INCREMENTAL; page reclaim disabled. \
             Delete and recreate the file to enable incremental auto-vacuum.",
        );
    }

    let version: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    tracing::debug!(
        surface = "node",
        service = "log_store",
        action = "sqlite.migrate.start",
        from_version = version,
        auto_vacuum = av,
        "checking node log SQLite migrations",
    );
    if version < 1 {
        // FACT: auto_vacuum and page_size must be set here, before execute_batch.
        // Connection-level pragma hooks cannot set them after the first DB write.
        conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
        conn.pragma_update(None, "page_size", 8192_i64)?;
        conn.execute_batch(SCHEMA_SQL)?;
        conn.pragma_update(None, "user_version", 1)?;
        tracing::info!(
            surface = "node",
            service = "log_store",
            action = "sqlite.migrate.apply",
            from_version = version,
            to_version = 1,
            "applied node log SQLite migration",
        );
    } else {
        tracing::debug!(
            surface = "node",
            service = "log_store",
            action = "sqlite.migrate.skip",
            version,
            "node log SQLite schema already current",
        );
    }
    Ok(())
}

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS node_logs (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    node_id           TEXT    NOT NULL,
    source            TEXT    NOT NULL,
    timestamp_unix_ms INTEGER NOT NULL,
    level             TEXT,
    message           TEXT    NOT NULL,
    fields            TEXT    NOT NULL DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS idx_node_logs_node_ts ON node_logs(node_id, timestamp_unix_ms);
CREATE INDEX IF NOT EXISTS idx_node_logs_ts      ON node_logs(timestamp_unix_ms);
";

fn db_batch_insert(conn: &Connection, events: &[NodeLogEvent]) -> Result<(), String> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("begin transaction: {e}"))?;

    for event in events {
        let fields_str = serde_json::to_string(&event.fields).unwrap_or_else(|_| "{}".to_string());
        tx.execute(
            "INSERT INTO node_logs (node_id, source, timestamp_unix_ms, level, message, fields)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                event.node_id,
                event.source,
                event.timestamp_unix_ms,
                event.level,
                event.message,
                fields_str,
            ],
        )
        .map_err(|e| format!("insert log: {e}"))?;
    }

    tx.commit().map_err(|e| format!("commit: {e}"))?;
    Ok(())
}

/// Escape a user-supplied query string for use as a SQLite LIKE pattern value.
///
/// Replacement order: `\` first, then `%`, then `_` — reversed order would
/// double-escape the backslash we introduce in the first pass.
fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn db_search(
    conn: &Connection,
    node_id: &str,
    needle: &str,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
    offset: usize,
    limit: usize,
) -> rusqlite::Result<Vec<NodeLogEvent>> {
    let effective_limit = limit.max(1).min(1_000);
    let escaped = escape_like(needle);

    // Build the WHERE clause; node_id is always required (uses the composite index).
    // FACT: LIKE with a leading wildcard skips the index. We require node_id first
    // so the composite index idx_node_logs_node_ts prunes to a single node before
    // the LIKE scan runs over the reduced set.
    let mut sql = String::from(
        "SELECT node_id, source, timestamp_unix_ms, level, message, fields \
         FROM node_logs \
         WHERE node_id = ?1",
    );
    let mut arg_idx = 2i32;

    if needle.is_empty() {
        // No LIKE filter needed — return all for node.
    } else {
        sql.push_str(&format!(
            " AND message LIKE '%' || ?{arg_idx} || '%' ESCAPE '\\'"
        ));
        arg_idx += 1;
    }

    if since_ms.is_some() {
        sql.push_str(&format!(" AND timestamp_unix_ms >= ?{arg_idx}"));
        arg_idx += 1;
    }
    if until_ms.is_some() {
        sql.push_str(&format!(" AND timestamp_unix_ms <= ?{arg_idx}"));
        arg_idx += 1;
    }

    sql.push_str(&format!(
        " ORDER BY timestamp_unix_ms DESC \
         LIMIT ?{} OFFSET ?{}",
        arg_idx,
        arg_idx + 1,
    ));

    // Build the parameter list.
    let mut args: Vec<rusqlite::types::Value> = vec![node_id.to_owned().into()];
    if !needle.is_empty() {
        args.push(escaped.into());
    }
    if let Some(since) = since_ms {
        args.push(since.into());
    }
    if let Some(until) = until_ms {
        args.push(until.into());
    }
    args.push((effective_limit as i64).into());
    args.push((offset as i64).into());

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), |row| {
        let fields_str: String = row.get("fields")?;
        let fields: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&fields_str).unwrap_or_default();
        Ok(NodeLogEvent {
            node_id: row.get("node_id")?,
            source: row.get("source")?,
            timestamp_unix_ms: row.get("timestamp_unix_ms")?,
            level: row.get("level")?,
            message: row.get("message")?,
            fields,
        })
    })?;

    rows.collect()
}

// ── Path resolution ───────────────────────────────────────────────────────────

fn resolve_db_path() -> Result<PathBuf, String> {
    if let Ok(path) = std::env::var("LAB_NODE_LOG_DB") {
        let path = PathBuf::from(path);
        reject_db_path_traversal(&path)?;
        return Ok(path);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    Ok(PathBuf::from(home).join(".lab").join("node-logs.db"))
}

fn reject_db_path_traversal(path: &Path) -> Result<(), String> {
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!(
            "node log db path rejected: `{}` must not contain `..`",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod log_store_tests;
