use std::time::Instant;

use axum::http::HeaderMap;

pub(crate) fn request_id(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
}

pub(crate) fn log_auth_dispatch_start(action: &str, request_id: Option<&str>) {
    tracing::info!(
        surface = "api",
        service = "auth",
        action,
        request_id,
        "auth oauth dispatch start"
    );
}

pub(crate) fn log_auth_dispatch(
    action: &str,
    request_id: Option<&str>,
    started: Instant,
    kind: Option<&str>,
    actor_key: Option<&str>,
) {
    let elapsed_ms = started.elapsed().as_millis();
    match kind {
        None => tracing::info!(
            surface = "api",
            service = "auth",
            action,
            request_id,
            actor_key,
            elapsed_ms,
            "auth oauth dispatch complete"
        ),
        Some("internal_error" | "server_error" | "decode_error") => tracing::error!(
            surface = "api",
            service = "auth",
            action,
            request_id,
            actor_key,
            elapsed_ms,
            kind,
            "auth oauth dispatch failed"
        ),
        Some(kind) => tracing::warn!(
            surface = "api",
            service = "auth",
            action,
            request_id,
            actor_key,
            elapsed_ms,
            kind,
            "auth oauth dispatch failed"
        ),
    }
}
