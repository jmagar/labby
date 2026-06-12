use std::{convert::Infallible, net::SocketAddr, sync::Arc};

use axum::{
    Extension, Json, Router,
    extract::{ConnectInfo, State},
    http::HeaderMap,
    response::sse::{Event, KeepAlive, Sse},
    routing::{get, post},
};
use futures::stream;
use serde_json::Value;
use tracing::info;

use crate::api::oauth::AuthContext;
use crate::api::services::helpers::{dispatch_meta_from_headers, handle_action_with_meta};
use crate::api::{ActionRequest, state::AppState};
use crate::dispatch::doctor::ACTIONS;
use crate::dispatch::error::ToolError;

pub fn routes(_state: AppState) -> Router<AppState> {
    Router::new()
        .route("/", post(handle))
        .route("/audit-full/stream", get(stream_audit_full))
}

async fn handle(
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    Json(req): Json<ActionRequest>,
) -> Result<Json<Value>, ToolError> {
    let clients = state.clients.clone();
    handle_action_with_meta(
        "doctor",
        "api",
        dispatch_meta_from_headers(
            &headers,
            auth.as_ref().map(|value| &value.0),
            peer.map(|Extension(ConnectInfo(addr))| addr),
        ),
        req,
        ACTIONS,
        move |action, params| async move {
            crate::dispatch::doctor::dispatch_with_clients(&clients, &action, params).await
        },
    )
    .await
}

/// `GET /v1/doctor/audit-full/stream` — SSE stream of `audit.full` results.
async fn stream_audit_full(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>>>, ToolError> {
    const ACTION: &str = "audit.full";
    let request_id = headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let start = std::time::Instant::now();

    info!(
        surface = "api",
        service = "doctor",
        action = ACTION,
        request_id = request_id.as_deref(),
        "dispatch start"
    );

    let (tx, rx) = tokio::sync::mpsc::channel::<crate::dispatch::doctor::Finding>(64);
    let clients = Arc::clone(&state.clients);

    tokio::spawn(async move {
        crate::dispatch::doctor::service::stream_audit_full(clients, tx).await;
    });

    info!(
        surface = "api",
        service = "doctor",
        action = ACTION,
        request_id = request_id.as_deref(),
        elapsed_ms = start.elapsed().as_millis(),
        "dispatch ok"
    );

    let opened_at = std::time::Instant::now();

    let event_stream = stream::unfold(
        (rx, request_id, opened_at),
        move |(mut rx, request_id, opened_at)| async move {
            match rx.recv().await {
                Some(finding) => match serde_json::to_string(&finding) {
                    Ok(payload) => Some((
                        Ok(Event::default().data(payload)),
                        (rx, request_id, opened_at),
                    )),
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to serialize doctor finding; skipping");
                        Some((
                            Ok(Event::default().event("error").data(e.to_string())),
                            (rx, request_id, opened_at),
                        ))
                    }
                },
                None => {
                    info!(
                        surface = "api",
                        service = "doctor",
                        action = ACTION,
                        request_id = request_id.as_deref(),
                        elapsed_ms = opened_at.elapsed().as_millis(),
                        "dispatch finish"
                    );
                    None
                }
            }
        },
    );

    Ok(Sse::new(event_stream).keep_alive(KeepAlive::default()))
}
