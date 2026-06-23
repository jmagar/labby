use axum::{Json, extract::State};

use crate::api::{ToolError, error::ApiError, nodes::NodeAck, state::AppState};
use crate::node::checkin::NodeStatus;

pub async fn handle(
    State(state): State<AppState>,
    Json(mut payload): Json<NodeStatus>,
) -> Result<Json<NodeAck>, ApiError> {
    payload.node_id = super::normalize_node_id_value(&payload.node_id, "node_id")?;
    let store = state
        .node_store
        .clone()
        .ok_or_else(|| ToolError::internal_message("node store is not configured"))?;
    store.record_status(payload).await;
    Ok(super::ok())
}
