//! Generic timed-capability-call skeleton shared by `tools_call`, `resources_read`,
//! and `prompts_get`.
//!
//! The three capability modules each follow the same structure:
//!
//! 1. Record start time + build `UpstreamRequestLog`.
//! 2. Optionally acquire a subject-scoped peer (OAuth path) or the pool peer (normal path).
//! 3. Issue the RPC with `tokio::time::timeout`.
//! 4. On success: check the response size cap, record circuit-breaker success, log finish.
//! 5. On upstream error: record circuit-breaker failure, evict subject connection, log error.
//! 6. On timeout: record circuit-breaker failure, evict subject connection, log error.
//!
//! `timed_capability_call` encapsulates steps 3–6 so each capability module only
//! declares its own peer-acquisition and response-normalization logic.
//!
//! **Observable behaviour is byte-identical** to the previous per-file implementations:
//! same log fields, same error kinds, same timeout and size-cap semantics.

use std::future::Future;
use std::time::Instant;

use super::super::types::UpstreamCapability;
use super::UpstreamPool;
use super::helpers::max_response_bytes;
use super::logging::{UpstreamRequestLog, log_upstream_request_error, log_upstream_request_finish};

/// Outcome of a timed capability call before size-cap enforcement.
pub(super) enum RawCallOutcome<R> {
    Ok(R),
    /// The upstream returned an error (not a timeout).
    UpstreamError(rmcp::ServiceError),
    /// The tokio timeout elapsed.
    Timeout,
}

/// Helper to convert a `tokio::time::timeout` + `rmcp` result pair into
/// `RawCallOutcome`.
pub(super) fn classify_timeout_result<R>(
    result: Result<Result<R, rmcp::ServiceError>, tokio::time::error::Elapsed>,
) -> RawCallOutcome<R> {
    match result {
        Ok(Ok(r)) => RawCallOutcome::Ok(r),
        Ok(Err(e)) => RawCallOutcome::UpstreamError(e),
        Err(_) => RawCallOutcome::Timeout,
    }
}

/// Execute `rpc_future` under the pool's request timeout, enforce the
/// response-size cap (using `size_fn`), and emit structured log events.
///
/// # Parameters
///
/// - `pool` — used for circuit-breaker recording and `request_timeout`.
/// - `upstream_name` — name of the upstream, for logs and circuit-breaker keys.
/// - `capability` — which MCP capability is being exercised.
/// - `event` — pre-built `UpstreamRequestLog` (caller sets capability/item/transport).
/// - `start` — `Instant` recorded *before* peer acquisition (caller owns it so
///   elapsed time includes the peer-acquire step).
/// - `rpc_future` — the actual MCP call (`peer.call_tool(…)` / `peer.read_resource(…)` / …).
/// - `size_fn` — extracts the byte count from a successful response; use
///   `estimate_response_size` / `estimate_resource_response_size`.
/// - `subject` — `Some(subject)` when this is a subject-scoped OAuth call so that a
///   broken connection is evicted on error; `None` for the normal pool path.
/// - `error_message_fn` — builds the user-visible error string from the upstream
///   error display value.
/// - `timeout_message` — user-visible error string for the timeout case.
///
/// Returns `Ok(R)` on success, `Err(String)` for every failure kind.
#[allow(clippy::too_many_arguments)]
pub(super) async fn timed_capability_call<R, Fut, SizeFn>(
    pool: &UpstreamPool,
    upstream_name: &str,
    capability: UpstreamCapability,
    event: UpstreamRequestLog<'_>,
    start: Instant,
    rpc_future: Fut,
    size_fn: SizeFn,
    subject: Option<&str>,
    error_message_fn: impl Fn(&dyn std::fmt::Display) -> String,
    timeout_message: String,
) -> Result<R, String>
where
    Fut: Future<Output = Result<R, rmcp::ServiceError>>,
    SizeFn: Fn(&R) -> usize,
{
    let outcome =
        classify_timeout_result(tokio::time::timeout(pool.request_timeout, rpc_future).await);

    match outcome {
        RawCallOutcome::Ok(result) => {
            let response_size = size_fn(&result);
            let max_bytes = max_response_bytes();
            if response_size > max_bytes {
                pool.record_failure_for(
                    upstream_name,
                    capability,
                    format!("response too large: {response_size} bytes"),
                )
                .await;
                log_upstream_request_error(
                    event,
                    start.elapsed().as_millis(),
                    "response_too_large",
                    None,
                    Some(response_size),
                    Some(max_bytes),
                );
                return Err(format!(
                    "upstream response too large ({response_size} bytes, max {max_bytes})"
                ));
            }
            pool.record_success_for(upstream_name, capability).await;
            log_upstream_request_finish(event, start.elapsed().as_millis(), Some(response_size));
            Ok(result)
        }
        RawCallOutcome::UpstreamError(error) => {
            pool.record_failure_for(upstream_name, capability, error_message_fn(&error))
                .await;
            if let Some(subj) = subject {
                pool.evict_subject_connection(upstream_name, subj).await;
            }
            log_upstream_request_error(
                event,
                start.elapsed().as_millis(),
                "upstream_error",
                Some(&error),
                None,
                None,
            );
            Err(error_message_fn(&error))
        }
        RawCallOutcome::Timeout => {
            pool.record_failure_for(upstream_name, capability, timeout_message.clone())
                .await;
            if let Some(subj) = subject {
                pool.evict_subject_connection(upstream_name, subj).await;
            }
            log_upstream_request_error(
                event,
                start.elapsed().as_millis(),
                "timeout",
                None,
                None,
                None,
            );
            Err(timeout_message)
        }
    }
}
