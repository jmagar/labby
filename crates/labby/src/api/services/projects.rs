use crate::{
    api::{ActionRequest, error::ApiError, state::AppState},
    dispatch::error::ToolError,
};
use axum::{Extension, Json, extract::State, http::HeaderMap, routing::post};
use labby_auth::{AuthContext, VerifiedIdentity};
use serde_json::Value;
pub fn routes(_state: AppState) -> crate::api::route_registry::RouteGroup {
    crate::api::route_registry::RouteGroup::empty().route(descriptors().remove(0), post(handle))
}
pub(crate) fn descriptors() -> Vec<crate::api::route_registry::RouteDescriptor> {
    use crate::api::route_registry::{RouteAuth, RouteDescriptor};
    vec![
        RouteDescriptor::new("POST", "/", "handle", "projects", RouteAuth::V1)
            .when("mounted only when API authentication is configured"),
    ]
}
async fn handle(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
    Json(request): Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    let auth = auth.ok_or_else(denied)?;
    let identity = identity.ok_or_else(denied)?.0;
    if !matches!(request.action.as_str(), "projects.list" | "projects.get") {
        super::require_session_csrf(&request.action, &headers, Some(&auth.0))?;
    }
    let store = state
        .access_runtime
        .store()
        .await
        .map_err(|_| unavailable())?;
    crate::dispatch::projects::dispatch(store, identity, &request.action, request.params)
        .await
        .map(Json)
        .map_err(ApiError::from)
}
fn denied() -> ToolError {
    ToolError::Forbidden {
        message: "Project access requires host-established identity".into(),
        required_scopes: vec![],
    }
}
fn unavailable() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "service_unavailable".into(),
        message: "Project service unavailable".into(),
    }
}
