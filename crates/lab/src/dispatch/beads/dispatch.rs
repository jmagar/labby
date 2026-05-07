use serde_json::Value;

use lab_apis::beads::BeadsClient;

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::{action_schema, help_payload, require_str, to_json};

use super::catalog::ACTIONS;
use super::client::require_client;
use super::params::{optional_project, optional_status, optional_u32};

pub async fn dispatch(action: &str, params: Value) -> Result<Value, ToolError> {
    match action {
        "help" => Ok(help_payload("beads", ACTIONS)),
        "schema" => {
            let action = require_str(&params, "action")?;
            action_schema(ACTIONS, action)
        }
        _ if !ACTIONS.iter().any(|a| a.name == action) => Err(unknown_action(action)),
        _ => dispatch_with_client(&require_client()?, action, params).await,
    }
}

pub async fn dispatch_with_client(
    client: &BeadsClient,
    action: &str,
    params: Value,
) -> Result<Value, ToolError> {
    match action {
        "help" => Ok(help_payload("beads", ACTIONS)),
        "schema" => {
            let action = require_str(&params, "action")?;
            action_schema(ACTIONS, action)
        }
        "contract.status" => to_json(client.contract_status()),
        "health.status" => to_json(client.health_status().await?),
        "version.get" => to_json(client.version().await?),
        "project.list" => to_json(client.databases().await?),
        "context.get" => {
            let project = optional_project(&params)?;
            to_json(client.context(project.as_deref()).await?)
        }
        "status.summary" => {
            let project = optional_project(&params)?;
            to_json(client.status_summary(project.as_deref()).await?)
        }
        "issue.list" => {
            let project = optional_project(&params)?;
            let status = optional_status(&params)?;
            let limit = optional_u32(&params, "limit")?;
            to_json(
                client
                    .list(project.as_deref(), status.as_deref(), limit)
                    .await?,
            )
        }
        "issue.ready" => {
            let project = optional_project(&params)?;
            let limit = optional_u32(&params, "limit")?;
            to_json(client.ready(project.as_deref(), limit).await?)
        }
        "issue.show" => {
            let project = optional_project(&params)?;
            let id = require_str(&params, "id")?;
            to_json(client.show(project.as_deref(), id).await?)
        }
        "graph.show" => {
            let project = optional_project(&params)?;
            let id = require_str(&params, "id")?;
            to_json(client.graph(project.as_deref(), id).await?)
        }
        unknown => Err(unknown_action(unknown)),
    }
}

fn unknown_action(action: &str) -> ToolError {
    ToolError::UnknownAction {
        message: format!("unknown action `{action}` for service `beads`"),
        valid: ACTIONS.iter().map(|a| a.name.to_string()).collect(),
        hint: None,
    }
}
