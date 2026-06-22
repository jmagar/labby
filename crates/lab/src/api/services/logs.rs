use std::{convert::Infallible, net::SocketAddr};

use axum::{
    Extension, Json, Router,
    extract::{ConnectInfo, State},
    http::HeaderMap,
    response::sse::{Event, KeepAlive, Sse},
    routing::{get, post},
};
use futures::stream;
use serde_json::Value;
use tracing::{error, info, warn};

use crate::api::error::ApiError;
use crate::api::oauth::AuthContext;
use crate::api::services::helpers::{dispatch_meta_from_headers, handle_action_with_meta};
use crate::api::{ActionRequest, state::AppState};
use crate::dispatch::error::ToolError;
use crate::dispatch::logs::client::is_ingest_enabled;
use crate::dispatch::logs::types::{PeerIngestRequest, PeerIngestResponse};

pub fn routes(_state: AppState) -> Router<AppState> {
    Router::new()
        .route("/", post(handle))
        .route("/stream", get(stream_logs))
        .route("/ingest", post(ingest_peer_events))
}

async fn handle(
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    Json(req): Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    let logs_system = state.logs_system.clone().ok_or_else(|| {
        ToolError::internal_message("local log system is not wired into API state")
    })?;
    handle_action_with_meta(
        "logs",
        "api",
        dispatch_meta_from_headers(
            &headers,
            auth.as_ref().map(|value| &value.0),
            peer.map(|Extension(ConnectInfo(addr))| addr),
        ),
        req,
        crate::dispatch::logs::ACTIONS,
        move |action, params| async move {
            crate::dispatch::logs::dispatch::dispatch_with_system(&logs_system, &action, params)
                .await
        },
    )
    .await
}

async fn stream_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    const ACTION: &str = "logs.stream";
    let request_id = headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let start = std::time::Instant::now();

    info!(
        surface = "api",
        service = "logs",
        action = ACTION,
        request_id = request_id.as_deref(),
        "dispatch start"
    );

    let Some(logs_system) = state.logs_system.clone() else {
        let error = ToolError::internal_message("local log system is not wired into API state");
        error!(
            surface = "api",
            service = "logs",
            action = ACTION,
            request_id = request_id.as_deref(),
            elapsed_ms = start.elapsed().as_millis(),
            kind = error.kind(),
            "dispatch error"
        );
        return Err(error.into());
    };
    let receiver = match logs_system
        .subscribe(crate::dispatch::logs::types::StreamSubscription::default())
        .await
    {
        Ok(receiver) => receiver,
        Err(error) => {
            if error.is_internal() {
                error!(
                    surface = "api",
                    service = "logs",
                    action = ACTION,
                    request_id = request_id.as_deref(),
                    elapsed_ms = start.elapsed().as_millis(),
                    kind = error.kind(),
                    "dispatch error"
                );
            } else {
                warn!(
                    surface = "api",
                    service = "logs",
                    action = ACTION,
                    request_id = request_id.as_deref(),
                    elapsed_ms = start.elapsed().as_millis(),
                    kind = error.kind(),
                    "dispatch error"
                );
            }
            return Err(error.into());
        }
    };

    info!(
        surface = "api",
        service = "logs",
        action = ACTION,
        request_id = request_id.as_deref(),
        elapsed_ms = start.elapsed().as_millis(),
        "dispatch ok"
    );

    let opened_at = std::time::Instant::now();

    let stream = stream::unfold(
        (receiver, request_id, opened_at),
        move |(mut receiver, request_id, opened_at)| async move {
            loop {
                match receiver.recv().await {
                    Ok(event) => match serde_json::to_string(&event) {
                        Ok(payload) => {
                            return Some((
                                Ok(Event::default().data(payload)),
                                (receiver, request_id, opened_at),
                            ));
                        }
                        Err(error) => {
                            warn!(error = %error, "failed to serialize log event for SSE; skipping");
                        }
                    },
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(skipped, "SSE log subscriber lagged; dropping events");
                        return Some((
                            Ok(Event::default().event("lag").data(skipped.to_string())),
                            (receiver, request_id, opened_at),
                        ));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        info!(
                            surface = "api",
                            service = "logs",
                            action = ACTION,
                            request_id = request_id.as_deref(),
                            elapsed_ms = opened_at.elapsed().as_millis(),
                            "dispatch finish"
                        );
                        return None;
                    }
                }
            }
        },
    );

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

async fn ingest_peer_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<PeerIngestRequest>,
) -> Result<Json<PeerIngestResponse>, ApiError> {
    const ACTION: &str = "logs.ingest";
    let request_id = headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    if !is_ingest_enabled() {
        return Err(ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message:
                "peer log ingest is not enabled on this node; set LAB_LOGS_INGEST_ENABLED=true"
                    .to_string(),
        }
        .into());
    }

    const MAX_EVENTS_PER_BATCH: usize = 500;
    const MAX_MESSAGE_BYTES: usize = 8_192;

    if req.events.len() > MAX_EVENTS_PER_BATCH {
        return Err(ToolError::InvalidParam {
            message: format!(
                "batch too large: {} events exceeds maximum of {MAX_EVENTS_PER_BATCH}",
                req.events.len()
            ),
            param: "events".to_string(),
        }
        .into());
    }

    for event in &req.events {
        if event.message.len() > MAX_MESSAGE_BYTES {
            return Err(ToolError::InvalidParam {
                message: format!("event message exceeds maximum size of {MAX_MESSAGE_BYTES} bytes"),
                param: "events[].message".to_string(),
            }
            .into());
        }
    }

    let logs_system = state.logs_system.clone().ok_or_else(|| {
        ToolError::internal_message("local log system is not wired into API state")
    })?;

    let total = req.events.len();
    info!(
        surface = "api",
        service = "logs",
        action = ACTION,
        request_id = request_id.as_deref(),
        node_id = req.node_id.as_str(),
        count = total,
        "dispatch start"
    );
    let start = std::time::Instant::now();

    let mut accepted = 0usize;
    let mut dropped = 0usize;

    const ALLOWED_SOURCE_KINDS: &[&str] = &["syslog", "journald", "application", "peer"];

    for mut event in req.events {
        // The master always controls node identity — never trust self-reported field.
        event.source_node_id = Some(req.node_id.clone());
        // Constrain source_kind to the known allowlist so peers cannot spoof
        // system-level kinds (e.g., "audit") that would be indistinguishable
        // from locally-generated entries.
        if let Some(ref kind) = event.source_kind {
            if !ALLOWED_SOURCE_KINDS.contains(&kind.as_str()) {
                return Err(ToolError::InvalidParam {
                    message: format!(
                        "source_kind `{kind}` is not allowed; valid: {ALLOWED_SOURCE_KINDS:?}"
                    ),
                    param: "events[].source_kind".to_string(),
                }
                .into());
            }
        } else {
            event.source_kind = Some("syslog".to_string());
        }
        match logs_system.try_ingest(event) {
            Ok(()) => accepted += 1,
            Err(ref e) if e.kind() == "rate_limited" => dropped += 1,
            Err(e) => {
                // Channel closed or other fatal ingest error — abort early.
                error!(
                    surface = "api",
                    service = "logs",
                    action = ACTION,
                    request_id = request_id.as_deref(),
                    node_id = req.node_id.as_str(),
                    kind = e.kind(),
                    elapsed_ms = start.elapsed().as_millis(),
                    "dispatch error"
                );
                return Err(e.into());
            }
        }
    }

    if accepted == 0 && dropped > 0 {
        // All events were dropped due to a full ingest queue. Return 429 so
        // peers apply back-pressure and retry rather than silently losing data.
        warn!(
            surface = "api",
            service = "logs",
            action = ACTION,
            request_id = request_id.as_deref(),
            node_id = req.node_id.as_str(),
            dropped,
            elapsed_ms = start.elapsed().as_millis(),
            "dispatch warn: all events dropped (queue full)"
        );
        return Err(ToolError::Sdk {
            sdk_kind: "rate_limited".to_string(),
            message: format!(
                "log ingest queue is full; all {dropped} event(s) dropped — retry after backoff"
            ),
        }
        .into());
    }

    if dropped > 0 {
        warn!(
            surface = "api",
            service = "logs",
            action = ACTION,
            request_id = request_id.as_deref(),
            node_id = req.node_id.as_str(),
            accepted,
            dropped,
            elapsed_ms = start.elapsed().as_millis(),
            "dispatch ok (partial: some events dropped due to queue full)"
        );
    } else {
        info!(
            surface = "api",
            service = "logs",
            action = ACTION,
            request_id = request_id.as_deref(),
            node_id = req.node_id.as_str(),
            accepted,
            elapsed_ms = start.elapsed().as_millis(),
            "dispatch ok"
        );
    }

    Ok(Json(PeerIngestResponse { accepted, dropped }))
}
