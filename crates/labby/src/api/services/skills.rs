use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{ConnectInfo, State},
    http::HeaderMap,
    routing::post,
};
use labby_auth::{Authenticator, VerifiedIdentity};
use serde_json::Value;

use crate::api::error::ApiError;
use crate::api::oauth::AuthContext;
use crate::api::services::helpers::{dispatch_meta_from_headers, handle_action_with_meta};
use crate::api::{ActionRequest, state::AppState};
use crate::dispatch::error::ToolError;

#[cfg(feature = "skills")]
fn map_import_adapter_error(
    error: crate::dispatch::skill_library::import::ImportAdapterError,
) -> ToolError {
    crate::dispatch::skill_library::map_import_error(error)
}

pub fn routes(_state: AppState) -> crate::api::route_registry::RouteGroup {
    use crate::api::route_registry::RouteGroup;
    RouteGroup::empty().route(descriptors().remove(0), post(handle))
}

pub(crate) fn descriptors() -> Vec<crate::api::route_registry::RouteDescriptor> {
    use crate::api::route_registry::{RouteAuth, RouteDescriptor};
    vec![
        RouteDescriptor::new("POST", "/", "handle", "artifacts", RouteAuth::V1)
            .feature("skills")
            .when("mounted only when API authentication is configured"),
    ]
}

#[cfg(all(test, feature = "skills"))]
mod skill_library_error_tests {
    use crate::dispatch::skill_library::auth::SkillLibraryAuthorizationError;
    use crate::dispatch::skill_library::dispatch::SkillLibraryDispatchError;
    use labby_runtime::artifacts::ArtifactError;

    #[test]
    fn management_errors_keep_stable_recovery_kinds() {
        let cases = [
            (
                SkillLibraryDispatchError::Artifact(ArtifactError::Busy),
                "queue_saturated",
            ),
            (
                SkillLibraryDispatchError::Artifact(ArtifactError::Conflict(
                    "library_version_changed",
                )),
                "conflict",
            ),
            (
                SkillLibraryDispatchError::Artifact(ArtifactError::NotFound("library_record")),
                "not_found",
            ),
            (
                SkillLibraryDispatchError::Authorization(SkillLibraryAuthorizationError::Denied),
                "forbidden",
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(
                crate::dispatch::skill_library::map_dispatch_error(error).kind(),
                expected
            );
        }
    }

    #[test]
    fn management_internal_error_redacts_os_message_and_path() {
        const CANARY: &str = "/private/canary/never-return-this";
        let mapped = crate::dispatch::skill_library::map_dispatch_error(
            SkillLibraryDispatchError::Artifact(ArtifactError::Io(std::io::Error::other(CANARY))),
        );
        let wire = serde_json::to_string(&mapped).expect("serialize ToolError");
        assert_eq!(mapped.kind(), "internal_error");
        assert!(!wire.contains(CANARY));
        assert!(!wire.contains("private"));
    }

    #[test]
    fn indeterminate_mutation_error_requires_same_key_reconciliation() {
        let mapped = crate::dispatch::skill_library::map_dispatch_error(
            SkillLibraryDispatchError::BlockingIndeterminate {
                operation: "skill_artifact_commit",
            },
        );

        assert_eq!(mapped.kind(), "service_unavailable");
        let fields = mapped.extra_fields();
        assert_eq!(fields["operation"], "skill_artifact_commit");
        assert_eq!(fields["outcome"], "indeterminate");
        assert_eq!(fields["recovery"], "retry_with_same_idempotency_key");
    }

    #[test]
    fn worker_failure_is_not_misreported_as_a_timeout() {
        let mapped = crate::dispatch::skill_library::map_dispatch_error(
            SkillLibraryDispatchError::BlockingWorkerFailed {
                operation: "skill_library_list",
            },
        );

        assert_eq!(mapped.kind(), "internal_error");
        assert_ne!(mapped.kind(), "timeout");
    }
}

fn has_read_scope(auth: Option<&Extension<AuthContext>>) -> bool {
    auth.is_some_and(|ctx| {
        ctx.0
            .scopes
            .iter()
            .any(|scope| matches!(scope.as_str(), "lab:read" | "lab" | "lab:admin"))
    })
}

fn require_read_scope(
    action: &str,
    request_id: Option<&str>,
    auth: Option<&Extension<AuthContext>>,
) -> Result<(), ToolError> {
    if has_read_scope(auth) {
        return Ok(());
    }
    tracing::warn!(
        surface = "api",
        service = "artifacts",
        action,
        request_id,
        kind = "forbidden",
        "artifacts action rejected: read scope required"
    );
    Err(ToolError::Forbidden {
        message: "artifacts require one of scopes: lab:read, lab, lab:admin".to_string(),
        required_scopes: vec![
            "lab:read".to_string(),
            "lab".to_string(),
            "lab:admin".to_string(),
        ],
    })
}

pub(crate) async fn handle(
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
    Json(req): Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    let request_id = headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok());
    if identity
        .as_ref()
        .is_some_and(|identity| identity.authenticator() == Authenticator::ProductCredential)
    {
        return Err(ToolError::Forbidden {
            message: "project product credentials must use their bound protected MCP route"
                .to_string(),
            required_scopes: Vec::new(),
        }
        .into());
    }
    require_read_scope(&req.action, request_id, auth.as_ref())?;

    let skill_library = state.skill_library.clone();
    let skill_library_imports = state.skill_library_imports.clone();
    let access_runtime = Arc::clone(&state.access_runtime);
    let verified_identity = identity.map(|Extension(value)| value);
    let project_id = headers
        .get("x-labby-project-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let correlation = request_id.map(str::to_owned);
    let auth_for_library = auth.clone().map(|Extension(value)| value);
    let library_headers = headers.clone();

    handle_action_with_meta(
        "artifacts",
        "api",
        dispatch_meta_from_headers(
            &headers,
            auth.as_ref().map(|value| &value.0),
            peer.map(|Extension(ConnectInfo(addr))| addr),
        ),
        req,
        crate::dispatch::skills::api_actions(),
        move |action, params| async move {
            if action.starts_with("artifacts.") {
                if crate::dispatch::remote_control::REMOTE_ARTIFACT_ACTIONS
                    .iter()
                    .any(|candidate| candidate.name == action)
                {
                    let spec = crate::dispatch::remote_control::REMOTE_ARTIFACT_ACTIONS
                        .iter()
                        .find(|candidate| candidate.name == action);
                    let operation =
                        crate::dispatch::remote_control::operation("artifacts", &action)
                            .ok_or_else(|| ToolError::UnknownAction {
                                message: format!("Unknown action: {action}"),
                                valid: Vec::new(),
                                hint: None,
                            })?;
                    let permission =
                        crate::dispatch::artifact_control::operation_permission(operation);
                    if spec.is_some_and(|spec| spec.requires_admin) {
                        crate::api::services::remote_control::require_session_csrf(
                            &action,
                            &library_headers,
                            auth_for_library.as_ref(),
                        )?;
                    }
                    let context =
                        crate::api::services::remote_control::authorize_authority_context(
                            &access_runtime,
                            verified_identity,
                            project_id.as_deref(),
                            library_headers
                                .get("x-labby-team-id")
                                .and_then(|value| value.to_str().ok()),
                            permission,
                        )
                        .await?;
                    return crate::dispatch::remote_control::dispatch_with_context(
                        "artifacts",
                        &action,
                        params,
                        Some(&context),
                    )
                    .await;
                }
                let service = skill_library.ok_or_else(|| ToolError::Sdk {
                    sdk_kind: "skill_library_unavailable".to_owned(),
                    message: "Skill Library is unavailable".to_owned(),
                })?;
                let identity = verified_identity.ok_or_else(|| ToolError::Forbidden {
                    message: "Skill Library identity is required".to_owned(),
                    required_scopes: vec![],
                })?;
                let auth = auth_for_library.ok_or_else(|| ToolError::Forbidden {
                    message: "Skill Library authentication is required".to_owned(),
                    required_scopes: vec![],
                })?;
                let project_id = project_id.ok_or_else(|| ToolError::Forbidden {
                    message: "Skill Library project context is required".to_owned(),
                    required_scopes: vec![],
                })?;
                static REQUESTS: std::sync::atomic::AtomicU64 =
                    std::sync::atomic::AtomicU64::new(1);
                let correlation = correlation.unwrap_or_else(|| {
                    format!(
                        "api-{}",
                        REQUESTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    )
                });
                let csrf_verified = auth.via_session
                    && auth.csrf_token.as_deref().is_some_and(|token| {
                        library_headers
                            .get("x-csrf-token")
                            .and_then(|value| value.to_str().ok())
                            == Some(token)
                    });
                let transport = if auth.via_session {
                    crate::dispatch::skill_library::auth::SkillLibraryTransport::browser(
                        true,
                        csrf_verified,
                    )
                } else {
                    crate::dispatch::skill_library::auth::SkillLibraryTransport::bearer(
                        crate::dispatch::skill_library::auth::SkillLibrarySurface::ApiBearer,
                        true,
                    )
                };
                let selected_team_id = library_headers
                    .get("x-labby-team-id")
                    .map(|value| {
                        value
                            .to_str()
                            .map(str::to_owned)
                            .map_err(|_| ToolError::InvalidParam {
                                message: "Skill Library team context is invalid".to_owned(),
                                param: "x-labby-team-id".to_owned(),
                            })
                    })
                    .transpose()?;
                let caller = crate::dispatch::skill_library::auth::SkillLibraryCaller::new(
                    identity,
                    auth.scopes,
                    transport,
                )
                .with_selected_team_id(selected_team_id);
                let correlation =
                    crate::dispatch::skill_library::audit::SkillLibraryCorrelationId::parse(
                        correlation,
                    )
                    .map_err(|()| ToolError::InvalidParam {
                        message: "invalid request correlation".to_owned(),
                        param: "x-request-id".to_owned(),
                    })?;
                if action == "artifacts.import_batch" {
                    let import_params: crate::dispatch::skill_library::params::ImportBatchParams =
                        serde_json::from_value(params).map_err(|_| ToolError::InvalidParam {
                            message: "Artifact batch import parameters are invalid".to_owned(),
                            param: "params".to_owned(),
                        })?;
                    crate::dispatch::skill_library::params::validate_idempotency_key(
                        &import_params.idempotency_key,
                    )
                    .map_err(|_| ToolError::InvalidParam {
                        message: "Artifact batch idempotency key is invalid".to_owned(),
                        param: "idempotency_key".to_owned(),
                    })?;
                    let imports = skill_library_imports.ok_or_else(|| ToolError::Sdk {
                        sdk_kind: "source_unavailable".to_owned(),
                        message: "Artifact import sources are not configured".to_owned(),
                    })?;
                    return imports
                        .import_batch_selected(
                            &service,
                            &access_runtime,
                            caller,
                            &project_id,
                            import_params.sources,
                            import_params.expected_library_version,
                            import_params.idempotency_key,
                            &correlation,
                        )
                        .await
                        .map_err(map_import_adapter_error);
                }
                if action == "artifacts.import" {
                    let import_params: crate::dispatch::skill_library::params::ImportParams =
                        serde_json::from_value(params).map_err(|_| ToolError::InvalidParam {
                            message: "Skill Library import parameters are invalid".to_owned(),
                            param: "params".to_owned(),
                        })?;
                    crate::dispatch::skill_library::params::validate_idempotency_key(
                        &import_params.idempotency_key,
                    )
                    .map_err(|_| ToolError::InvalidParam {
                        message: "Skill Library idempotency key is invalid".to_owned(),
                        param: "idempotency_key".to_owned(),
                    })?;
                    let imports = skill_library_imports.ok_or_else(|| ToolError::Sdk {
                        sdk_kind: "source_unavailable".to_owned(),
                        message: "Skill import sources are not configured".to_owned(),
                    })?;
                    return imports
                        .import_selected(
                            &service,
                            &access_runtime,
                            caller,
                            &project_id,
                            import_params.source,
                            import_params.expected_library_version,
                            import_params.idempotency_key,
                            &correlation,
                        )
                        .await
                        .map_err(map_import_adapter_error);
                }
                return service
                    .dispatch(
                        &access_runtime,
                        caller,
                        &project_id,
                        &action,
                        params,
                        &correlation,
                    )
                    .await
                    .map_err(crate::dispatch::skill_library::map_dispatch_error);
            }
            Err(ToolError::UnknownAction {
                message: format!("unknown action '{action}' for service 'artifacts'"),
                valid: crate::dispatch::skills::api_actions()
                    .iter()
                    .map(|candidate| candidate.name.to_owned())
                    .collect(),
                hint: None,
            })
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use axum::{
        Extension, Router,
        body::Body,
        http::{Request, StatusCode, header},
    };
    use labby_auth::{Authenticator, VerifiedIdentity};
    use serde_json::json;
    use tower::ServiceExt;

    use crate::api::{oauth::AuthContext, state::AppState};

    fn auth(scopes: &[&str]) -> AuthContext {
        AuthContext {
            sub: "reader".to_string(),
            actor_key: None,
            scopes: scopes.iter().map(|scope| (*scope).to_string()).collect(),
            issuer: "test".to_string(),
            via_session: false,
            csrf_token: None,
            email: None,
        }
    }

    fn app(auth: Option<AuthContext>, identity: Option<VerifiedIdentity>) -> Router {
        let state = AppState::from_registry(crate::registry::build_default_registry());
        let mut app = super::routes(state.clone()).router.with_state(state);
        if let Some(auth) = auth {
            app = app.layer(Extension(auth));
        }
        if let Some(identity) = identity {
            app = app.layer(Extension(identity));
        }
        app
    }

    async fn post(app: Router, body: serde_json::Value) -> axum::response::Response {
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response")
    }

    #[tokio::test]
    async fn unauthenticated_request_is_forbidden() {
        let response = post(
            app(None, None),
            json!({ "action": "skills.list", "params": {} }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn authenticated_non_read_scope_cannot_use_artifact_actions() {
        let help = post(
            app(Some(auth(&["profile"])), None),
            json!({ "action": "help", "params": {} }),
        )
        .await;
        assert_eq!(help.status(), StatusCode::FORBIDDEN);

        let list = post(
            app(Some(auth(&["profile"])), None),
            json!({ "action": "skills.list", "params": {} }),
        )
        .await;
        assert_eq!(list.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn native_skill_action_is_not_an_http_alias() {
        let response = post(
            app(Some(auth(&["lab:read"])), None),
            json!({ "action": "skills.list", "params": { "limit": 5 } }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn product_credential_is_rejected_by_generic_skills_api() {
        let identity = VerifiedIdentity::local_credential(
            Authenticator::ProductCredential,
            "project-credential-1",
        )
        .expect("product credential identity");
        let response = post(
            app(Some(auth(&["lab:read"])), Some(identity)),
            json!({ "action": "skills.list", "params": {} }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
