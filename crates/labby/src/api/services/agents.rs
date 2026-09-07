use crate::{
    api::{ActionRequest, error::ApiError, state::AppState},
    dispatch::{agents::AgentDispatchContext, error::ToolError},
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
        RouteDescriptor::new("POST", "/", "handle", "agents", RouteAuth::V1)
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
    if !matches!(
        request.action.as_str(),
        "help" | "schema" | "agents.list" | "agents.get" | "agents.session.status"
    ) {
        super::require_session_csrf(&request.action, &headers, Some(&auth.0))?;
    }
    let store = state
        .access_runtime
        .store()
        .await
        .map_err(|_| unavailable())?;
    Ok(Json(
        crate::dispatch::agents::dispatch(
            AgentDispatchContext {
                store,
                identity,
                ceiling: crate::access::AuthorityCeiling::from_auth_context(&auth.0),
            },
            &request.action,
            request.params,
        )
        .await?,
    ))
}
fn denied() -> ToolError {
    ToolError::Forbidden {
        message: "Agent access requires host-established identity".into(),
        required_scopes: vec![],
    }
}
fn unavailable() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "service_unavailable".into(),
        message: "Agent service unavailable".into(),
    }
}
