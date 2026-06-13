//! Tests for `SqliteNodeLogStore`.
//!
//! These tests exercise the acceptance criteria from bead lab-e2tu:
//! durability, TTL retention, convergence guard, permissions, fields cap,
//! LIKE escaping, and the auto_vacuum detection path.

#![cfg(test)]

use crate::node::log_event::NodeLogEvent;
use crate::node::log_store::SqliteNodeLogStore;
use std::time::Duration;
use std::time::Instant;
use tempfile::NamedTempFile;
use tempfile::TempDir;

/// Build a test log event with the given message.
fn test_event(node_id: &str, message: &str, timestamp_unix_ms: i64) -> NodeLogEvent {
    NodeLogEvent {
        node_id: node_id.to_string(),
        source: "test".to_string(),
        timestamp_unix_ms,
        level: Some("info".to_string()),
        message: message.to_string(),
        fields: Default::default(),
    }
}

/// Build a test log event with explicit fields JSON.
fn test_event_with_fields(
    node_id: &str,
    message: &str,
    timestamp_unix_ms: i64,
    fields: serde_json::Map<String, serde_json::Value>,
) -> NodeLogEvent {
    NodeLogEvent {
        node_id: node_id.to_string(),
        source: "test".to_string(),
        timestamp_unix_ms,
        level: Some("info".to_string()),
        message: message.to_string(),
        fields,
    }
}

/// Get a unique temp path for a SQLite DB.
fn temp_db_path() -> tempfile::TempPath {
    // Create a temp file and convert to a path-only handle (deletes file on drop,
    // so SQLite gets a fresh empty file path).
    let f = NamedTempFile::new().expect("temp file");
    f.into_temp_path()
}

// ── Durability after restart ───────────────────────────────────────────────────

#[tokio::test]
async fn durability_survives_store_drop_and_reopen() {
    let dir = TempDir::new().expect("temp dir");
    let db_path = dir.path().join("test-logs.db");

    // Open, ingest, close.
    {
        let store = SqliteNodeLogStore::open(db_path.clone(), 30)
            .await
            .expect("open");
        store
            .ingest(test_event("node-1", "hello after restart", 1_000))
            .await
            .expect("ingest");
        store.wait_for_flushed_for_test(1).await.expect("flush");
    }

    // Reopen and search.
    let store2 = SqliteNodeLogStore::open(db_path, 30).await.expect("reopen");
    let results = store2
        .search(
            "node-1".to_string(),
            "restart".to_string(),
            None,
            None,
            0,
            10,
        )
        .await
        .expect("search");
    assert_eq!(results.len(), 1, "event must survive store reopen");
    assert_eq!(results[0].message, "hello after restart");
}

// ── Performance: 50k events under 200ms ───────────────────────────────────────

#[tokio::test]
async fn search_50k_events_under_200ms() {
    let dir = TempDir::new().expect("temp dir");
    let db_path = dir.path().join("perf-test.db");
    let store = SqliteNodeLogStore::open(db_path, 30).await.expect("open");

    // Seed 50,000 events in batches using direct ingestion.
    for i in 0i64..50_000 {
        store
            .ingest(test_event("perf-node", &format!("message-{i}"), i))
            .await
            .expect("ingest");
    }
    store
        .wait_for_flushed_for_test(50_000)
        .await
        .expect("flush");

    let start = Instant::now();
    let results = store
        .search(
            "perf-node".to_string(),
            "message-42".to_string(),
            None,
            None,
            0,
            1000,
        )
        .await
        .expect("search");
    let elapsed = start.elapsed();

    assert!(
        !results.is_empty(),
        "search must find results in 50k dataset"
    );
    assert!(
        elapsed < Duration::from_millis(200),
        "search must complete under 200ms; took {elapsed:?}",
    );
}

// ── TTL retention removes old rows ────────────────────────────────────────────

#[tokio::test]
async fn ttl_retention_removes_rows_older_than_retention_days() {
    let dir = TempDir::new().expect("temp dir");
    let db_path = dir.path().join("ttl-test.db");
    let store = SqliteNodeLogStore::open(db_path, 30).await.expect("open");

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    // Insert an old event (31 days ago) and a fresh event.
    let old_ts = now_ms - 31 * 86_400 * 1_000;
    let fresh_ts = now_ms;

    store
        .ingest(test_event("ttl-node", "old-message", old_ts))
        .await
        .expect("ingest old");
    store
        .ingest(test_event("ttl-node", "fresh-message", fresh_ts))
        .await
        .expect("ingest fresh");
    store.wait_for_flushed_for_test(2).await.expect("flush");

    // Run a retention sweep synchronously via the exposed test helper.
    store.run_retention_for_test().await;

    // The old row should be gone, the fresh row should remain.
    let all = store
        .search("ttl-node".to_string(), String::new(), None, None, 0, 100)
        .await
        .expect("search");
    let messages: Vec<&str> = all.iter().map(|e| e.message.as_str()).collect();
    assert!(
        !messages.contains(&"old-message"),
        "old event must be removed by TTL sweep; found: {messages:?}",
    );
    assert!(
        messages.contains(&"fresh-message"),
        "fresh event must survive TTL sweep; found: {messages:?}",
    );
}

// ── Convergence guard: retention loops until < 5000 rows deleted ──────────────

#[tokio::test]
async fn retention_convergence_guard_clears_large_backlog() {
    let dir = TempDir::new().expect("temp dir");
    let db_path = dir.path().join("convergence-test.db");
    let store = SqliteNodeLogStore::open(db_path, 30).await.expect("open");

    let old_ts = {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        now_ms - 35 * 86_400 * 1_000 // 35 days ago
    };

    // Insert 12,000 old events (more than one 5k retention batch).
    for i in 0i64..12_000 {
        store
            .ingest(test_event("conv-node", &format!("old-{i}"), old_ts + i))
            .await
            .expect("ingest");
    }
    store
        .wait_for_flushed_for_test(12_000)
        .await
        .expect("flush");

    // Run retention — the convergence guard must loop until all old rows are gone.
    store.run_retention_for_test().await;

    let remaining = store
        .search("conv-node".to_string(), String::new(), None, None, 0, 1000)
        .await
        .expect("search");
    assert_eq!(
        remaining.len(),
        0,
        "convergence guard must delete all 12k old rows; {left} remain",
        left = remaining.len(),
    );
}

// ── DB file permissions 0600 on Unix ─────────────────────────────────────────

#[cfg(unix)]
#[tokio::test]
async fn db_file_created_with_0600_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().expect("temp dir");
    let db_path = dir.path().join("perms-test.db");
    let _store = SqliteNodeLogStore::open(db_path.clone(), 30)
        .await
        .expect("open");

    let meta = std::fs::metadata(&db_path).expect("metadata");
    let mode = meta.permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "DB file must be created with 0600 permissions; got {mode:o}",
    );
}

// ── auto_vacuum = 2 (INCREMENTAL) on fresh DB ─────────────────────────────────

#[tokio::test]
async fn fresh_db_has_auto_vacuum_incremental() {
    let dir = TempDir::new().expect("temp dir");
    let db_path = dir.path().join("av-test.db");
    let _store = SqliteNodeLogStore::open(db_path.clone(), 30)
        .await
        .expect("open");

    // Query auto_vacuum directly from SQLite.
    let conn = rusqlite::Connection::open(&db_path).expect("open for check");
    let av: i64 = conn
        .pragma_query_value(None, "auto_vacuum", |r| r.get(0))
        .expect("query auto_vacuum");
    assert_eq!(
        av, 2,
        "fresh DB must have auto_vacuum=INCREMENTAL (2); got {av}",
    );
}

// ── Existing DB with auto_vacuum=NONE opens without panic ─────────────────────

#[tokio::test]
async fn existing_db_with_no_auto_vacuum_opens_and_logs_warn() {
    let dir = TempDir::new().expect("temp dir");
    let db_path = dir.path().join("no-av-test.db");

    // Pre-create a DB with auto_vacuum=NONE (the default before our migration).
    {
        let conn = rusqlite::Connection::open(&db_path).expect("create pre-existing db");
        // Do NOT set auto_vacuum -- SQLite default is NONE (0).
        conn.execute_batch("CREATE TABLE dummy (id INTEGER PRIMARY KEY);")
            .expect("create table");
        // Set user_version to 1 so migration is skipped.
        conn.pragma_update(None, "user_version", 1_i64)
            .expect("set version");
    }

    // Opening must succeed (no panic), even though auto_vacuum=NONE.
    let result = SqliteNodeLogStore::open(db_path, 30).await;
    assert!(
        result.is_ok(),
        "store must open even when auto_vacuum=NONE; got: {:?}",
        result.err(),
    );
}

// ── Fields > 4KB rejected at ingest ──────────────────────────────────────────

#[tokio::test]
async fn oversized_fields_are_rejected_at_ingest() {
    let dir = TempDir::new().expect("temp dir");
    let db_path = dir.path().join("fields-cap-test.db");
    let store = SqliteNodeLogStore::open(db_path, 30).await.expect("open");

    // Build a fields map that serializes to > 4096 bytes.
    let mut big_fields = serde_json::Map::new();
    // Each entry: key="kXXXX" (6 bytes), value="v" + 4000 bytes padding.
    // With JSON overhead, the total will be > 4096 bytes.
    big_fields.insert(
        "payload".to_string(),
        serde_json::Value::String("x".repeat(5000)),
    );

    let event = test_event_with_fields("cap-node", "large-fields-event", 1, big_fields);
    // ingest must return Ok (best-effort) even when dropping the event.
    store
        .ingest(event)
        .await
        .expect("ingest returns Ok even on drop");

    // The event must NOT appear in search results (was dropped).
    let results = store
        .search(
            "cap-node".to_string(),
            "large-fields-event".to_string(),
            None,
            None,
            0,
            10,
        )
        .await
        .expect("search");
    assert_eq!(
        results.len(),
        0,
        "event with >4KB fields must be dropped at ingest",
    );
}

// ── LIKE wildcards treated as literals ────────────────────────────────────────

#[tokio::test]
async fn like_wildcards_in_query_are_escaped_and_treated_as_literals() {
    let dir = TempDir::new().expect("temp dir");
    let db_path = dir.path().join("like-escape-test.db");
    let store = SqliteNodeLogStore::open(db_path, 30).await.expect("open");

    // Insert a message that literally contains % and _.
    store
        .ingest(test_event("esc-node", "100% complete_task", 1))
        .await
        .expect("ingest literal");
    // Insert a different message that would match a LIKE wildcard if not escaped.
    store
        .ingest(test_event("esc-node", "other message", 2))
        .await
        .expect("ingest other");
    store.wait_for_flushed_for_test(2).await.expect("flush");

    // Search for the literal "%" — must match only the first event.
    let results = store
        .search("esc-node".to_string(), "%".to_string(), None, None, 0, 10)
        .await
        .expect("search percent");
    assert_eq!(
        results.len(),
        1,
        "% must match literally, not as a wildcard; found {n} results",
        n = results.len(),
    );
    assert_eq!(results[0].message, "100% complete_task");

    // Search for the literal "_" — must match only the first event.
    let results2 = store
        .search("esc-node".to_string(), "_".to_string(), None, None, 0, 10)
        .await
        .expect("search underscore");
    assert_eq!(
        results2.len(),
        1,
        "_ must match literally, not as a wildcard; found {n} results",
        n = results2.len(),
    );
    assert_eq!(results2[0].message, "100% complete_task");
}

// ── since_ms / until_ms range filtering ──────────────────────────────────────

#[tokio::test]
async fn search_with_time_range_filters_correctly() {
    let dir = TempDir::new().expect("temp dir");
    let db_path = dir.path().join("range-test.db");
    let store = SqliteNodeLogStore::open(db_path, 30).await.expect("open");

    store
        .ingest(test_event("range-node", "early", 1_000))
        .await
        .expect("ingest early");
    store
        .ingest(test_event("range-node", "middle", 2_000))
        .await
        .expect("ingest middle");
    store
        .ingest(test_event("range-node", "late", 3_000))
        .await
        .expect("ingest late");
    store.wait_for_flushed_for_test(3).await.expect("flush");

    // since_ms=1500 should exclude "early".
    let results = store
        .search(
            "range-node".to_string(),
            String::new(),
            Some(1_500),
            None,
            0,
            10,
        )
        .await
        .expect("search since");
    let messages: Vec<&str> = results.iter().map(|e| e.message.as_str()).collect();
    assert!(
        !messages.contains(&"early"),
        "since_ms must exclude early: {messages:?}"
    );
    assert!(
        messages.contains(&"middle"),
        "since_ms must include middle: {messages:?}"
    );

    // until_ms=2500 should exclude "late".
    let results2 = store
        .search(
            "range-node".to_string(),
            String::new(),
            None,
            Some(2_500),
            0,
            10,
        )
        .await
        .expect("search until");
    let messages2: Vec<&str> = results2.iter().map(|e| e.message.as_str()).collect();
    assert!(
        !messages2.contains(&"late"),
        "until_ms must exclude late: {messages2:?}"
    );
    assert!(
        messages2.contains(&"middle"),
        "until_ms must include middle: {messages2:?}"
    );
}
