//! Thin HTTP adapters for remote Artifact control-plane services.

use std::{net::SocketAddr, sync::LazyLock};

use axum::{
    Extension, Json,
    body::Body,
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, header},
    routing::{post, put},
};
use http_body_util::{BodyExt as _, Limited};
use labby_auth::VerifiedIdentity;
use serde::Deserialize;
use serde_json::Value;

use crate::access::Permission;
use crate::api::error::ApiError;
use crate::api::oauth::AuthContext;
use crate::api::services::helpers::{dispatch_meta_from_headers, handle_action_with_meta};
use crate::api::{ActionRequest, state::AppState};

const MAX_UPLOAD_BYTES: usize = 50_000_000;
static UPLOAD_ADMISSION: LazyLock<tokio::sync::Semaphore> =
    LazyLock::new(|| tokio::sync::Semaphore::new(16));

pub fn routes(service: &'static str, _state: AppState) -> crate::api::route_registry::RouteGroup {
    use crate::api::route_registry::RouteGroup;
    let handler = match service {
        "sources" => post(handle_sources),
        "jobs" => post(handle_jobs),
        "uploads" => post(handle_uploads),
        "bundles" => post(handle_bundles),
        _ => return RouteGroup::empty(),
    };
    let mut route_descriptors = descriptors(service).into_iter();
    let group = RouteGroup::empty().route(route_descriptors.next().unwrap(), handler);
    if service == "uploads" {
        group.route(
            route_descriptors.next().unwrap(),
            put(upload_bytes).layer(axum::extract::DefaultBodyLimit::max(50_000_000)),
        )
    } else {
        group
    }
}

pub(crate) fn descriptors(
    service: &'static str,
) -> Vec<crate::api::route_registry::RouteDescriptor> {
    use crate::api::route_registry::{RouteAuth, RouteDescriptor};
    let mut descriptors = vec![
        RouteDescriptor::new("POST", "/", "handle", service, RouteAuth::V1)
            .feature("skills")
            .when("mounted only when API authentication is configured")
            .host_validated()
            .private_no_store(),
    ];
    if service == "uploads" {
        descriptors.push(
            RouteDescriptor::new("PUT", "/{id}", "upload_bytes", service, RouteAuth::V1)
                .feature("skills")
                .when("mounted only when API authentication is configured")
                .host_validated()
                .private_no_store()
                .side_effects("stores bounded bytes in a principal-bound remote upload slot"),
        );
    }
    descriptors
}

#[derive(Debug, Deserialize)]
struct UploadQuery {
    connection_id: Option<String>,
}

async fn upload_bytes(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
    Path(id): Path<String>,
    Query(query): Query<UploadQuery>,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<Value>, ApiError> {
    let is_admin = auth
        .as_ref()
        .is_some_and(|Extension(auth)| auth.scopes.iter().any(|scope| scope == "lab:admin"));
    if !is_admin {
        return Err(crate::dispatch::error::ToolError::Forbidden {
            message: "Artifact uploads require lab:admin scope".to_owned(),
            required_scopes: vec!["lab:admin".to_owned()],
        }
        .into());
    }
    require_session_csrf(
        "uploads.put",
        &headers,
        auth.as_ref().map(|Extension(auth)| auth),
    )?;
    let content_length = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if content_length.is_some_and(|length| length > MAX_UPLOAD_BYTES as u64) {
        return Err(crate::dispatch::error::ToolError::InvalidParam {
            message: "Artifact upload exceeds 50000000 bytes".to_owned(),
            param: "body".to_owned(),
        }
        .into());
    }
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream");
    let project_id = headers
        .get("x-labby-project-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| crate::dispatch::error::ToolError::Forbidden {
            message: "Artifact uploads require project context".to_owned(),
            required_scopes: vec!["lab:admin".to_owned()],
        })?;
    let selected_team_id = headers
        .get("x-labby-team-id")
        .and_then(|value| value.to_str().ok());
    let context = authorize_authority_context(
        &state.access_runtime,
        identity.map(|Extension(identity)| identity),
        Some(project_id),
        selected_team_id,
        Permission::ProjectManage,
    )
    .await?;
    let request_id = headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok());
    let controls = crate::dispatch::skill_library::process_controls().ok_or_else(|| {
        crate::dispatch::error::ToolError::Sdk {
            sdk_kind: "source_unavailable".to_owned(),
            message: "Remote Artifact control plane is unavailable".to_owned(),
        }
    })?;
    let _admission = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        UPLOAD_ADMISSION.acquire(),
    )
    .await
    .map_err(|_| crate::dispatch::error::ToolError::Sdk {
        sdk_kind: "queue_saturated".to_owned(),
        message: "Artifact upload queue is saturated".to_owned(),
    })?
    .map_err(|_| crate::dispatch::error::ToolError::Sdk {
        sdk_kind: "source_unavailable".to_owned(),
        message: "Artifact upload queue is unavailable".to_owned(),
    })?;
    tracing::info!(surface = "api", service = "uploads", action = "uploads.put", request_id, actor_id = %context.actor_id, project_id = %context.project_id, "remote upload started");
    let bytes = Limited::new(body, MAX_UPLOAD_BYTES)
        .collect()
        .await
        .map_err(|_| crate::dispatch::error::ToolError::InvalidParam {
            message: "Artifact upload exceeds 50000000 bytes or could not be read".to_owned(),
            param: "body".to_owned(),
        })?
        .to_bytes();
    let actual_length = u64::try_from(bytes.len()).map_err(|_| {
        crate::dispatch::error::ToolError::InvalidParam {
            message: "Artifact upload length is invalid".to_owned(),
            param: "body".to_owned(),
        }
    })?;
    if content_length.is_some_and(|declared| declared != actual_length) {
        return Err(crate::dispatch::error::ToolError::InvalidParam {
            message: "Artifact upload content length does not match its body".to_owned(),
            param: "content-length".to_owned(),
        }
        .into());
    }
    use sha2::Digest as _;
    let content_digest = format!("sha256:{}", hex::encode(sha2::Sha256::digest(&bytes)));
    let result = controls
        .upload(
            query.connection_id.as_deref(),
            &id,
            reqwest::Body::from(bytes),
            Some(actual_length),
            content_type,
            &content_digest,
            &context,
        )
        .await;
    match &result {
        Ok(_) => {
            tracing::info!(surface = "api", service = "uploads", action = "uploads.put", request_id, actor_id = %context.actor_id, project_id = %context.project_id, "remote upload completed")
        }
        Err(error) => {
            tracing::warn!(surface = "api", service = "uploads", action = "uploads.put", request_id, actor_id = %context.actor_id, project_id = %context.project_id, kind = error.kind(), "remote upload failed")
        }
    }
    result.map(Json).map_err(Into::into)
}

async fn handle_sources(
    state: State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
    body: Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    handle("sources", state, peer, headers, auth, identity, body).await
}
async fn handle_jobs(
    state: State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
    body: Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    handle("jobs", state, peer, headers, auth, identity, body).await
}
async fn handle_uploads(
    state: State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
    body: Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    handle("uploads", state, peer, headers, auth, identity, body).await
}
async fn handle_bundles(
    state: State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
    body: Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    handle("bundles", state, peer, headers, auth, identity, body).await
}

async fn handle(
    service: &'static str,
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
    Json(req): Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    let identity = identity.map(|Extension(identity)| identity);
    let project_id = headers
        .get("x-labby-project-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let selected_team_id = headers
        .get("x-labby-team-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let request_headers = headers.clone();
    let request_auth = auth.clone();
    handle_action_with_meta(
        service,
        "api",
        dispatch_meta_from_headers(
            &headers,
            auth.as_ref().map(|value| &value.0),
            peer.map(|Extension(ConnectInfo(addr))| addr),
        ),
        req,
        crate::dispatch::remote_control::actions(service),
        move |action, params| async move {
            let spec = crate::dispatch::remote_control::actions(service)
                .iter()
                .find(|candidate| candidate.name == action);
            let operation = crate::dispatch::remote_control::operation(service, &action)
                .ok_or_else(|| crate::dispatch::error::ToolError::UnknownAction {
                    message: format!("Unknown action: {action}"),
                    valid: Vec::new(),
                    hint: None,
                })?;
            let permission = crate::dispatch::artifact_control::operation_permission(operation);
            if spec.is_some_and(|spec| spec.requires_admin) {
                require_session_csrf(
                    &action,
                    &request_headers,
                    request_auth.as_ref().map(|Extension(auth)| auth),
                )?;
            }
            let context = authorize_authority_context(
                &state.access_runtime,
                identity,
                project_id.as_deref(),
                selected_team_id.as_deref(),
                permission,
            )
            .await?;
            crate::dispatch::remote_control::dispatch_with_context(
                service,
                &action,
                params,
                Some(&context),
            )
            .await
        },
    )
    .await
}

pub(crate) fn require_session_csrf(
    action: &str,
    headers: &HeaderMap,
    auth: Option<&AuthContext>,
) -> Result<(), crate::dispatch::error::ToolError> {
    super::require_session_csrf(action, headers, auth)
}

pub(crate) async fn authorize_authority_context(
    runtime: &crate::access::AccessRuntime,
    identity: Option<VerifiedIdentity>,
    project_id: Option<&str>,
    selected_team_id: Option<&str>,
    permission: Permission,
) -> Result<crate::dispatch::artifact_control::AuthorityContext, crate::dispatch::error::ToolError>
{
    let identity = identity.ok_or_else(|| crate::dispatch::error::ToolError::Forbidden {
        message: "Remote Artifact operations require verified identity".to_owned(),
        required_scopes: vec!["lab:read".to_owned()],
    })?;
    let project_id = project_id
        .filter(|project_id| !project_id.trim().is_empty())
        .ok_or_else(|| crate::dispatch::error::ToolError::Forbidden {
            message: "Remote Artifact operations require project context".to_owned(),
            required_scopes: vec!["lab:read".to_owned()],
        })?;
    crate::dispatch::artifact_control::authorize_authority_context(
        runtime,
        identity,
        project_id,
        selected_team_id,
        permission,
    )
    .await
}

#[cfg(test)]
mod tests {
    use axum::response::IntoResponse;

    use super::*;

    fn auth(scopes: &[&str]) -> Option<Extension<AuthContext>> {
        Some(Extension(AuthContext {
            sub: "operator".to_owned(),
            actor_key: None,
            scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
            issuer: "test".to_owned(),
            via_session: false,
            csrf_token: None,
            email: None,
        }))
    }

    #[test]
    fn browser_mutations_require_the_shared_session_csrf_header() {
        let mut session = auth(&["lab:admin"]).unwrap().0;
        session.via_session = true;
        session.csrf_token = Some("csrf-secret".to_owned());
        let mut headers = HeaderMap::new();
        assert!(require_session_csrf("jobs.start", &headers, Some(&session)).is_err());
        headers.insert(
            labby_auth::session::BROWSER_CSRF_HEADER_NAME,
            "csrf-secret".parse().unwrap(),
        );
        assert!(require_session_csrf("jobs.start", &headers, Some(&session)).is_ok());

        session.csrf_token = None;
        headers.remove(labby_auth::session::BROWSER_CSRF_HEADER_NAME);
        assert!(require_session_csrf("jobs.start", &headers, Some(&session)).is_err());
    }

    #[test]
    fn bearer_mutations_do_not_require_browser_csrf() {
        let bearer = auth(&["lab:admin"]).unwrap().0;
        assert!(require_session_csrf("jobs.start", &HeaderMap::new(), Some(&bearer)).is_ok());
    }

    #[test]
    fn every_remote_authority_route_is_private_no_store() {
        for service in ["sources", "jobs", "uploads", "bundles"] {
            for descriptor in descriptors(service) {
                assert_eq!(descriptor.cache_posture, "private, no-store", "{service}");
            }
        }
    }

    #[tokio::test]
    async fn raw_upload_requires_admin_before_remote_dispatch() {
        let error = upload_bytes(
            State(AppState::default()),
            None,
            None,
            Path("upload-1".to_owned()),
            Query(UploadQuery {
                connection_id: None,
            }),
            HeaderMap::new(),
            Body::empty(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.into_response().status(),
            axum::http::StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn raw_upload_rejects_non_admin_execution_scope() {
        let error = upload_bytes(
            State(AppState::default()),
            auth(&["lab"]),
            None,
            Path("upload-1".to_owned()),
            Query(UploadQuery {
                connection_id: None,
            }),
            HeaderMap::new(),
            Body::empty(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.into_response().status(),
            axum::http::StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn raw_upload_enforces_bound_before_remote_dispatch() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_LENGTH, "50000001".parse().unwrap());
        let error = upload_bytes(
            State(AppState::default()),
            auth(&["lab:admin"]),
            None,
            Path("upload-1".to_owned()),
            Query(UploadQuery {
                connection_id: None,
            }),
            headers,
            Body::empty(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.into_response().status(),
            axum::http::StatusCode::UNPROCESSABLE_ENTITY
        );
    }
}
