//! MCP adapter for the `deploy` service — thin bridge to
//! `dispatch::deploy`.
//!
//! Destructive deploy actions require live MCP elicitation. The `entry`
//! helper maps the `elicited` signal from the MCP server into the
//! `McpContext` enum read by `authz::reject_headless_bypass`.

use std::future::Future;
use std::pin::Pin;

use serde_json::Value;

use crate::dispatch::deploy;
use crate::dispatch::deploy::authz::McpContext;
use crate::dispatch::deploy::runner::DefaultRunner;
use crate::dispatch::error::ToolError;

pub const ACTIONS: &[labby_apis::core::action::ActionSpec] = deploy::catalog::ACTIONS;

/// Dispatch using the process-global `DefaultRunner` (built once from
/// on-disk config and `~/.ssh/config`). The MCP context is set to
/// `McpElicited` — callers that reach destructive actions (`run`,
/// `rollback`) via MCP are expected to have completed an elicitation
/// exchange at the protocol layer before the tool is invoked.
///
/// Returns a `Pin<Box<dyn Future + Send + 'static>>` so the `dispatch_fn!`
/// macro's outer `Box::pin` wraps an already-Send future, avoiding the
/// higher-ranked trait bound (HRTB) Send limitation with `async fn` futures
/// that capture lifetime-parameterised references (Rust issue #100013).
pub fn dispatch(
    action: &str,
    params: Value,
) -> Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send + 'static>> {
    let runner: &'static DefaultRunner = deploy::client::static_runner();
    let action = action.to_owned();
    // The scope must wrap the *call* to dispatch_mcp, not just its returned
    // future. dispatch_mcp does synchronous validation (authz context check)
    // before constructing the async block, so the task-local must be set
    // before dispatch_mcp's body runs (Rust issue #100013 workaround — the
    // sync pre-work is done inside the scope here, not outside it).
    Box::pin(async move {
        deploy::authz::MCP_CONTEXT
            .scope(McpContext::McpElicited, async move {
                deploy::dispatch_mcp(action, params, runner).await
            })
            .await
    })
}

/// Dispatch with an explicit MCP context (used by tests and by the MCP
/// server after it has negotiated elicitation capability).
#[allow(dead_code)]
pub async fn dispatch_with_context(
    action: &str,
    params: Value,
    ctx: McpContext,
) -> Result<Value, ToolError> {
    let runner: &'static DefaultRunner = deploy::client::static_runner();
    deploy::authz::MCP_CONTEXT
        .scope(ctx, deploy::dispatch_mcp(action.to_owned(), params, runner))
        .await
}
