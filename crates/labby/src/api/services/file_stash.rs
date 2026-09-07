//! Authenticated, streaming HTTP adapter for the principal-scoped File Stash.

use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
};
use std::{
    pin::Pin,
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
            .when("Linux or Android with API auth configured; operations require runtime readiness")
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
struct OwnerQuery {
    owner_kind: Option<String>,
    owner_id: Option<String>,
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

async fn action(
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
    let principal = selected_principal(
        &state,
        identity,
        auth.as_ref(),
        request
            .params
            .get("owner_kind")
            .and_then(serde_json::Value::as_str),
        request
            .params
            .get("owner_id")
            .and_then(serde_json::Value::as_str),
        &request.action,
    )
    .await?;
    principal
        .validate_before_commit()
        .await
        .map_err(|_| stable("not_found"))?;
    let action = request.action;
    let response = crate::dispatch::file_stash::dispatch_for_principal(
        &service(&state),
        &principal,
        "api",
        &action,
        request.params,
    )
    .await
    .map_err(|error| ApiError::new(error).with_service_action("stash", &action))?;
    Ok(result(response))
}

async fn selected_principal(
    state: &AppState,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    auth: Option<&axum::Extension<AuthContext>>,
    kind: Option<&str>,
    owner_id: Option<&str>,
    action: &str,
) -> Result<crate::access::FileStashOwnerAuthorization, ApiError> {
    use labby_primitives::access::{Capability, OwnerScope, PrincipalId, TeamId};
    let identity = identity.ok_or_else(|| stable("not_found"))?.0;
    let owner = match kind {
        None | Some("personal") => {
            let principal = state
                .access_runtime
                .resolve_file_stash_principal(identity.clone())
                .await
                .map_err(|_| stable("not_found"))?;
            OwnerScope::Personal(
                PrincipalId::new(principal.as_str()).map_err(|_| stable("not_found"))?,
            )
        }
        Some("team") => OwnerScope::Team(
            TeamId::new(owner_id.ok_or_else(|| stable("not_found"))?)
                .map_err(|_| stable("not_found"))?,
        ),
        _ => return Err(stable("not_found")),
    };
    let capability = match action {
        "stash.list" | "stash.search" | "stash.stats" | "stash.metadata" | "stash.download" => {
            Capability::ScopeRead
        }
        "stash.delete" => Capability::ScopeDelete,
        "stash.upload" => Capability::ScopeCreate,
        _ => Capability::ScopeManage,
    };
    let ceiling = auth.map_or_else(crate::access::AuthorityCeiling::trusted_local, |a| {
        crate::access::AuthorityCeiling::from_auth_context(&a.0)
    });
    state
        .access_runtime
        .authorize_file_stash_owner(identity, ceiling, owner, action, capability, unix_millis())
        .await
        .map_err(|_| stable("not_found"))
}

fn selected_owner_headers(headers: &HeaderMap) -> (Option<&str>, Option<&str>) {
    (
        headers
            .get("x-labby-owner-kind")
            .and_then(|v| v.to_str().ok()),
        headers
            .get("x-labby-owner-id")
            .and_then(|v| v.to_str().ok()),
    )
}
fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |v| u64::try_from(v.as_millis()).unwrap_or(u64::MAX))
}

fn service(state: &AppState) -> FileStashService {
    FileStashService::new(
        state.file_stash_runtime.clone(),
        state.access_runtime.clone(),
        usize::from(state.config.file_stash.page_size),
        state.config.file_stash.max_query_bytes,
    )
}

async fn principal(
    state: &AppState,
    identity: Option<axum::Extension<VerifiedIdentity>>,
) -> Result<crate::access::AccessPrincipalId, ApiError> {
    let Some(axum::Extension(identity)) = identity else {
        return Err(stable("not_found"));
    };
    state
        .access_runtime
        .resolve_file_stash_principal(identity)
        .await
        .map_err(|error| match error {
            crate::access::FileStashPrincipalResolutionError::IdentityUnavailable => {
                stable("not_found")
            }
            crate::access::FileStashPrincipalResolutionError::StoreUnavailable => {
                stable("service_unavailable")
            }
            crate::access::FileStashPrincipalResolutionError::Runtime(_) => {
                stable("service_unavailable")
            }
        })
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

async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    Query(q): Query<PageQuery>,
) -> Result<Response, ApiError> {
    let (kind, id) = selected_owner_headers(&headers);
    let principal =
        selected_principal(&state, identity, auth.as_ref(), kind, id, "stash.list").await?;
    principal
        .validate_before_commit()
        .await
        .map_err(|_| stable("not_found"))?;
    let page = if let Some(query) = q.query {
        let page = service(&state)
            .search(&principal, &query, q.cursor.as_deref(), q.limit)
            .await?;
        crate::dispatch::file_stash::observe_operation(
            "api",
            "stash.search",
            "success",
            None,
            None,
            None,
            false,
        );
        page
    } else {
        let page = service(&state)
            .list(&principal, q.cursor.as_deref(), q.limit)
            .await?;
        crate::dispatch::file_stash::observe_operation(
            "api",
            "stash.list",
            "success",
            None,
            None,
            None,
            false,
        );
        page
    };
    Ok(result(page))
}
async fn stats(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
) -> Result<Response, ApiError> {
    let (kind, id) = selected_owner_headers(&headers);
    let principal =
        selected_principal(&state, identity, auth.as_ref(), kind, id, "stash.stats").await?;
    principal
        .validate_before_commit()
        .await
        .map_err(|_| stable("not_found"))?;
    let stats = service(&state).stats(&principal).await?;
    crate::dispatch::file_stash::observe_operation(
        "api",
        "stash.stats",
        "success",
        None,
        None,
        Some(stats.owned_committed_bytes),
        false,
    );
    Ok(result(stats))
}
async fn recipients(
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
    let principal = principal(&state, identity).await?;
    let query = q.query.trim();
    if query.chars().count() < 3 || query.len() > 128 {
        return Err(stable("invalid_param"));
    }
    let store = state
        .access_runtime
        .store()
        .await
        .map_err(|_| stable("service_unavailable"))?;
    let values = tokio::time::timeout(
        std::time::Duration::from_millis(state.config.file_stash.database_deadline_ms),
        store.search_file_stash_recipients(principal, query.to_owned(), 20),
    )
    .await
    .map_err(|_| stable("busy"))?
    .map_err(|_| stable("service_unavailable"))?;
    Ok(result(serde_json::json!({"recipients": values})))
}
async fn metadata(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    Path(file_id): Path<String>,
) -> Result<Response, ApiError> {
    let (kind, id) = selected_owner_headers(&headers);
    let principal =
        selected_principal(&state, identity, auth.as_ref(), kind, id, "stash.metadata").await?;
    principal
        .validate_before_commit()
        .await
        .map_err(|_| stable("not_found"))?;
    let file = service(&state).metadata(&principal, &file_id).await?;
    crate::dispatch::file_stash::observe_operation(
        "api",
        "stash.metadata",
        "success",
        Some(&file_id),
        None,
        Some(file.size_bytes),
        false,
    );
    Ok(result(file))
}
async fn rename(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    Path(file_id): Path<String>,
    Json(body): Json<RenameRequest>,
) -> Result<Response, ApiError> {
    mutation_csrf(&headers, auth.as_ref(), "stash.rename")?;
    let (kind, id) = selected_owner_headers(&headers);
    let principal =
        selected_principal(&state, identity, auth.as_ref(), kind, id, "stash.rename").await?;
    principal
        .validate_before_commit()
        .await
        .map_err(|_| stable("not_found"))?;
    let file = service(&state)
        .rename(&principal, &file_id, &body.display_name)
        .await?;
    crate::dispatch::file_stash::observe_operation(
        "api",
        "stash.rename",
        "success",
        Some(&file_id),
        None,
        Some(file.size_bytes),
        false,
    );
    Ok(result(file))
}
async fn remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    Path(file_id): Path<String>,
) -> Result<Response, ApiError> {
    mutation_csrf(&headers, auth.as_ref(), "stash.delete")?;
    let (kind, id) = selected_owner_headers(&headers);
    let principal =
        selected_principal(&state, identity, auth.as_ref(), kind, id, "stash.delete").await?;
    principal
        .validate_before_commit()
        .await
        .map_err(|_| stable("not_found"))?;
    service(&state).delete(&principal, &file_id).await?;
    crate::dispatch::file_stash::observe_operation(
        "api",
        "stash.delete",
        "success",
        Some(&file_id),
        None,
        None,
        true,
    );
    Ok(StatusCode::NO_CONTENT.into_response())
}
async fn create_grant(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    Path(file_id): Path<String>,
    Json(body): Json<GrantRequest>,
) -> Result<Response, ApiError> {
    mutation_csrf(&headers, auth.as_ref(), "stash.grants.create")?;
    let (kind, id) = selected_owner_headers(&headers);
    let principal = selected_principal(
        &state,
        identity,
        auth.as_ref(),
        kind,
        id,
        "stash.grants.create",
    )
    .await?;
    principal
        .validate_before_commit()
        .await
        .map_err(|_| stable("not_found"))?;
    let grant = service(&state)
        .create_grant_for_recipient_id(&principal, &file_id, body.grantee_principal_id)
        .await?;
    crate::dispatch::file_stash::observe_operation(
        "api",
        "stash.grants.create",
        "success",
        Some(&file_id),
        Some(&grant.grant_id),
        None,
        false,
    );
    Ok((StatusCode::CREATED, result(grant)).into_response())
}
async fn list_grants(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    Path(file_id): Path<String>,
    Query(q): Query<PageQuery>,
) -> Result<Response, ApiError> {
    let (kind, id) = selected_owner_headers(&headers);
    let principal = selected_principal(
        &state,
        identity,
        auth.as_ref(),
        kind,
        id,
        "stash.grants.list",
    )
    .await?;
    principal
        .validate_before_commit()
        .await
        .map_err(|_| stable("not_found"))?;
    let grants = service(&state)
        .grants(&principal, &file_id, q.cursor.as_deref(), q.limit)
        .await?;
    crate::dispatch::file_stash::observe_operation(
        "api",
        "stash.grants.list",
        "success",
        Some(&file_id),
        None,
        None,
        false,
    );
    Ok(result(grants))
}
async fn revoke_grant(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    Path((file_id, grant_id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    mutation_csrf(&headers, auth.as_ref(), "stash.grants.revoke")?;
    let (kind, id) = selected_owner_headers(&headers);
    let principal = selected_principal(
        &state,
        identity,
        auth.as_ref(),
        kind,
        id,
        "stash.grants.revoke",
    )
    .await?;
    principal
        .validate_before_commit()
        .await
        .map_err(|_| stable("not_found"))?;
    service(&state)
        .revoke_grant(&principal, &file_id, &grant_id)
        .await?;
    crate::dispatch::file_stash::observe_operation(
        "api",
        "stash.grants.revoke",
        "success",
        Some(&file_id),
        Some(&grant_id),
        None,
        false,
    );
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    body: Body,
) -> Result<Response, ApiError> {
    validate_header_budget(&headers, state.config.file_stash.max_header_bytes)?;
    mutation_csrf(&headers, auth.as_ref(), "stash.upload")?;
    let (kind, id) = selected_owner_headers(&headers);
    let principal =
        selected_principal(&state, identity, auth.as_ref(), kind, id, "stash.upload").await?;
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
    let file_id = upload.await.map_err(|_| stable("service_unavailable"))??;
    if principal.validate_before_commit().await.is_err() {
        drop(service(&state).delete(&principal, &file_id).await);
        return Err(stable("not_found"));
    }
    guard.0 = None;
    crate::dispatch::file_stash::observe_operation(
        "api",
        "stash.upload",
        "success",
        Some(&file_id),
        None,
        Some(declared),
        false,
    );
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
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    Path(file_id): Path<String>,
    Query(owner): Query<OwnerQuery>,
) -> Result<Response, ApiError> {
    let (header_kind, header_id) = selected_owner_headers(&headers);
    let kind = owner.owner_kind.as_deref().or(header_kind);
    let id = owner.owner_id.as_deref().or(header_id);
    let principal =
        selected_principal(&state, identity, auth.as_ref(), kind, id, "stash.download").await?;
    let (file, opened) = service(&state)
        .open_download(&principal, &file_id, false)
        .await?;
    principal
        .validate_before_commit()
        .await
        .map_err(|_| stable("not_found"))?;
    let size = opened.size;
    crate::dispatch::file_stash::observe_operation(
        "api",
        "stash.download",
        "success",
        Some(&file_id),
        None,
        Some(size),
        false,
    );
    let mut response = Response::new(blob_body(opened));
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

/// Async reader that deliberately owns the complete opened blob. The semaphore
/// permits therefore remain held until the response body reaches EOF or drops.
struct HeldBlob(crate::file_stash::OpenedBlob);

impl AsyncRead for HeldBlob {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().0.file).poll_read(cx, buf)
    }
}

fn blob_body(opened: crate::file_stash::OpenedBlob) -> Body {
    Body::from_stream(ReaderStream::new(HeldBlob(opened)))
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
    #[cfg(any(target_os = "linux", target_os = "android"))]
    use axum::body::Bytes;
    use axum::{Router, http::Request};
    #[cfg(any(target_os = "linux", target_os = "android"))]
    use std::sync::Arc;
    use tower::ServiceExt as _;

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

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[tokio::test]
    async fn response_body_holds_download_admission_until_drop() {
        let temp = tempfile::TempDir::new().unwrap();
        let preferences = crate::config::FileStashPreferences {
            max_file_bytes: 16,
            principal_quota_bytes: 16,
            instance_quota_bytes: 16,
            max_concurrent_downloads: 1,
            database_deadline_ms: 20,
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
        assert_eq!(
            runtime.wait_for_recovery().await,
            crate::file_stash::FileStashStatus::Ready
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
        let body = blob_body(opened);
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

    #[cfg(any(target_os = "linux", target_os = "android"))]
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
        assert_eq!(
            stash.wait_for_recovery().await,
            crate::file_stash::FileStashStatus::Ready
        );
        let state = AppState::new()
            .with_access_runtime(Arc::clone(&access))
            .with_file_stash_runtime(Arc::clone(&stash));
        let service = service(&state);
        let auth = AuthContext {
            sub: "owner".into(),
            issuer: "https://accounts.google.com".into(),
            scopes: vec!["lab".into()],
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

    #[cfg(any(target_os = "linux", target_os = "android"))]
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

    #[cfg(any(target_os = "linux", target_os = "android"))]
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
