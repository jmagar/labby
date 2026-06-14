//! HTTP route group for the `marketplace` service.

use std::time::Duration;
use std::{convert::Infallible, net::SocketAddr};

use axum::{
    Extension, Json, Router,
    extract::{ConnectInfo, Query, State},
    http::HeaderMap,
    response::sse::{Event, KeepAlive, Sse},
    routing::{get, post},
};
use futures::stream;
use serde::Deserialize;
use serde_json::Value;
use tracing::{info, warn};

use crate::api::oauth::AuthContext;
use crate::api::services::helpers::{dispatch_meta_from_headers, handle_action_with_meta};
use crate::api::{ActionRequest, state::AppState};
use crate::dispatch::error::ToolError;
use crate::dispatch::marketplace::NodeRpcPort;
use crate::dispatch::node::send::{send_rpc_to_node, subscribe_progress};

pub fn routes(_state: AppState) -> Router<AppState> {
    Router::new()
        .route("/", post(handle))
        .route("/artifact/fork", post(handle_artifact_fork))
        .route("/artifact/list", post(handle_artifact_list))
        .route("/artifact/unfork", post(handle_artifact_unfork))
        .route("/artifact/reset", post(handle_artifact_reset))
        .route("/artifact/diff", post(handle_artifact_diff))
        .route("/artifact/patch", post(handle_artifact_patch))
        .route("/artifact/update/check", post(handle_artifact_update_check))
        .route(
            "/artifact/update/preview",
            post(handle_artifact_update_preview),
        )
        .route("/artifact/update/apply", post(handle_artifact_update_apply))
        .route(
            "/artifact/merge/suggest",
            post(handle_artifact_merge_suggest),
        )
        .route("/artifact/config/set", post(handle_artifact_config_set))
        .route("/cherry-pick/progress", get(cherry_pick_progress))
}

pub(crate) struct WsNodeRpcPort;

impl NodeRpcPort for WsNodeRpcPort {
    async fn send_rpc(
        &self,
        node_id: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, ToolError> {
        send_rpc_to_node(node_id, method, params).await
    }
}

async fn handle(
    State(_state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    Json(req): Json<ActionRequest>,
) -> Result<Json<Value>, ToolError> {
    handle_marketplace_action(
        peer.map(|Extension(ConnectInfo(addr))| addr),
        headers,
        auth,
        req,
    )
    .await
}

async fn handle_marketplace_action(
    peer_addr: Option<SocketAddr>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    req: ActionRequest,
) -> Result<Json<Value>, ToolError> {
    let request_id = headers.get("x-request-id").and_then(|v| v.to_str().ok());
    require_marketplace_admin(&req.action, request_id, auth.as_ref())?;
    handle_action_with_meta(
        "marketplace",
        "api",
        dispatch_meta_from_headers(&headers, auth.as_ref().map(|value| &value.0), peer_addr),
        req,
        crate::dispatch::marketplace::actions(),
        |action, params| async move {
            crate::dispatch::marketplace::dispatch_with_port(&action, params, &WsNodeRpcPort).await
        },
    )
    .await
}

fn has_admin_scope(auth: Option<&Extension<AuthContext>>) -> bool {
    auth.is_some_and(|ctx| ctx.0.scopes.iter().any(|scope| scope == "lab:admin"))
}

fn require_marketplace_admin(
    action: &str,
    request_id: Option<&str>,
    auth: Option<&Extension<AuthContext>>,
) -> Result<(), ToolError> {
    let bare = action.strip_prefix("marketplace.").unwrap_or(action);
    if bare == "help"
        || bare == "schema"
        || !marketplace_action_requires_admin(bare)
        || has_admin_scope(auth)
    {
        return Ok(());
    }

    tracing::warn!(
        surface = "api",
        service = "marketplace",
        action,
        request_id,
        kind = "forbidden",
        "marketplace action rejected: lab:admin scope required"
    );
    Err(ToolError::Sdk {
        sdk_kind: "forbidden".to_string(),
        message: format!("action `{action}` requires `lab:admin` scope"),
    })
}

fn marketplace_action_requires_admin(action: &str) -> bool {
    crate::dispatch::marketplace::actions()
        .iter()
        .find(|spec| spec.name == action)
        .map(|spec| spec.requires_admin)
        .unwrap_or(true)
}

async fn handle_artifact_fork(
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    body: Option<Json<Value>>,
) -> Result<Json<Value>, ToolError> {
    handle_artifact_path_action(headers, auth, "artifact.fork", body).await
}

async fn handle_artifact_list(
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    body: Option<Json<Value>>,
) -> Result<Json<Value>, ToolError> {
    handle_artifact_path_action(headers, auth, "artifact.list", body).await
}

async fn handle_artifact_unfork(
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    body: Option<Json<Value>>,
) -> Result<Json<Value>, ToolError> {
    handle_artifact_path_action(headers, auth, "artifact.unfork", body).await
}

async fn handle_artifact_reset(
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    body: Option<Json<Value>>,
) -> Result<Json<Value>, ToolError> {
    handle_artifact_path_action(headers, auth, "artifact.reset", body).await
}

async fn handle_artifact_diff(
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    body: Option<Json<Value>>,
) -> Result<Json<Value>, ToolError> {
    handle_artifact_path_action(headers, auth, "artifact.diff", body).await
}

async fn handle_artifact_patch(
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    body: Option<Json<Value>>,
) -> Result<Json<Value>, ToolError> {
    handle_artifact_path_action(headers, auth, "artifact.patch", body).await
}

async fn handle_artifact_update_check(
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    body: Option<Json<Value>>,
) -> Result<Json<Value>, ToolError> {
    handle_artifact_path_action(headers, auth, "artifact.update.check", body).await
}

async fn handle_artifact_update_preview(
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    body: Option<Json<Value>>,
) -> Result<Json<Value>, ToolError> {
    handle_artifact_path_action(headers, auth, "artifact.update.preview", body).await
}

async fn handle_artifact_update_apply(
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    body: Option<Json<Value>>,
) -> Result<Json<Value>, ToolError> {
    handle_artifact_path_action(headers, auth, "artifact.update.apply", body).await
}

async fn handle_artifact_merge_suggest(
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    body: Option<Json<Value>>,
) -> Result<Json<Value>, ToolError> {
    handle_artifact_path_action(headers, auth, "artifact.merge.suggest", body).await
}

async fn handle_artifact_config_set(
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    body: Option<Json<Value>>,
) -> Result<Json<Value>, ToolError> {
    handle_artifact_path_action(headers, auth, "artifact.config.set", body).await
}

async fn handle_artifact_path_action(
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    action: &'static str,
    body: Option<Json<Value>>,
) -> Result<Json<Value>, ToolError> {
    let params = body
        .map(|Json(value)| value)
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    handle_marketplace_action(
        None,
        headers,
        auth,
        ActionRequest {
            action: action.to_string(),
            params,
        },
    )
    .await
}

// ---------------------------------------------------------------------------
// Cherry-pick SSE progress endpoint (lab-zxx5.16)
// ---------------------------------------------------------------------------
// GET /v1/marketplace/cherry-pick/progress?rpc_id=<uuid>
//
// Subscribes to the per-rpc_id progress broadcast channel and forwards each
// `install/progress` frame as an SSE `data: {json}\n\n` event. The stream
// closes when the broadcast channel is dropped — which happens when the
// correlated RPC response arrives (see `resolve_pending_rpc`).
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct CherryPickProgressQuery {
    rpc_id: String,
}

async fn cherry_pick_progress(
    State(_state): State<AppState>,
    Query(query): Query<CherryPickProgressQuery>,
) -> Result<Sse<impl stream::Stream<Item = Result<Event, Infallible>>>, ToolError> {
    let rpc_id = query.rpc_id.trim();
    if rpc_id.is_empty() {
        return Err(ToolError::MissingParam {
            param: "rpc_id".into(),
            message: "`rpc_id` query parameter is required".into(),
        });
    }
    // Validate rpc_id shape — cherry_pick generates UUIDv4 strings, and we
    // reject anything else to keep a tight surface for this endpoint.
    if uuid::Uuid::parse_str(rpc_id).is_err() {
        return Err(ToolError::InvalidParam {
            param: "rpc_id".into(),
            message: "`rpc_id` must be a UUID".into(),
        });
    }

    let receiver = subscribe_progress(rpc_id);
    let rpc_id_owned = rpc_id.to_string();
    let opened_at = std::time::Instant::now();

    info!(
        surface = "api",
        service = "marketplace",
        action = "cherry_pick.progress.subscribe",
        rpc_id = %rpc_id_owned,
        "cherry-pick progress SSE stream opened"
    );

    let event_stream = stream::unfold(
        (receiver, rpc_id_owned, opened_at),
        move |(mut receiver, rpc_id, opened_at)| async move {
            loop {
                match receiver.recv().await {
                    Ok(frame) => match serde_json::to_string(&frame) {
                        Ok(payload) => {
                            return Some((
                                Ok(Event::default().data(payload)),
                                (receiver, rpc_id, opened_at),
                            ));
                        }
                        Err(error) => {
                            warn!(
                                error = %error,
                                rpc_id = %rpc_id,
                                "serialize progress frame for SSE"
                            );
                        }
                    },
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(
                            skipped,
                            rpc_id = %rpc_id,
                            "cherry-pick SSE subscriber lagged"
                        );
                        return Some((
                            Ok(Event::default().event("lag").data(skipped.to_string())),
                            (receiver, rpc_id, opened_at),
                        ));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        info!(
                            surface = "api",
                            service = "marketplace",
                            action = "cherry_pick.progress.finish",
                            rpc_id = %rpc_id,
                            elapsed_ms = opened_at.elapsed().as_millis(),
                            "cherry-pick progress SSE stream closed (rpc complete)"
                        );
                        return None;
                    }
                }
            }
        },
    );

    Ok(Sse::new(event_stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::http::{HeaderMap, HeaderValue};

    use super::*;

    #[test]
    fn marketplace_artifact_routes_preserve_auth_context_metadata() {
        let mut headers = HeaderMap::new();
        headers.insert("x-request-id", HeaderValue::from_static("req-artifact-1"));
        let mut auth = auth_context("artifact-user", &["lab:read", "lab:admin"]);
        auth.actor_key = Some(Arc::<str>::from("actor-artifact"));
        auth.via_session = true;

        let meta = dispatch_meta_from_headers(&headers, Some(&auth), None);

        assert_eq!(meta.request_id, Some("req-artifact-1"));
        assert_eq!(meta.actor_key, Some("actor-artifact"));
        assert_eq!(meta.agent_kind, Some("device"));
    }

    #[test]
    fn marketplace_artifact_routes_preserve_request_metadata_without_auth() {
        let mut headers = HeaderMap::new();
        headers.insert("x-request-id", HeaderValue::from_static("req-public-1"));

        let meta = dispatch_meta_from_headers(&headers, None, None);

        assert_eq!(meta.request_id, Some("req-public-1"));
        assert_eq!(meta.actor_key, None);
        assert_eq!(meta.agent_kind, None);
    }

    fn auth_context(sub: &str, scopes: &[&str]) -> AuthContext {
        AuthContext {
            sub: sub.to_string(),
            actor_key: None,
            scopes: scopes.iter().map(|scope| (*scope).to_string()).collect(),
            issuer: "test".to_string(),
            via_session: false,
            csrf_token: None,
            email: Some(format!("{sub}@example.com")),
        }
    }

    #[test]
    fn marketplace_write_actions_require_admin_scope() {
        let read_auth = auth_context("reader", &["lab:read"]);
        let admin_auth = auth_context("admin", &["lab:read", "lab:admin"]);
        let read_ext = Extension(read_auth);
        let admin_ext = Extension(admin_auth);

        for action in [
            "artifact.fork",
            "marketplace.artifact.fork",
            "artifact.unfork",
            "artifact.reset",
            "artifact.update.apply",
            "artifact.patch",
            "artifact.config.set",
        ] {
            assert_eq!(
                require_marketplace_admin(action, Some("req"), Some(&read_ext))
                    .unwrap_err()
                    .kind(),
                "forbidden",
                "{action}"
            );
            require_marketplace_admin(action, Some("req"), Some(&admin_ext))
                .unwrap_or_else(|error| panic!("{action}: {error}"));
        }

        require_marketplace_admin("artifact.list", Some("req"), Some(&read_ext)).unwrap();
        require_marketplace_admin("help", Some("req"), None).unwrap();
        require_marketplace_admin("schema", Some("req"), None).unwrap();
    }

    #[test]
    fn marketplace_catalog_admin_actions_drive_rest_gate() {
        let read_auth = Extension(auth_context("reader", &["lab:read"]));
        let admin_auth = Extension(auth_context("admin", &["lab:read", "lab:admin"]));

        for spec in crate::dispatch::marketplace::actions()
            .iter()
            .filter(|spec| spec.requires_admin)
        {
            assert_eq!(
                require_marketplace_admin(spec.name, Some("req"), Some(&read_auth))
                    .unwrap_err()
                    .kind(),
                "forbidden",
                "{}",
                spec.name
            );
            require_marketplace_admin(spec.name, Some("req"), Some(&admin_auth))
                .unwrap_or_else(|error| panic!("{}: {error}", spec.name));
        }

        for spec in crate::dispatch::marketplace::actions()
            .iter()
            .filter(|spec| !spec.requires_admin)
        {
            require_marketplace_admin(spec.name, Some("req"), Some(&read_auth))
                .unwrap_or_else(|error| panic!("{}: {error}", spec.name));
        }
    }
}
