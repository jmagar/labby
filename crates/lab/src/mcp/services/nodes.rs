use lab_apis::core::action::{ActionSpec, ParamSpec};
use serde_json::{Value, json};

use crate::dispatch::error::ToolError;
use crate::node::enrollment::store::EnrollmentStore;

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        name: "help",
        description: "Show this action catalog",
        destructive: false,
        requires_admin: false,
        returns: "Catalog",
        params: &[],
    },
    ActionSpec {
        name: "schema",
        description: "Return the parameter schema for a named action",
        destructive: false,
        requires_admin: false,
        returns: "Schema",
        params: &[ParamSpec {
            name: "action",
            ty: "string",
            required: true,
            description: "Action name to describe",
        }],
    },
    ActionSpec {
        name: "enrollments.list",
        description: "List pending, approved, and denied node enrollments",
        destructive: false,
        requires_admin: false,
        returns: "Value",
        params: &[],
    },
    ActionSpec {
        name: "enrollments.approve",
        description: "Approve a pending node enrollment",
        destructive: true,
        requires_admin: false,
        returns: "Value",
        params: &[
            ParamSpec {
                name: "node_id",
                ty: "string",
                required: true,
                description: "Node ID to approve",
            },
            ParamSpec {
                name: "note",
                ty: "string",
                required: false,
                description: "Optional approval note",
            },
        ],
    },
    ActionSpec {
        name: "enrollments.deny",
        description: "Deny a pending or approved node enrollment",
        destructive: true,
        requires_admin: false,
        returns: "Value",
        params: &[
            ParamSpec {
                name: "node_id",
                ty: "string",
                required: true,
                description: "Node ID to deny",
            },
            ParamSpec {
                name: "reason",
                ty: "string",
                required: false,
                description: "Optional denial reason",
            },
        ],
    },
];

pub async fn dispatch(action: &str, params: Value) -> Result<Value, ToolError> {
    let store = EnrollmentStore::open(crate::node::enrollment::store::default_store_path())
        .await
        .map_err(|error| ToolError::internal_message(format!("open enrollment store: {error}")))?;

    match action {
        "help" => Ok(actions_json()),
        "schema" => {
            let action_name = params
                .get("action")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::MissingParam {
                    message: "missing required param `action`".to_string(),
                    param: "action".to_string(),
                })?;
            let schema = ACTIONS
                .iter()
                .find(|spec| spec.name == action_name)
                .ok_or_else(|| ToolError::UnknownAction {
                    message: format!("unknown action `{action_name}`"),
                    valid: ACTIONS.iter().map(|spec| spec.name.to_string()).collect(),
                    hint: None,
                })?;
            Ok(action_json(schema))
        }
        "enrollments.list" => {
            serde_json::to_value(store.list().await.map_err(|error| {
                ToolError::internal_message(format!("list enrollments: {error}"))
            })?)
            .map_err(|error| ToolError::internal_message(format!("serialize enrollments: {error}")))
        }
        "enrollments.approve" => {
            let node_id = params
                .get("node_id")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::MissingParam {
                    message: "missing required param `node_id`".to_string(),
                    param: "node_id".to_string(),
                })?;
            let note = params
                .get("note")
                .and_then(Value::as_str)
                .map(str::to_string);
            let approved = match store.approve(node_id, note).await {
                Ok(approved) => {
                    tracing::info!(
                        surface = "mcp",
                        service = "nodes",
                        action = "enrollments.approve",
                        actor = "mcp_client",
                        outcome = "success",
                        entity_kind = "node",
                        entity_id = %approved.node_id,
                        token_fingerprint = %approved.token_fingerprint,
                        "node enrollment approved via MCP",
                    );
                    approved
                }
                Err(error) => {
                    tracing::warn!(
                        surface = "mcp",
                        service = "nodes",
                        action = "enrollments.approve",
                        actor = "mcp_client",
                        outcome = "failure",
                        kind = "not_found",
                        entity_kind = "node",
                        entity_id = %node_id,
                        "node enrollment approval via MCP failed",
                    );
                    return Err(ToolError::Sdk {
                        sdk_kind: "not_found".to_string(),
                        message: error.to_string(),
                    });
                }
            };
            serde_json::to_value(approved).map_err(|error| {
                ToolError::internal_message(format!("serialize approved enrollment: {error}"))
            })
        }
        "enrollments.deny" => {
            let node_id = params
                .get("node_id")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::MissingParam {
                    message: "missing required param `node_id`".to_string(),
                    param: "node_id".to_string(),
                })?;
            let reason = params
                .get("reason")
                .and_then(Value::as_str)
                .map(str::to_string);
            let denied = match store.deny(node_id, reason).await {
                Ok(denied) => {
                    tracing::info!(
                        surface = "mcp",
                        service = "nodes",
                        action = "enrollments.deny",
                        actor = "mcp_client",
                        outcome = "success",
                        entity_kind = "node",
                        entity_id = %denied.node_id,
                        token_fingerprint = %denied.token_fingerprint,
                        "node enrollment denied via MCP",
                    );
                    denied
                }
                Err(error) => {
                    tracing::warn!(
                        surface = "mcp",
                        service = "nodes",
                        action = "enrollments.deny",
                        actor = "mcp_client",
                        outcome = "failure",
                        kind = "not_found",
                        entity_kind = "node",
                        entity_id = %node_id,
                        "node enrollment denial via MCP failed",
                    );
                    return Err(ToolError::Sdk {
                        sdk_kind: "not_found".to_string(),
                        message: error.to_string(),
                    });
                }
            };
            serde_json::to_value(denied).map_err(|error| {
                ToolError::internal_message(format!("serialize denied enrollment: {error}"))
            })
        }
        other => Err(ToolError::UnknownAction {
            message: format!("unknown action `{other}`"),
            valid: ACTIONS.iter().map(|spec| spec.name.to_string()).collect(),
            hint: None,
        }),
    }
}

fn actions_json() -> Value {
    Value::Array(ACTIONS.iter().map(action_json).collect())
}

fn action_json(spec: &ActionSpec) -> Value {
    json!({
        "name": spec.name,
        "description": spec.description,
        "destructive": spec.destructive,
        "returns": spec.returns,
        "params": spec.params.iter().map(|param| {
            json!({
                "name": param.name,
                "ty": param.ty,
                "required": param.required,
                "description": param.description,
            })
        }).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::enrollment::store::{EnrollmentAttempt, TailnetIdentity};

    #[tokio::test]
    async fn nodes_mcp_dispatch_supports_enrollment_actions() {
        let path = crate::node::enrollment::store::default_store_path();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.expect("mkdir");
        }
        let store = EnrollmentStore::open(path).await.expect("open");
        store
            .record_pending(EnrollmentAttempt {
                node_id: "device-1".to_string(),
                token: "token-1".to_string(),
                tailnet_identity: TailnetIdentity {
                    node_key: "node-key".to_string(),
                    login_name: "user@example.com".to_string(),
                    hostname: "device-1".to_string(),
                },
                client_version: "0.7.3".to_string(),
                metadata: None,
            })
            .await
            .expect("record");

        let listed = dispatch("enrollments.list", json!({})).await.expect("list");
        assert!(listed["pending"]["device-1"].is_object());

        let approved = dispatch("enrollments.approve", json!({"node_id": "device-1"}))
            .await
            .expect("approve");
        assert_eq!(approved["node_id"], "device-1");

        let denied = dispatch("enrollments.deny", json!({"node_id": "device-1"}))
            .await
            .expect("deny");
        assert_eq!(denied["node_id"], "device-1");
    }
}
