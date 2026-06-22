use std::time::Instant;

use axum::{Json, extract::State, http::HeaderMap};
use serde::Deserialize;

use crate::api::{ToolError, error::ApiError, nodes::NodeAck, state::AppState};
use crate::node::log_event::NodeLogEvent;

use super::normalize_node_id_value;

#[derive(Debug, Deserialize)]
pub struct NodeSyslogBatch {
    pub node_id: String,
    pub events: Vec<NodeLogEvent>,
}

pub async fn handle_batch(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(mut payload): Json<NodeSyslogBatch>,
) -> Result<Json<NodeAck>, ApiError> {
    let start = Instant::now();
    let node_id = normalize_node_id_value(&payload.node_id, "node_id")?;
    for (index, event) in payload.events.iter_mut().enumerate() {
        let event_id =
            normalize_node_id_value(&event.node_id, &format!("events[{index}].node_id"))?;
        if event_id != node_id {
            return Err(ApiError(ToolError::InvalidParam {
                message: format!("events[{index}].node_id must match batch node_id `{node_id}`"),
                param: format!("events[{index}].node_id"),
            }));
        }
        event.node_id = node_id.clone();
    }

    let event_count = payload.events.len();
    let request_id = headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok());
    let store = state
        .node_store
        .clone()
        .ok_or_else(|| ToolError::internal_message("node store is not configured"))?;
    store.record_logs(&node_id, payload.events).await;
    tracing::info!(
        surface = "api",
        service = "nodes",
        action = "syslog.batch",
        request_id,
        node_id = %node_id,
        event_count,
        elapsed_ms = start.elapsed().as_millis(),
        "node syslog batch recorded"
    );
    Ok(super::ok())
}
