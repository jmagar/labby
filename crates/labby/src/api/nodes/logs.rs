use axum::{Json, extract::State};
use serde::Deserialize;

use crate::api::{error::ApiError, state::AppState};

use super::{fleet::require_master_store, normalize_node_id_value};

const MAX_LOG_SEARCH_LIMIT: usize = 1_000;

#[derive(Debug, Deserialize)]
pub struct SearchLogsRequest {
    pub node_id: String,
    pub query: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}

pub async fn search(
    State(state): State<AppState>,
    Json(payload): Json<SearchLogsRequest>,
) -> Result<Json<Vec<crate::node::log_event::NodeLogEvent>>, ApiError> {
    let node_id = normalize_node_id_value(&payload.node_id, "node_id")?;
    let store = require_master_store(&state)?;
    let needle = payload.query.to_ascii_lowercase();
    let offset = payload.offset.unwrap_or(0);
    let limit = payload.limit.unwrap_or(200).min(MAX_LOG_SEARCH_LIMIT);
    let events = store
        .search_logs_for_node(&node_id, &needle, offset, limit)
        .await
        .into_iter()
        .collect();
    Ok(Json(events))
}
