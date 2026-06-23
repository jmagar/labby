//! `AcpPersistence` trait — persistence contract for ACP sessions and events.
//!
//! The trait itself has no SQLite dependency; it is safe to use from `lab-apis`.
//! The SQLite implementation lives in `crates/lab/src/dispatch/acp/persistence.rs`.

use std::future::Future;

use crate::acp::error::AcpError;
use crate::acp::types::{AcpEvent, AcpSessionState, AcpSessionSummary};

/// Persistence contract for ACP sessions, events, and permission requests.
///
/// Implementors must be `Send + Sync + Clone + 'static` so they can be stored
/// in `Arc`-wrapped state and shared across tokio tasks.
///
/// # Error model
/// All methods return `AcpError`, which wraps `PersistenceError` for
/// storage-layer failures and `serde_json::Error` for serialisation failures.
pub trait AcpPersistence: Send + Sync + Clone + 'static {
    /// Return all session summaries, ordered by `updated_at` descending.
    fn load_sessions(
        &self,
    ) -> impl Future<Output = Result<Vec<AcpSessionSummary>, AcpError>> + Send;

    /// Return all events for the given session ordered by `seq` ascending.
    fn load_events(
        &self,
        session_id: &str,
    ) -> impl Future<Output = Result<Vec<AcpEvent>, AcpError>> + Send;

    /// Return events with `seq > since_seq` for the given session, ordered by
    /// `seq` ascending. Useful for resuming an SSE stream without re-loading
    /// the full event log.
    fn load_events_since(
        &self,
        session_id: &str,
        since_seq: u64,
    ) -> impl Future<Output = Result<Vec<AcpEvent>, AcpError>> + Send;

    /// Return at most `limit` events with `seq > since_seq` for the given
    /// session, preserving "last N" semantics — when more events than `limit`
    /// would match, the most recent `limit` events are returned, ordered by
    /// `seq` ascending. Implementations must apply the cap at the storage
    /// layer (e.g. `LIMIT` in SQL) rather than truncating in memory; the
    /// purpose of this method is to avoid materialising the full event range
    /// for large sessions during SSE backfill.
    fn load_events_since_capped(
        &self,
        session_id: &str,
        since_seq: u64,
        limit: u64,
    ) -> impl Future<Output = Result<Vec<AcpEvent>, AcpError>> + Send;

    /// Upsert (INSERT OR REPLACE) a session summary row.
    fn save_session(
        &self,
        summary: &AcpSessionSummary,
    ) -> impl Future<Output = Result<(), AcpError>> + Send;

    /// Append a single event to the event log for its session.
    ///
    /// Implementations are encouraged to batch consecutive `append_event`
    /// calls (e.g., via an mpsc writer task) rather than issuing one INSERT
    /// per call on the hot path.
    fn append_event(&self, event: &AcpEvent) -> impl Future<Output = Result<(), AcpError>> + Send;

    /// Atomically update the `state` and `updated_at` columns for a session.
    fn update_session_state(
        &self,
        session_id: &str,
        state: AcpSessionState,
    ) -> impl Future<Output = Result<(), AcpError>> + Send;

    /// Return the maximum persisted event sequence number for every session.
    ///
    /// Sessions with no events are not included in the map; callers should
    /// treat a missing entry as `0` and seed `next_seq = max + 1`.
    fn load_max_seqs(
        &self,
    ) -> impl Future<Output = Result<std::collections::HashMap<String, u64>, AcpError>> + Send;
}
