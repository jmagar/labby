use std::convert::Infallible;

use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::ActionRequest;
use crate::api::oauth::AuthContext;
use crate::api::services::helpers::handle_action;
use crate::api::state::AppState;
use crate::dispatch::acp::catalog::ACTIONS;
use crate::dispatch::acp::dispatch::dispatch_with_registry;
use crate::dispatch::acp::dispatch::validate_subscribe_ticket;
use crate::dispatch::error::ToolError;

/// Hard cap on incoming prompt text (64 000 chars ≈ 16 000 tokens at 4 chars/token).
const PROMPT_MAX_CHARS: usize = 64_000;

fn required_principal(auth: Option<Extension<AuthContext>>) -> Result<String, ToolError> {
    let principal = auth
        .map(|Extension(ctx)| ctx.sub)
        .filter(|sub| !sub.trim().is_empty())
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "auth_failed".to_string(),
            message: "authenticated ACP principal required".to_string(),
        })?;
    Ok(principal)
}

pub fn routes(_state: AppState) -> Router<AppState> {
    Router::new()
        .route("/", post(handle))
        .route("/provider", get(provider_health))
        .route("/sessions", get(list_sessions).post(create_session))
        .route("/sessions/{session_id}/prompt", post(prompt_session))
        .route("/sessions/{session_id}/cancel", post(cancel_session))
        .route(
            "/sessions/{session_id}/permissions/{request_id}/approve",
            post(approve_permission),
        )
        .route(
            "/sessions/{session_id}/permissions/{request_id}/reject",
            post(reject_permission),
        )
        .route(
            "/sessions/{session_id}/subscribe_ticket",
            post(subscribe_ticket),
        )
        .route("/sessions/{session_id}/events", get(stream_events))
}

async fn handle(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    Json(mut req): Json<ActionRequest>,
) -> Result<Json<Value>, ToolError> {
    let request_id = headers.get("x-request-id").and_then(|v| v.to_str().ok());
    if req.action.starts_with("session.") {
        let principal = required_principal(auth)?;
        match req.params {
            Value::Object(ref mut params) => {
                params.insert("principal".to_string(), Value::String(principal));
            }
            Value::Null => {
                req.params = json!({ "principal": principal });
            }
            _ => {
                return Err(ToolError::InvalidParam {
                    message: "params must be an object".to_string(),
                    param: "params".to_string(),
                });
            }
        }
    }

    handle_action(
        "acp",
        "api",
        request_id,
        req,
        ACTIONS,
        move |action, mut params| {
            let registry = state.acp_registry;
            async move {
                if ACTIONS
                    .iter()
                    .any(|spec| spec.name == action && spec.destructive)
                    && let Value::Object(ref mut params) = params
                {
                    params.insert("confirm".to_string(), Value::Bool(true));
                }
                dispatch_with_registry(&registry, &action, params).await
            }
        },
    )
    .await
}

async fn provider_health(State(state): State<AppState>) -> impl IntoResponse {
    match dispatch_with_registry(&state.acp_registry, "provider.list", json!({})).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn list_sessions(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
) -> impl IntoResponse {
    let principal = match required_principal(auth) {
        Ok(principal) => principal,
        Err(error) => return error.into_response(),
    };
    match dispatch_with_registry(
        &state.acp_registry,
        "session.list",
        json!({ "principal": principal }),
    )
    .await
    {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateSessionBody {
    provider: Option<String>,
    cwd: Option<String>,
    title: Option<String>,
    model: Option<String>,
    #[serde(alias = "model_id")]
    model_id: Option<String>,
}

async fn create_session(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    Json(body): Json<CreateSessionBody>,
) -> impl IntoResponse {
    let principal = match required_principal(auth) {
        Ok(principal) => principal,
        Err(error) => return error.into_response(),
    };
    let params = json!({
        "provider": body.provider,
        "cwd": body.cwd,
        "title": body.title,
        "model": body.model.or(body.model_id),
        "principal": principal,
    });
    match dispatch_with_registry(&state.acp_registry, "session.start", params).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}

/// Optional structured page context from the frontend.
/// All validation and injection logic lives in `dispatch/acp/page_context.rs`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageContextBody {
    route: String,
    entity_type: Option<String>,
    entity_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PromptBody {
    prompt: String,
    /// Optional structured page context. Passed to dispatch; injection is handled there.
    page_context: Option<PageContextBody>,
    model: Option<String>,
    #[serde(alias = "model_id")]
    model_id: Option<String>,
}

async fn prompt_session(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    Path(session_id): Path<String>,
    Json(body): Json<PromptBody>,
) -> impl IntoResponse {
    let principal = match required_principal(auth) {
        Ok(principal) => principal,
        Err(error) => return error.into_response(),
    };
    if body.prompt.trim().is_empty() {
        return ToolError::MissingParam {
            message: "prompt is required".to_string(),
            param: "prompt".to_string(),
        }
        .into_response();
    }

    if body.prompt.len() > PROMPT_MAX_CHARS {
        return ToolError::InvalidParam {
            message: format!(
                "prompt exceeds maximum allowed length ({} > {} chars)",
                body.prompt.len(),
                PROMPT_MAX_CHARS
            ),
            param: "prompt".to_string(),
        }
        .into_response();
    }

    // Pass page_context as a JSON object to the dispatch layer.
    // All sanitization and prefix assembly happens there — HTTP handler is a thin shim.
    let page_context_value = body.page_context.as_ref().map(|ctx| {
        json!({
            "route": ctx.route,
            "entityType": ctx.entity_type,
            "entityId": ctx.entity_id,
        })
    });

    let params = json!({
        "session_id": session_id,
        "text": body.prompt.trim(),
        "page_context": page_context_value,
        "model": body.model.or(body.model_id),
        "principal": principal,
    });
    match dispatch_with_registry(&state.acp_registry, "session.prompt", params).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn cancel_session(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let principal = match required_principal(auth) {
        Ok(principal) => principal,
        Err(error) => return error.into_response(),
    };
    let params = json!({
        "session_id": session_id,
        "confirm": true,
        "principal": principal,
    });
    match dispatch_with_registry(&state.acp_registry, "session.cancel", params).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}

#[derive(Deserialize)]
struct ApprovePermissionBody {
    option_id: String,
}

async fn approve_permission(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    Path((session_id, request_id)): Path<(String, String)>,
    Json(body): Json<ApprovePermissionBody>,
) -> impl IntoResponse {
    let principal = match required_principal(auth) {
        Ok(principal) => principal,
        Err(error) => return error.into_response(),
    };
    if body.option_id.trim().is_empty() {
        return ToolError::MissingParam {
            message: "option_id is required".to_string(),
            param: "option_id".to_string(),
        }
        .into_response();
    }
    let params = json!({
        "session_id": session_id,
        "request_id": request_id,
        "option_id": body.option_id,
        "confirm": true,
        "principal": principal,
    });
    match dispatch_with_registry(&state.acp_registry, "session.permission.approve", params).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn reject_permission(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    Path((session_id, request_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let principal = match required_principal(auth) {
        Ok(principal) => principal,
        Err(error) => return error.into_response(),
    };
    let params = json!({
        "session_id": session_id,
        "request_id": request_id,
        "principal": principal,
    });
    match dispatch_with_registry(&state.acp_registry, "session.permission.reject", params).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn subscribe_ticket(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let principal = match required_principal(auth) {
        Ok(principal) => principal,
        Err(error) => return error.into_response(),
    };
    let params = json!({ "session_id": session_id, "principal": principal });
    match dispatch_with_registry(&state.acp_registry, "session.subscribe_ticket", params).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}

#[derive(Deserialize)]
struct EventQuery {
    since: Option<u64>,
    ticket: Option<String>,
}

async fn stream_events(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<EventQuery>,
) -> Response {
    // Validate SSE ticket before establishing stream.
    let principal = if let Some(ref ticket) = query.ticket {
        match validate_subscribe_ticket(ticket) {
            Ok((tid, principal)) => {
                if tid != session_id {
                    return (
                        StatusCode::UNAUTHORIZED,
                        Json(json!({ "kind": "auth_failed", "message": "ticket session_id mismatch" })),
                    )
                        .into_response();
                }
                principal
            }
            Err(e) => {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::to_value(&e).unwrap_or_default()),
                )
                    .into_response();
            }
        }
    } else {
        // No ticket provided — reject immediately (Phase 2 will wire full auth;
        // until then, every SSE caller must obtain a ticket via session.subscribe_ticket).
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "kind": "auth_failed", "message": "SSE ticket required; call session.subscribe_ticket first" })),
        )
            .into_response();
    };

    let since = query.since.unwrap_or(0);
    let stream_result = state
        .acp_registry
        .subscribe(&session_id, since, &principal)
        .await;

    match stream_result {
        Err(e) => e.into_response(),
        Ok(event_stream) => {
            let sse_stream = event_stream.map(|event| {
                let data = serde_json::to_string(&*event).unwrap_or_else(|_| "{}".to_string());
                Ok::<Event, Infallible>(Event::default().id(event.seq().to_string()).data(data))
            });
            Sse::new(sse_stream)
                .keep_alive(KeepAlive::default())
                .into_response()
        }
    }
}
