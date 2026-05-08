//! Shared sync-guard state for the mcpregistry background supervisor and MCP dispatch.
//!
//! Both the hourly background supervisor (`cli/serve.rs`) and the on-demand MCP
//! `sync` action (`dispatch/marketplace/mcp_dispatch.rs`) must go through
//! `perform_sync` so that `SYNC_IN_PROGRESS` and `LAST_SYNC_AT` are visible
//! to both callers.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use lab_apis::mcpregistry::McpRegistryClient;

use crate::dispatch::error::ToolError;
use crate::dispatch::marketplace::store::RegistryStore;

/// Guards against concurrent syncs. `true` while a sync is in progress.
pub static SYNC_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Tracks when the last rate-limited sync attempt completed, regardless of success.
pub static LAST_SYNC_AT: OnceLock<std::sync::Mutex<Option<std::time::Instant>>> = OnceLock::new();

/// RAII guard: resets `SYNC_IN_PROGRESS` on drop, even on panic.
pub struct SyncGuard;

impl Drop for SyncGuard {
    fn drop(&mut self) {
        SYNC_IN_PROGRESS.store(false, Ordering::Release);
    }
}

/// Minimum interval between syncs (enforced for on-demand calls only).
const MIN_SYNC_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

fn last_sync_at() -> &'static std::sync::Mutex<Option<std::time::Instant>> {
    LAST_SYNC_AT.get_or_init(|| std::sync::Mutex::new(None))
}

/// Attempt a sync, enforcing the concurrent-sync and rate-limit guards.
///
/// - `rate_limit`: when `true`, rejects calls within `MIN_SYNC_INTERVAL` of
///   the last completed rate-limited sync attempt.
/// - Returns the count of rows synced on success.
pub async fn perform_sync(
    store: &RegistryStore,
    client: &McpRegistryClient,
    rate_limit: bool,
    trigger: &'static str,
) -> Result<usize, ToolError> {
    let started = std::time::Instant::now();
    let poll_interval_secs = sync_poll_interval_secs(rate_limit, trigger);
    if SYNC_IN_PROGRESS
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        let error = ToolError::Sdk {
            sdk_kind: "sync_in_progress".to_string(),
            message: "sync already in progress".to_string(),
        };
        tracing::warn!(
            surface = sync_surface(trigger),
            service = "marketplace",
            action = "mcp.sync",
            event = "sync.poll.skipped",
            trigger,
            poll_interval_secs,
            elapsed_ms = started.elapsed().as_millis(),
            kind = error.kind(),
            "marketplace registry sync skipped"
        );
        return Err(error);
    }
    let _guard = SyncGuard;

    tracing::info!(
        surface = sync_surface(trigger),
        service = "marketplace",
        action = "mcp.sync",
        event = "sync.poll.started",
        trigger,
        rate_limited = rate_limit,
        poll_interval_secs,
        "marketplace registry sync started"
    );

    if rate_limit {
        let guard = last_sync_at().lock().unwrap();
        if let Some(t) = *guard {
            if t.elapsed() < MIN_SYNC_INTERVAL {
                let remaining = MIN_SYNC_INTERVAL.saturating_sub(t.elapsed()).as_secs();
                let error = ToolError::Sdk {
                    sdk_kind: "rate_limited".to_string(),
                    message: format!("sync rate-limited; next allowed in {remaining}s"),
                };
                tracing::warn!(
                    surface = sync_surface(trigger),
                    service = "marketplace",
                    action = "mcp.sync",
                    event = "sync.poll.skipped",
                    trigger,
                    poll_interval_secs,
                    elapsed_ms = started.elapsed().as_millis(),
                    kind = error.kind(),
                    remaining_secs = remaining,
                    "marketplace registry sync rate limited"
                );
                return Err(error);
            }
        }
    }

    let sync_result = store
        .sync_from_upstream(client, trigger)
        .await
        .map_err(ToolError::from);

    if rate_limit {
        *last_sync_at().lock().unwrap() = Some(std::time::Instant::now());
    }

    match &sync_result {
        Ok(items_synced) => tracing::info!(
            surface = sync_surface(trigger),
            service = "marketplace",
            action = "mcp.sync",
            event = "sync.poll.finished",
            trigger,
            poll_interval_secs,
            elapsed_ms = started.elapsed().as_millis(),
            items_synced = *items_synced,
            error_count = 0usize,
            "marketplace registry sync finished"
        ),
        Err(error) if error.is_internal() => tracing::error!(
            surface = sync_surface(trigger),
            service = "marketplace",
            action = "mcp.sync",
            event = "sync.poll.failed",
            trigger,
            poll_interval_secs,
            elapsed_ms = started.elapsed().as_millis(),
            kind = error.kind(),
            items_synced = 0usize,
            error_count = 1usize,
            "marketplace registry sync failed"
        ),
        Err(error) => tracing::warn!(
            surface = sync_surface(trigger),
            service = "marketplace",
            action = "mcp.sync",
            event = "sync.poll.failed",
            trigger,
            poll_interval_secs,
            elapsed_ms = started.elapsed().as_millis(),
            kind = error.kind(),
            items_synced = 0usize,
            error_count = 1usize,
            "marketplace registry sync failed"
        ),
    }

    sync_result
}

fn sync_poll_interval_secs(rate_limit: bool, trigger: &str) -> u64 {
    if rate_limit {
        return MIN_SYNC_INTERVAL.as_secs();
    }
    match trigger {
        "startup" | "hourly" => 3600,
        _ => 0,
    }
}

fn sync_surface(trigger: &str) -> &'static str {
    match trigger {
        "manual" | "mcp.list-empty-store" => "mcp",
        _ => "background",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_observability_logs_interval_items_and_errors() {
        let source = include_str!("sync.rs");

        for required in [
            "event = \"sync.poll.started\"",
            "event = \"sync.poll.finished\"",
            "event = \"sync.poll.failed\"",
            "surface = sync_surface(trigger)",
            "poll_interval_secs",
            "items_synced",
            "error_count",
            "kind = error.kind()",
            "map_err(ToolError::from)",
        ] {
            assert!(source.contains(required), "missing {required}");
        }
    }

    #[test]
    fn sync_poll_interval_reports_background_and_rate_limited_intervals() {
        assert_eq!(sync_poll_interval_secs(true, "manual"), 60);
        assert_eq!(sync_poll_interval_secs(false, "hourly"), 3600);
        assert_eq!(sync_poll_interval_secs(false, "startup"), 3600);
        assert_eq!(sync_poll_interval_secs(false, "mcp.list-empty-store"), 0);
    }

    #[test]
    fn sync_surface_preserves_manual_and_background_callers() {
        assert_eq!(sync_surface("manual"), "mcp");
        assert_eq!(sync_surface("mcp.list-empty-store"), "mcp");
        assert_eq!(sync_surface("startup"), "background");
        assert_eq!(sync_surface("hourly"), "background");
    }
}
