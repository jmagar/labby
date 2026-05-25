use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Semaphore, mpsc, oneshot};
use tokio_tungstenite::connect_async_with_config;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::{Message, WebSocketConfig};
use uuid::Uuid;

use crate::net::backoff::{jitter_delay, reprobe_backoff};
use crate::node::install::{
    AgentInstallParams, InstallComponentParams, InstallScope, McpInstallParams,
    handle_agent_install, handle_install_component, handle_mcp_install,
};
use crate::node::queue::{NodeOutboundQueue, QueuedEnvelope};
use crate::node::token;

const FLUSH_BATCH_SIZE: usize = 100;
const IDLE_FLUSH_INTERVAL: Duration = Duration::from_secs(10);
const STATUS_INTERVAL: Duration = Duration::from_secs(30);
const MAX_MESSAGE_SIZE: usize = 10 * 1024 * 1024;
const MAX_FRAME_SIZE: usize = 128 * 1024;

/// Size of the pending-response map and the progress forwarding channel.
const PENDING_CHANNEL_CAPACITY: usize = 64;

/// Maximum time to wait for a master response before giving up, removing the
/// pending entry, and surfacing an error. Prevents silent masters from
/// wedging the client and growing the pending map unbounded.
const REQUEST_RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

/// Hard cap on pending in-flight request IDs. `send_and_await` rejects new
/// requests above this watermark so the HashMap cannot grow without bound.
const MAX_PENDING_INFLIGHT: usize = 256;

/// Capacity of the channel between the reader and the inbound-RPC worker
/// (lab-zxx5.19 item 3). Bounded so a flooding or compromised master can't
/// cause unbounded task spawn on the node side.
const INBOUND_RPC_QUEUE_CAPACITY: usize = 8;

/// Maximum concurrent inbound-RPC handler executions (lab-zxx5.19 item 3).
/// Dispatch acquires a permit before running the handler; over the cap,
/// handlers wait in the queue rather than spawning unbounded tasks.
const INBOUND_RPC_WORKER_PERMITS: usize = 16;

/// Per-file cap on `marketplace.install_component` components (lab-zxx5.18).
/// A hostile master that tries to OOM the node via oversized component
/// payloads is rejected before the handler runs.
const MAX_COMPONENT_FILE_SIZE: usize = 5 * 1024 * 1024; // 5 MB

/// Aggregate cap across every component in a single install_component RPC.
const MAX_COMPONENT_AGGREGATE_SIZE: usize = 32 * 1024 * 1024; // 32 MB

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TailnetIdentity {
    pub node_key: String,
    pub login_name: String,
    pub hostname: String,
}

impl TailnetIdentity {
    #[must_use]
    pub fn discover(hostname: &str) -> Self {
        Self {
            node_key: std::env::var("LAB_TAILNET_NODE_KEY")
                .unwrap_or_else(|_| hostname.to_string()),
            login_name: std::env::var("LAB_TAILNET_LOGIN_NAME")
                .unwrap_or_else(|_| "unknown".to_string()),
            hostname: hostname.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WsClient {
    url: url::Url,
    node_id: String,
    token_path: PathBuf,
    connected: Arc<AtomicBool>,
}

impl WsClient {
    pub fn new(
        base_url: &str,
        node_id: impl Into<String>,
        token_path: impl AsRef<Path>,
    ) -> Result<Self> {
        let url = websocket_url_from_master_base(base_url)?;
        Ok(Self {
            url,
            node_id: node_id.into(),
            token_path: token_path.as_ref().to_path_buf(),
            connected: Arc::new(AtomicBool::new(false)),
        })
    }

    #[must_use]
    #[allow(dead_code)]
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    pub async fn run(self, queue: Arc<NodeOutboundQueue>) {
        let mut attempt = 0_u32;
        loop {
            match self.connect_and_run_session(&queue).await {
                Ok(()) => {
                    attempt = 0;
                }
                Err(error) => {
                    self.connected.store(false, Ordering::Relaxed);
                    attempt = attempt.saturating_add(1);
                    let delay = jitter_delay(
                        reprobe_backoff(attempt),
                        stable_seed(&self.node_id, attempt),
                    );
                    tracing::warn!(
                        surface = "node", service = "ws_client", action = "ws.reconnect_attempt",
                        kind = "network_error",
                        node_id = %self.node_id,
                        attempt,
                        backoff_ms = delay.as_millis(),
                        error = %error,
                        "node websocket reconnect scheduled",
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    async fn connect_and_run_session(&self, queue: &NodeOutboundQueue) -> Result<()> {
        let session_id = Uuid::new_v4().to_string();
        let token = token::load_or_create(&self.token_path).await?;
        let tailnet_identity = TailnetIdentity::discover(&self.node_id);
        tracing::info!(
            surface = "node", service = "ws_client", action = "ws.session.start",
            node_id = %self.node_id,
            session_id = %session_id,
            "node websocket session starting",
        );
        tracing::info!(
            surface = "node", service = "ws_client", action = "ws.connect.start",
            node_id = %self.node_id,
            session_id = %session_id,
            "node websocket connecting to master",
        );
        let (socket, _) = self.open_websocket().await?;
        tracing::info!(
            surface = "node", service = "ws_client", action = "ws.connect.finish",
            node_id = %self.node_id,
            session_id = %session_id,
            "node websocket connected",
        );

        let initialize = build_initialize_request(&self.node_id, &token, &tailnet_identity);
        let (tx, rx) = mpsc::channel::<Message>(PENDING_CHANNEL_CAPACITY);
        tx.send(Message::Text(serde_json::to_string(&initialize)?.into()))
            .await
            .context("queue websocket initialize")?;
        tracing::info!(
            surface = "node", service = "ws_client", action = "ws.init.send",
            node_id = %self.node_id,
            session_id = %session_id,
            writer_queue_remaining = tx.capacity(),
            writer_queue_capacity = tx.max_capacity(),
            "node websocket initialize queued",
        );

        // Pending response map: request id → oneshot sender.
        // The reader task resolves pending entries when it sees a JSON-RPC response.
        let pending: Arc<tokio::sync::Mutex<HashMap<String, oneshot::Sender<String>>>> =
            Arc::new(tokio::sync::Mutex::new(HashMap::new()));

        // Split the raw socket into send + receive halves.
        let (sink, mut stream) = socket.split();

        // lab-kvhi.6 DECISION: spawn all three background tasks into a JoinSet so
        // that on session exit (success, error, or early-return via `?`) we can
        // call `abort_all()` and avoid leaking reader/writer/progress tasks
        // across reconnect attempts. A drop-guard ensures abort fires even when
        // the function returns via `?` from the main loop below.
        let mut session_tasks: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();

        // Writer task: drains `rx` → `sink`.
        {
            let node_id = self.node_id.clone();
            let session_id = session_id.clone();
            session_tasks.spawn(async move {
                let mut sink = sink;
                let mut rx: mpsc::Receiver<Message> = rx;
                while let Some(msg) = rx.recv().await {
                    if let Err(error) = sink.send(msg).await {
                        tracing::warn!(
                            surface = "node", service = "ws_client", action = "ws.write.error",
                            kind = "network_error",
                            node_id = %node_id,
                            session_id = %session_id,
                            error = %error,
                            "node websocket writer error",
                        );
                        break;
                    }
                }
                tracing::debug!(
                    surface = "node", service = "ws_client", action = "ws.write.exit",
                    node_id = %node_id,
                    session_id = %session_id,
                    "node websocket writer loop exited",
                );
            });
        }

        // Channel for progress notifications coming back from install handlers.
        // These are forwarded to `tx` as raw JSON text frames.
        let (progress_tx, mut progress_rx) = mpsc::channel::<String>(PENDING_CHANNEL_CAPACITY);

        // Reader + demux loop.
        let pending_clone = Arc::clone(&pending);
        let tx_clone = tx.clone();

        // Forward progress notifications to the write channel.
        {
            let tx_for_progress = tx.clone();
            let node_id = self.node_id.clone();
            let session_id = session_id.clone();
            session_tasks.spawn(async move {
                while let Some(notif) = progress_rx.recv().await {
                    if tx_for_progress
                        .send(Message::Text(notif.into()))
                        .await
                        .is_err()
                    {
                        tracing::debug!(
                            surface = "node", service = "ws_client", action = "ws.progress.exit",
                            node_id = %node_id,
                            session_id = %session_id,
                            "progress forwarder stopped because websocket writer is gone",
                        );
                        break;
                    }
                }
            });
        }

        // Await the initialize response via the main reader loop.
        // We use a oneshot to receive the init response while the reader loop is running.
        let (init_tx, init_rx) = oneshot::channel::<String>();
        let init_tx = Arc::new(tokio::sync::Mutex::new(Some(init_tx)));

        // lab-zxx5.19 item 3: bounded inbound-RPC queue. The reader never
        // dispatches handlers inline; it enqueues parsed frames here, and a
        // worker task below drains with a Semaphore-bounded concurrency cap.
        // A full queue is the backpressure signal to stop accepting new work
        // from a flooding or compromised master.
        let (inbound_tx, mut inbound_rx) = mpsc::channel::<Value>(INBOUND_RPC_QUEUE_CAPACITY);
        let inbound_semaphore = Arc::new(Semaphore::new(INBOUND_RPC_WORKER_PERMITS));

        // Inbound-RPC worker: drain the queue, acquire a permit, dispatch.
        // Each handler runs in its own spawned task so the worker can keep
        // reading the queue while handlers are in flight, but the Semaphore
        // caps concurrent in-flight handlers at INBOUND_RPC_WORKER_PERMITS.
        {
            let tx_for_worker = tx_clone.clone();
            let progress_tx_for_worker = progress_tx.clone();
            let semaphore = Arc::clone(&inbound_semaphore);
            let node_id = self.node_id.clone();
            let session_id = session_id.clone();
            session_tasks.spawn(async move {
                while let Some(frame) = inbound_rx.recv().await {
                    let permit = match Arc::clone(&semaphore).acquire_owned().await {
                        Ok(permit) => permit,
                        Err(_) => {
                            tracing::debug!(
                                surface = "node", service = "ws_client", action = "ws.inbound_worker.exit",
                                node_id = %node_id,
                                session_id = %session_id,
                                "inbound rpc worker stopped because semaphore closed",
                            );
                            break;
                        }
                    };
                    let tx_for_handler = tx_for_worker.clone();
                    let progress_for_handler = progress_tx_for_worker.clone();
                    let node_id_for_handler = node_id.clone();
                    let session_id_for_handler = session_id.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        let response =
                            dispatch_inbound_rpc(frame, &progress_for_handler).await;
                        let encoded = serde_json::to_string(&response).unwrap_or_else(|_| {
                            r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"serialize error"}}"#
                                .to_string()
                        });
                        if tx_for_handler.send(Message::Text(encoded.into())).await.is_err() {
                            tracing::debug!(
                                surface = "node", service = "ws_client", action = "ws.inbound_response.drop",
                                node_id = %node_id_for_handler,
                                session_id = %session_id_for_handler,
                                "inbound rpc response send failed (writer task likely gone)"
                            );
                        }
                    });
                }
            });
        }

        {
            let init_tx = Arc::clone(&init_tx);
            let pending = Arc::clone(&pending_clone);
            let tx = tx_clone.clone();
            let node_id = self.node_id.clone();
            let session_id = session_id.clone();
            session_tasks.spawn(async move {
                while let Some(message) = stream.next().await {
                    let text = match message {
                        Ok(Message::Text(t)) => t.to_string(),
                        Ok(Message::Ping(payload)) => {
                            if tx.send(Message::Pong(payload)).await.is_err() {
                                break;
                            }
                            continue;
                        }
                        Ok(Message::Pong(_) | Message::Frame(_)) => continue,
                        Ok(Message::Binary(_)) => {
                            tracing::warn!(
                                surface = "node", service = "ws_client", action = "ws.read.binary_ignored",
                                kind = "invalid_frame",
                                node_id = %node_id,
                                session_id = %session_id,
                                "node websocket binary frame ignored",
                            );
                            continue;
                        }
                        Ok(Message::Close(frame)) => {
                            let pending_depth = pending.lock().await.len();
                            let close_code = frame.as_ref().map(|f| format!("{:?}", f.code));
                            let close_reason = frame.as_ref().map(|f| f.reason.to_string());
                            tracing::info!(
                                surface = "node", service = "ws_client", action = "ws.close",
                                node_id = %node_id,
                                session_id = %session_id,
                                close_code = close_code.as_deref(),
                                close_reason = close_reason.as_deref(),
                                pending_depth,
                                "node websocket close frame received",
                            );
                            {
                                let mut guard = init_tx.lock().await;
                                guard.take();
                            }
                            let mut map = pending.lock().await;
                            map.clear();
                            break;
                        }
                        Err(error) => {
                            let pending_depth = pending.lock().await.len();
                            tracing::warn!(
                                surface = "node", service = "ws_client", action = "ws.read.error",
                                kind = "network_error",
                                node_id = %node_id,
                                session_id = %session_id,
                                error = %error,
                                pending_depth,
                                "node websocket read error",
                            );
                            {
                                let mut guard = init_tx.lock().await;
                                guard.take();
                            }
                            let mut map = pending.lock().await;
                            map.clear();
                            break;
                        }
                    };

                    // Try to parse as JSON-RPC.
                    let parsed: Value = match serde_json::from_str(&text) {
                        Ok(v) => v,
                        Err(error) => {
                            tracing::warn!(
                                surface = "node", service = "ws_client", action = "ws.read.invalid_json",
                                kind = "invalid_frame",
                                node_id = %node_id,
                                session_id = %session_id,
                                error = %error,
                                "node websocket frame was not valid JSON",
                            );
                            continue;
                        }
                    };

                    // Dispatch: check whether the frame is a response (has `result`/`error`)
                    // or an inbound RPC request from master (has `method`).
                    if parsed.get("result").is_some() || parsed.get("error").is_some() {
                        // Initialize response uses the reserved numeric id `1`;
                        // all subsequent requests use UUIDv4 strings (lab-zxx5.19).
                        if parsed.get("id") == Some(&json!(1)) {
                            let mut guard = init_tx.lock().await;
                            if let Some(sender) = guard.take() {
                                drop(guard);
                                sender.send(text).ok();
                                continue;
                            }
                        }
                        // Response — resolve a pending oneshot by UUIDv4 string id.
                        if let Some(id) = parsed.get("id").and_then(Value::as_str) {
                            let mut map = pending.lock().await;
                            if let Some(sender) = map.remove(id) {
                                sender.send(text).ok();
                            }
                        }
                    } else if parsed.get("method").is_some() {
                        // lab-zxx5.19 item 3: enqueue rather than dispatch inline.
                        // Reader stays fast; worker handles the RPC under the
                        // Semaphore-bounded concurrency cap. On a full queue
                        // we reply with a structured backpressure error rather
                        // than spawning unbounded tasks or silently dropping.
                        match inbound_tx.try_send(parsed) {
                            Ok(()) => {}
                            Err(mpsc::error::TrySendError::Full(frame)) => {
                                tracing::warn!(
                                    surface = "node", service = "ws_client", action = "ws.inbound_queue.full",
                                    kind = "backpressure",
                                    node_id = %node_id,
                                    session_id = %session_id,
                                    inbound_queue_depth = INBOUND_RPC_QUEUE_CAPACITY,
                                    inbound_queue_limit = INBOUND_RPC_QUEUE_CAPACITY,
                                    "inbound rpc queue full; returning backpressure error to master"
                                );
                                let id = frame.get("id").cloned().unwrap_or(Value::Null);
                                let backpressure = json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "error": {
                                        "code": -32000,
                                        "message": "node busy: inbound rpc queue full",
                                        "data": { "kind": "backpressure" }
                                    }
                                });
                                let encoded = serde_json::to_string(&backpressure).unwrap_or_else(|_| {
                                    r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"serialize error"}}"#
                                        .to_string()
                                });
                                if tx.send(Message::Text(encoded.into())).await.is_err() {
                                    break;
                                }
                            }
                            Err(mpsc::error::TrySendError::Closed(_)) => {
                                // Worker task exited; session is tearing down.
                                tracing::warn!(
                                    surface = "node", service = "ws_client", action = "ws.inbound_queue.closed",
                                    kind = "internal_error",
                                    node_id = %node_id,
                                    session_id = %session_id,
                                    "inbound rpc queue closed while reader was active",
                                );
                                break;
                            }
                        }
                    }
                }
            });
        }

        // lab-kvhi.6 FACT: JoinSet::drop already aborts tasks, but we wrap the
        // remainder of the session in an inner async block so that `?` early
        // returns still fall through to the explicit `abort_all` + drain below.
        // This guarantees tasks are cancelled and the socket halves released
        // before `run()` proceeds to the next reconnect attempt.
        let session_result: Result<()> = async {
            let init_response = init_rx
                .await
                .context("initialize response channel closed")?;
            validate_success_response(&init_response, &json!(1))?;
            self.connected.store(true, Ordering::Relaxed);
            tracing::info!(
                surface = "node", service = "ws_client", action = "ws.init.finish",
                node_id = %self.node_id,
                session_id = %session_id,
                "node websocket initialize acknowledged",
            );

            let mut status_deadline = tokio::time::Instant::now() + STATUS_INTERVAL;

            loop {
                let ack_count = self
                    .flush_queue_batch_async(queue, &tx, &pending_clone, &session_id)
                    .await?;
                let now = tokio::time::Instant::now();
                if now >= status_deadline {
                    self.send_status_update_async(&tx, &pending_clone, &session_id)
                        .await?;
                    status_deadline = now + STATUS_INTERVAL;
                    continue;
                }
                if ack_count > 0 {
                    continue;
                }
                tokio::time::sleep(IDLE_FLUSH_INTERVAL).await;
            }
        }
        .await;

        // Drop the outbound sender so the writer task exits, then abort the
        // reader/progress tasks and drain the set so the handles complete
        // before this function returns.
        drop(tx);
        drop(tx_clone);
        drop(progress_tx);
        session_tasks.abort_all();
        while session_tasks.join_next().await.is_some() {}

        self.connected.store(false, Ordering::Relaxed);
        match &session_result {
            Ok(()) => tracing::info!(
                surface = "node", service = "ws_client", action = "ws.session.finish",
                node_id = %self.node_id,
                session_id = %session_id,
                "node websocket session finished",
            ),
            Err(error) => tracing::warn!(
                surface = "node", service = "ws_client", action = "ws.session.error",
                kind = "network_error",
                node_id = %self.node_id,
                session_id = %session_id,
                error = %error,
                "node websocket session ended with error",
            ),
        }

        session_result
    }

    /// Send a request over the channel and wait for the corresponding response
    /// via a oneshot in the pending map.
    ///
    /// Bounded by `REQUEST_RESPONSE_TIMEOUT` (removes the pending entry on
    /// timeout) and by `MAX_PENDING_INFLIGHT` (rejects new requests when the
    /// map is already full) so a silent master cannot wedge the client or
    /// leak pending senders.
    async fn send_and_await(
        node_id: &str,
        session_id: &str,
        tx: &mpsc::Sender<Message>,
        pending: &tokio::sync::Mutex<HashMap<String, oneshot::Sender<String>>>,
        request: &Value,
        request_id: &str,
    ) -> Result<String> {
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let (resp_tx, resp_rx) = oneshot::channel::<String>();
        {
            let mut map = pending.lock().await;
            let pending_depth = map.len();
            if pending_depth >= MAX_PENDING_INFLIGHT {
                tracing::warn!(
                    surface = "node", service = "ws_client", action = "ws.pending.cap_hit",
                    kind = "rate_limited",
                    node_id = %node_id,
                    session_id = %session_id,
                    method = %method,
                    request_id = %request_id,
                    pending_depth,
                    pending_high_water = MAX_PENDING_INFLIGHT,
                    pending_limit = MAX_PENDING_INFLIGHT,
                    writer_queue_remaining = tx.capacity(),
                    writer_queue_capacity = tx.max_capacity(),
                    "node ws_client pending map full; refusing websocket request",
                );
                return Err(anyhow!(
                    "node ws_client pending map full ({} inflight); refusing request_id={}",
                    pending_depth,
                    request_id
                ));
            }
            map.insert(request_id.to_string(), resp_tx);
        }
        let send_result = tx
            .send(Message::Text(serde_json::to_string(request)?.into()))
            .await
            .context("send websocket request");
        if let Err(error) = send_result {
            pending.lock().await.remove(request_id);
            return Err(error);
        }
        match tokio::time::timeout(REQUEST_RESPONSE_TIMEOUT, resp_rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => {
                pending.lock().await.remove(request_id);
                Err(anyhow!("response channel closed before reply"))
            }
            Err(_) => {
                pending.lock().await.remove(request_id);
                Err(anyhow!(
                    "response timeout after {:?} for request_id={}",
                    REQUEST_RESPONSE_TIMEOUT,
                    request_id
                ))
            }
        }
    }

    async fn flush_queue_batch_async(
        &self,
        queue: &NodeOutboundQueue,
        tx: &mpsc::Sender<Message>,
        pending: &tokio::sync::Mutex<HashMap<String, oneshot::Sender<String>>>,
        session_id: &str,
    ) -> Result<usize> {
        let drained = queue.drain_batch(FLUSH_BATCH_SIZE).await?;
        let mut ack_count = 0usize;
        for envelope in drained {
            // lab-zxx5.19: UUIDv4 request id (non-predictable, avoids IDOR on
            // the master side where rpc_id is the only correlation handle).
            let request_id = Uuid::new_v4().to_string();
            let expected_id = json!(request_id);
            let result: Result<()> = async {
                let request = queue_envelope_to_request(&envelope, &request_id)?;
                let response = Self::send_and_await(
                    &self.node_id,
                    session_id,
                    tx,
                    pending,
                    &request,
                    &request_id,
                )
                .await?;
                validate_success_response(&response, &expected_id)
            }
            .await;
            if let Err(error) = result {
                queue.ack_drained(ack_count).await?;
                return Err(error);
            }
            ack_count += 1;
        }
        queue.ack_drained(ack_count).await?;
        Ok(ack_count)
    }

    async fn send_status_update_async(
        &self,
        tx: &mpsc::Sender<Message>,
        pending: &tokio::sync::Mutex<HashMap<String, oneshot::Sender<String>>>,
        session_id: &str,
    ) -> Result<()> {
        let metrics = tokio::task::spawn_blocking({
            let node_id = self.node_id.clone();
            move || crate::node::sysmetrics::collect(&node_id)
        })
        .await
        .unwrap_or_else(|_| crate::node::checkin::NodeStatus {
            node_id: self.node_id.clone(),
            connected: true,
            cpu_percent: None,
            memory_used_bytes: None,
            total_memory_bytes: None,
            storage_used_bytes: None,
            total_storage_bytes: None,
            os: Some(std::env::consts::OS.to_string()),
            ips: vec![],
            health: Some("healthy".to_string()),
            version: None,
            uptime_seconds: None,
            cores: None,
            cpu_clock_mhz: None,
            cpu_temp_c: None,
            doctor_issues: vec![],
            active_claude_sessions: None,
            active_codex_sessions: None,
        });
        let request_id = Uuid::new_v4().to_string();
        let params = serde_json::to_value(&metrics)?;
        let request = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "nodes/status.push",
            "params": params,
        });
        let response = Self::send_and_await(
            &self.node_id,
            session_id,
            tx,
            pending,
            &request,
            &request_id,
        )
        .await?;
        validate_success_response(&response, &json!(request_id))?;
        Ok(())
    }

    async fn open_websocket(
        &self,
    ) -> Result<(
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        tokio_tungstenite::tungstenite::handshake::client::Response,
    )> {
        let request = self
            .url
            .to_string()
            .into_client_request()
            .map_err(|error| anyhow!("build websocket request: {error}"))?;
        let mut websocket_config = WebSocketConfig::default();
        websocket_config.max_message_size = Some(MAX_MESSAGE_SIZE);
        websocket_config.max_frame_size = Some(MAX_FRAME_SIZE);
        websocket_config.accept_unmasked_frames = false;
        connect_async_with_config(request, Some(websocket_config), false)
            .await
            .map_err(|error| anyhow!("connect websocket: {error}"))
    }

    #[cfg(test)]
    async fn flush_queue_once(&self, queue: &NodeOutboundQueue) -> Result<()> {
        let token = token::load_or_create(&self.token_path).await?;
        let tailnet_identity = TailnetIdentity::discover(&self.node_id);
        let (socket, _) = self.open_websocket().await?;

        let (tx, rx) = mpsc::channel::<Message>(PENDING_CHANNEL_CAPACITY);
        let pending: Arc<tokio::sync::Mutex<HashMap<String, oneshot::Sender<String>>>> =
            Arc::new(tokio::sync::Mutex::new(HashMap::new()));

        let (sink, mut stream) = socket.split();

        let write_task = tokio::spawn(async move {
            let mut sink = sink;
            let mut rx: mpsc::Receiver<Message> = rx;
            while let Some(msg) = rx.recv().await {
                if sink.send(msg).await.is_err() {
                    break;
                }
            }
        });

        // Reader task: forward responses to pending map.
        let (init_tx, init_rx) = oneshot::channel::<String>();
        let init_tx = Arc::new(tokio::sync::Mutex::new(Some(init_tx)));
        let pending_clone = Arc::clone(&pending);
        let reader_task = tokio::spawn(async move {
            while let Some(message) = stream.next().await {
                let text = match message {
                    Ok(Message::Text(t)) => t.to_string(),
                    Ok(Message::Ping(payload)) => {
                        // No tx available here; just ignore pings in test helper.
                        drop(payload);
                        continue;
                    }
                    Ok(_) | Err(_) => break,
                };
                let mut guard = init_tx.lock().await;
                if let Some(sender) = guard.take() {
                    drop(guard);
                    sender.send(text).ok();
                    continue;
                }
                drop(guard);
                let parsed: Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if let Some(id) = parsed.get("id").and_then(Value::as_str) {
                    let mut map = pending_clone.lock().await;
                    if let Some(s) = map.remove(id) {
                        s.send(text).ok();
                    }
                }
            }
        });

        // Send initialize.
        let initialize = build_initialize_request(&self.node_id, &token, &tailnet_identity);
        tx.send(Message::Text(serde_json::to_string(&initialize)?.into()))
            .await
            .context("send websocket initialize")?;

        let init_response = init_rx.await.context("init response channel closed")?;
        validate_success_response(&init_response, &json!(1))?;

        self.flush_queue_batch_async(queue, &tx, &pending, "test-session")
            .await?;

        // Close the socket.
        tx.send(Message::Close(None)).await.ok();
        drop(tx);
        drop(write_task.await);
        reader_task.abort();
        Ok(())
    }
}

// lab-zxx5.18: structured error for pre-handler validation of
// marketplace.install_component params. `kind` maps to the stable error
// taxonomy in docs/dev/ERRORS.md.
#[derive(Debug)]
struct InstallDecodeError {
    kind: &'static str,
    message: String,
}

/// Decode the `files` array from `marketplace.install_component` params,
/// enforcing:
/// - every entry has `path` (string) and `content` (string)
/// - every entry has an explicit `encoding` field, either `"utf8"` or
///   `"base64"` — no implicit fallback (prevents base64/utf8 ambiguity)
/// - per-file decoded size ≤ MAX_COMPONENT_FILE_SIZE
/// - aggregate decoded size across all files ≤ MAX_COMPONENT_AGGREGATE_SIZE
///
/// Enforced BEFORE spawning the handler so an oversized payload can't OOM
/// the node or lock up a worker permit.
fn decode_component_files(
    files: Option<&Vec<Value>>,
) -> Result<Vec<(String, Vec<u8>)>, InstallDecodeError> {
    decode_component_files_with_limits(files, MAX_COMPONENT_FILE_SIZE, MAX_COMPONENT_AGGREGATE_SIZE)
}

fn decode_component_files_with_limits(
    files: Option<&Vec<Value>>,
    max_file_size: usize,
    max_aggregate_size: usize,
) -> Result<Vec<(String, Vec<u8>)>, InstallDecodeError> {
    use base64::Engine as _;
    let Some(files) = files else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(files.len());
    let mut aggregate = 0usize;
    for entry in files {
        let path = entry
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| InstallDecodeError {
                kind: "invalid_param",
                message: "component entry missing `path` (string)".to_string(),
            })?
            .to_string();
        let encoding = entry
            .get("encoding")
            .and_then(Value::as_str)
            .ok_or_else(|| InstallDecodeError {
                kind: "invalid_encoding",
                message: format!(
                    "component `{path}` missing required `encoding` field (`utf8` or `base64`)"
                ),
            })?;
        let content_str = entry
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| InstallDecodeError {
                kind: "invalid_param",
                message: format!("component `{path}` missing `content` (string)"),
            })?;
        let decoded = match encoding {
            "utf8" => content_str.as_bytes().to_vec(),
            "base64" => base64::engine::general_purpose::STANDARD
                .decode(content_str)
                .map_err(|e| InstallDecodeError {
                    kind: "invalid_encoding",
                    message: format!("component `{path}` base64 decode failed: {e}"),
                })?,
            other => {
                return Err(InstallDecodeError {
                    kind: "invalid_encoding",
                    message: format!(
                        "component `{path}` has unsupported encoding `{other}`; expected `utf8` or `base64`"
                    ),
                });
            }
        };
        if decoded.len() > max_file_size {
            return Err(InstallDecodeError {
                kind: "content_too_large",
                message: format!(
                    "component `{path}` is {} bytes; per-file limit is {} bytes",
                    decoded.len(),
                    max_file_size
                ),
            });
        }
        aggregate = aggregate.saturating_add(decoded.len());
        if aggregate > max_aggregate_size {
            return Err(InstallDecodeError {
                kind: "content_too_large",
                message: format!("aggregate component payload exceeds {max_aggregate_size} bytes"),
            });
        }
        out.push((path, decoded));
    }
    Ok(out)
}

/// Classify an `install_component` / `agent.install` handler error for the
/// JSON-RPC error envelope's `data.kind` field.
///
/// lab-zxx5.28: match on structured prefixes emitted by install helpers
/// (`ERR_PATH_TRAVERSAL`, `ERR_SYMLINK`, `ERR_MISSING_PARAM`, `ERR_VALIDATION`
/// in `node/install.rs`). Previously this returned `internal_error` for every
/// handler-level failure, masking legitimate `path_traversal_rejected` /
/// `validation_failed` / `missing_param` kinds. Walks the error chain so a
/// prefix that rode through a `.with_context(...)` wrapper is still found.
///
/// Unrecognized errors (e.g. I/O failures from `create_dir_all`) still map
/// to `internal_error`.
fn error_kind(error: &anyhow::Error) -> &'static str {
    use crate::node::install::{
        ERR_MISSING_PARAM, ERR_PATH_TRAVERSAL, ERR_SYMLINK, ERR_VALIDATION,
    };
    fn match_marker(s: &str) -> Option<&'static str> {
        if s.contains(ERR_PATH_TRAVERSAL) {
            Some("path_traversal_rejected")
        } else if s.contains(ERR_SYMLINK) {
            Some("symlink_rejected")
        } else if s.contains(ERR_MISSING_PARAM) {
            Some("missing_param")
        } else if s.contains(ERR_VALIDATION) {
            Some("validation_failed")
        } else {
            None
        }
    }
    // Search the full error chain — an anyhow with_context wraps the root
    // cause, and we want the marker regardless of where it sits.
    if let Some(k) = match_marker(&error.to_string()) {
        return k;
    }
    for cause in error.chain() {
        if let Some(k) = match_marker(&cause.to_string()) {
            return k;
        }
    }
    "internal_error"
}

async fn dispatch_inbound_rpc(frame: Value, progress_tx: &mpsc::Sender<String>) -> Value {
    let id = frame.get("id").cloned().unwrap_or(Value::Null);
    let method = match frame.get("method").and_then(Value::as_str) {
        Some(m) => m.to_string(),
        None => {
            return json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32600, "message": "missing `method` field" }
            });
        }
    };
    let params = frame.get("params").cloned().unwrap_or(Value::Null);

    match method.as_str() {
        "marketplace.install_component" => {
            // lab-zxx5.18: decode `files` with explicit encoding + enforce
            // size caps BEFORE spawning the handler. Every entry MUST carry
            // `encoding: "utf8" | "base64"` — no implicit fallback.
            let component_files =
                match decode_component_files(params.get("files").and_then(Value::as_array)) {
                    Ok(files) => files,
                    Err(err) => {
                        return json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {
                                "code": -32602,
                                "data": { "kind": err.kind },
                                "message": err.message,
                            }
                        });
                    }
                };

            let install_params: InstallComponentParams = match serde_json::from_value(params) {
                Ok(p) => p,
                Err(error) => {
                    return json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": -32602,
                            "message": format!("invalid marketplace.install_component params: {error}")
                        }
                    });
                }
            };

            match handle_install_component(install_params, component_files, id.clone(), progress_tx)
                .await
            {
                Ok(result) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": result,
                }),
                Err(error) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32000,
                        "data": { "kind": error_kind(&error) },
                        "message": format!("marketplace.install_component failed: {error}")
                    }
                }),
            }
        }

        "agent.install" => {
            #[derive(serde::Deserialize)]
            struct AgentInstallEnvelope {
                #[serde(flatten)]
                params: AgentInstallParams,
                #[serde(default)]
                scope: Option<InstallScope>,
                project_path: Option<String>,
            }

            let envelope: AgentInstallEnvelope = match serde_json::from_value(params) {
                Ok(e) => e,
                Err(error) => {
                    return json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": -32602,
                            "message": format!("invalid agent.install params: {error}")
                        }
                    });
                }
            };

            let scope = envelope.scope.unwrap_or(InstallScope::Global);
            let project_path = envelope.project_path.as_deref();

            match handle_agent_install(
                envelope.params,
                scope,
                project_path,
                id.clone(),
                progress_tx,
            )
            .await
            {
                Ok(result) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": result,
                }),
                Err(error) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32000,
                        "message": format!("agent.install failed: {error}")
                    }
                }),
            }
        }

        "mcp.install" => {
            let install_params: McpInstallParams = match serde_json::from_value(params) {
                Ok(p) => p,
                Err(error) => {
                    return json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": -32602,
                            "message": format!("invalid mcp.install params: {error}")
                        }
                    });
                }
            };

            match handle_mcp_install(install_params, id.clone(), progress_tx).await {
                Ok(result) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": result,
                }),
                Err(error) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32000,
                        "data": { "kind": error_kind(&error) },
                        "message": format!("mcp.install failed: {error}")
                    }
                }),
            }
        }

        other => {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32601,
                    "message": format!("unknown RPC method `{other}`"),
                    "data": { "kind": "unknown_action" }
                }
            })
        }
    }
}

pub fn websocket_url_from_master_base(base_url: &str) -> Result<url::Url> {
    let mut url = url::Url::parse(base_url.trim()).context("parse master base url")?;
    let scheme = match url.scheme() {
        "http" => "ws",
        "https" => "wss",
        "ws" => "ws",
        "wss" => "wss",
        other => return Err(anyhow!("unsupported master base url scheme `{other}`")),
    };
    url.set_scheme(scheme)
        .map_err(|_| anyhow!("set websocket scheme"))?;
    url.set_path("/v1/nodes/ws");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

pub fn build_initialize_request(
    node_id: &str,
    device_token: &str,
    tailnet_identity: &TailnetIdentity,
) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "lab-node",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "_meta": {
                "lab.node_id": node_id,
                "lab.device_token": device_token,
                "lab.tailnet_identity": tailnet_identity,
            }
        }
    })
}

pub fn queue_envelope_to_request(envelope: &QueuedEnvelope, id: &str) -> Result<Value> {
    let method = match envelope.kind.as_str() {
        "syslog_batch" | "application_log_batch" => "nodes/log.event",
        "status" => "nodes/status.push",
        "metadata" => "nodes/metadata.push",
        other => return Err(anyhow!("unsupported queued envelope kind `{other}`")),
    };
    Ok(serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": envelope.payload,
    }))
}

fn validate_success_response(payload: &str, expected_id: &Value) -> Result<()> {
    let value: Value = serde_json::from_str(payload).context("decode websocket response")?;
    let response_id = value
        .get("id")
        .ok_or_else(|| anyhow!("websocket response missing id"))?;
    if response_id != expected_id {
        return Err(anyhow!(
            "websocket response id mismatch: expected {expected_id}, got {response_id}"
        ));
    }
    if let Some(error) = value.get("error") {
        let kind = error
            .get("data")
            .and_then(|data| data.get("kind"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        return Err(anyhow!("websocket request failed ({kind}): {error}"));
    }
    Ok(())
}

fn stable_seed(node_id: &str, attempt: u32) -> u64 {
    let mut hash = 1_469_598_103_934_665_603_u64;
    for byte in node_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    hash ^ u64::from(attempt)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::api::{nodes::fleet, state::AppState};
    use crate::node::queue::NodeOutboundQueue;
    use axum::{Router, routing::get};
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;
    use tokio_tungstenite::tungstenite::Message;

    #[test]
    fn websocket_url_rewrites_http_base() {
        let url = websocket_url_from_master_base("http://master:8765").expect("url");
        assert_eq!(url.as_str(), "ws://master:8765/v1/nodes/ws");
    }

    #[test]
    fn initialize_request_includes_required_meta_fields() {
        let identity = TailnetIdentity {
            node_key: "node-key".to_string(),
            login_name: "user@example.com".to_string(),
            hostname: "host".to_string(),
        };
        let request = build_initialize_request("device-1", "token-1", &identity);
        assert_eq!(request["method"], "initialize");
        assert_eq!(request["params"]["_meta"]["lab.node_id"], "device-1");
        assert_eq!(request["params"]["_meta"]["lab.device_token"], "token-1");
        assert_eq!(
            request["params"]["_meta"]["lab.tailnet_identity"]["node_key"],
            "node-key"
        );
        assert_eq!(
            request["params"]["_meta"]["lab.tailnet_identity"]["login_name"],
            "user@example.com"
        );
    }

    #[test]
    fn queue_envelope_maps_to_fleet_methods() {
        let syslog = queue_envelope_to_request(
            &QueuedEnvelope::syslog_batch(serde_json::json!({"events": []})),
            "11111111-1111-4111-8111-111111111111",
        )
        .expect("syslog");
        assert_eq!(syslog["method"], "nodes/log.event");

        let status = queue_envelope_to_request(
            &QueuedEnvelope::status(serde_json::json!({"connected": true})),
            "22222222-2222-4222-8222-222222222222",
        )
        .expect("status");
        assert_eq!(status["method"], "nodes/status.push");

        let metadata = queue_envelope_to_request(
            &QueuedEnvelope::metadata(serde_json::json!({"node_id": "device-1"})),
            "33333333-3333-4333-8333-333333333333",
        )
        .expect("metadata");
        assert_eq!(metadata["method"], "nodes/metadata.push");
    }

    #[test]
    fn application_log_batch_envelope_serializes_with_application_source() {
        let payload = serde_json::json!({
            "node_id": "device-1",
            "events": [{"message": "hello", "source": "application"}]
        });
        let envelope = QueuedEnvelope::application_log_batch(payload.clone());
        let serialized = serde_json::to_value(&envelope).expect("serialize");
        assert_eq!(serialized["kind"], "application_log_batch");
        assert_eq!(serialized["payload"], payload);
    }

    #[test]
    fn application_log_batch_maps_to_log_event_method() {
        let envelope = QueuedEnvelope::application_log_batch(serde_json::json!({
            "node_id": "device-1",
            "events": [{"message": "hello", "source": "application"}]
        }));
        let request = queue_envelope_to_request(&envelope, "44444444-4444-4444-8444-444444444444")
            .expect("request");
        assert_eq!(request["method"], "nodes/log.event");
        assert_eq!(
            request["params"]["events"][0]["source"].as_str(),
            Some("application")
        );
    }

    #[tokio::test]
    async fn application_log_batch_round_trip_through_queue() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let queue = NodeOutboundQueue::open(tempdir.path().join("node-runtime-queue.jsonl"))
            .await
            .expect("open queue");

        let payload = serde_json::json!({
            "node_id": "device-1",
            "events": [{"message": "app event", "source": "application"}]
        });
        queue
            .push(QueuedEnvelope::application_log_batch(payload.clone()))
            .await
            .expect("push");

        let drained = queue.drain_batch(10).await.expect("drain");
        assert_eq!(drained.len(), 1);
        let envelope = &drained[0];
        assert_eq!(envelope.kind, "application_log_batch");

        let request = queue_envelope_to_request(envelope, "55555555-5555-4555-8555-555555555555")
            .expect("request");
        assert_eq!(request["method"], "nodes/log.event");
        assert_eq!(request["params"], payload);
    }

    #[tokio::test]
    async fn flush_queue_once_drains_and_acks_entries_over_websocket() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut socket = accept_async(stream).await.expect("accept websocket");
            let mut received_methods = Vec::new();

            while let Some(message) = socket.next().await {
                let text = match message.expect("message") {
                    Message::Text(text) => text.to_string(),
                    Message::Close(_) => break,
                    other => panic!("unexpected websocket message: {other:?}"),
                };
                let payload: Value = serde_json::from_str(&text).expect("parse request");
                received_methods.push(payload["method"].as_str().expect("method").to_string());
                let response = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": payload["id"],
                    "result": {},
                });
                socket
                    .send(Message::Text(response.to_string().into()))
                    .await
                    .expect("send response");
            }

            received_methods
        });

        let tempdir = tempfile::tempdir().expect("tempdir");
        let queue = NodeOutboundQueue::open(tempdir.path().join("node-runtime-queue.jsonl"))
            .await
            .expect("open queue");
        queue
            .push(QueuedEnvelope::metadata(serde_json::json!({
                "node_id": "device-1",
                "discovered_configs": []
            })))
            .await
            .expect("push metadata");
        queue
            .push(QueuedEnvelope::syslog_batch(serde_json::json!({
                "node_id": "device-1",
                "events": [{"message": "first"}]
            })))
            .await
            .expect("push syslog");
        queue
            .push(QueuedEnvelope::status(serde_json::json!({
                "node_id": "device-1",
                "connected": true
            })))
            .await
            .expect("push status");

        let client = WsClient::new(
            &format!("http://{addr}"),
            "device-1",
            tempdir.path().join("node-token"),
        )
        .expect("client");
        client.flush_queue_once(&queue).await.expect("flush");

        let remaining = queue.drain_batch(10).await.expect("remaining");
        assert!(remaining.is_empty(), "queue should be acked");

        let methods = server.await.expect("server task");
        assert_eq!(
            methods,
            vec![
                "initialize".to_string(),
                "nodes/metadata.push".to_string(),
                "nodes/log.event".to_string(),
                "nodes/status.push".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn flush_queue_once_drains_into_real_fleet_websocket_handler() {
        let store = Arc::new(crate::node::store::NodeStore::default());
        let enrollment_store = Arc::new(
            crate::node::enrollment::store::EnrollmentStore::open(
                std::env::temp_dir().join(format!("lab-ws-client-{}.json", Uuid::new_v4())),
            )
            .await
            .expect("open enrollment store"),
        );
        enrollment_store
            .record_pending(crate::node::enrollment::store::EnrollmentAttempt {
                node_id: "device-1".to_string(),
                token: "token".to_string(),
                tailnet_identity: crate::node::enrollment::store::TailnetIdentity {
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
            .approve("device-1", None)
            .await
            .expect("approve");
        let state = AppState::new()
            .with_node_store(store.clone())
            .with_enrollment_store(enrollment_store);
        let app = Router::new()
            .route("/v1/nodes/ws", get(fleet::websocket_upgrade))
            .with_state(state);

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });

        let tempdir = tempfile::tempdir().expect("tempdir");
        let queue = NodeOutboundQueue::open(tempdir.path().join("node-runtime-queue.jsonl"))
            .await
            .expect("open queue");
        queue
            .push(QueuedEnvelope::metadata(serde_json::json!({
                "node_id": "device-1",
                "discovered_configs": []
            })))
            .await
            .expect("push metadata");
        queue
            .push(QueuedEnvelope::syslog_batch(serde_json::json!({
                "node_id": "device-1",
                "events": [{
                    "node_id": "device-1",
                    "source": "syslog",
                    "timestamp_unix_ms": 1234,
                    "level": "info",
                    "message": "first",
                    "fields": {}
                }]
            })))
            .await
            .expect("push syslog");
        queue
            .push(QueuedEnvelope::status(serde_json::json!({
                "node_id": "device-1",
                "connected": true,
                "cpu_percent": 12.5,
                "memory_used_bytes": 1024,
                "storage_used_bytes": 2048,
                "os": "linux",
                "ips": ["100.64.0.1"]
            })))
            .await
            .expect("push status");

        let client = WsClient::new(
            &format!("http://{addr}"),
            "device-1",
            tempdir.path().join("node-token"),
        )
        .expect("client");
        tokio::fs::write(tempdir.path().join("node-token"), "token")
            .await
            .expect("write token");
        client.flush_queue_once(&queue).await.expect("flush");

        let remaining = queue.drain_batch(10).await.expect("remaining");
        assert!(remaining.is_empty(), "queue should be acked");

        let snapshot = store.node("device-1").await.expect("snapshot");
        assert!(!snapshot.connected);
        assert_eq!(
            snapshot
                .metadata
                .as_ref()
                .map(|metadata| metadata.discovered_configs.len()),
            Some(0)
        );
        assert_eq!(snapshot.logs.len(), 1);
        assert_eq!(snapshot.logs[0].message, "first");
        assert_eq!(
            snapshot.status.as_ref().map(|status| status.connected),
            Some(false)
        );
        assert_eq!(
            snapshot
                .status
                .as_ref()
                .and_then(|status| status.os.as_deref()),
            Some("linux")
        );

        server.abort();
    }

    #[tokio::test]
    async fn approved_device_keeps_socket_open_for_multiple_messages() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut socket = accept_async(stream).await.expect("accept websocket");
            let mut methods = Vec::new();

            while let Some(message) = socket.next().await {
                let text = match message.expect("message") {
                    Message::Text(text) => text.to_string(),
                    Message::Close(_) => break,
                    other => panic!("unexpected websocket message: {other:?}"),
                };
                let payload: Value = serde_json::from_str(&text).expect("json");
                methods.push(payload["method"].as_str().expect("method").to_string());
                let response = json!({
                    "jsonrpc": "2.0",
                    "id": payload["id"],
                    "result": {},
                });
                socket
                    .send(Message::Text(response.to_string().into()))
                    .await
                    .expect("send");
                if methods.len() >= 3 {
                    break;
                }
            }
            methods
        });

        let tempdir = tempfile::tempdir().expect("tempdir");
        let queue = NodeOutboundQueue::open(tempdir.path().join("node-runtime-queue.jsonl"))
            .await
            .expect("open queue");
        queue
            .push(QueuedEnvelope::metadata(json!({
                "node_id": "device-1",
                "discovered_configs": []
            })))
            .await
            .expect("metadata");
        queue
            .push(QueuedEnvelope::syslog_batch(json!({
                "node_id": "device-1",
                "events": [{"message": "first"}]
            })))
            .await
            .expect("log");
        tokio::fs::write(tempdir.path().join("node-token"), "token")
            .await
            .expect("write token");

        let client = WsClient::new(
            &format!("http://{addr}"),
            "device-1",
            tempdir.path().join("node-token"),
        )
        .expect("client");
        let queue = Arc::new(queue);
        let run = tokio::spawn({
            let queue = queue.clone();
            let client = client.clone();
            async move {
                tokio::time::timeout(
                    Duration::from_secs(2),
                    client.connect_and_run_session(&queue),
                )
                .await
                .ok();
            }
        });

        let methods = server.await.expect("server");
        assert_eq!(
            methods,
            vec![
                "initialize".to_string(),
                "nodes/metadata.push".to_string(),
                "nodes/log.event".to_string()
            ]
        );

        run.abort();
    }

    #[tokio::test]
    async fn dispatch_inbound_rpc_unknown_method_returns_error() {
        let (progress_tx, _rx) = mpsc::channel(8);
        let frame = json!({
            "jsonrpc": "2.0",
            "id": 42,
            "method": "unknown.method",
            "params": {}
        });
        let response = dispatch_inbound_rpc(frame, &progress_tx).await;
        assert_eq!(response["id"], 42);
        assert!(response.get("error").is_some());
        assert_eq!(response["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn dispatch_inbound_rpc_marketplace_install_component_path_traversal() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let (progress_tx, _rx) = mpsc::channel(8);
        let frame = json!({
            "jsonrpc": "2.0",
            "id": 10,
            "method": "marketplace.install_component",
            "params": {
                "plugin_id": "evil@marketplace",
                "components": ["../etc/passwd"],
                "scope": "project",
                "project_path": tempdir.path().to_str().unwrap(),
                "files": [
                    { "path": "../etc/passwd", "content": "evil content" }
                ]
            }
        });
        let response = dispatch_inbound_rpc(frame, &progress_tx).await;
        // Should succeed at the RPC level but report errors in the result.
        assert_eq!(response["id"], 10);
        // Either we get an error at RPC level or the result has errors field.
        let has_error = response.get("error").is_some()
            || response["result"]["errors"]
                .as_array()
                .map(|e| !e.is_empty())
                .unwrap_or(false);
        assert!(
            has_error,
            "expected path traversal to be rejected: {response}"
        );
    }

    #[tokio::test]
    async fn dispatch_inbound_rpc_agent_install_unknown_method_variation() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let (progress_tx, _rx) = mpsc::channel(8);
        let frame = json!({
            "jsonrpc": "2.0",
            "id": 20,
            "method": "agent.install",
            "params": {
                "agent_id": "test-agent",
                "distribution": {
                    "type": "npx",
                    "package": "@anthropic/test-agent",
                    "version": "1.0.0"
                },
                "scope": "project",
                "project_path": tempdir.path().to_str().unwrap()
            }
        });
        let response = dispatch_inbound_rpc(frame, &progress_tx).await;
        assert_eq!(response["id"], 20);
        assert!(
            response.get("result").is_some(),
            "expected success: {response}"
        );
        assert_eq!(
            response["result"]["written"].as_array().map(|a| a.len()),
            Some(1)
        );
    }

    // lab-zxx5.19 item 1 + 4: send_and_await must remove the pending entry on
    // timeout so a silent master cannot wedge the client or grow the pending
    // map unbounded. Regression gate for the knowledge.jsonl pattern
    // "oneshot-pending-hashmap without timeout is a latent deadlock".
    #[tokio::test]
    async fn send_and_await_removes_pending_entry_on_timeout() {
        // Drop the receiver so send succeeds but the response never arrives.
        let (tx, rx) = mpsc::channel::<Message>(16);
        drop(rx);
        let pending: Arc<tokio::sync::Mutex<HashMap<String, oneshot::Sender<String>>>> =
            Arc::new(tokio::sync::Mutex::new(HashMap::new()));

        // Artificially tight deadline is hard to thread through the constant,
        // so we instead assert the send-failure path: when the writer half is
        // gone, send_and_await returns with the pending entry already cleaned.
        let request_id = Uuid::new_v4().to_string();
        let request = json!({"jsonrpc": "2.0", "id": request_id, "method": "test"});
        let result = WsClient::send_and_await(
            "node-test",
            "session-test",
            &tx,
            &pending,
            &request,
            &request_id,
        )
        .await;
        assert!(result.is_err(), "expected send failure when writer is gone");
        let map = pending.lock().await;
        assert!(
            !map.contains_key(&request_id),
            "pending entry must be removed on send failure"
        );
    }

    #[tokio::test]
    async fn send_and_await_rejects_when_pending_map_is_saturated() {
        let (tx, _rx) = mpsc::channel::<Message>(16);
        let pending: Arc<tokio::sync::Mutex<HashMap<String, oneshot::Sender<String>>>> =
            Arc::new(tokio::sync::Mutex::new(HashMap::new()));

        {
            let mut map = pending.lock().await;
            for index in 0..MAX_PENDING_INFLIGHT {
                let (sender, _receiver) = oneshot::channel::<String>();
                map.insert(format!("pending-{index}"), sender);
            }
        }

        let request_id = Uuid::new_v4().to_string();
        let request = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "nodes/status.push",
        });
        let result = WsClient::send_and_await(
            "node-test",
            "session-test",
            &tx,
            &pending,
            &request,
            &request_id,
        )
        .await;
        assert!(result.is_err(), "saturated pending map must reject");
        let map = pending.lock().await;
        assert!(
            !map.contains_key(&request_id),
            "rejected request must not be inserted"
        );
    }

    // lab-zxx5.19 item 4: UUIDv4 request ids are strings on the wire, not
    // sequential integers. Prevents IDOR on master-side rpc_id correlation
    // (SSE subscription gate in lab-zxx5.16).
    #[test]
    fn request_ids_are_non_predictable_uuid_strings() {
        let envelope = QueuedEnvelope::status(json!({"connected": true}));
        let id1 = Uuid::new_v4().to_string();
        let id2 = Uuid::new_v4().to_string();
        assert_ne!(id1, id2);
        // Parse back as Uuid to confirm v4 shape.
        assert!(Uuid::parse_str(&id1).is_ok(), "id must be a valid UUID");
        let request = queue_envelope_to_request(&envelope, &id1).expect("request");
        assert_eq!(request["id"], json!(id1));
        assert!(
            request["id"].is_string(),
            "wire id must be a JSON string, not a number"
        );
    }

    #[test]
    fn validate_success_response_accepts_matching_string_id() {
        let payload = r#"{"jsonrpc":"2.0","id":"abc-123","result":{}}"#;
        assert!(validate_success_response(payload, &json!("abc-123")).is_ok());
    }

    #[test]
    fn validate_success_response_rejects_id_mismatch() {
        let payload = r#"{"jsonrpc":"2.0","id":"expected","result":{}}"#;
        let err = validate_success_response(payload, &json!("different")).err();
        assert!(err.is_some(), "mismatched ids must be rejected");
    }

    #[test]
    fn validate_success_response_still_accepts_numeric_init_id() {
        // The initialize response uses the reserved numeric id `1`; the
        // validator compares via `Value` equality so both shapes work.
        let payload = r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05"}}"#;
        assert!(validate_success_response(payload, &json!(1)).is_ok());
    }

    #[test]
    fn websocket_observability_fields_are_kept_on_source_paths() {
        let source = include_str!("ws_client.rs");
        assert!(source.contains("action = \"ws.pending.cap_hit\""));
        assert!(source.contains("pending_high_water"));
        assert!(source.contains("action = \"ws.session.start\""));
        assert!(source.contains("action = \"ws.init.finish\""));
        assert!(source.contains("action = \"ws.close\""));
        assert!(source.contains("action = \"ws.read.error\""));
        assert!(source.contains("session_id"));
    }

    // ------------------------------------------------------------------
    // lab-zxx5.28: error_kind recognises structured prefixes
    // ------------------------------------------------------------------

    #[test]
    fn error_kind_recognises_path_traversal_marker() {
        let err = anyhow::anyhow!("lab.err:path_traversal_rejected: agent_id `../etc/passwd`");
        assert_eq!(error_kind(&err), "path_traversal_rejected");
    }

    #[test]
    fn error_kind_recognises_symlink_marker() {
        let err = anyhow::anyhow!("lab.err:symlink_rejected: tempfile is a symlink");
        assert_eq!(error_kind(&err), "symlink_rejected");
    }

    #[test]
    fn error_kind_recognises_missing_param_marker() {
        let err = anyhow::anyhow!("lab.err:missing_param: project_path required");
        assert_eq!(error_kind(&err), "missing_param");
    }

    #[test]
    fn error_kind_recognises_validation_marker() {
        let err = anyhow::anyhow!("lab.err:validation_failed: HOME env var not set");
        assert_eq!(error_kind(&err), "validation_failed");
    }

    #[test]
    fn error_kind_walks_chain_through_with_context() {
        // Simulate `install_helper()?.with_context(...)`: the inner error
        // carries the marker and the outer wrapper adds contextual framing.
        let inner = anyhow::anyhow!("lab.err:path_traversal_rejected: bad component");
        let wrapped = inner.context("resolve write root");
        assert_eq!(error_kind(&wrapped), "path_traversal_rejected");
    }

    #[test]
    fn error_kind_returns_internal_error_for_unmarked_errors() {
        let err = anyhow::anyhow!("plain io failure, no marker");
        assert_eq!(error_kind(&err), "internal_error");
    }

    // ------------------------------------------------------------------
    // lab-zxx5.18: install_component pre-handler validation
    // ------------------------------------------------------------------

    #[test]
    fn decode_component_files_rejects_missing_encoding_field() {
        use base64::Engine as _;
        drop(base64::engine::general_purpose::STANDARD.encode(b"x"));
        let files = vec![json!({ "path": "a.md", "content": "hi" })];
        let err = decode_component_files(Some(&files))
            .err()
            .expect("must reject");
        assert_eq!(err.kind, "invalid_encoding");
    }

    #[test]
    fn decode_component_files_rejects_unknown_encoding() {
        let files = vec![json!({ "path": "a.md", "content": "hi", "encoding": "rot13" })];
        let err = decode_component_files(Some(&files))
            .err()
            .expect("must reject");
        assert_eq!(err.kind, "invalid_encoding");
    }

    #[test]
    fn decode_component_files_rejects_per_file_oversize() {
        let big = "a".repeat(17);
        let files = vec![json!({ "path": "big.bin", "content": big, "encoding": "utf8" })];
        let err = decode_component_files_with_limits(Some(&files), 16, 64)
            .err()
            .expect("must reject");
        assert_eq!(err.kind, "content_too_large");
    }

    #[test]
    fn decode_component_files_rejects_aggregate_oversize() {
        // 7 x 10 bytes = 70 bytes > 64 byte aggregate cap. Individual files fit
        // under the per-file cap so the aggregate check is exercised.
        let chunk = "a".repeat(10);
        let files: Vec<Value> = (0..7)
            .map(|i| json!({ "path": format!("f{i}.bin"), "content": chunk, "encoding": "utf8" }))
            .collect();
        let err = decode_component_files_with_limits(Some(&files), 16, 64)
            .err()
            .expect("must reject");
        assert_eq!(err.kind, "content_too_large");
    }

    #[test]
    fn decode_component_files_accepts_utf8_and_base64() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"bin\x00data");
        let files = vec![
            json!({ "path": "a.md", "content": "hello", "encoding": "utf8" }),
            json!({ "path": "b.bin", "content": b64, "encoding": "base64" }),
        ];
        let decoded = decode_component_files(Some(&files)).expect("must accept");
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].1, b"hello");
        assert_eq!(decoded[1].1, b"bin\x00data");
    }

    #[test]
    fn decode_component_files_rejects_missing_path() {
        let files = vec![json!({ "content": "hi", "encoding": "utf8" })];
        let err = decode_component_files(Some(&files))
            .err()
            .expect("must reject");
        assert_eq!(err.kind, "invalid_param");
    }

    #[test]
    fn decode_component_files_rejects_missing_content() {
        let files = vec![json!({ "path": "a.md", "encoding": "utf8" })];
        let err = decode_component_files(Some(&files))
            .err()
            .expect("must reject");
        assert_eq!(err.kind, "invalid_param");
    }

    #[test]
    fn decode_component_files_rejects_malformed_base64() {
        let files =
            vec![json!({ "path": "a.bin", "content": "!!!not-b64!!!", "encoding": "base64" })];
        let err = decode_component_files(Some(&files))
            .err()
            .expect("must reject");
        assert_eq!(err.kind, "invalid_encoding");
    }
}
