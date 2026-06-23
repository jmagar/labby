use axum::{
    Json,
    extract::{Path, State},
};
use serde::Deserialize;

use crate::api::{ToolError, error::ApiError, state::AppState};

use super::normalize_node_id_value;

#[derive(Debug, Deserialize, Default)]
pub struct ApproveEnrollmentRequest {
    pub note: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct DenyEnrollmentRequest {
    pub reason: Option<String>,
}

pub async fn list(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let store = super::fleet::require_enrollment_store(&state)?;
    let snapshot = store
        .list()
        .await
        .map_err(|error| ToolError::internal_message(format!("list enrollments: {error}")))?;
    Ok(Json(serde_json::to_value(snapshot).map_err(|error| {
        ToolError::internal_message(format!("serialize enrollments: {error}"))
    })?))
}

pub async fn approve(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Json(payload): Json<ApproveEnrollmentRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let node_id = normalize_node_id_value(&node_id, "node_id")?;
    let store = super::fleet::require_enrollment_store(&state)?;
    let approved = store
        .approve(&node_id, payload.note)
        .await
        .map_err(|error| map_enrollment_error("approve enrollment", error))?;
    Ok(Json(serde_json::to_value(approved).map_err(|error| {
        ToolError::internal_message(format!("serialize approved enrollment: {error}"))
    })?))
}

pub async fn deny(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Json(payload): Json<DenyEnrollmentRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let node_id = normalize_node_id_value(&node_id, "node_id")?;
    let store = super::fleet::require_enrollment_store(&state)?;
    let denied = store
        .deny(&node_id, payload.reason)
        .await
        .map_err(|error| map_enrollment_error("deny enrollment", error))?;
    Ok(Json(serde_json::to_value(denied).map_err(|error| {
        ToolError::internal_message(format!("serialize denied enrollment: {error}"))
    })?))
}

fn map_enrollment_error(context: &str, error: anyhow::Error) -> ToolError {
    let message = error.to_string();
    if message.contains("not found") {
        ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message,
        }
    } else {
        ToolError::internal_message(format!("{context}: {error}"))
    }
}
