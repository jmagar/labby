//! Authenticated HTTP adapter for Dev Container actions.

use axum::{Extension, Json, extract::State, http::HeaderMap, routing::post};
use labby_auth::{AuthContext, VerifiedIdentity};
use labby_primitives::access::OwnerKind;
use serde_json::Value;

use crate::api::{ActionRequest, error::ApiError, state::AppState};

pub fn routes(_state: AppState) -> crate::api::route_registry::RouteGroup {
    crate::api::route_registry::RouteGroup::empty().route(descriptors().remove(0), post(handle))
}

pub(crate) fn descriptors() -> Vec<crate::api::route_registry::RouteDescriptor> {
    use crate::api::route_registry::{RouteAuth, RouteDescriptor};
    vec![
        RouteDescriptor::new("POST", "/", "handle", "dev_containers", RouteAuth::V1)
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
    super::require_session_csrf(&request.action, &headers, Some(&auth.0))?;
    let owner = match request.params.get("owner_kind").and_then(Value::as_str) {
        Some("installation") => OwnerKind::Installation,
        Some("team") => OwnerKind::Team,
        Some("project") => OwnerKind::Project,
        Some("personal") | None => OwnerKind::Personal,
        Some(_) => return Err(ApiError::from(denied())),
    };
    let _required = crate::dispatch::dev_containers::required_capability(&request.action, owner)
        .ok_or_else(denied)?;
    crate::dispatch::dev_containers::dispatch(
        crate::dispatch::dev_containers::DevContainerDispatchContext {
            access_runtime: state.access_runtime,
            identity,
            ceiling: crate::access::AuthorityCeiling::from_auth_context(&auth.0),
        },
        &request.action,
        request.params,
    )
    .await
    .map(Json)
    .map_err(ApiError::from)
}

fn denied() -> crate::dispatch::error::ToolError {
    crate::dispatch::error::ToolError::Forbidden {
        message: "Dev Container operation is not authorized".into(),
        required_scopes: Vec::new(),
    }
}
