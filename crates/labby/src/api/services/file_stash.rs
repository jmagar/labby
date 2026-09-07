//! Authenticated, streaming HTTP adapter for the principal-scoped File Stash.

use axum::{
    Json,
    body::Body,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};

use futures::TryStreamExt;
use labby_auth::{AuthContext, VerifiedIdentity};
use serde::Deserialize;
use tokio::io::{AsyncRead, ReadBuf};
use tokio_util::{
    io::{ReaderStream, StreamReader},
    sync::CancellationToken,
};
use tracing::Instrument as _;

use crate::{
    api::{
        error::{ApiError, ToolError},
        route_registry::{RouteAuth, RouteDescriptor, RouteGroup},
        state::AppState,
    },
    dispatch::file_stash::FileStashService,
};

pub fn routes(_state: AppState) -> RouteGroup {
    descriptors()
        .into_iter()
        .fold(RouteGroup::empty(), |group, descriptor| {
            let method = match (descriptor.method, descriptor.path.as_str()) {
                ("GET", "/") => get(list),
                ("POST", "/") => post(action),
                ("GET", "/stats") => get(stats),
                ("POST", "/recipients") => post(recipients),
                ("POST", "/uploads") => post(upload),
                ("GET", "/files/{file_id}") => get(metadata),
                ("GET", "/files/{file_id}/content") => get(download),
                ("PATCH", "/files/{file_id}") => patch(rename),
                ("DELETE", "/files/{file_id}") => delete(remove),
                ("POST", "/files/{file_id}/grants") => post(create_grant),
                ("GET", "/files/{file_id}/grants") => get(list_grants),
                ("DELETE", "/files/{file_id}/grants/{grant_id}") => delete(revoke_grant),
                _ => unreachable!("descriptor and route table must stay aligned"),
            };
            group.route(descriptor, method)
        })
}

pub(crate) fn descriptors() -> Vec<RouteDescriptor> {
    [
        ("GET", "/", "stash_list", "none_expected"),
        ("POST", "/", "stash_action", "action-defined"),
        ("GET", "/stats", "stash_stats", "none_expected"),
        (
            "POST",
            "/recipients",
            "stash_recipients",
            "directory lookup",
        ),
        ("POST", "/uploads", "stash_upload", "creates a file"),
        ("GET", "/files/{file_id}", "stash_metadata", "none_expected"),
        (
            "GET",
            "/files/{file_id}/content",
            "stash_download",
            "none_expected",
        ),
        (
            "PATCH",
            "/files/{file_id}",
            "stash_rename",
            "renames a file",
        ),
        (
            "DELETE",
            "/files/{file_id}",
            "stash_delete",
            "deletes a file and grants",
        ),
        (
            "POST",
            "/files/{file_id}/grants",
            "stash_grant_create",
            "creates a read grant",
        ),
        (
            "GET",
            "/files/{file_id}/grants",
            "stash_grant_list",
            "none_expected",
        ),
        (
            "DELETE",
            "/files/{file_id}/grants/{grant_id}",
            "stash_grant_revoke",
            "revokes a grant",
        ),
    ]
    .into_iter()
    .map(|(method, path, handler, effects)| {
        RouteDescriptor::new(method, path, handler, "stash", RouteAuth::V1)
            .when("Linux with API auth configured; operations require runtime readiness")
            .private_no_store()
            .non_enumerating()
            .side_effects(effects)
    })
    .collect()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PageQuery {
    cursor: Option<String>,
    limit: Option<usize>,
    query: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipientQuery {
    query: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameRequest {
    display_name: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantRequest {
    grantee_principal_id: String,
}

macro_rules! observed_handler {
    ($name:ident, $inner:ident, $action:literal, $destructive:literal, ($($arg:ident : $ty:ty),* $(,)?)) => {
        async fn $name($($arg: $ty),*) -> Result<Response, ApiError> {
            observe_api($action, None, None, $destructive, $inner($($arg),*)).await
        }
    };
}

async fn action(
    state: State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    request: Result<Json<crate::api::ActionRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let action = request
        .as_ref()
        .map_or("stash.action", |request| request.action.as_str())
        .to_owned();
    let destructive = action == "stash.delete";
    observe_api(&action, None, None, destructive, async move {
        let request = request.map_err(|_| stable("invalid_param"))?;
        action_impl(state, headers, auth, identity, request).await
    })
    .await
}

async fn list(
    state: State<AppState>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let action = if query.as_ref().is_ok_and(|query| query.query.is_some()) {
        "stash.search"
    } else {
        "stash.list"
    };
    observe_api(action, None, None, false, async move {
        let query = query.map_err(|_| stable("invalid_param"))?;
        list_impl(state, identity, query).await
    })
    .await
}
observed_handler!(stats, stats_impl, "stash.stats", false, (
    state: State<AppState>, identity: Option<axum::Extension<VerifiedIdentity>>,
));
async fn recipients(
    state: State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    query: Result<Json<RecipientQuery>, JsonRejection>,
) -> Result<Response, ApiError> {
    observe_api("stash.recipients.search", None, None, false, async move {
        recipients_impl(
            state,
            headers,
            auth,
            identity,
            query.map_err(|_| stable("invalid_param"))?,
        )
        .await
    })
    .await
}
async fn metadata(
    state: State<AppState>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    file_id: Path<String>,
) -> Result<Response, ApiError> {
    let object_id = file_id.0.clone();
    observe_api(
        "stash.metadata",
        Some(&object_id),
        None,
        false,
        metadata_impl(state, identity, file_id),
    )
    .await
}
async fn rename(
    state: State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    file_id: Path<String>,
    body: Result<Json<RenameRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let object_id = file_id.0.clone();
    observe_api("stash.rename", Some(&object_id), None, false, async move {
        rename_impl(
            state,
            headers,
            auth,
            identity,
            file_id,
            body.map_err(|_| stable("invalid_param"))?,
        )
        .await
    })
    .await
}
async fn remove(
    state: State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    file_id: Path<String>,
) -> Result<Response, ApiError> {
    let object_id = file_id.0.clone();
    observe_api(
        "stash.delete",
        Some(&object_id),
        None,
        true,
        remove_impl(state, headers, auth, identity, file_id),
    )
    .await
}
async fn create_grant(
    state: State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    file_id: Path<String>,
    body: Result<Json<GrantRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let object_id = file_id.0.clone();
    observe_api(
        "stash.grants.create",
        Some(&object_id),
        None,
        false,
        async move {
            create_grant_impl(
                state,
                headers,
                auth,
                identity,
                file_id,
                body.map_err(|_| stable("invalid_param"))?,
            )
            .await
        },
    )
    .await
}
async fn list_grants(
    state: State<AppState>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    file_id: Path<String>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let object_id = file_id.0.clone();
    observe_api(
        "stash.grants.list",
        Some(&object_id),
        None,
        false,
        async move {
            list_grants_impl(
                state,
                identity,
                file_id,
                query.map_err(|_| stable("invalid_param"))?,
            )
            .await
        },
    )
    .await
}
async fn revoke_grant(
    state: State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    path: Path<(String, String)>,
) -> Result<Response, ApiError> {
    let object_id = path.0.0.clone();
    let grant_id = path.0.1.clone();
    observe_api(
        "stash.grants.revoke",
        Some(&object_id),
        Some(&grant_id),
        false,
        revoke_grant_impl(state, headers, auth, identity, path),
    )
    .await
}
observed_handler!(upload, upload_impl, "stash.upload", false, (
    state: State<AppState>, headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>, body: Body,
));
async fn action_impl(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    Json(request): Json<crate::api::ActionRequest>,
) -> Result<Response, ApiError> {
    if matches!(
        request.action.as_str(),
        "stash.rename" | "stash.delete" | "stash.grants.create" | "stash.grants.revoke"
    ) {
        mutation_csrf(&headers, auth.as_ref(), &request.action)?;
    }
    let action = request.action;
    let (principal, validated_grantee) = if action == "stash.grants.create" {
        let recipient = request
            .params
            .get("grantee_principal_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| stable("invalid_param"))?
            .to_owned();
        let (owner, recipient, lease) =
            principal_and_recipient(&state, identity, recipient).await?;
        (
            ResolvedStashPrincipal {
                id: owner,
                _lease: lease,
            },
            Some(recipient),
        )
    } else {
        (principal(&state, identity).await?, None)
    };
    let response = crate::dispatch::file_stash::dispatch_for_principal(
        &service(&state),
        &principal,
        "api",
        &action,
        request.params,
        validated_grantee.as_ref(),
    )
    .await
    .map_err(|error| ApiError::new(error).with_service_action("stash", &action))?;
    Ok(result(response))
}

async fn principal_and_recipient(
    state: &AppState,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    recipient: String,
) -> Result<
    (
        crate::access::AccessPrincipalId,
        crate::access::AccessPrincipalId,
        crate::access::ActiveFileStashPrincipalLease,
    ),
    ApiError,
> {
    let Some(axum::Extension(identity)) = identity else {
        return Err(stable("not_found"));
    };
    state
        .access_runtime
        .resolve_and_lease_file_stash_participants(identity, recipient)
        .await
        .map_err(map_principal_error)
}

fn map_principal_error(error: crate::access::FileStashPrincipalResolutionError) -> ApiError {
    match error {
        crate::access::FileStashPrincipalResolutionError::IdentityUnavailable => {
            stable("not_found")
        }
        crate::access::FileStashPrincipalResolutionError::StoreUnavailable
        | crate::access::FileStashPrincipalResolutionError::Runtime(_) => {
            stable("service_unavailable")
        }
    }
}

fn service(state: &AppState) -> FileStashService {
    FileStashService::new(
        state.file_stash_runtime.clone(),
        state.access_runtime.clone(),
        usize::from(state.config.file_stash.page_size),
        state.config.file_stash.max_query_bytes,
    )
}

async fn observe_api<T>(
    action: &str,
    object_id: Option<&str>,
    grant_id: Option<&str>,
    destructive: bool,
    future: impl Future<Output = Result<T, ApiError>>,
) -> Result<T, ApiError> {
    let started = std::time::Instant::now();
    let (result, details) = crate::dispatch::file_stash::collect_observation_details(future).await;
    crate::dispatch::file_stash::observe_operation(
        "api",
        action,
        if result.is_ok() { "success" } else { "error" },
        details.object_id.as_deref().or(object_id),
        details.grant_id.as_deref().or(grant_id),
        details.byte_count,
        destructive,
        u64::try_from(started.elapsed().as_millis())
            .unwrap_or(u64::MAX)
            .max(1),
        result.as_ref().err().map(|error| error.error.kind()),
    );
    result
}

async fn principal(
    state: &AppState,
    identity: Option<axum::Extension<VerifiedIdentity>>,
) -> Result<ResolvedStashPrincipal, ApiError> {
    let Some(axum::Extension(identity)) = identity else {
        return Err(stable("not_found"));
    };
    state
        .access_runtime
        .resolve_and_lease_file_stash_principal(identity)
        .await
        .map(|(id, lease)| ResolvedStashPrincipal { id, _lease: lease })
        .map_err(map_principal_error)
}

struct ResolvedStashPrincipal {
    id: crate::access::AccessPrincipalId,
    _lease: crate::access::ActiveFileStashPrincipalLease,
}

impl std::ops::Deref for ResolvedStashPrincipal {
    type Target = crate::access::AccessPrincipalId;

    fn deref(&self) -> &Self::Target {
        &self.id
    }
}

fn stable(kind: &str) -> ApiError {
    ApiError::new(ToolError::Sdk {
        sdk_kind: kind.to_owned(),
        message: "File Stash operation failed".to_owned(),
    })
    .with_service_action("stash", "stash.http")
}
fn result<T: serde::Serialize>(value: T) -> Response {
    let mut response = Json(value).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}
fn mutation_csrf(
    headers: &HeaderMap,
    auth: Option<&axum::Extension<AuthContext>>,
    action: &str,
) -> Result<(), ApiError> {
    crate::api::services::require_session_csrf(action, headers, auth.map(|v| &v.0))
        .map_err(ApiError::from)
}

async fn list_impl(
    State(state): State<AppState>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    Query(q): Query<PageQuery>,
) -> Result<Response, ApiError> {
    let principal = principal(&state, identity).await?;
    let page = if let Some(query) = q.query {
        let stash = service(&state);
        let page = crate::dispatch::file_stash::observe_result(
            "api",
            "stash.search",
            None,
            None,
            None,
            false,
            stash.search(&principal, &query, q.cursor.as_deref(), q.limit),
        )
        .await?;
        crate::dispatch::file_stash::capture_observation_details(None, None, None);
        page
    } else {
        let stash = service(&state);
        let page = crate::dispatch::file_stash::observe_result(
            "api",
            "stash.list",
            None,
            None,
            None,
            false,
            stash.list(&principal, q.cursor.as_deref(), q.limit),
        )
        .await?;
        crate::dispatch::file_stash::capture_observation_details(None, None, None);
        page
    };
    Ok(result(page))
}
async fn stats_impl(
    State(state): State<AppState>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
) -> Result<Response, ApiError> {
    let principal = principal(&state, identity).await?;
    let stash = service(&state);
    let stats = crate::dispatch::file_stash::observe_result(
        "api",
        "stash.stats",
        None,
        None,
        None,
        false,
        stash.stats(&principal),
    )
    .await?;
    crate::dispatch::file_stash::capture_observation_details(
        None,
        None,
        Some(stats.owned_committed_bytes),
    );
    Ok(result(stats))
}
async fn recipients_impl(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    Json(q): Json<RecipientQuery>,
) -> Result<Response, ApiError> {
    mutation_csrf(&headers, auth.as_ref(), "stash.recipients.search")?;
    if !auth
        .as_ref()
        .is_some_and(|context| context.0.scopes.iter().any(|scope| scope == "lab:admin"))
    {
        return Err(stable("not_found"));
    }
    let Some(axum::Extension(identity)) = identity else {
        return Err(stable("not_found"));
    };
    // Recipient search performs its own bounded AccessStore operation. Resolve
    // the caller without retaining the connection-admission lease so the
    // search can acquire it and install a cancellable SQLite deadline.
    let principal = state
        .access_runtime
        .resolve_file_stash_principal(identity)
        .await
        .map_err(map_principal_error)?;
    let query = q.query.trim();
    if query.chars().count() < 3 || query.len() > 128 {
        return Err(stable("invalid_param"));
    }
    let store = state
        .access_runtime
        .store()
        .await
        .map_err(|_| stable("service_unavailable"))?;
    let values = crate::dispatch::file_stash::observe_result(
        "api",
        "stash.recipients.search",
        None,
        None,
        None,
        false,
        async {
            store
                .search_file_stash_recipients(
                    principal,
                    query.to_owned(),
                    20,
                    std::time::Duration::from_millis(state.config.file_stash.database_deadline_ms),
                )
                .await
                .map_err(|error| ToolError::Sdk {
                    sdk_kind: if error.to_string().contains("deadline exceeded") {
                        "busy"
                    } else {
                        "service_unavailable"
                    }
                    .into(),
                    message: "File Stash operation failed".into(),
                })
        },
    )
    .await?;
    Ok(result(serde_json::json!({"recipients": values})))
}
async fn metadata_impl(
    State(state): State<AppState>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    Path(file_id): Path<String>,
) -> Result<Response, ApiError> {
    let principal = principal(&state, identity).await?;
    let stash = service(&state);
    let file = crate::dispatch::file_stash::observe_result(
        "api",
        "stash.metadata",
        Some(&file_id),
        None,
        None,
        false,
        stash.metadata(&principal, &file_id),
    )
    .await?;
    crate::dispatch::file_stash::capture_observation_details(
        Some(&file_id),
        None,
        Some(file.size_bytes),
    );
    Ok(result(file))
}
async fn rename_impl(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    Path(file_id): Path<String>,
    Json(body): Json<RenameRequest>,
) -> Result<Response, ApiError> {
    mutation_csrf(&headers, auth.as_ref(), "stash.rename")?;
    let principal = principal(&state, identity).await?;
    let stash = service(&state);
    let file = crate::dispatch::file_stash::observe_result(
        "api",
        "stash.rename",
        Some(&file_id),
        None,
        None,
        false,
        stash.rename(&principal, &file_id, &body.display_name),
    )
    .await?;
    crate::dispatch::file_stash::capture_observation_details(
        Some(&file_id),
        None,
        Some(file.size_bytes),
    );
    Ok(result(file))
}
async fn remove_impl(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    Path(file_id): Path<String>,
) -> Result<Response, ApiError> {
    mutation_csrf(&headers, auth.as_ref(), "stash.delete")?;
    let principal = principal(&state, identity).await?;
    let stash = service(&state);
    crate::dispatch::file_stash::observe_result(
        "api",
        "stash.delete",
        Some(&file_id),
        None,
        None,
        true,
        stash.delete(&principal, &file_id),
    )
    .await?;
    crate::dispatch::file_stash::capture_observation_details(Some(&file_id), None, None);
    Ok(StatusCode::NO_CONTENT.into_response())
}
async fn create_grant_impl(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    Path(file_id): Path<String>,
    Json(body): Json<GrantRequest>,
) -> Result<Response, ApiError> {
    mutation_csrf(&headers, auth.as_ref(), "stash.grants.create")?;
    let (principal, grantee, lease) =
        principal_and_recipient(&state, identity, body.grantee_principal_id).await?;
    let principal = ResolvedStashPrincipal {
        id: principal,
        _lease: lease,
    };
    let stash = service(&state);
    let grant = crate::dispatch::file_stash::observe_result(
        "api",
        "stash.grants.create",
        Some(&file_id),
        None,
        None,
        false,
        stash.create_grant_validated(&principal, &file_id, &grantee),
    )
    .await?;
    crate::dispatch::file_stash::capture_observation_details(
        Some(&file_id),
        Some(&grant.grant_id),
        None,
    );
    Ok((StatusCode::CREATED, result(grant)).into_response())
}
async fn list_grants_impl(
    State(state): State<AppState>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    Path(file_id): Path<String>,
    Query(q): Query<PageQuery>,
) -> Result<Response, ApiError> {
    let principal = principal(&state, identity).await?;
    let stash = service(&state);
    let grants = crate::dispatch::file_stash::observe_result(
        "api",
        "stash.grants.list",
        Some(&file_id),
        None,
        None,
        false,
        stash.grants(&principal, &file_id, q.cursor.as_deref(), q.limit),
    )
    .await?;
    crate::dispatch::file_stash::capture_observation_details(Some(&file_id), None, None);
    Ok(result(grants))
}
async fn revoke_grant_impl(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    Path((file_id, grant_id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    mutation_csrf(&headers, auth.as_ref(), "stash.grants.revoke")?;
    let principal = principal(&state, identity).await?;
    let stash = service(&state);
    crate::dispatch::file_stash::observe_result(
        "api",
        "stash.grants.revoke",
        Some(&file_id),
        Some(&grant_id),
        None,
        false,
        stash.revoke_grant(&principal, &file_id, &grant_id),
    )
    .await?;
    crate::dispatch::file_stash::capture_observation_details(Some(&file_id), Some(&grant_id), None);
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn upload_impl(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    body: Body,
) -> Result<Response, ApiError> {
    validate_header_budget(&headers, state.config.file_stash.max_header_bytes)?;
    mutation_csrf(&headers, auth.as_ref(), "stash.upload")?;
    let principal = principal(&state, identity).await?;
    let display_name = headers
        .get("x-labby-stash-filename")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            percent_encoding::percent_decode_str(value)
                .decode_utf8()
                .ok()
        })
        .map(|value| value.into_owned())
        .ok_or_else(|| stable("validation_failed"))?;
    let declared = exact_content_length(&headers)?;
    crate::dispatch::file_stash::capture_observation_details(None, None, Some(declared));
    validate_transfer_headers(&headers)?;
    let svc = service(&state);
    let (reservation, admission) = svc
        .reserve_upload(&principal, &display_name, declared)
        .await?;
    let stream = body.into_data_stream().map_err(std::io::Error::other);
    let reader = StreamReader::new(stream);
    let cancel = CancellationToken::new();
    let mut guard = CancelOnDrop(Some(cancel.clone()));
    // Keep finalization alive after an HTTP request future is dropped so the
    // cancellation signal can drive the shared service's reservation cleanup.
    let upload = tokio::spawn(async move {
        svc.finalize_upload(reservation, admission, reader, cancel)
            .await
    });
    let file_id = crate::dispatch::file_stash::observe_result(
        "api",
        "stash.upload",
        None,
        None,
        Some(declared),
        false,
        async {
            upload.await.map_err(|_| ToolError::Sdk {
                sdk_kind: "service_unavailable".into(),
                message: "File Stash operation failed".into(),
            })?
        },
    )
    .await?;
    guard.0 = None;
    crate::dispatch::file_stash::capture_observation_details(Some(&file_id), None, Some(declared));
    Ok((
        StatusCode::CREATED,
        result(
            serde_json::json!({"file_id": file_id, "uri": format!("stash://me/files/{file_id}")}),
        ),
    )
        .into_response())
}

async fn download(
    State(state): State<AppState>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    Path(file_id): Path<String>,
) -> Result<Response, ApiError> {
    let started = std::time::Instant::now();
    let result: Result<Response, ApiError> = async {
        let principal = principal(&state, identity).await?;
        let stash = service(&state);
        let (file, opened) = stash.open_download(&principal, &file_id, false).await?;
        let size = opened.size;
        let mut response = Response::new(blob_body(opened, file_id.clone(), started));
        *response.status_mut() = StatusCode::OK;
        let headers = response.headers_mut();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        );
        headers.insert(
            header::CONTENT_LENGTH,
            HeaderValue::from_str(&size.to_string()).map_err(|_| stable("integrity_error"))?,
        );
        headers.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&content_disposition(&file.display_name))
                .map_err(|_| stable("integrity_error"))?,
        );
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, no-store"),
        );
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
        headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'none'; sandbox"),
        );
        Ok(response)
    }
    .await;
    if let Err(error) = &result {
        crate::dispatch::file_stash::observe_operation(
            "api",
            "stash.download",
            "error",
            Some(&file_id),
            None,
            None,
            false,
            u64::try_from(started.elapsed().as_millis())
                .unwrap_or(u64::MAX)
                .max(1),
            Some(error.error.kind()),
        );
    }
    result
}

/// Async reader that owns the opened blob until EOF/drop, while its independent
/// watchdog cancels the stream and releases admission at the total deadline.
struct HeldBlob {
    blob: crate::file_stash::OpenedBlob,
    cancel: CancellationToken,
    idle: Pin<Box<tokio::time::Sleep>>,
    observation: DownloadObservation,
}

impl HeldBlob {
    fn new(
        blob: crate::file_stash::OpenedBlob,
        file_id: String,
        started: std::time::Instant,
    ) -> Self {
        let cancel = blob.cancellation();
        let idle = Box::pin(tokio::time::sleep(blob.idle_timeout));
        Self {
            blob,
            cancel: cancel.clone(),
            idle,
            observation: DownloadObservation::new(file_id, started, cancel),
        }
    }
}

impl AsyncRead for HeldBlob {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if this.cancel.is_cancelled() {
            this.observation.finish("error", Some("timeout"));
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "File Stash download exceeded its total deadline",
            )));
        }
        let before = buf.filled().len();
        match Pin::new(&mut this.blob).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                if buf.filled().len() > before {
                    this.observation.add_bytes(buf.filled().len() - before);
                    this.idle
                        .as_mut()
                        .reset(tokio::time::Instant::now() + this.blob.idle_timeout);
                } else {
                    this.observation.finish("success", None);
                }
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(error)) => {
                this.observation
                    .finish("error", Some("service_unavailable"));
                Poll::Ready(Err(error))
            }
            Poll::Pending => match this.idle.as_mut().poll(cx) {
                Poll::Ready(()) => {
                    this.observation.finish("error", Some("timeout"));
                    Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "File Stash download exceeded its idle deadline",
                    )))
                }
                Poll::Pending => Poll::Pending,
            },
        }
    }
}

impl Drop for HeldBlob {
    fn drop(&mut self) {
        self.observation.finish("error", Some("cancelled"));
    }
}

struct DownloadObservation {
    state: Arc<Mutex<DownloadObservationState>>,
}

struct DownloadObservationState {
    file_id: String,
    started: std::time::Instant,
    transferred_bytes: u64,
    finished: bool,
}

impl DownloadObservation {
    fn new(file_id: String, started: std::time::Instant, cancel: CancellationToken) -> Self {
        let state = Arc::new(Mutex::new(DownloadObservationState {
            file_id,
            started,
            transferred_bytes: 0,
            finished: false,
        }));
        let watchdog_state = Arc::clone(&state);
        let request_span = tracing::Span::current();
        tokio::spawn(
            async move {
                cancel.cancelled().await;
                finish_download_observation(&watchdog_state, "error", Some("timeout"));
            }
            .instrument(request_span),
        );
        Self { state }
    }

    fn add_bytes(&self, count: usize) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.finished {
            state.transferred_bytes = state
                .transferred_bytes
                .saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
        }
    }

    fn finish(&self, result: &'static str, kind: Option<&str>) {
        finish_download_observation(&self.state, result, kind);
    }
}

fn finish_download_observation(
    observation: &Arc<Mutex<DownloadObservationState>>,
    result: &'static str,
    kind: Option<&str>,
) {
    let terminal = {
        let mut state = observation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.finished {
            return;
        }
        state.finished = true;
        (
            state.file_id.clone(),
            state.transferred_bytes,
            u64::try_from(state.started.elapsed().as_millis())
                .unwrap_or(u64::MAX)
                .max(1),
        )
    };
    crate::dispatch::file_stash::observe_operation(
        "api",
        "stash.download",
        result,
        Some(&terminal.0),
        None,
        Some(terminal.1),
        false,
        terminal.2,
        kind,
    );
}

impl Drop for DownloadObservation {
    fn drop(&mut self) {
        self.finish("error", Some("cancelled"));
    }
}

fn blob_body(
    opened: crate::file_stash::OpenedBlob,
    file_id: String,
    started: std::time::Instant,
) -> Body {
    Body::from_stream(ReaderStream::new(HeldBlob::new(opened, file_id, started)))
}

fn validate_header_budget(headers: &HeaderMap, limit: usize) -> Result<(), ApiError> {
    let total = headers.iter().try_fold(0usize, |total, (name, value)| {
        // Include conservative HTTP delimiter overhead in addition to the
        // bytes controlled by the caller.
        total
            .checked_add(name.as_str().len())?
            .checked_add(value.as_bytes().len())?
            .checked_add(4)
    });
    if total.is_some_and(|total| total <= limit) {
        Ok(())
    } else {
        Err(stable("invalid_param"))
    }
}

fn exact_content_length(headers: &HeaderMap) -> Result<u64, ApiError> {
    let values = headers.get_all(header::CONTENT_LENGTH);
    let mut iter = values.iter();
    let value = iter.next().ok_or_else(|| stable("invalid_param"))?;
    if iter.next().is_some() {
        return Err(stable("invalid_param"));
    }
    value
        .to_str()
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or_else(|| stable("invalid_param"))
}
fn validate_transfer_headers(headers: &HeaderMap) -> Result<(), ApiError> {
    if headers.contains_key(header::TRANSFER_ENCODING) {
        return Err(stable("invalid_param"));
    }
    let mut encodings = headers.get_all(header::CONTENT_ENCODING).iter();
    if let Some(value) = encodings.next() {
        if encodings.next().is_some()
            || !value
                .to_str()
                .ok()
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("identity"))
        {
            return Err(stable("invalid_param"));
        }
    }
    Ok(())
}
fn content_disposition(name: &str) -> String {
    let fallback: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() && !matches!(c, '"' | '\\' | '/' | ';') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let encoded = rfc5987(name.as_bytes());
    format!("attachment; filename=\"{fallback}\"; filename*=UTF-8''{encoded}")
}
fn rfc5987(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(bytes.len());
    for &byte in bytes {
        if byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'!' | b'#' | b'$' | b'&' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
            )
        {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    out
}
struct CancelOnDrop(Option<CancellationToken>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(token) = self.0.take() {
            token.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    use axum::body::Bytes;
    use axum::{Router, http::Request};
    #[cfg(target_os = "linux")]
    use std::sync::Arc;
    use tower::ServiceExt as _;
    use tracing_subscriber::prelude::*;

    fn mounted(state: AppState) -> Router {
        Router::new()
            .nest("/v1/stash", routes(state.clone()).router)
            .with_state(state)
    }

    #[test]
    fn upload_requires_one_decimal_content_length() {
        let mut headers = HeaderMap::new();
        assert!(exact_content_length(&headers).is_err());
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("12"));
        assert_eq!(exact_content_length(&headers).unwrap(), 12);
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("-1"));
        assert!(exact_content_length(&headers).is_err());
        headers.append(header::CONTENT_LENGTH, HeaderValue::from_static("1"));
        assert!(exact_content_length(&headers).is_err());
    }

    #[tokio::test]
    async fn api_terminal_observation_covers_outer_handler_failures() {
        let _lock = crate::test_support::TRACING_TEST_LOCK.lock().unwrap();
        let logs = crate::test_support::SharedBuf::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_ansi(false)
                .without_time()
                .with_writer(logs.clone()),
        );
        let dispatch = tracing::Dispatch::new(subscriber);
        let _subscriber = tracing::dispatcher::set_default(&dispatch);
        crate::test_support::rebuild_tracing_interest_cache();
        let request = tracing::info_span!("http.request", request_id = "request-api-123");
        let _request = request.enter();

        let response = mounted(AppState::new())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/stash")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let output = crate::test_support::captured_logs(&logs);
        assert_eq!(output.matches("file stash operation").count(), 1);
        assert!(output.contains("\"surface\":\"api\""));
        assert!(output.contains("\"action\":\"stash.action\""));
        assert!(output.contains("\"result\":\"error\""));
        assert!(output.contains("\"kind\":\"invalid_param\""));
        assert!(output.contains("request-api-123"));
    }

    #[tokio::test]
    async fn download_observation_tracks_eof_partial_drop_and_unpolled_timeout_exactly_once() {
        let _lock = crate::test_support::TRACING_TEST_LOCK.lock().unwrap();
        let logs = crate::test_support::SharedBuf::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_ansi(false)
                .without_time()
                .with_writer(logs.clone()),
        );
        let dispatch = tracing::Dispatch::new(subscriber);
        let _subscriber = tracing::dispatcher::set_default(&dispatch);
        crate::test_support::rebuild_tracing_interest_cache();

        let eof_cancel = CancellationToken::new();
        let eof = DownloadObservation::new(
            "01ARZ3NDEKTSV4RRFFQ69G5FAV".into(),
            std::time::Instant::now(),
            eof_cancel.clone(),
        );
        eof.add_bytes(7);
        eof.finish("success", None);
        eof_cancel.cancel();
        drop(eof);

        let partial_cancel = CancellationToken::new();
        let partial = DownloadObservation::new(
            "01ARZ3NDEKTSV4RRFFQ69G5FAW".into(),
            std::time::Instant::now(),
            partial_cancel.clone(),
        );
        partial.add_bytes(3);
        drop(partial);
        partial_cancel.cancel();

        let timeout_cancel = CancellationToken::new();
        let timeout = {
            let request =
                tracing::info_span!("http.request", request_id = "request-download-timeout-123");
            let _request = request.enter();
            DownloadObservation::new(
                "01ARZ3NDEKTSV4RRFFQ69G5FAX".into(),
                std::time::Instant::now(),
                timeout_cancel.clone(),
            )
        };
        timeout_cancel.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if crate::test_support::captured_logs(&logs).contains("\"kind\":\"timeout\"") {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        drop(timeout);

        let output = crate::test_support::captured_logs(&logs);
        assert_eq!(output.matches("file stash operation").count(), 3);
        assert!(output.contains("\"result\":\"success\""));
        assert!(output.contains("\"byte_count\":7"));
        assert!(output.contains("\"kind\":\"cancelled\""));
        assert!(output.contains("\"byte_count\":3"));
        assert_eq!(output.matches("\"kind\":\"timeout\"").count(), 1);
        let timeout_event = output
            .lines()
            .find(|line| line.contains("\"kind\":\"timeout\""))
            .expect("timeout terminal event");
        assert!(timeout_event.contains("request-download-timeout-123"));
    }

    #[test]
    fn upload_rejects_transfer_and_nonidentity_content_codings() {
        let mut headers = HeaderMap::new();
        assert!(validate_transfer_headers(&headers).is_ok());
        headers.insert(
            header::CONTENT_ENCODING,
            HeaderValue::from_static("identity"),
        );
        assert!(validate_transfer_headers(&headers).is_ok());
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        assert!(validate_transfer_headers(&headers).is_err());
        headers.remove(header::CONTENT_ENCODING);
        headers.insert(
            header::TRANSFER_ENCODING,
            HeaderValue::from_static("chunked"),
        );
        assert!(validate_transfer_headers(&headers).is_err());
    }

    #[test]
    fn upload_header_budget_accepts_boundary_and_rejects_one_byte_over() {
        let mut headers = HeaderMap::new();
        headers.insert("x", HeaderValue::from_static("1234"));
        assert!(validate_header_budget(&headers, 9).is_ok());
        assert!(validate_header_budget(&headers, 8).is_err());
        assert_eq!(
            crate::config::FileStashPreferences::default().max_header_bytes,
            16 * 1024
        );
    }

    #[test]
    fn attachment_header_has_safe_fallback_and_rfc5987_name() {
        assert_eq!(
            content_disposition("résumé \"final\"/v1.txt"),
            "attachment; filename=\"r_sum___final__v1.txt\"; filename*=UTF-8''r%C3%A9sum%C3%A9%20%22final%22%2Fv1.txt"
        );
    }

    #[test]
    fn route_inventory_is_private_authenticated_and_non_enumerating() {
        let descriptors = descriptors();
        assert_eq!(descriptors.len(), 12);
        assert!(descriptors.iter().all(|route| route.auth == RouteAuth::V1));
        assert!(
            descriptors
                .iter()
                .all(|route| route.cache_posture == "private, no-store")
        );
        assert!(
            descriptors
                .iter()
                .all(|route| route.failure_disclosure == "uniform non-enumerating denial")
        );
    }

    #[test]
    fn cookie_mutations_require_exact_csrf_while_bearer_does_not() {
        let browser = AuthContext {
            sub: "principal".into(),
            issuer: "https://issuer.example".into(),
            scopes: Vec::new(),
            actor_key: None,
            email: None,
            via_session: true,
            csrf_token: Some("secret".into()),
        };
        let mut browser = axum::Extension(browser);
        assert!(mutation_csrf(&HeaderMap::new(), Some(&browser), "stash.rename").is_err());
        let mut headers = HeaderMap::new();
        headers.insert(
            labby_auth::session::BROWSER_CSRF_HEADER_NAME,
            HeaderValue::from_static("secret"),
        );
        assert!(mutation_csrf(&headers, Some(&browser), "stash.rename").is_ok());
        browser.0.via_session = false;
        browser.0.csrf_token = None;
        assert!(mutation_csrf(&HeaderMap::new(), Some(&browser), "stash.rename").is_ok());
    }

    #[tokio::test]
    async fn router_fails_closed_without_a_verified_identity() {
        let response = mounted(AppState::new())
            .oneshot(
                Request::builder()
                    .uri("/v1/stash/stats")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn router_rejects_cookie_mutation_without_csrf_before_dispatch() {
        let auth = AuthContext {
            sub: "principal".into(),
            issuer: "https://issuer.example".into(),
            scopes: Vec::new(),
            actor_key: None,
            email: None,
            via_session: true,
            csrf_token: Some("secret".into()),
        };
        let response = mounted(AppState::new())
            .layer(axum::Extension(auth))
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/v1/stash/files/01J00000000000000000000000")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"display_name":"renamed.txt"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn router_rejects_oversized_upload_headers_before_identity_resolution() {
        let state = AppState::new();
        let oversized = "x".repeat(state.config.file_stash.max_header_bytes);
        let response = mounted(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/stash/uploads")
                    .header("x-labby-stash-filename", "a.txt")
                    .header("x-fill", oversized)
                    .header(header::CONTENT_LENGTH, "0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn response_body_holds_download_admission_until_drop() {
        let temp = tempfile::TempDir::new().unwrap();
        let preferences = crate::config::FileStashPreferences {
            max_file_bytes: 16,
            principal_quota_bytes: 16,
            instance_quota_bytes: 16,
            max_concurrent_downloads: 1,
            database_deadline_ms: 10_000,
            janitor_interval_seconds: 3_600,
            ..crate::config::FileStashPreferences::default()
        };
        let runtime = Arc::new(
            crate::file_stash::FileStashRuntime::initialize_with_preferences(
                std::fs::canonicalize(temp.path()).unwrap().join("stash"),
                preferences,
            )
            .await,
        );
        let service = FileStashService::new(
            Arc::clone(&runtime),
            Arc::new(crate::access::AccessRuntime::blocked_unavailable()),
            50,
            256,
        );
        let principal = crate::access::AccessPrincipalId::for_test("owner");
        let (reservation, admission) = service
            .reserve_upload(&principal, "held.txt", 1)
            .await
            .unwrap();
        let file_id = service
            .finalize_upload(reservation, admission, &b"x"[..], CancellationToken::new())
            .await
            .unwrap();

        let (_, opened) = service
            .open_download(&principal, &file_id, false)
            .await
            .unwrap();
        let body = blob_body(opened, file_id.clone(), std::time::Instant::now());
        assert!(
            service
                .open_download(&principal, &file_id, false)
                .await
                .is_err()
        );
        drop(body);
        service
            .open_download(&principal, &file_id, false)
            .await
            .unwrap();
        runtime.shutdown().await;
    }

    #[cfg(target_os = "linux")]
    async fn ready_router_fixture() -> (
        Router,
        FileStashService,
        crate::access::AccessPrincipalId,
        Arc<crate::file_stash::FileStashRuntime>,
        tempfile::TempDir,
    ) {
        use labby_auth::Authenticator;
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::TempDir::new().unwrap();
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let access =
            Arc::new(crate::access::AccessRuntime::initialize(root.join("access.db")).await);
        let identity = VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            "owner",
        )
        .unwrap();
        crate::access::bootstrap_owner(&access, identity.clone(), "Local".into(), "Default".into())
            .await
            .unwrap();
        let principal = access
            .resolve_file_stash_principal(identity.clone())
            .await
            .unwrap();
        let stash =
            Arc::new(crate::file_stash::FileStashRuntime::initialize(root.join("stash")).await);
        let state = AppState::new()
            .with_access_runtime(Arc::clone(&access))
            .with_file_stash_runtime(Arc::clone(&stash));
        let service = service(&state);
        let auth = AuthContext {
            sub: "owner".into(),
            issuer: "https://accounts.google.com".into(),
            scopes: Vec::new(),
            actor_key: None,
            email: None,
            via_session: false,
            csrf_token: None,
        };
        let router = mounted(state)
            .layer(axum::Extension(identity))
            .layer(axum::Extension(auth));
        (router, service, principal, stash, temp)
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn router_reports_raw_body_length_mismatch_without_committing_usage() {
        let (router, service, principal, runtime, _temp) = ready_router_fixture().await;
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/stash/uploads")
                    .header("x-labby-stash-filename", "a.txt")
                    .header(header::CONTENT_LENGTH, "2")
                    .body(Body::from("x"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            service
                .stats(&principal)
                .await
                .unwrap()
                .owned_committed_bytes,
            0
        );
        runtime.shutdown().await;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn dropped_router_upload_cancels_and_releases_reserved_usage() {
        let (router, service, principal, runtime, _temp) = ready_router_fixture().await;
        let pending = futures::stream::pending::<Result<Bytes, std::io::Error>>();
        let request = Request::builder()
            .method("POST")
            .uri("/v1/stash/uploads")
            .header("x-labby-stash-filename", "pending.txt")
            .header(header::CONTENT_LENGTH, "1")
            .body(Body::from_stream(pending))
            .unwrap();
        let task = tokio::spawn(router.oneshot(request));
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if service
                    .stats(&principal)
                    .await
                    .unwrap()
                    .owned_reserved_bytes
                    == 1
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            service
                .stats(&principal)
                .await
                .unwrap()
                .owned_reserved_bytes,
            1
        );
        task.abort();
        drop(task.await);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if service
                    .stats(&principal)
                    .await
                    .unwrap()
                    .owned_reserved_bytes
                    == 0
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            service
                .stats(&principal)
                .await
                .unwrap()
                .owned_reserved_bytes,
            0
        );
        runtime.shutdown().await;
    }
}
