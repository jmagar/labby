use rmcp::RoleServer;
use rmcp::model::{
    CreateElicitationRequestParams, ElicitationAction, ElicitationSchema, PrimitiveSchema,
};
use rmcp::service::RequestContext;
use serde_json::Value;

pub(crate) enum ElicitResult {
    /// User confirmed the destructive action.
    Confirmed,
    /// User explicitly declined.
    Declined,
    /// User cancelled (closed the dialog without choosing).
    Cancelled,
    /// MCP client does not support the elicitation capability.
    NotSupported,
    /// The client advertised elicitation support, but the RPC failed.
    Failed,
}

pub(crate) async fn elicit_confirm(
    context: &RequestContext<RoleServer>,
    service: &str,
    action: &str,
) -> ElicitResult {
    if context.peer.supported_elicitation_modes().is_empty() {
        tracing::warn!(
            surface = "mcp",
            service,
            action,
            actor = "mcp_client",
            outcome = "not_supported",
            entity_kind = "destructive_action",
            entity_id = %format!("{service}.{action}"),
            kind = "confirmation_required",
            "destructive action elicitation not supported",
        );
        return ElicitResult::NotSupported;
    }

    let Ok(schema) = ElicitationSchema::builder()
        .required_property(
            "confirm",
            PrimitiveSchema::Boolean(rmcp::model::BooleanSchema::default()),
        )
        .build()
    else {
        tracing::warn!(
            surface = "mcp",
            service,
            action,
            actor = "lab",
            outcome = "schema_failed",
            entity_kind = "destructive_action",
            entity_id = %format!("{service}.{action}"),
            kind = "internal_error",
            "destructive action elicitation schema build failed",
        );
        return ElicitResult::NotSupported;
    };

    let params = CreateElicitationRequestParams::FormElicitationParams {
        meta: None,
        message: format!(
            "Action `{service}.{action}` is destructive and cannot be undone. \
             Set `confirm` to true to proceed."
        ),
        requested_schema: schema,
    };

    match context.peer.create_elicitation(params).await {
        Ok(result) => match result.action {
            ElicitationAction::Accept => {
                let confirmed = result
                    .content
                    .as_ref()
                    .and_then(|v| v.get("confirm"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if confirmed {
                    tracing::info!(
                        surface = "mcp",
                        service,
                        action,
                        actor = "mcp_client",
                        outcome = "confirmed",
                        entity_kind = "destructive_action",
                        entity_id = %format!("{service}.{action}"),
                        "destructive action elicitation confirmed",
                    );
                    ElicitResult::Confirmed
                } else {
                    tracing::warn!(
                        surface = "mcp",
                        service,
                        action,
                        actor = "mcp_client",
                        outcome = "declined",
                        entity_kind = "destructive_action",
                        entity_id = %format!("{service}.{action}"),
                        kind = "confirmation_required",
                        "destructive action elicitation accepted without confirmation",
                    );
                    ElicitResult::Declined
                }
            }
            ElicitationAction::Decline => {
                tracing::warn!(
                    surface = "mcp",
                    service,
                    action,
                    actor = "mcp_client",
                    outcome = "declined",
                    entity_kind = "destructive_action",
                    entity_id = %format!("{service}.{action}"),
                    kind = "confirmation_required",
                    "destructive action elicitation declined",
                );
                ElicitResult::Declined
            }
            ElicitationAction::Cancel => {
                tracing::warn!(
                    surface = "mcp",
                    service,
                    action,
                    actor = "mcp_client",
                    outcome = "cancelled",
                    entity_kind = "destructive_action",
                    entity_id = %format!("{service}.{action}"),
                    kind = "confirmation_required",
                    "destructive action elicitation cancelled",
                );
                ElicitResult::Cancelled
            }
        },
        Err(_) => {
            tracing::warn!(
                surface = "mcp",
                service,
                action,
                actor = "mcp_client",
                outcome = "failed",
                entity_kind = "destructive_action",
                entity_id = %format!("{service}.{action}"),
                kind = "confirmation_required",
                "destructive action elicitation request failed",
            );
            ElicitResult::Failed
        }
    }
}
