//! Top-level action router for the `logs` dispatch service.

use serde_json::Value;

use super::catalog::ACTIONS;
use super::client;
use super::params::{parse_metrics_params, parse_search_params, parse_tail_params};
use super::types::LogSystem;
use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::{action_schema, help_payload, require_str, to_json};

pub async fn dispatch(action: &str, params: Value) -> Result<Value, ToolError> {
    match action {
        "help" => Ok(help_payload("logs", ACTIONS)),
        "schema" => {
            let a = require_str(&params, "action")?;
            action_schema(ACTIONS, a)
        }
        other => {
            // Validate the action name before requiring an installed system,
            // so `unknown_action` surfaces even when no LogSystem is available.
            if !ACTIONS.iter().any(|a| a.name == other) {
                return Err(ToolError::UnknownAction {
                    message: format!("unknown action `{other}` for service `logs`"),
                    valid: ACTIONS.iter().map(|a| a.name.to_string()).collect(),
                    hint: None,
                });
            }
            let system = client::require_system()?;
            dispatch_with_system(&system, other, params).await
        }
    }
}

pub async fn dispatch_with_system(
    system: &LogSystem,
    action: &str,
    params: Value,
) -> Result<Value, ToolError> {
    match action {
        "help" => Ok(help_payload("logs", ACTIONS)),
        "schema" => {
            let a = require_str(&params, "action")?;
            action_schema(ACTIONS, a)
        }
        "logs.search" => to_json(system.search(parse_search_params(params)?).await?),
        "logs.tail" => to_json(system.tail(parse_tail_params(params)?).await?),
        "logs.stats" => to_json(system.stats().await?),
        "logs.metrics" => to_json(system.metrics(parse_metrics_params(params)?).await?),
        "logs.stream" => Err(ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message: "live push is HTTP SSE only; connect to GET /v1/logs/stream to receive events"
                .to_string(),
        }),
        unknown => Err(ToolError::UnknownAction {
            message: format!("unknown action `{unknown}` for service `logs`"),
            valid: ACTIONS.iter().map(|a| a.name.to_string()).collect(),
            hint: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn help_returns_catalog_object() {
        let v = dispatch("help", serde_json::json!({})).await.unwrap();
        assert!(v.is_object());
        assert_eq!(v["service"], "logs");
    }

    #[tokio::test]
    async fn schema_returns_action_shape() {
        let v = dispatch("schema", serde_json::json!({"action": "logs.search"}))
            .await
            .unwrap();
        assert!(v.is_object());
    }

    #[tokio::test]
    async fn unknown_action_returns_kind() {
        let e = dispatch("logs.serch", serde_json::json!({}))
            .await
            .unwrap_err();
        assert_eq!(e.kind(), "unknown_action");
    }
}
