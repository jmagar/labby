//! Read-only REST handlers for the MCP server registry at `/v0.1`.
//!
//! Three GET endpoints mirror the upstream mcpregistry.io API shape:
//!   GET /v0.1/servers
//!   GET /v0.1/servers/:serverName/versions
//!   GET /v0.1/servers/:serverName/versions/:version

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::get,
};
use serde::Deserialize;
use serde_json::json;

use crate::api::error::ApiError;
use crate::api::state::AppState;
use crate::dispatch::error::ToolError;
use crate::dispatch::marketplace::store::StoreListParams;

/// Query params for `GET /v0.1/servers`.
///
/// Thin deserializable wrapper — `StoreListParams` is not `Deserialize`.
#[derive(Debug, Deserialize, Default)]
pub struct ListServersQuery {
    /// Substring search on server name.
    pub search: Option<String>,
    /// GitHub username or org. Convenience: translated to
    /// `search = "io.github.{owner}/"` (lowercased, trimmed) when `search` is
    /// unset. Ignored if `search` is also set. Rejected with `invalid_param`
    /// if empty or containing `/` or whitespace.
    pub owner: Option<String>,
    /// Opaque pagination cursor from a previous response.
    pub cursor: Option<String>,
    /// Max results per page (server-side clamped to 1–100, default 20).
    pub limit: Option<u32>,
    /// Exact version match on the stored registry snapshot.
    pub version: Option<String>,
    /// Inclusive lower bound on the upstream registry updated timestamp.
    pub updated_since: Option<String>,
    /// Include servers with `status = 'deleted'`.
    #[serde(default)]
    pub include_deleted: bool,
    pub featured: Option<bool>,
    pub reviewed: Option<bool>,
    pub recommended: Option<bool>,
    pub hidden: Option<bool>,
    pub tag: Option<String>,
}

/// Maximum length allowed for a `:serverName` path parameter (bytes).
const SERVER_NAME_MAX_LEN: usize = 512;

/// `GET /v0.1/servers` — list servers with optional search/cursor/limit.
async fn list_servers(
    State(state): State<AppState>,
    Query(query): Query<ListServersQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let Some(store) = state.registry_store.as_ref() else {
        return Err(ApiError(ToolError::Sdk {
            sdk_kind: "service_unavailable".into(),
            message: "registry store initializing — try again in a few seconds".into(),
        }));
    };

    let mut params = StoreListParams {
        cursor: query.cursor,
        limit: query.limit,
        include_deleted: query.include_deleted,
        latest_only: false,
        search: None,
        version: query.version,
        updated_since: query.updated_since,
        featured: query.featured,
        reviewed: query.reviewed,
        recommended: query.recommended,
        hidden: query.hidden,
        tag: query.tag,
    };
    let effective_search = crate::dispatch::marketplace::resolve_search_for_rest(
        query.search.as_deref(),
        query.owner.as_deref(),
    )?;
    if let Some(search) = effective_search {
        params = params.with_search(search);
    }

    let paged = store
        .list_servers(params)
        .await
        .map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("registry store list_servers: {e}"),
        })?;

    let body = json!({
        "servers": paged.servers,
        "next_cursor": paged.next_cursor,
    });
    Ok(Json(body))
}

/// `GET /v0.1/servers/:serverName/versions` — list all versions for a server.
async fn list_versions(
    State(state): State<AppState>,
    Path(server_name): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if server_name.len() > SERVER_NAME_MAX_LEN {
        return Err(ApiError(ToolError::Sdk {
            sdk_kind: "invalid_param".into(),
            message: format!("serverName must be at most {SERVER_NAME_MAX_LEN} bytes"),
        }));
    }

    let Some(store) = state.registry_store.as_ref() else {
        return Err(ApiError(ToolError::Sdk {
            sdk_kind: "service_unavailable".into(),
            message: "registry store initializing — try again in a few seconds".into(),
        }));
    };

    let versions = store
        .list_versions(&server_name)
        .await
        .map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("registry store list_versions: {e}"),
        })?;

    Ok(Json(json!({ "versions": versions })))
}

/// `GET /v0.1/servers/:serverName/versions/:version` — get a single server version.
async fn get_server(
    State(state): State<AppState>,
    Path((server_name, version)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if server_name.len() > SERVER_NAME_MAX_LEN {
        return Err(ApiError(ToolError::Sdk {
            sdk_kind: "invalid_param".into(),
            message: format!("serverName must be at most {SERVER_NAME_MAX_LEN} bytes"),
        }));
    }

    let Some(store) = state.registry_store.as_ref() else {
        return Err(ApiError(ToolError::Sdk {
            sdk_kind: "service_unavailable".into(),
            message: "registry store initializing — try again in a few seconds".into(),
        }));
    };

    match store.get_server(&server_name, &version).await {
        Ok(Some(server)) => Ok(Json(json!(server))),
        Ok(None) => Err(ApiError(ToolError::Sdk {
            sdk_kind: "not_found".into(),
            message: format!("server '{server_name}' version '{version}' not found"),
        })),
        Err(e) => Err(ApiError(ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("registry store get_server: {e}"),
        })),
    }
}

/// Build the `/v0.1` sub-router (routes only — auth middleware applied in `router.rs`).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/servers", get(list_servers))
        .route("/servers/{serverName}/versions", get(list_versions))
        .route("/servers/{serverName}/versions/{version}", get(get_server))
}
