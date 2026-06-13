use std::collections::HashMap;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use axum::{
    Json,
    extract::{
        Path, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::Response,
};
use dashmap::DashMap;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::api::{ToolError, state::AppState};
use crate::config::NodeRole;
use crate::dispatch::node::send::{SessionToken, sender_registry};
use crate::node::checkin::{NodeHello, NodeMetadataUpload, NodeStatus};
use crate::node::enrollment::store::{
    EnrollmentAttempt, EnrollmentDecision, EnrollmentStore, TailnetIdentity,
};
use crate::node::log_event::NodeLogEvent;

// --------------------------------------------------------------------------
// Node dispatch helpers live in `dispatch/node/send.rs` (lab-zxx5.25 dropped
// the `pub use` re-export from this file to prevent laundering of a future
// `dispatch/ → api/` cycle). Callers should import directly from
// `crate::dispatch::node::send::*`.
// --------------------------------------------------------------------------

// --------------------------------------------------------------------------
// Session token counter
// --------------------------------------------------------------------------

fn next_session_token() -> SessionToken {
    static NEXT: OnceLock<AtomicU64> = OnceLock::new();
    NEXT.get_or_init(|| AtomicU64::new(1))
        .fetch_add(1, Ordering::Relaxed)
}

// --------------------------------------------------------------------------
// Auth model: the WS endpoint at /v1/nodes/ws is intentionally outside
// bearer-auth middleware. An unauthenticated WS connection can only call
// `initialize`; all other methods require a live authenticated session.
// --------------------------------------------------------------------------

const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(10);
const INITIALIZE_DEBOUNCE: Duration = Duration::from_secs(30);

fn debounce_map() -> &'static DashMap<String, Instant> {
    static MAP: OnceLock<DashMap<String, Instant>> = OnceLock::new();
    MAP.get_or_init(DashMap::new)
}

// --------------------------------------------------------------------------
// Command state (per-session, per-command)
// --------------------------------------------------------------------------

const COMMAND_CHANNEL_CAPACITY: usize = 512;
const COMMAND_TTL: Duration = Duration::from_secs(5 * 60);
const COMMAND_SWEEP_INTERVAL: Duration = Duration::from_secs(60);
/// How often to send a WebSocket Ping frame to keep connections alive.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
/// Evict a node connection if no Pong has been received within this window.
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(60);

struct CommandState {
    output_tx: mpsc::Sender<serde_json::Value>,
    started_at: Instant,
    node_id: String,
}

// --------------------------------------------------------------------------
// MCP demux allowlist
// --------------------------------------------------------------------------

const DEMUX_ALLOWLIST: &[&str] = &["lab.help", "lab.catalog", "lab.status"];

pub async fn list_nodes(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ToolError> {
    let store = require_master_store(&state)?;
    let nodes = store.list_nodes().await;
    Ok(Json(serde_json::Value::Array(
        nodes
            .into_iter()
            .map(|snapshot| {
                json!({
                    "node_id": snapshot.node_id,
                    "connected": snapshot.connected,
                    "role": snapshot.role,
                    "log_count": snapshot.logs.len(),
                    "discovered_config_count": snapshot
                        .metadata
                        .as_ref()
                        .map(|metadata| metadata.discovered_configs.len())
                        .unwrap_or(0),
                })
            })
            .collect(),
    )))
}

pub async fn get_node(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
) -> Result<Json<serde_json::Value>, ToolError> {
    let store = require_master_store(&state)?;
    let node_id = super::normalize_node_id_value(&node_id, "node_id")?;
    let snapshot = store.node(&node_id).await.ok_or_else(|| ToolError::Sdk {
        sdk_kind: "not_found".to_string(),
        message: format!("unknown node `{node_id}`"),
    })?;
    Ok(Json(json!({
        "node_id": snapshot.node_id,
        "connected": snapshot.connected,
        "role": snapshot.role,
        "status": snapshot.status,
        "metadata": snapshot.metadata,
        "log_count": snapshot.logs.len(),
    })))
}

pub(crate) fn require_master_store(
    state: &AppState,
) -> Result<Arc<crate::node::store::NodeStore>, ToolError> {
    if matches!(state.node_role, Some(NodeRole::NonMaster)) {
        return Err(ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message: "node control queries are only available on the controller".to_string(),
        });
    }
    state
        .node_store
        .clone()
        .ok_or_else(|| ToolError::internal_message("node store is not configured"))
}

pub(crate) fn require_enrollment_store(
    state: &AppState,
) -> Result<Arc<EnrollmentStore>, ToolError> {
    state
        .enrollment_store
        .clone()
        .ok_or_else(|| ToolError::internal_message("enrollment store is not configured"))
}

pub async fn websocket_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> Result<Response, ToolError> {
    let store = require_master_store(&state)?;
    let enrollment_store = require_enrollment_store(&state)?;
    let registry = Arc::clone(&state.registry);
    Ok(ws
        .max_message_size(10 * 1024 * 1024)
        .on_upgrade(move |socket| async move {
            if let Err(error) = handle_websocket(socket, store, enrollment_store, registry).await {
                tracing::warn!(error = %error, "nodes websocket session failed");
            }
        }))
}

fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

async fn handle_websocket(
    socket: WebSocket,
    store: Arc<crate::node::store::NodeStore>,
    enrollment_store: Arc<EnrollmentStore>,
    registry: Arc<crate::registry::ToolRegistry>,
) -> Result<(), anyhow::Error> {
    use axum::extract::ws::WebSocket;
    use futures::stream::SplitSink;

    // Split the WebSocket into independent send/receive halves.
    // This avoids holding a Mutex across awaits and allows the sender registry
    // to push frames to the node independently of the reader loop.
    let (sink, mut stream) = socket.split();

    // mpsc channel that funnels all outbound frames (RPC responses and
    // master-initiated send_to_node pushes) into the writer task below.
    let (tx, rx) = mpsc::channel::<Message>(64);

    // Dedicated writer task: drains `rx` → `sink`.
    let write_task: tokio::task::JoinHandle<Result<SplitSink<WebSocket, Message>, anyhow::Error>> =
        tokio::spawn(async move {
            let mut sink = sink;
            let mut rx: mpsc::Receiver<Message> = rx;
            while let Some(msg) = rx.recv().await {
                sink.send(msg)
                    .await
                    .map_err(|error| anyhow::anyhow!("ws send: {error}"))?;
            }
            Ok(sink)
        });

    let mut session_node_id: Option<String> = None;
    let session_token = next_session_token();
    let mut command_states: HashMap<Uuid, CommandState> = HashMap::new();

    // Background sweeper: every 60s, sends a sentinel to trigger GC in the main loop.
    let tx_sweep = tx.clone();
    let sweep_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(COMMAND_SWEEP_INTERVAL);
        interval.tick().await;
        loop {
            interval.tick().await;
            let sent = tx_sweep
                .send(Message::Text(
                    json!({"_lab_internal":"sweep_tick"}).to_string().into(),
                ))
                .await;
            if sent.is_err() || tx_sweep.is_closed() {
                break;
            }
        }
    });

    // Track last received Pong timestamp (unix seconds) for heartbeat eviction.
    let last_pong = Arc::new(AtomicU64::new(unix_now_secs()));

    // Heartbeat task: send a Ping every HEARTBEAT_INTERVAL seconds.
    let tx_heartbeat = tx.clone();
    let last_pong_hb = Arc::clone(&last_pong);
    let heartbeat_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(HEARTBEAT_INTERVAL);
        interval.tick().await; // skip immediate tick
        loop {
            interval.tick().await;
            // Evict if we haven't received a Pong within HEARTBEAT_TIMEOUT.
            let age_s = unix_now_secs().saturating_sub(last_pong_hb.load(Ordering::Relaxed));
            if age_s > HEARTBEAT_TIMEOUT.as_secs() {
                tracing::warn!(
                    surface = "api",
                    service = "nodes",
                    action = "ws.heartbeat.evict",
                    last_pong_ago_s = age_s,
                    "evicting stale node connection: no pong received in >{}s",
                    HEARTBEAT_TIMEOUT.as_secs(),
                );
                break; // dropping tx_heartbeat will close the channel → write_task exits
            }
            if tx_heartbeat
                .send(Message::Ping(vec![].into()))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    // Security gate: require `initialize` within INITIALIZE_TIMEOUT (10s).
    let init_started = Instant::now();
    let first_msg_result = tokio::time::timeout(INITIALIZE_TIMEOUT, stream.next()).await;
    let first_message = match first_msg_result {
        Err(_timeout) => {
            tracing::warn!(
                surface = "api",
                service = "nodes",
                action = "ws.initialize",
                kind = "timeout",
                "nodes websocket: initialize timeout — closing connection"
            );
            drop(tx);
            sweep_task.abort();
            heartbeat_task.abort();
            drop(write_task.await);
            return Ok(());
        }
        Ok(None) => {
            drop(tx);
            sweep_task.abort();
            heartbeat_task.abort();
            drop(write_task.await);
            return Ok(());
        }
        Ok(Some(msg)) => msg,
    };

    // Process the first message.
    match first_message? {
        Message::Text(text) => {
            if !text.contains("_lab_internal") {
                let request: RpcRequest =
                    serde_json::from_str(&text).map_err(|e| anyhow::anyhow!(e))?;
                let first_was_initialize = request.method == "initialize";
                let response = handle_rpc_request(
                    request,
                    &store,
                    &enrollment_store,
                    &registry,
                    &mut session_node_id,
                    session_token,
                    &tx,
                    &mut command_states,
                )
                .await;
                if tx
                    .send(Message::Text(response.to_string().into()))
                    .await
                    .is_err()
                {
                    drop(tx);
                    sweep_task.abort();
                    drop(write_task.await);
                    return Ok(());
                }
                // Close only when `initialize` was attempted and rejected — the
                // node tried to authenticate but was denied (e.g. pending
                // enrollment, token mismatch). Do NOT close when the first
                // message was a non-`initialize` method: the node may have sent
                // a status push by mistake and will retry with `initialize`.
                if first_was_initialize && session_node_id.is_none() {
                    drop(tx.send(Message::Close(None)).await);
                    drop(tx);
                    sweep_task.abort();
                    heartbeat_task.abort();
                    drop(write_task.await);
                    return Ok(());
                }
            }
        }
        Message::Ping(payload) => {
            let _pong = tx.send(Message::Pong(payload)).await;
        }
        Message::Pong(_) => {
            last_pong.store(unix_now_secs(), Ordering::Relaxed);
        }
        Message::Close(_) | Message::Binary(_) => {}
    }

    // Main read loop.
    loop {
        let message = if session_node_id.is_none() {
            let elapsed = init_started.elapsed();
            let Some(remaining) = INITIALIZE_TIMEOUT.checked_sub(elapsed) else {
                tracing::warn!(
                    surface = "api",
                    service = "nodes",
                    action = "ws.initialize",
                    kind = "timeout",
                    "nodes websocket: initialize timeout — closing connection"
                );
                break;
            };
            match tokio::time::timeout(remaining, stream.next()).await {
                Ok(message) => message,
                Err(_timeout) => {
                    tracing::warn!(
                        surface = "api",
                        service = "nodes",
                        action = "ws.initialize",
                        kind = "timeout",
                        "nodes websocket: initialize timeout — closing connection"
                    );
                    break;
                }
            }
        } else {
            stream.next().await
        };

        let Some(message) = message else {
            break;
        };

        match message? {
            Message::Text(text) => {
                // Sweep sentinel — GC stale commands.
                if text.contains("_lab_internal") {
                    let now = Instant::now();
                    command_states.retain(|cmd_id, state| {
                        if now.duration_since(state.started_at) > COMMAND_TTL {
                            tracing::warn!(
                                surface = "api", service = "nodes", action = "ws.command.sweep",
                                command_id = %cmd_id, node_id = %state.node_id,
                                "sweeper: dropping stale command entry"
                            );
                            false
                        } else {
                            true
                        }
                    });
                    continue;
                }
                // lab-zxx5.6: frames from the node may be either (a) RPC
                // requests initiated by the node (have `method`) or (b) RPC
                // responses to master-initiated requests (have `result` or
                // `error`, no `method`). Try the response path first using
                // the pending-map resolver; if no pending id matches, fall
                // through to the request path.
                let parsed_value: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(error) => {
                        return Err(anyhow::anyhow!(error));
                    }
                };
                let looks_like_response = parsed_value.get("method").is_none()
                    && (parsed_value.get("result").is_some()
                        || parsed_value.get("error").is_some());
                if looks_like_response {
                    let id = parsed_value
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string);
                    if let Some(id) = id {
                        if crate::dispatch::node::send::resolve_pending_rpc(&id, parsed_value) {
                            continue;
                        }
                    }
                    // Response whose id doesn't match any pending entry: drop
                    // with a warning; don't break the session.
                    tracing::warn!(
                        surface = "api",
                        service = "nodes",
                        "received rpc response with unknown id"
                    );
                    continue;
                }

                // lab-zxx5.16: install/progress notifications have
                // `method = "install/progress"` and carry the originating
                // rpc_id in `params.rpc_id`. These are notifications, not
                // requests (no reply expected) — publish to the progress
                // broadcast channel so any subscribed SSE streams forward
                // them to clients, then drop.
                if parsed_value
                    .get("method")
                    .and_then(serde_json::Value::as_str)
                    == Some("install/progress")
                {
                    if let Some(rpc_id) = parsed_value
                        .get("params")
                        .and_then(|p| p.get("rpc_id"))
                        .and_then(serde_json::Value::as_str)
                    {
                        let rpc_id = rpc_id.to_string();
                        // lab-zxx5.20: reject forged progress frames; a
                        // compromised node can only publish progress for
                        // rpc_ids the master actually dispatched TO IT.
                        //
                        // lab-zxx5.32: tier the log levels so the genuine
                        // forgery case (rpc_id IS in flight, but to a
                        // DIFFERENT node) is the only case that emits WARN.
                        // The benign race case (rpc_id was already resolved,
                        // pending_owners cleared, late progress frame from
                        // the legitimate owner) drops to DEBUG so it can't
                        // drown real audit signal.
                        match session_node_id.as_deref() {
                            Some(sender_id)
                                if crate::dispatch::node::send::rpc_id_owned_by(
                                    &rpc_id, sender_id,
                                ) =>
                            {
                                crate::dispatch::node::send::publish_progress(
                                    &rpc_id,
                                    parsed_value,
                                );
                            }
                            Some(sender_id) => {
                                let in_flight =
                                    crate::dispatch::node::send::rpc_id_in_flight(&rpc_id);
                                if in_flight {
                                    // Genuine forgery: rpc_id IS in flight,
                                    // sender does NOT own it.
                                    tracing::warn!(
                                        surface = "api", service = "nodes",
                                        sender_node_id = %sender_id,
                                        rpc_id = %rpc_id,
                                        "forged install/progress dropped (rpc_id not owned by sender)"
                                    );
                                } else {
                                    // Benign late frame: rpc_id already
                                    // resolved or never existed. DEBUG only.
                                    tracing::debug!(
                                        surface = "api", service = "nodes",
                                        sender_node_id = %sender_id,
                                        rpc_id = %rpc_id,
                                        "install/progress for unknown rpc_id (likely late frame post-resolve)"
                                    );
                                }
                            }
                            None => {
                                tracing::warn!(
                                    surface = "api", service = "nodes",
                                    rpc_id = %rpc_id,
                                    "install/progress from uninitialized session dropped"
                                );
                            }
                        }
                    } else {
                        // lab-zxx5.32: include sender node_id and frame size
                        // so an operator searching logs by node can correlate.
                        let frame_size = parsed_value.to_string().len();
                        tracing::warn!(
                            surface = "api", service = "nodes",
                            sender_node_id = ?session_node_id,
                            frame_size,
                            "install/progress notification missing params.rpc_id; frame dropped"
                        );
                    }
                    continue;
                }
                let request: RpcRequest = match serde_json::from_value(parsed_value) {
                    Ok(r) => r,
                    Err(error) => return Err(anyhow::anyhow!(error)),
                };
                let response = handle_rpc_request(
                    request,
                    &store,
                    &enrollment_store,
                    &registry,
                    &mut session_node_id,
                    session_token,
                    &tx,
                    &mut command_states,
                )
                .await;
                if tx
                    .send(Message::Text(response.to_string().into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Message::Ping(payload) => {
                if tx.send(Message::Pong(payload)).await.is_err() {
                    break;
                }
            }
            Message::Pong(_) => {
                last_pong.store(unix_now_secs(), Ordering::Relaxed);
                tracing::trace!(
                    surface = "api", service = "nodes", action = "ws.pong",
                    node_id = ?session_node_id, "pong received",
                );
            }
            Message::Binary(_) => {
                if tx
                    .send(Message::Text(
                        error_response(None, -32600, "binary websocket frames are not supported")
                            .to_string()
                            .into(),
                    ))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Message::Close(_) => break,
        }
    }

    // Remove sender from registry BEFORE dropping `tx` so the write task
    // drains any pending frames cleanly.
    if let Some(ref node_id) = session_node_id {
        let should_disconnect = {
            let mut registry_map = sender_registry().write().await;
            if registry_map.get(node_id).map(|(token, _)| *token) == Some(session_token) {
                registry_map.remove(node_id);
                true
            } else {
                false
            }
        };
        if should_disconnect {
            store.set_connected(node_id, false).await;
        }
    }

    drop(command_states);
    sweep_task.abort();
    heartbeat_task.abort();
    drop(tx);
    drop(write_task.await);

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_rpc_request(
    request: RpcRequest,
    store: &crate::node::store::NodeStore,
    enrollment_store: &EnrollmentStore,
    registry: &crate::registry::ToolRegistry,
    session_node_id: &mut Option<String>,
    session_token: SessionToken,
    tx: &mpsc::Sender<Message>,
    command_states: &mut HashMap<Uuid, CommandState>,
) -> serde_json::Value {
    match request.method.as_str() {
        "initialize" => {
            let params: InitializeParams =
                match serde_json::from_value(request.params.unwrap_or(serde_json::Value::Null)) {
                    Ok(params) => params,
                    Err(error) => {
                        return error_response(
                            request.id,
                            -32602,
                            format!("invalid initialize params: {error}"),
                        );
                    }
                };

            match handle_initialize(store, enrollment_store, &params).await {
                Ok(initialized) => {
                    // Register this node's outbound sender so the controller can push RPC
                    // requests later via `send_to_node`.
                    sender_registry()
                        .write()
                        .await
                        .insert(initialized.node_id.clone(), (session_token, tx.clone()));
                    *session_node_id = Some(initialized.node_id.clone());
                    success_response(
                        request.id,
                        json!({
                            "protocolVersion": params.protocol_version,
                            "serverInfo": {
                                "name": "lab-nodes",
                                "version": env!("CARGO_PKG_VERSION"),
                            },
                            "_meta": {
                                "lab.node_id": initialized.node_id,
                            }
                        }),
                    )
                }
                Err(error) => tool_error_response(request.id, &error),
            }
        }
        "nodes/status.push" => {
            match require_initialized_node_id(session_node_id).and_then(|node_id| {
                let params = request.params.unwrap_or(serde_json::Value::Null);
                parse_status_params(params, &node_id)
            }) {
                Ok(status) => {
                    store.record_status(status).await;
                    success_response(request.id, json!({}))
                }
                Err(error) => tool_error_response(request.id, &error),
            }
        }
        "nodes/metadata.push" => {
            match require_initialized_node_id(session_node_id).and_then(|node_id| {
                let params = request.params.unwrap_or(serde_json::Value::Null);
                parse_metadata_params(params, &node_id)
            }) {
                Ok(metadata) => {
                    store.record_metadata(metadata).await;
                    success_response(request.id, json!({}))
                }
                Err(error) => tool_error_response(request.id, &error),
            }
        }
        "nodes/log.event" => {
            let start = Instant::now();
            match require_initialized_node_id(session_node_id).and_then(|node_id| {
                let params = request.params.unwrap_or(serde_json::Value::Null);
                parse_log_events(params, &node_id).map(|events| (node_id, events))
            }) {
                Ok((node_id, events)) => {
                    let event_count = events.len();
                    store.record_logs(&node_id, events).await;
                    tracing::info!(
                        surface = "api",
                        service = "nodes",
                        action = "ws.log.event",
                        node_id = %node_id,
                        event_count,
                        elapsed_ms = start.elapsed().as_millis(),
                        "nodes websocket log batch recorded"
                    );
                    success_response(request.id, json!({}))
                }
                Err(error) => tool_error_response(request.id, &error),
            }
        }
        "nodes/ping" => {
            if let Err(error) = require_initialized_node_id(session_node_id) {
                return tool_error_response(request.id, &error);
            }
            success_response(request.id, json!({}))
        }
        "nodes/device.enroll" => {
            // Require an initialized session: the WS endpoint is intentionally
            // outside bearer-auth middleware, but every method except
            // `initialize` must run on a session that has already passed
            // `handle_initialize` (device-token + tailnet-identity validation).
            // Without this gate any unauthenticated peer that opened the
            // socket could upsert/overwrite arbitrary `node_id` enrollments.
            let node_id_opt = match require_initialized_node_id(session_node_id) {
                Ok(id) => Some(id),
                Err(error) => return tool_error_response(request.id, &error),
            };
            match handle_device_enroll(
                store,
                request.params.unwrap_or(serde_json::Value::Null),
                node_id_opt,
            )
            .await
            {
                Ok(enrolled_node_id) => success_response(
                    request.id,
                    json!({"enrolled": true, "node_id": enrolled_node_id}),
                ),
                Err(error) => json!({
                    "jsonrpc": "2.0", "id": request.id,
                    "error": {"code": -32602, "message": error.to_string(), "data": {"kind": "enroll_rejected"}}
                }),
            }
        }
        "nodes/command.invoke" => {
            let node_id = match require_initialized_node_id(session_node_id) {
                Ok(id) => id,
                Err(error) => return tool_error_response(request.id, &error),
            };
            let params = request.params.unwrap_or(serde_json::Value::Null);
            let command_id = Uuid::new_v4();
            let (output_tx, _output_rx) =
                mpsc::channel::<serde_json::Value>(COMMAND_CHANNEL_CAPACITY);
            command_states.insert(
                command_id,
                CommandState {
                    output_tx,
                    started_at: Instant::now(),
                    node_id: node_id.clone(),
                },
            );
            let invoke_msg = json!({
                "jsonrpc": "2.0", "id": command_id.to_string(), "method": "nodes/command.invoke",
                "params": {"command_id": command_id.to_string(), "command": params.get("command").cloned().unwrap_or(serde_json::Value::Null)}
            });
            drop(tx.send(Message::Text(invoke_msg.to_string().into())).await);
            tracing::info!(surface = "api", service = "nodes", action = "ws.command.invoke", node_id = %node_id, command_id = %command_id, "nodes websocket command invoked");
            success_response(request.id, json!({"command_id": command_id.to_string()}))
        }
        "nodes/command.output" => {
            let node_id = match require_initialized_node_id(session_node_id) {
                Ok(id) => id,
                Err(error) => return tool_error_response(request.id, &error),
            };
            let params = request.params.unwrap_or(serde_json::Value::Null);
            let command_id_str = match params.get("command_id").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => {
                    return error_response(
                        request.id,
                        -32602,
                        "missing command_id in command.output",
                    );
                }
            };
            let command_id = match Uuid::parse_str(&command_id_str) {
                Ok(id) => id,
                Err(_) => return error_response(request.id, -32602, "invalid command_id format"),
            };
            if let Some(cmd_state) = command_states.get(&command_id) {
                let chunk = params
                    .get("chunk")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                drop(cmd_state.output_tx.try_send(chunk));
                tracing::debug!(surface = "api", service = "nodes", action = "ws.command.output", node_id = %node_id, command_id = %command_id, "nodes websocket command output chunk");
            }
            success_response(request.id, json!({}))
        }
        "nodes/command.result" => {
            let node_id = match require_initialized_node_id(session_node_id) {
                Ok(id) => id,
                Err(error) => return tool_error_response(request.id, &error),
            };
            let params = request.params.unwrap_or(serde_json::Value::Null);
            let command_id_str = match params.get("command_id").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => {
                    return error_response(
                        request.id,
                        -32602,
                        "missing command_id in command.result",
                    );
                }
            };
            let command_id = match Uuid::parse_str(&command_id_str) {
                Ok(id) => id,
                Err(_) => return error_response(request.id, -32602, "invalid command_id format"),
            };
            command_states.remove(&command_id);
            let exit_code = params
                .get("exit_code")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let success_flag = params
                .get("success")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            tracing::info!(surface = "api", service = "nodes", action = "ws.command.result", node_id = %node_id, command_id = %command_id, exit_code, success = success_flag, "nodes websocket command completed");
            success_response(
                request.id,
                json!({"command_id": command_id.to_string(), "exit_code": exit_code, "success": success_flag}),
            )
        }
        "nodes/peer.invoke" => {
            tracing::debug!(
                surface = "api",
                service = "nodes",
                action = "ws.peer.invoke",
                "nodes/peer.invoke is not yet implemented"
            );
            json!({"jsonrpc": "2.0", "id": request.id, "error": {"code": -32601, "message": "peer.invoke not yet implemented", "data": {"kind": "not_implemented"}}})
        }
        other => {
            let method_clamped = other.chars().take(256).collect::<String>();
            if other.starts_with("nodes/") {
                error_response(
                    request.id,
                    -32601,
                    format!("unsupported nodes websocket method `{method_clamped}`"),
                )
            } else if DEMUX_ALLOWLIST.contains(&other) {
                let node_id = session_node_id
                    .clone()
                    .unwrap_or_else(|| "<unauthenticated>".to_string());
                handle_demux(request.id, other, request.params, registry, &node_id).await
            } else {
                tracing::debug!(surface = "api", service = "nodes", action = "ws.demux.blocked", method = %method_clamped, "demux: method not in allowlist");
                json!({"jsonrpc": "2.0", "id": request.id, "error": {"code": -32601, "message": "method not permitted over fleet WS", "data": {"kind": "not_permitted"}}})
            }
        }
    }
}

/// Handle MCP demux for allowlisted non-nodes/ methods.
async fn handle_demux(
    id: Option<serde_json::Value>,
    method: &str,
    params: Option<serde_json::Value>,
    registry: &crate::registry::ToolRegistry,
    node_id: &str,
) -> serde_json::Value {
    let (service_name, action) = match method.split_once('.') {
        Some((s, a)) => (s, a),
        None => {
            return json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"malformed demux method","data":{"kind":"not_permitted"}}});
        }
    };
    let svc = match registry.service(service_name) {
        Some(s) => s,
        None => {
            return json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":format!("service `{service_name}` not found"),"data":{"kind":"not_permitted"}}});
        }
    };
    let dispatch_params = params.unwrap_or(serde_json::Value::Null);
    tracing::info!(surface = "api", service = "nodes", action = "ws.demux.forward", method = %method, node_id = %node_id, "demux: forwarding allowlisted method to registry");
    let dispatch_fn = svc.dispatch;
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        dispatch_fn(action.to_string(), dispatch_params),
    )
    .await;
    match result {
        Err(_timeout) => {
            tracing::warn!(surface = "api", service = "nodes", action = "ws.demux.timeout", method = %method, node_id = %node_id, kind = "upstream_timeout", "demux: upstream call timed out");
            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32001,"message":"upstream timeout","data":{"kind":"upstream_timeout"}}})
        }
        Ok(Ok(value)) => success_response(id, value),
        Ok(Err(error)) => tool_error_response(id, &error),
    }
}

/// Handle `nodes/device.enroll` — upsert a node enrollment record.
async fn handle_device_enroll(
    store: &crate::node::store::NodeStore,
    params: serde_json::Value,
    _session_node_id: Option<String>,
) -> Result<String, ToolError> {
    let node_id = params
        .get("node_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ToolError::InvalidParam {
            message: "nodes/device.enroll requires non-empty `node_id`".to_string(),
            param: "node_id".to_string(),
        })?
        .to_string();
    let role = params
        .get("role")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ToolError::InvalidParam {
            message: "nodes/device.enroll requires non-empty `role`".to_string(),
            param: "role".to_string(),
        })?
        .to_string();
    let version = params
        .get("version")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ToolError::InvalidParam {
            message: "nodes/device.enroll requires non-empty `version`".to_string(),
            param: "version".to_string(),
        })?
        .to_string();

    const KNOWN_ROLES: &[&str] = &["node", "master"];
    if !KNOWN_ROLES.contains(&role.as_str()) {
        return Err(ToolError::InvalidParam {
            message: format!(
                "unknown role `{role}`; accepted roles: {}",
                KNOWN_ROLES.join(", ")
            ),
            param: "role".to_string(),
        });
    }
    if let Some(snapshot) = store.node(&node_id).await {
        let existing_role = snapshot.role.as_deref().unwrap_or("");
        if !existing_role.is_empty() && existing_role != role {
            tracing::warn!(surface = "api", service = "nodes", action = "ws.device.enroll", node_id = %node_id, existing_role = %existing_role, requested_role = %role, "enroll conflict: role mismatch");
            return Err(ToolError::Sdk {
                sdk_kind: "enroll_conflict".to_string(),
                message: "re-enrollment requires explicit force flag".to_string(),
            });
        }
    }
    store
        .record_hello(NodeHello {
            node_id: node_id.clone(),
            role: role.clone(),
            version: version.clone(),
        })
        .await;
    tracing::info!(surface = "api", service = "nodes", action = "ws.device.enroll", node_id = %node_id, role = %role, version = %version, "nodes/device.enroll: node enrolled");
    Ok(node_id)
}

async fn handle_initialize(
    store: &crate::node::store::NodeStore,
    enrollment_store: &EnrollmentStore,
    params: &InitializeParams,
) -> Result<InitializedDevice, ToolError> {
    let meta = params
        .meta
        .as_ref()
        .ok_or_else(|| ToolError::InvalidParam {
            message: "initialize params must include `_meta`".to_string(),
            param: "_meta".to_string(),
        })?;
    let node_id = super::normalize_node_id_value(&meta.node_id, "_meta.lab.node_id")?;
    if meta.device_token.trim().is_empty() {
        return Err(ToolError::InvalidParam {
            message: "initialize `_meta.lab.device_token` must not be empty".to_string(),
            param: "_meta.lab.device_token".to_string(),
        });
    }
    let tailnet_identity =
        meta.tailnet_identity
            .clone()
            .ok_or_else(|| ToolError::InvalidParam {
                message: "initialize `_meta.lab.tailnet_identity` must be present".to_string(),
                param: "_meta.lab.tailnet_identity".to_string(),
            })?;

    match enrollment_store
        .validate(&node_id, &meta.device_token)
        .await
        .map_err(|error| ToolError::internal_message(format!("validate enrollment: {error}")))?
    {
        EnrollmentDecision::Approved(_) => {}
        EnrollmentDecision::PendingRequired => {
            // Per-node enrollment debounce: reject if same node_id sent initialize within 30s.
            {
                let now = Instant::now();
                let map = debounce_map();
                if let Some(last_seen) = map.get(&node_id) {
                    if now.duration_since(*last_seen) < INITIALIZE_DEBOUNCE {
                        return Err(ToolError::Sdk {
                            sdk_kind: "enrollment_required".to_string(),
                            message: format!(
                                "node `{node_id}` sent initialize within debounce window; retry after 30s"
                            ),
                        });
                    }
                }
                map.insert(node_id.clone(), now);
                // Inline GC: drop entries whose debounce window has expired so
                // the map can't grow unbounded across distinct node_ids.
                map.retain(|_, last_seen| now.duration_since(*last_seen) < INITIALIZE_DEBOUNCE);
            }

            // Pending enrollment cap check.
            const MAX_PENDING_ENROLLMENTS: usize = 1000;
            let snapshot = enrollment_store.list().await.map_err(|error| {
                ToolError::internal_message(format!("list enrollments: {error}"))
            })?;
            if snapshot.pending.len() >= MAX_PENDING_ENROLLMENTS {
                return Err(ToolError::Sdk {
                    sdk_kind: "enrollment_cap_exceeded".to_string(),
                    message: "enrollment queue is full; try again later".to_string(),
                });
            }

            enrollment_store
                .record_pending(EnrollmentAttempt {
                    node_id: node_id.clone(),
                    token: meta.device_token.clone(),
                    tailnet_identity,
                    client_version: params.client_info.version.clone(),
                    metadata: None,
                })
                .await
                .map_err(|error| {
                    ToolError::internal_message(format!("record pending enrollment: {error}"))
                })?;
            return Err(ToolError::Sdk {
                sdk_kind: "enrollment_required".to_string(),
                message: format!("node `{node_id}` is pending enrollment approval"),
            });
        }
        EnrollmentDecision::Denied(_) => {
            return Err(ToolError::Sdk {
                sdk_kind: "access_denied".to_string(),
                message: format!("node `{node_id}` has been denied enrollment"),
            });
        }
        EnrollmentDecision::TokenMismatch(_) => {
            return Err(ToolError::Sdk {
                sdk_kind: "auth_failed".to_string(),
                message: format!("node `{node_id}` presented an unexpected token"),
            });
        }
    }

    store
        .record_hello(NodeHello {
            node_id: node_id.clone(),
            role: "node".to_string(),
            version: params.client_info.version.clone(),
        })
        .await;
    store.set_connected(&node_id, true).await;
    Ok(InitializedDevice { node_id })
}

fn parse_status_params(
    params: serde_json::Value,
    session_node_id: &str,
) -> Result<NodeStatus, ToolError> {
    let mut status: NodeStatus =
        serde_json::from_value(params).map_err(|error| ToolError::InvalidParam {
            message: format!("invalid nodes/status.push params: {error}"),
            param: "params".to_string(),
        })?;
    status.node_id = super::normalize_node_id_value(&status.node_id, "params.node_id")?;
    if status.node_id != session_node_id {
        return Err(ToolError::InvalidParam {
            message: format!(
                "status node_id `{}` does not match initialized node `{session_node_id}`",
                status.node_id
            ),
            param: "params.node_id".to_string(),
        });
    }
    Ok(status)
}

fn parse_log_events(
    params: serde_json::Value,
    session_node_id: &str,
) -> Result<Vec<NodeLogEvent>, ToolError> {
    let payload: NodeLogEventParams =
        serde_json::from_value(params).map_err(|error| ToolError::InvalidParam {
            message: format!("invalid nodes/log.event params: {error}"),
            param: "params".to_string(),
        })?;
    let node_id = super::normalize_node_id_value(&payload.node_id, "params.node_id")?;
    if node_id != session_node_id {
        return Err(ToolError::InvalidParam {
            message: format!(
                "log batch node_id `{node_id}` does not match initialized node `{session_node_id}`"
            ),
            param: "params.node_id".to_string(),
        });
    }

    let mut events = payload.events;
    for (index, event) in events.iter_mut().enumerate() {
        event.node_id =
            super::normalize_node_id_value(&event.node_id, &format!("events[{index}].node_id"))?;
        if event.node_id != node_id {
            return Err(ToolError::InvalidParam {
                message: format!("events[{index}].node_id must match batch node_id `{node_id}`"),
                param: format!("events[{index}].node_id"),
            });
        }
    }
    Ok(events)
}

fn parse_metadata_params(
    params: serde_json::Value,
    session_node_id: &str,
) -> Result<NodeMetadataUpload, ToolError> {
    let mut metadata: NodeMetadataUpload =
        serde_json::from_value(params).map_err(|error| ToolError::InvalidParam {
            message: format!("invalid nodes/metadata.push params: {error}"),
            param: "params".to_string(),
        })?;
    metadata.node_id = super::normalize_node_id_value(&metadata.node_id, "params.node_id")?;
    if metadata.node_id != session_node_id {
        return Err(ToolError::InvalidParam {
            message: format!(
                "metadata node_id `{}` does not match initialized node `{session_node_id}`",
                metadata.node_id
            ),
            param: "params.node_id".to_string(),
        });
    }
    Ok(metadata)
}

fn require_initialized_node_id(session_node_id: &Option<String>) -> Result<String, ToolError> {
    session_node_id.clone().ok_or_else(|| ToolError::Sdk {
        sdk_kind: "auth_failed".to_string(),
        message: "websocket session must send initialize before node methods".to_string(),
    })
}

fn tool_error_response(id: Option<serde_json::Value>, error: &ToolError) -> serde_json::Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": tool_error_code(error),
            "message": error.to_string(),
            "data": error,
        }
    })
}

fn tool_error_code(error: &ToolError) -> i64 {
    match error.kind() {
        "invalid_param" | "missing_param" | "validation_failed" => -32602,
        "auth_failed" | "access_denied" | "enrollment_required" => -32001,
        _ => -32000,
    }
}

fn success_response(id: Option<serde_json::Value>, result: serde_json::Value) -> serde_json::Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

fn error_response(
    id: Option<serde_json::Value>,
    code: i64,
    message: impl Into<String>,
) -> serde_json::Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message.into(),
        }
    })
}

#[derive(Debug, Deserialize)]
struct RpcRequest {
    id: Option<serde_json::Value>,
    method: String,
    #[serde(default)]
    params: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitializeParams {
    protocol_version: String,
    client_info: ClientInfo,
    #[serde(rename = "_meta")]
    meta: Option<InitializeMeta>,
}

#[derive(Debug, Deserialize)]
struct ClientInfo {
    version: String,
}

#[derive(Debug, Deserialize)]
struct InitializeMeta {
    #[serde(rename = "lab.node_id")]
    node_id: String,
    #[serde(rename = "lab.device_token")]
    device_token: String,
    #[serde(rename = "lab.tailnet_identity")]
    tailnet_identity: Option<TailnetIdentity>,
}

#[derive(Debug, Deserialize)]
struct NodeLogEventParams {
    node_id: String,
    events: Vec<NodeLogEvent>,
}

struct InitializedDevice {
    node_id: String,
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use std::sync::Arc;

    use axum::{Router, routing::get};
    use futures::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    use super::*;

    #[tokio::test]
    async fn websocket_initialize_metadata_status_and_logs_round_trip_into_store() {
        // Use a unique node_id to avoid global sender_registry collisions with other
        // concurrent tests that also use "device-1".
        let node_id = format!("device-roundtrip-{}", Uuid::new_v4());
        let store = Arc::new(crate::node::store::NodeStore::default());
        let enrollment_store = Arc::new(
            EnrollmentStore::open(test_enrollment_store_path("fleet-happy"))
                .await
                .expect("open enrollment store"),
        );
        enrollment_store
            .record_pending(EnrollmentAttempt {
                node_id: node_id.clone(),
                token: "token-1".to_string(),
                tailnet_identity: TailnetIdentity {
                    node_key: "node-key".to_string(),
                    login_name: "user@example.com".to_string(),
                    hostname: node_id.clone(),
                },
                client_version: "0.7.3".to_string(),
                metadata: None,
            })
            .await
            .expect("record pending");
        enrollment_store
            .approve(&node_id, None)
            .await
            .expect("approve");
        let state = AppState::new()
            .with_node_store(store.clone())
            .with_enrollment_store(enrollment_store);
        let app = Router::new()
            .route("/v1/nodes/ws", get(websocket_upgrade))
            .with_state(state);

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });

        let (mut socket, _) = connect_async(format!("ws://{addr}/v1/nodes/ws"))
            .await
            .expect("connect");

        socket
            .send(Message::Text(
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {},
                        "clientInfo": {
                            "name": "lab-node",
                            "version": "0.7.3",
                        },
                        "_meta": {
                            "lab.node_id": node_id,
                            "lab.device_token": "token-1",
                            "lab.tailnet_identity": {
                                "node_key": "node-key",
                                "login_name": "user@example.com",
                                "hostname": node_id,
                            }
                        }
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("send initialize");
        let init_response = next_text(&mut socket).await;
        assert_eq!(init_response["result"]["_meta"]["lab.node_id"], node_id);

        socket
            .send(Message::Text(
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "nodes/metadata.push",
                    "params": {
                        "node_id": node_id,
                        "discovered_configs": []
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("send metadata");
        let metadata_response = next_text(&mut socket).await;
        assert!(metadata_response.get("error").is_none());

        socket
            .send(Message::Text(
                json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "nodes/status.push",
                    "params": {
                        "node_id": node_id,
                        "connected": true,
                        "cpu_percent": 12.5,
                        "memory_used_bytes": 1024,
                        "storage_used_bytes": 2048,
                        "os": "linux",
                        "ips": ["100.64.0.1"]
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("send status");
        let status_response = next_text(&mut socket).await;
        assert!(status_response.get("error").is_none());

        socket
            .send(Message::Text(
                json!({
                    "jsonrpc": "2.0",
                    "id": 4,
                    "method": "nodes/log.event",
                    "params": {
                        "node_id": node_id,
                        "events": [{
                            "node_id": node_id,
                            "source": "syslog",
                            "timestamp_unix_ms": 1234,
                            "level": "info",
                            "message": "hello from ws",
                            "fields": {}
                        }]
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("send logs");
        let log_response = next_text(&mut socket).await;
        assert!(log_response.get("error").is_none());

        socket.close(None).await.expect("close");
        tokio::time::sleep(Duration::from_millis(100)).await;

        let snapshot = store.node(&node_id).await.expect("snapshot");
        assert!(!snapshot.connected, "node must be disconnected after close");
        assert_eq!(snapshot.role.as_deref(), Some("node"));
        assert_eq!(
            snapshot
                .metadata
                .as_ref()
                .map(|metadata| metadata.discovered_configs.len()),
            Some(0)
        );
        assert_eq!(
            snapshot.status.as_ref().and_then(|s| s.os.as_deref()),
            Some("linux")
        );
        assert_eq!(snapshot.logs.len(), 1);
        assert_eq!(snapshot.logs[0].message, "hello from ws");

        server.abort();
    }

    #[tokio::test]
    async fn initialize_unknown_device_creates_pending_and_rejects() {
        let store = Arc::new(crate::node::store::NodeStore::default());
        let enrollment_store = Arc::new(
            EnrollmentStore::open(test_enrollment_store_path("fleet-unknown"))
                .await
                .expect("open"),
        );
        let state = AppState::new()
            .with_node_store(store)
            .with_enrollment_store(enrollment_store.clone());
        let app = Router::new()
            .route("/v1/nodes/ws", get(websocket_upgrade))
            .with_state(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let (mut socket, _) = connect_async(format!("ws://{addr}/v1/nodes/ws"))
            .await
            .expect("connect");

        send_initialize(&mut socket, "device-unknown", "token-unknown").await;
        let response = next_text(&mut socket).await;
        assert_eq!(response["error"]["data"]["kind"], "enrollment_required");

        let snapshot = enrollment_store.list().await.expect("list");
        assert!(snapshot.pending.contains_key("device-unknown"));
        assert!(snapshot.approved.is_empty());

        let closed = tokio::time::timeout(Duration::from_secs(1), socket.next())
            .await
            .expect("server should close unauthenticated websocket after enrollment rejection");
        assert!(
            matches!(closed, None | Some(Ok(Message::Close(_)))),
            "unexpected websocket frame after enrollment rejection: {closed:?}"
        );

        server.abort();
    }

    #[tokio::test]
    async fn initialize_approved_device_is_admitted() {
        let state = approved_ws_state("device-1", "token-1").await;
        let app = Router::new()
            .route("/v1/nodes/ws", get(websocket_upgrade))
            .with_state(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let (mut socket, _) = connect_async(format!("ws://{addr}/v1/nodes/ws"))
            .await
            .expect("connect");

        send_initialize(&mut socket, "device-1", "token-1").await;
        let response = next_text(&mut socket).await;
        assert!(response.get("error").is_none());
        assert_eq!(response["result"]["_meta"]["lab.node_id"], "device-1");

        server.abort();
    }

    #[tokio::test]
    async fn initialize_denied_device_is_rejected() {
        let store = Arc::new(crate::node::store::NodeStore::default());
        let enrollment_store = Arc::new(
            EnrollmentStore::open(test_enrollment_store_path("fleet-denied"))
                .await
                .expect("open"),
        );
        enrollment_store
            .record_pending(EnrollmentAttempt {
                node_id: "device-1".to_string(),
                token: "token-1".to_string(),
                tailnet_identity: TailnetIdentity {
                    node_key: "node-key".to_string(),
                    login_name: "user@example.com".to_string(),
                    hostname: "device-1".to_string(),
                },
                client_version: "0.7.3".to_string(),
                metadata: None,
            })
            .await
            .expect("record pending");
        enrollment_store
            .deny("device-1", Some("no".to_string()))
            .await
            .expect("deny");
        let state = AppState::new()
            .with_node_store(store)
            .with_enrollment_store(enrollment_store);
        let app = Router::new()
            .route("/v1/nodes/ws", get(websocket_upgrade))
            .with_state(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let (mut socket, _) = connect_async(format!("ws://{addr}/v1/nodes/ws"))
            .await
            .expect("connect");

        send_initialize(&mut socket, "device-1", "token-1").await;
        let response = next_text(&mut socket).await;
        assert_eq!(response["error"]["data"]["kind"], "access_denied");

        server.abort();
    }

    #[tokio::test]
    async fn initialize_wrong_token_for_approved_device_is_rejected() {
        let state = approved_ws_state("device-1", "token-1").await;
        let app = Router::new()
            .route("/v1/nodes/ws", get(websocket_upgrade))
            .with_state(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let (mut socket, _) = connect_async(format!("ws://{addr}/v1/nodes/ws"))
            .await
            .expect("connect");

        send_initialize(&mut socket, "device-1", "wrong-token").await;
        let response = next_text(&mut socket).await;
        assert_eq!(response["error"]["data"]["kind"], "auth_failed");

        server.abort();
    }

    #[tokio::test]
    async fn node_methods_before_initialize_return_request_error_without_closing_socket() {
        let state = approved_ws_state("device-1", "token-1").await;
        let app = Router::new()
            .route("/v1/nodes/ws", get(websocket_upgrade))
            .with_state(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let (mut socket, _) = connect_async(format!("ws://{addr}/v1/nodes/ws"))
            .await
            .expect("connect");

        socket
            .send(Message::Text(
                json!({
                    "jsonrpc": "2.0",
                    "id": 99,
                    "method": "nodes/status.push",
                    "params": {
                        "node_id": "device-1",
                        "connected": true,
                        "ips": [],
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("send pre-init status");
        let pre_init_response = next_text(&mut socket).await;
        assert_eq!(pre_init_response["error"]["data"]["kind"], "auth_failed");

        send_initialize(&mut socket, "device-1", "token-1").await;
        let init_response = next_text(&mut socket).await;
        assert!(init_response.get("error").is_none());

        server.abort();
    }

    async fn approved_ws_state(node_id: &str, token: &str) -> AppState {
        let store = Arc::new(crate::node::store::NodeStore::default());
        let enrollment_store = Arc::new(
            EnrollmentStore::open(test_enrollment_store_path("fleet-approved"))
                .await
                .expect("open"),
        );
        enrollment_store
            .record_pending(EnrollmentAttempt {
                node_id: node_id.to_string(),
                token: token.to_string(),
                tailnet_identity: TailnetIdentity {
                    node_key: "node-key".to_string(),
                    login_name: "user@example.com".to_string(),
                    hostname: node_id.to_string(),
                },
                client_version: "0.7.3".to_string(),
                metadata: None,
            })
            .await
            .expect("record pending");
        enrollment_store
            .approve(node_id, None)
            .await
            .expect("approve");
        AppState::new()
            .with_node_store(store)
            .with_enrollment_store(enrollment_store)
    }

    async fn send_initialize(
        socket: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        node_id: &str,
        token: &str,
    ) {
        socket
            .send(Message::Text(
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {},
                        "clientInfo": {
                            "name": "lab-node",
                            "version": "0.7.3",
                        },
                        "_meta": {
                            "lab.node_id": node_id,
                            "lab.device_token": token,
                            "lab.tailnet_identity": {
                                "node_key": "node-key",
                                "login_name": "user@example.com",
                                "hostname": node_id,
                            }
                        }
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("send initialize");
    }

    #[tokio::test]
    async fn nodes_ping_returns_empty_result() {
        let state = approved_ws_state("device-ping", "token-ping").await;
        let app = Router::new()
            .route("/v1/nodes/ws", get(websocket_upgrade))
            .with_state(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let (mut socket, _) = connect_async(format!("ws://{addr}/v1/nodes/ws"))
            .await
            .expect("connect");

        send_initialize(&mut socket, "device-ping", "token-ping").await;
        let _init = next_text(&mut socket).await;

        socket
            .send(Message::Text(
                json!({
                    "jsonrpc": "2.0",
                    "id": 10,
                    "method": "nodes/ping",
                    "params": {}
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("send ping");
        let ping_response = next_text(&mut socket).await;
        assert!(
            ping_response.get("error").is_none(),
            "ping must not return error: {ping_response}"
        );
        assert_eq!(
            ping_response["result"],
            json!({}),
            "ping result must be empty object"
        );

        server.abort();
    }

    #[tokio::test]
    async fn nodes_ping_before_initialize_returns_auth_failed() {
        let state = approved_ws_state("device-ping2", "token-ping2").await;
        let app = Router::new()
            .route("/v1/nodes/ws", get(websocket_upgrade))
            .with_state(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let (mut socket, _) = connect_async(format!("ws://{addr}/v1/nodes/ws"))
            .await
            .expect("connect");

        // Send ping without initializing first.
        socket
            .send(Message::Text(
                json!({
                    "jsonrpc": "2.0",
                    "id": 10,
                    "method": "nodes/ping",
                    "params": {}
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("send ping pre-init");
        let response = next_text(&mut socket).await;
        assert_eq!(
            response["error"]["data"]["kind"], "auth_failed",
            "pre-init ping must return auth_failed: {response}"
        );

        server.abort();
    }

    #[tokio::test]
    async fn nodes_command_invoke_and_result_round_trip() {
        let state = approved_ws_state("device-cmd", "token-cmd").await;
        let app = Router::new()
            .route("/v1/nodes/ws", get(websocket_upgrade))
            .with_state(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let (mut socket, _) = connect_async(format!("ws://{addr}/v1/nodes/ws"))
            .await
            .expect("connect");

        send_initialize(&mut socket, "device-cmd", "token-cmd").await;
        let _init = next_text(&mut socket).await;

        // Invoke a command.
        socket
            .send(Message::Text(
                json!({
                    "jsonrpc": "2.0",
                    "id": 20,
                    "method": "nodes/command.invoke",
                    "params": {"command": "echo hello"}
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("send command.invoke");

        // Expect the RPC response (has "result" with "command_id").
        // Skip server-push frames (frames with "method" field) — in the test harness
        // the server and client share the same WS connection, so the server-push
        // nodes/command.invoke frame arrives before the RPC response.
        let invoke_response = next_text_skip_method(&mut socket).await;
        assert!(
            invoke_response.get("error").is_none(),
            "command.invoke must not error: {invoke_response}"
        );
        let command_id = invoke_response["result"]["command_id"]
            .as_str()
            .expect("command_id string")
            .to_string();
        assert!(!command_id.is_empty(), "command_id must be non-empty");

        // Send command.result back.
        socket
            .send(Message::Text(
                json!({
                    "jsonrpc": "2.0",
                    "id": 21,
                    "method": "nodes/command.result",
                    "params": {"command_id": command_id, "exit_code": 0, "success": true}
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("send command.result");
        let result_response = next_text(&mut socket).await;
        assert!(
            result_response.get("error").is_none(),
            "command.result must not error: {result_response}"
        );
        assert_eq!(
            result_response["result"]["exit_code"], 0,
            "exit_code must be 0"
        );

        server.abort();
    }

    #[tokio::test]
    async fn demux_non_allowlisted_method_returns_not_permitted() {
        let state = approved_ws_state("device-demux", "token-demux").await;
        let app = Router::new()
            .route("/v1/nodes/ws", get(websocket_upgrade))
            .with_state(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let (mut socket, _) = connect_async(format!("ws://{addr}/v1/nodes/ws"))
            .await
            .expect("connect");

        send_initialize(&mut socket, "device-demux", "token-demux").await;
        let _init = next_text(&mut socket).await;

        // Send a non-allowlisted method (not in DEMUX_ALLOWLIST).
        socket
            .send(Message::Text(
                json!({
                    "jsonrpc": "2.0",
                    "id": 30,
                    "method": "radarr.movie.list",
                    "params": {}
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("send non-allowlisted demux");
        let response = next_text(&mut socket).await;
        assert_eq!(
            response["error"]["data"]["kind"], "not_permitted",
            "non-allowlisted demux must return not_permitted: {response}"
        );

        server.abort();
    }

    /// Like `next_text` but additionally skips frames that have a `"method"` field
    /// (server-push frames). Use when the server pushes a frame to the node before
    /// returning the RPC response (e.g. `nodes/command.invoke`).
    async fn next_text_skip_method(
        socket: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> serde_json::Value {
        loop {
            match socket.next().await.expect("message").expect("ok") {
                Message::Text(text) => {
                    let v: serde_json::Value = serde_json::from_str(&text).expect("json");
                    if v.get("_lab_internal").is_some() {
                        continue;
                    }
                    // Skip server-push frames (they carry a "method" key, not "result"/"error").
                    if v.get("method").is_some() {
                        continue;
                    }
                    return v;
                }
                Message::Ping(_) | Message::Pong(_) => continue,
                other => panic!("unexpected websocket message: {other:?}"),
            }
        }
    }

    fn test_enrollment_store_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("lab-{name}-{}.json", Uuid::new_v4()))
    }

    async fn next_text(
        socket: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> serde_json::Value {
        loop {
            match socket.next().await.expect("message").expect("ok") {
                Message::Text(text) => {
                    let v: serde_json::Value = serde_json::from_str(&text).expect("json");
                    // Skip internal sweep sentinels — they are not RPC responses.
                    if v.get("_lab_internal").is_some() {
                        continue;
                    }
                    return v;
                }
                Message::Ping(_) | Message::Pong(_) => continue,
                other => panic!("unexpected websocket message: {other:?}"),
            }
        }
    }

    // Authorization invariant: `require_master_store` must read `node_role`
    // (consolidated by bead lab-yn60). A NonMaster AppState must be rejected
    // even if a node_store happens to be attached. Regression test for the
    // authorization-surface split closed by the device→node consolidation.
    #[tokio::test]
    async fn require_master_store_rejects_non_master_app_state() {
        let store = Arc::new(crate::node::store::NodeStore::default());
        let state = AppState::new()
            .with_node_store(store)
            .with_node_role(NodeRole::NonMaster);

        let result = require_master_store(&state);
        assert!(result.is_err(), "NonMaster must be rejected");
        let err = result.expect_err("err");
        assert_eq!(
            err.kind(),
            "not_found",
            "kind must be not_found, was {}",
            err.kind()
        );
        assert!(
            !state.is_master(),
            "is_master() must agree with require_master_store()"
        );
    }

    #[tokio::test]
    async fn require_master_store_allows_master_with_store() {
        let store = Arc::new(crate::node::store::NodeStore::default());
        let state = AppState::new()
            .with_node_store(Arc::clone(&store))
            .with_node_role(NodeRole::Master);
        assert!(state.is_master(), "Master role must be is_master()");
        assert!(
            require_master_store(&state).is_ok(),
            "Master with store must be allowed"
        );
    }

    #[tokio::test]
    async fn require_master_store_allows_unset_role_with_store() {
        // Legacy: callers that don't set a role default to Master (via is_master()).
        let store = Arc::new(crate::node::store::NodeStore::default());
        let state = AppState::new().with_node_store(store);
        assert!(state.is_master(), "unset role defaults to master");
        assert!(require_master_store(&state).is_ok());
    }
}
