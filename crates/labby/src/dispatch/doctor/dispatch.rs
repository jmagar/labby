//! Action router for the doctor dispatch service.

use std::sync::Arc;

use serde_json::Value;

use crate::dispatch::clients::ServiceClients;
use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::{action_schema, help_payload, to_json};

use super::catalog::ACTIONS;
use super::gateway;
use super::params::{parse_proxy_check, parse_service_probe};
use super::proxy;
use super::service;
use super::system;
use super::types::Report;

/// Standard MCP-path dispatch: builds `ServiceClients` from env on demand.
pub async fn dispatch(action: &str, params: Value) -> Result<Value, ToolError> {
    match action {
        "help" => return Ok(help_payload("doctor", ACTIONS)),
        "schema" => {
            let a = crate::dispatch::helpers::require_str(&params, "action")?;
            return action_schema(ACTIONS, a);
        }
        "system.checks" => {
            let findings = tokio::task::spawn_blocking(system::run_system_checks)
                .await
                .map_err(|e| ToolError::Sdk {
                    sdk_kind: "internal_error".to_string(),
                    message: format!("system.checks task panicked: {e}"),
                })?;
            return to_json(Report { findings });
        }
        "auth.check" => {
            let findings = tokio::task::spawn_blocking(system::run_auth_checks)
                .await
                .map_err(|e| ToolError::Sdk {
                    sdk_kind: "internal_error".to_string(),
                    message: format!("auth.check task panicked: {e}"),
                })?;
            return to_json(Report { findings });
        }
        "gateway.upstreams" => {
            return to_json(gateway::check_gateway_upstreams().await);
        }
        "proxy.check" => {
            let p = parse_proxy_check(&params)?;
            return to_json(proxy::check_proxy(p).await?);
        }
        a if !ACTIONS.iter().any(|s| s.name == a) => {
            return Err(ToolError::UnknownAction {
                message: format!("unknown action `{action}` for service `doctor`"),
                valid: ACTIONS.iter().map(|s| s.name.to_string()).collect(),
                hint: None,
            });
        }
        _ => {}
    }
    // Actions below require ServiceClients — build from env.
    let clients = Arc::new(ServiceClients::from_env());
    dispatch_with_clients(&clients, action, params).await
}

/// API-path dispatch: uses pre-built `ServiceClients` from `AppState`.
pub async fn dispatch_with_clients(
    clients: &Arc<ServiceClients>,
    action: &str,
    params: Value,
) -> Result<Value, ToolError> {
    let start = std::time::Instant::now();
    tracing::info!(
        surface = "dispatch",
        service = "doctor",
        action,
        "dispatch start"
    );

    let result = match action {
        "help" => Ok(help_payload("doctor", ACTIONS)),
        "schema" => {
            let a = crate::dispatch::helpers::require_str(&params, "action")?;
            action_schema(ACTIONS, a)
        }
        "system.checks" => match tokio::task::spawn_blocking(system::run_system_checks).await {
            Ok(findings) => to_json(Report { findings }),
            Err(e) => Err(ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: format!("system.checks task panicked: {e}"),
            }),
        },
        "auth.check" => match tokio::task::spawn_blocking(system::run_auth_checks).await {
            Ok(findings) => to_json(Report { findings }),
            Err(e) => Err(ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: format!("auth.check task panicked: {e}"),
            }),
        },
        "gateway.upstreams" => to_json(gateway::check_gateway_upstreams().await),
        "proxy.check" => {
            let p = parse_proxy_check(&params)?;
            to_json(proxy::check_proxy(p).await?)
        }
        "service.probe" => {
            let p = parse_service_probe(&params)?;
            let finding = service::probe_service(clients, p.service, p.instance).await?;
            to_json(finding)
        }
        "audit.full" => {
            // Non-streaming path: collect all findings and return at once.
            // Streaming is handled by `api/services/doctor.rs` SSE endpoint.
            // gateway.upstreams is included so the full audit surfaces pool state.
            let (tx, mut rx) = tokio::sync::mpsc::channel(64);
            let clients = clients.clone();
            tokio::spawn(async move {
                service::stream_audit_full(clients, tx).await;
            });
            let mut findings = Vec::new();
            while let Some(f) = rx.recv().await {
                findings.push(f);
            }
            // Append gateway upstream findings to audit.full.
            let gw_report = gateway::check_gateway_upstreams().await;
            findings.extend(gw_report.findings);
            to_json(Report { findings })
        }
        unknown => Err(ToolError::UnknownAction {
            message: format!("unknown action `{unknown}` for service `doctor`"),
            valid: ACTIONS.iter().map(|s| s.name.to_string()).collect(),
            hint: None,
        }),
    };

    let elapsed_ms = start.elapsed().as_millis();
    match &result {
        Ok(_) => tracing::info!(
            surface = "dispatch",
            service = "doctor",
            action,
            elapsed_ms,
            "dispatch ok"
        ),
        Err(e) if e.is_internal() => tracing::error!(
            surface = "dispatch",
            service = "doctor",
            action,
            elapsed_ms,
            kind = e.kind(),
            "dispatch error"
        ),
        Err(e) => tracing::warn!(
            surface = "dispatch",
            service = "doctor",
            action,
            elapsed_ms,
            kind = e.kind(),
            "dispatch warn"
        ),
    }
    result
}
