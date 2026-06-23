//! Action routing for the deploy service.

use std::future::Future;
use std::pin::Pin;

use super::authz;
use super::catalog::ACTIONS;
use super::params;
use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::{action_schema, help_payload, require_str, to_json};
use labby_apis::deploy::{DeployError, DeployRequest};
use serde_json::Value;

/// Validate auth, parse params, and enforce the confirm flag for the `run`
/// and `rollback` actions.
///
/// This is a **synchronous** helper intentionally — all work runs before any
/// `Box::pin(async move { … })` block is constructed, keeping the lifetimes
/// `'static`-clean and avoiding HRTB errors (Rust issue #100013).
///
/// Steps:
/// 1. `require_deploy_token` — dedicated deploy token gate.
/// 2. `parse_run` — coerce and validate params into a `DeployRequest`.
/// 3. `reject_headless_bypass` — refuse headless `confirm: true` over MCP.
/// 4. Confirm-flag check — `confirm` must be `true` for destructive actions.
fn validate_deploy_action(action: &str, params_v: &Value) -> Result<DeployRequest, ToolError> {
    authz::require_deploy_token()?;
    let mut req = params::parse_run(params_v).map_err(ToolError::from)?;
    let ctx = authz::current_context();
    authz::reject_headless_bypass(params_v, ctx)?;
    // After a successful elicitation exchange the client has confirmed the
    // action interactively — mark the request confirmed so the gate below
    // passes even when the caller did not include "confirm": true in params.
    if matches!(ctx, authz::McpContext::McpElicited) {
        req.confirm = true;
    }
    if !req.confirm {
        return Err(DeployError::ValidationFailed {
            field: "confirm".into(),
            reason: format!("destructive deploy.{action} requires confirm=true"),
        }
        .into());
    }
    Ok(req)
}

/// Top-level dispatch without an attached runner — handles `help` / `schema`
/// and returns `internal_error` for any action that requires the runner.
///
/// Follows the standard dispatch.rs contract: `help` and `schema` work
/// unconditionally; every other action signals that it needs a runner without
/// running auth or param validation (authentication is the runner's concern).
#[allow(dead_code)]
pub async fn dispatch(action: &str, params_v: Value) -> Result<Value, ToolError> {
    match action {
        "help" => Ok(help_payload("deploy", ACTIONS)),
        "schema" => {
            let a = require_str(&params_v, "action")?;
            action_schema(ACTIONS, a)
        }
        other => {
            if !ACTIONS.iter().any(|a| a.name == other) {
                return Err(ToolError::UnknownAction {
                    message: format!("unknown action `{other}` for service `deploy`"),
                    valid: ACTIONS.iter().map(|a| a.name.to_string()).collect(),
                    hint: None,
                });
            }
            Err(ToolError::internal_message(
                "deploy actions require a runner; use dispatch_with_runner or dispatch_mcp",
            ))
        }
    }
}

// `dispatch_mcp` and `dispatch_with_runner` cannot be merged into a single
// function because they have incompatible calling conventions:
// - `dispatch_mcp` returns `Pin<Box<dyn Future + 'static>>` to satisfy the
//   `dispatch_fn!` macro's `Box::pin` wrapper without HRTB errors.
// - `dispatch_with_runner` is an `async fn` that can hold a non-`'static`
//   runner reference.
// Both share auth/validation logic via `validate_deploy_action` to prevent
// drift. Any new check or feature must go in that helper, not either function.

/// MCP-specific entry point: sync fn returning a `'static` boxed future so
/// the `dispatch_fn!` macro's `Box::pin` wrapper encloses a future with no
/// lifetime-parameterised captures (Rust issue #100013).
///
/// All synchronous work (auth, param parsing, bypass check) runs before the
/// returned future is created. The `async move` blocks capture only owned
/// values and `runner: &'static DefaultRunner`.
pub fn dispatch_mcp(
    action: String,
    params_v: Value,
    runner: &'static super::runner::DefaultRunner,
) -> Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send + 'static>> {
    match action.as_str() {
        "help" => {
            let v = help_payload("deploy", ACTIONS);
            Box::pin(async move { Ok(v) })
        }
        "schema" => {
            let result = require_str(&params_v, "action").and_then(|a| action_schema(ACTIONS, a));
            Box::pin(async move { result })
        }
        "config.list" => {
            let result = authz::require_deploy_token().and_then(|_| runner.config_list_impl());
            Box::pin(async move { result })
        }
        // `deploy.plan` is canonical; bare `plan` is a deprecated alias (Arch-M3).
        "deploy.plan" | "plan" => {
            let auth = authz::require_deploy_token();
            let req = auth.and_then(|_| params::parse_run(&params_v).map_err(ToolError::from));
            Box::pin(async move {
                let req = req?;
                to_json(runner.plan_impl(req).await?)
            })
        }
        // `deploy.run` is canonical; bare `run` is a deprecated alias (Arch-M3).
        "deploy.run" | "run" => {
            let result = validate_deploy_action("run", &params_v);
            Box::pin(async move {
                let req = result?;
                to_json(runner.run_impl(req).await?)
            })
        }
        // `deploy.rollback` is canonical; bare `rollback` is a deprecated alias (Arch-M3).
        "deploy.rollback" | "rollback" => {
            let result = validate_deploy_action("rollback", &params_v);
            Box::pin(async move {
                let req = result?;
                to_json(runner.rollback_impl(req).await?)
            })
        }
        other => {
            let err = Err(ToolError::UnknownAction {
                message: format!("unknown action `{other}` for service `deploy`"),
                valid: ACTIONS.iter().map(|a| a.name.to_string()).collect(),
                hint: None,
            });
            Box::pin(async move { err })
        }
    }
}

/// Dispatch against a concrete `DefaultRunner`. This is the entry point the
/// CLI surface goes through once startup has built the runner from config.
///
/// Differs from `dispatch_mcp` in calling convention: this is an `async fn`
/// that can take a non-`'static` `runner` reference, while `dispatch_mcp`
/// returns a `Pin<Box<dyn Future + 'static>>` to satisfy the MCP macro's
/// `Box::pin` wrapper without HRTB (Rust issue #100013). Both functions share
/// the same auth/validation logic via `validate_deploy_action`.
pub async fn dispatch_with_runner(
    action: &str,
    params_v: Value,
    runner: &super::runner::DefaultRunner,
) -> Result<Value, ToolError> {
    match action {
        "help" => Ok(help_payload("deploy", ACTIONS)),
        "schema" => {
            let a = require_str(&params_v, "action")?;
            action_schema(ACTIONS, a)
        }
        "config.list" => {
            authz::require_deploy_token()?;
            runner.config_list_impl()
        }
        // `deploy.plan` is canonical; bare `plan` is a deprecated alias (Arch-M3).
        "deploy.plan" | "plan" => {
            authz::require_deploy_token()?;
            let req = params::parse_run(&params_v).map_err(ToolError::from)?;
            to_json(runner.plan_impl(req).await?)
        }
        // `deploy.run` is canonical; bare `run` is a deprecated alias (Arch-M3).
        "deploy.run" | "run" => {
            let req = validate_deploy_action("run", &params_v)?;
            to_json(runner.run_impl(req).await?)
        }
        // `deploy.rollback` is canonical; bare `rollback` is a deprecated alias (Arch-M3).
        "deploy.rollback" | "rollback" => {
            let req = validate_deploy_action("rollback", &params_v)?;
            to_json(runner.rollback_impl(req).await?)
        }
        other => Err(ToolError::UnknownAction {
            message: format!("unknown action `{other}` for service `deploy`"),
            valid: ACTIONS.iter().map(|a| a.name.to_string()).collect(),
            hint: None,
        }),
    }
}
