use axum::{Extension, Json, extract::State, http::HeaderMap, routing::post};
use labby_auth::{AuthContext, VerifiedIdentity};
use serde_json::Value;

use crate::{
    api::{ActionRequest, error::ApiError, state::AppState},
    dispatch::{access::AccessDispatchContext, error::ToolError},
};

pub fn routes(_state: AppState) -> crate::api::route_registry::RouteGroup {
    crate::api::route_registry::RouteGroup::empty().route(descriptors().remove(0), post(handle))
}

pub(crate) fn descriptors() -> Vec<crate::api::route_registry::RouteDescriptor> {
    use crate::api::route_registry::{RouteAuth, RouteDescriptor};
    vec![
        RouteDescriptor::new("POST", "/", "handle", "access", RouteAuth::V1)
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
    let auth = auth.ok_or_else(identity_required)?;
    let identity = identity.ok_or_else(identity_required)?.0;
    if !matches!(
        request.action.as_str(),
        "help" | "schema" | "access.team.list" | "access.project.effective.list"
    ) {
        super::require_session_csrf(&request.action, &headers, Some(&auth.0))?;
    }
    let store = state
        .access_runtime
        .store()
        .await
        .map_err(|_| unavailable())?;
    let installation_id = state
        .installation_id
        .as_deref()
        .ok_or_else(unavailable)?
        .to_string();
    let value = crate::dispatch::access::dispatch(
        AccessDispatchContext {
            store,
            identity,
            ceiling: crate::access::AuthorityCeiling::from_auth_context(&auth.0),
            installation_id,
            #[cfg(feature = "gateway")]
            gateway_manager: state.gateway_manager.clone(),
        },
        &request.action,
        request.params,
    )
    .await?;
    Ok(Json(value))
}

fn identity_required() -> ToolError {
    ToolError::Forbidden {
        message: "access administration requires host-established identity".to_owned(),
        required_scopes: Vec::new(),
    }
}

fn unavailable() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "service_unavailable".to_owned(),
        message: "access administration is unavailable".to_owned(),
    }
}
