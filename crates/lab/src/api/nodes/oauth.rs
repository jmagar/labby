use std::net::SocketAddr;
use std::time::Duration;

use axum::{Json, extract::State, http::HeaderMap};
use serde::Deserialize;
use std::time::Instant;

use crate::api::auth_helpers::request_id;
use crate::api::{ToolError, error::ApiError, state::AppState};

#[derive(Debug, Deserialize)]
pub struct StartOauthRelayRequest {
    pub bind_addr: SocketAddr,
    pub target_url: String,
    #[serde(default)]
    pub default_port: Option<u16>,
    #[serde(default)]
    pub request_timeout_ms: Option<u64>,
}

#[derive(Debug, serde::Serialize)]
pub struct StartOauthRelayResponse {
    pub ok: bool,
    pub bind_addr: SocketAddr,
}

fn validate_bind_addr(bind_addr: SocketAddr) -> Result<(), ToolError> {
    if !bind_addr.ip().is_loopback() {
        return Err(ToolError::InvalidParam {
            message: "bind_addr must be a loopback address".to_string(),
            param: "bind_addr".to_string(),
        });
    }

    Ok(())
}

pub async fn handle_start(
    State(_state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<StartOauthRelayRequest>,
) -> Result<Json<StartOauthRelayResponse>, ApiError> {
    let start = Instant::now();
    let request_id = request_id(&headers).map(ToOwned::to_owned);
    validate_bind_addr(payload.bind_addr)?;
    let resolved_target =
        crate::oauth::target::resolve_explicit_target(&payload.target_url, payload.default_port)
            .map_err(|error| ToolError::InvalidParam {
                message: error.to_string(),
                param: "target_url".to_string(),
            })?;
    let timeout = Duration::from_millis(payload.request_timeout_ms.unwrap_or(30_000));

    let bound_addr =
        crate::node::oauth::start_local_oauth_relay(payload.bind_addr, resolved_target, timeout)
            .await
            .map_err(|error| {
                tracing::error!(
                    surface = "api",
                    service = "nodes",
                    action = "oauth.relay.start",
                    request_id = request_id.as_deref(),
                    elapsed_ms = start.elapsed().as_millis(),
                    kind = "internal_error",
                    error = %error,
                    "node oauth relay start failed"
                );
                ToolError::Sdk {
                    sdk_kind: "internal_error".to_string(),
                    message: error.to_string(),
                }
            })?;

    tracing::info!(
        surface = "api",
        service = "nodes",
        action = "oauth.relay.start",
        request_id = request_id.as_deref(),
        elapsed_ms = start.elapsed().as_millis(),
        "node oauth relay start complete"
    );

    Ok(Json(StartOauthRelayResponse {
        ok: true,
        bind_addr: bound_addr,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::api::state::AppState;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    #[tokio::test]
    async fn handle_start_rejects_non_loopback_bind_addr() {
        let response = handle_start(
            State(AppState::new()),
            HeaderMap::new(),
            Json(StartOauthRelayRequest {
                bind_addr: "192.168.1.10:0".parse().unwrap(),
                target_url: "http://127.0.0.1/callback".to_string(),
                default_port: None,
                request_timeout_ms: Some(100),
            }),
        )
        .await
        .unwrap_err()
        .into_response();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn handle_start_returns_bound_loopback_address() {
        let response = handle_start(
            State(AppState::new()),
            HeaderMap::new(),
            Json(StartOauthRelayRequest {
                bind_addr: "127.0.0.1:0".parse().unwrap(),
                target_url: "http://127.0.0.1/callback".to_string(),
                default_port: None,
                request_timeout_ms: Some(100),
            }),
        )
        .await
        .expect("relay start succeeds");

        assert!(response.0.ok);
        assert!(response.0.bind_addr.ip().is_loopback());
        assert_ne!(response.0.bind_addr.port(), 0);
    }
}
