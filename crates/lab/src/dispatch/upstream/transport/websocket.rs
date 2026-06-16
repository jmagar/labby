use std::borrow::Cow;

use futures::{SinkExt, StreamExt};
use rmcp::service::{RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::worker::{Worker, WorkerConfig, WorkerContext, WorkerQuitReason};
use rmcp::{RoleClient, transport::worker::WorkerTransport};
use tokio_tungstenite::connect_async_with_config;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::{Message, WebSocketConfig};
use tokio_tungstenite::tungstenite::{self};

const DEFAULT_MAX_MESSAGE_SIZE: usize = 10 * 1024 * 1024;
const DEFAULT_MAX_FRAME_SIZE: usize = 128 * 1024;

#[derive(Debug, Clone, thiserror::Error)]
pub enum WebSocketTransportError {
    #[error("{0}")]
    Message(String),
}

impl WebSocketTransportError {
    fn new(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}

#[derive(Debug, Clone)]
pub struct WebSocketTransportConfig {
    pub url: String,
    pub authorization: Option<String>,
    pub max_message_size: usize,
    pub max_frame_size: usize,
}

impl WebSocketTransportConfig {
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            authorization: None,
            max_message_size: DEFAULT_MAX_MESSAGE_SIZE,
            max_frame_size: DEFAULT_MAX_FRAME_SIZE,
        }
    }

    #[must_use]
    pub fn with_authorization(mut self, authorization: Option<String>) -> Self {
        self.authorization = authorization;
        self
    }
}

#[derive(Debug)]
pub struct WebSocketClientWorker {
    config: WebSocketTransportConfig,
}

impl WebSocketClientWorker {
    #[must_use]
    pub fn new(config: WebSocketTransportConfig) -> Self {
        Self { config }
    }
}

impl Worker for WebSocketClientWorker {
    type Error = WebSocketTransportError;
    type Role = RoleClient;

    fn err_closed() -> Self::Error {
        WebSocketTransportError::new("websocket transport is closed")
    }

    fn err_join(error: tokio::task::JoinError) -> Self::Error {
        WebSocketTransportError::new(format!("websocket transport task failed: {error}"))
    }

    fn config(&self) -> WorkerConfig {
        let mut config = WorkerConfig::default();
        config.name = Some("upstream-websocket-client".to_string());
        config.channel_buffer_capacity = 32;
        config
    }

    async fn run(
        self,
        mut context: WorkerContext<Self>,
    ) -> Result<(), WorkerQuitReason<Self::Error>> {
        let mut request = self
            .config
            .url
            .clone()
            .into_client_request()
            .map_err(|error| {
                WorkerQuitReason::fatal(
                    WebSocketTransportError::new(format!("invalid websocket request: {error}")),
                    "build websocket request",
                )
            })?;
        if let Some(authorization) = &self.config.authorization {
            let header =
                tungstenite::http::HeaderValue::from_str(authorization).map_err(|error| {
                    WorkerQuitReason::fatal(
                        WebSocketTransportError::new(format!(
                            "invalid websocket authorization header: {error}"
                        )),
                        "build websocket authorization header",
                    )
                })?;
            request
                .headers_mut()
                .insert(tungstenite::http::header::AUTHORIZATION, header);
        }

        let mut websocket_config = WebSocketConfig::default();
        websocket_config.max_message_size = Some(self.config.max_message_size);
        websocket_config.max_frame_size = Some(self.config.max_frame_size);
        websocket_config.accept_unmasked_frames = false;
        let (socket, _) = connect_async_with_config(request, Some(websocket_config), false)
            .await
            .map_err(|error| {
                WorkerQuitReason::fatal(
                    WebSocketTransportError::new(format!("websocket connect failed: {error}")),
                    "connect websocket upstream",
                )
            })?;
        let (mut writer, mut reader) = socket.split();
        let cancellation = context.cancellation_token.clone();

        loop {
            tokio::select! {
                _ = cancellation.cancelled() => {
                    drop(writer.send(Message::Close(None)).await);
                    return Err(WorkerQuitReason::Cancelled);
                }
                inbound = reader.next() => {
                    match inbound {
                        Some(Ok(Message::Text(text))) => {
                            let message = decode_server_message(text.as_str()).map_err(|error| {
                                WorkerQuitReason::fatal(error, "decode websocket frame")
                            })?;
                            context.send_to_handler(message).await?;
                        }
                        Some(Ok(Message::Binary(_))) => {
                            return Err(WorkerQuitReason::fatal(
                                WebSocketTransportError::new("binary websocket frames are not supported"),
                                "decode websocket frame",
                            ));
                        }
                        Some(Ok(Message::Ping(payload))) => {
                            writer.send(Message::Pong(payload)).await.map_err(|error| {
                                WorkerQuitReason::fatal(
                                    WebSocketTransportError::new(format!("websocket pong failed: {error}")),
                                    "send websocket pong",
                                )
                            })?;
                        }
                        Some(Ok(Message::Pong(_))) => {}
                        Some(Ok(Message::Frame(_))) => {}
                        Some(Ok(Message::Close(_))) | None => return Err(WorkerQuitReason::TransportClosed),
                        Some(Err(error)) => {
                            return Err(WorkerQuitReason::fatal(
                                WebSocketTransportError::new(format!("websocket receive failed: {error}")),
                                "receive websocket frame",
                            ));
                        }
                    }
                }
                outbound = context.recv_from_handler() => {
                    let outbound = outbound?;
                    let payload = encode_client_message(&outbound.message).map_err(|error| {
                        WorkerQuitReason::fatal(error, "encode websocket frame")
                    })?;
                    match writer.send(Message::Text(payload.into())).await {
                        Ok(()) => {
                            drop(outbound.responder.send(Ok(())));
                        }
                        Err(error) => {
                            let send_error = WebSocketTransportError::new(format!("websocket send failed: {error}"));
                            let cloned = send_error.clone();
                            drop(outbound.responder.send(Err(cloned)));
                            return Err(WorkerQuitReason::fatal(send_error, "send websocket frame"));
                        }
                    }
                }
            }
        }
    }
}

pub type WebSocketClientTransport = WorkerTransport<WebSocketClientWorker>;

pub fn connect(config: WebSocketTransportConfig) -> WebSocketClientTransport {
    WorkerTransport::spawn(WebSocketClientWorker::new(config))
}

pub fn parse_ws_url(raw: &str) -> Result<url::Url, WebSocketTransportError> {
    let url = url::Url::parse(raw.trim())
        .map_err(|error| WebSocketTransportError::new(format!("invalid websocket url: {error}")))?;
    match url.scheme() {
        "ws" | "wss" => Ok(url),
        scheme => Err(WebSocketTransportError::new(format!(
            "unsupported websocket url scheme: {scheme}"
        ))),
    }
}

pub fn encode_client_message(
    message: &TxJsonRpcMessage<RoleClient>,
) -> Result<String, WebSocketTransportError> {
    serde_json::to_string(message).map_err(|error| {
        WebSocketTransportError::new(format!("failed to encode json-rpc frame: {error}"))
    })
}

pub fn decode_server_message(
    payload: &str,
) -> Result<RxJsonRpcMessage<RoleClient>, WebSocketTransportError> {
    serde_json::from_str(payload).map_err(|error| {
        WebSocketTransportError::new(format!("failed to decode json-rpc frame: {error}"))
    })
}

// Re-export from net::backoff to keep existing callers within dispatch/upstream working.
pub use crate::net::backoff::{jitter_delay, reprobe_backoff};

pub fn log_context(reason: &'static str) -> Cow<'static, str> {
    Cow::Borrowed(reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::{ClientRequest, ErrorCode, ErrorData, NumberOrString, ServerResult};

    #[test]
    fn parse_ws_url_accepts_websocket_schemes() {
        assert!(parse_ws_url("ws://localhost:9000/mcp").is_ok());
        assert!(parse_ws_url("wss://example.com/socket").is_ok());
    }

    #[test]
    fn parse_ws_url_rejects_invalid_inputs() {
        assert!(parse_ws_url("localhost:9000").is_err());
        assert!(parse_ws_url("http://localhost:9000/mcp").is_err());
        assert!(parse_ws_url("ws://localhost:9000 bad").is_err());
    }

    #[test]
    fn json_rpc_frame_codec_round_trips_requests_responses_and_errors() {
        let request = TxJsonRpcMessage::<RoleClient>::request(
            ClientRequest::PingRequest(Default::default()),
            NumberOrString::Number(7),
        );
        let encoded_request = encode_client_message(&request).expect("encode request");
        let decoded_request: serde_json::Value =
            serde_json::from_str(&encoded_request).expect("decode request json");
        assert_eq!(decoded_request["jsonrpc"], "2.0");
        assert_eq!(decoded_request["id"], 7);

        let response = serde_json::to_string(&RxJsonRpcMessage::<RoleClient>::response(
            ServerResult::empty(()),
            NumberOrString::Number(9),
        ))
        .expect("encode response");
        let decoded_response = decode_server_message(&response).expect("decode response");
        assert!(matches!(
            decoded_response,
            RxJsonRpcMessage::<RoleClient>::Response(_)
        ));

        let error = serde_json::to_string(&RxJsonRpcMessage::<RoleClient>::error(
            ErrorData::new(ErrorCode::METHOD_NOT_FOUND, "method not found", None),
            Some(NumberOrString::Number(11)),
        ))
        .expect("encode error");
        let decoded_error = decode_server_message(&error).expect("decode error");
        assert!(matches!(
            decoded_error,
            RxJsonRpcMessage::<RoleClient>::Error(_)
        ));
    }
}
