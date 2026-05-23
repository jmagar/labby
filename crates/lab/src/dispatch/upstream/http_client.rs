//! HTTP client wrapper that enforces a maximum response body size at the
//! [`StreamableHttpClient`] trait layer, BEFORE deserialization.
//!
//! Background — the gateway proxies upstream MCP servers over HTTP via
//! rmcp's `StreamableHttpClientTransport`. Without a body cap, a hostile or
//! buggy upstream can return a multi-GB response that OOMs the gateway
//! before the post-hoc size check at `pool.rs:1748,2035,2532,2616` ever
//! fires. This wrapper inserts the cap at the transport layer.
//!
//! Cap semantics:
//! - `post_message` → `Json(_, _)`: cumulative cap on the buffered body.
//! - `post_message` → `Sse(_, _)`: PER-EVENT cap (not cumulative), so
//!   long-lived legitimate SSE subscriptions are not disconnected.
//! - `get_stream`: PER-EVENT cap (not cumulative).
//! - `delete_session`: no significant body; cap not applied.
//!
//! The cap applies to DECODED bytes — reqwest auto-decodes
//! `Content-Encoding: gzip|br|zstd` by default, and `bytes_stream()` yields
//! decoded chunks. A 1 KB gzip-bomb expanding to 50 MB therefore trips the
//! cap correctly.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use futures::StreamExt;
use futures::stream::BoxStream;
use reqwest::header::{ACCEPT, HeaderName, HeaderValue, WWW_AUTHENTICATE};
use rmcp::model::{ClientJsonRpcMessage, JsonRpcMessage, ServerJsonRpcMessage};
use rmcp::transport::common::http_header::{
    EVENT_STREAM_MIME_TYPE, HEADER_LAST_EVENT_ID, HEADER_SESSION_ID, JSON_MIME_TYPE,
};
use rmcp::transport::streamable_http_client::{
    AuthRequiredError, InsufficientScopeError, SseError, StreamableHttpClient, StreamableHttpError,
    StreamableHttpPostResponse,
};
use sse_stream::{Sse, SseStream};

// rmcp 1.6 exposes the constants above but keeps `validate_custom_header` and
// `extract_scope_from_header` as `pub(crate)`. Re-implementing them locally
// (small, well-defined contracts mirrored from rmcp's source) keeps this
// wrapper compatible without forking rmcp.
const RESERVED_HEADERS: &[&str] = &[
    "accept",
    HEADER_SESSION_ID,
    "MCP-Protocol-Version", // allowed through; worker injects post-init
    HEADER_LAST_EVENT_ID,
];

fn validate_custom_header(name: &HeaderName) -> Result<(), String> {
    if RESERVED_HEADERS
        .iter()
        .any(|&r| name.as_str().eq_ignore_ascii_case(r))
    {
        if name.as_str().eq_ignore_ascii_case("MCP-Protocol-Version") {
            return Ok(());
        }
        return Err(name.to_string());
    }
    Ok(())
}

fn extract_scope_from_header(header: &str) -> Option<String> {
    let header_lowercase = header.to_ascii_lowercase();
    let scope_key = "scope=";
    if let Some(pos) = header_lowercase.find(scope_key) {
        let start = pos + scope_key.len();
        let value_slice = &header[start..];
        if let Some(stripped) = value_slice.strip_prefix('"') {
            if let Some(end_quote) = stripped.find('"') {
                return Some(stripped[..end_quote].to_string());
            }
        } else {
            let end = value_slice
                .find(|c: char| c == ',' || c == ';' || c.is_whitespace())
                .unwrap_or(value_slice.len());
            if end > 0 {
                return Some(value_slice[..end].to_string());
            }
        }
    }
    None
}

/// Wraps a [`reqwest::Client`] and enforces a per-response decoded-body
/// size cap at the [`StreamableHttpClient`] trait layer.
#[derive(Clone)]
pub struct BodyCappedHttpClient {
    inner: reqwest::Client,
    max_bytes: usize,
}

impl BodyCappedHttpClient {
    #[must_use]
    pub fn new(inner: reqwest::Client, max_bytes: usize) -> Self {
        Self { inner, max_bytes }
    }

    #[must_use]
    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }
}

/// Apply `custom_headers` after validating them. Mirrors the helper in
/// rmcp's reqwest impl since `validate_custom_header` is public.
fn apply_custom_headers(
    mut builder: reqwest::RequestBuilder,
    custom_headers: HashMap<HeaderName, HeaderValue>,
) -> Result<reqwest::RequestBuilder, StreamableHttpError<reqwest::Error>> {
    for (name, value) in custom_headers {
        validate_custom_header(&name).map_err(StreamableHttpError::ReservedHeaderConflict)?;
        builder = builder.header(name, value);
    }
    Ok(builder)
}

fn parse_json_rpc_error(body: &str) -> Option<ServerJsonRpcMessage> {
    match serde_json::from_str::<ServerJsonRpcMessage>(body) {
        Ok(message @ JsonRpcMessage::Error(_)) => Some(message),
        _ => None,
    }
}

/// Read a reqwest response body fully into a `Vec<u8>` while enforcing
/// `max_bytes`. Checks `Content-Length` first for fast rejection, then
/// counts bytes as `bytes_stream()` yields chunks. Aborts the read the
/// moment the cumulative count exceeds `max_bytes`.
///
/// Returns `StreamableHttpError::UnexpectedServerResponse` with the
/// stable `response_too_large` prefix when the cap is exceeded.
async fn read_body_capped(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, StreamableHttpError<reqwest::Error>> {
    let max_u64 = max_bytes as u64;
    // Pre-check Content-Length when present (fast reject for hostile upstreams
    // that declare oversized bodies up front).
    let declared = response.content_length();
    if let Some(cl) = declared
        && cl > max_u64
    {
        return Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
            format!("response_too_large: declared {cl} bytes, max {max_bytes}"),
        )));
    }
    // Preallocate when Content-Length is honest and under cap. Saves
    // ~log2(N) reallocs on the hot path for every legitimate response.
    let initial_cap = declared.map(|cl| cl.min(max_u64) as usize).unwrap_or(0);
    let mut buf: Vec<u8> = Vec::with_capacity(initial_cap);
    let mut stream = response.bytes_stream();
    let mut count: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(StreamableHttpError::Client)?;
        count = count.saturating_add(chunk.len() as u64);
        if count > max_u64 {
            return Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
                format!("response_too_large: streamed {count} bytes, max {max_bytes}"),
            )));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// Stream-error type for the per-event SSE body cap. `SseStream::from_byte_stream`
/// is generic over any `E: std::error::Error`, so we don't need to synthesize
/// a `reqwest::Error` — a dedicated enum that wraps reqwest errors AND our cap
/// breach is cleaner and surfaces the `response_too_large:` token via Display.
#[derive(Debug)]
pub enum CappedStreamError {
    Reqwest(reqwest::Error),
    TooLarge { event_bytes: u64, max_bytes: usize },
}

impl std::fmt::Display for CappedStreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Keep the "upstream stream error:" prefix so log lines surface
            // that the failure came from inside the body-cap wrapper and
            // not bare reqwest. `source()` still chains to the inner error
            // for `{:#}` formatters.
            Self::Reqwest(e) => write!(f, "upstream stream error: {e}"),
            Self::TooLarge {
                event_bytes,
                max_bytes,
            } => write!(
                f,
                "response_too_large: single SSE event reached {event_bytes} bytes, max {max_bytes}"
            ),
        }
    }
}

impl std::error::Error for CappedStreamError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Reqwest(e) => Some(e),
            Self::TooLarge { .. } => None,
        }
    }
}

/// Wrap an SSE byte stream so any SINGLE event exceeding `max_bytes`
/// produces a stream error. Bytes are counted per-event: the counter
/// resets to 0 immediately after each `"\n\n"` delimiter, and bytes
/// after the delimiter (within the same chunk) count toward the next
/// event. Cumulative bytes across many events are unconstrained —
/// legitimate long-lived subscriptions keep working.
///
/// Cross-chunk delimiters (chunk N ends `\n`, chunk N+1 starts `\n`)
/// are detected via the `prev_ended_with_lf` state.
fn per_event_capped_byte_stream(
    inner: impl futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Send + 'static,
    max_bytes: usize,
) -> BoxStream<'static, Result<bytes::Bytes, CappedStreamError>> {
    use bytes::Bytes;
    let max_u64 = max_bytes as u64;
    // State: (running event-byte count, did the previous chunk end with '\n')
    let stream = inner.scan((0u64, false), move |state, chunk_res| {
        let res = match chunk_res {
            Ok(chunk) => match account_event_bytes(&chunk, state.0, state.1, max_u64) {
                Ok((new_count, new_prev_lf)) => {
                    *state = (new_count, new_prev_lf);
                    Ok::<Bytes, _>(chunk)
                }
                Err(event_bytes) => {
                    *state = (0, false);
                    Err(CappedStreamError::TooLarge {
                        event_bytes,
                        max_bytes,
                    })
                }
            },
            Err(e) => Err(CappedStreamError::Reqwest(e)),
        };
        futures::future::ready(Some(res))
    });
    stream.boxed()
}

/// Account the bytes of `chunk` against the per-event counter, resetting
/// the counter at each `"\n\n"` delimiter (which may span this chunk and
/// the previous one).
///
/// On success, returns `(new_count, prev_chunk_ended_with_lf)`. On cap
/// breach, returns `Err(event_byte_count_that_exceeded)` — caller maps to
/// `CappedStreamError::TooLarge`.
///
/// Counts bytes after the final `\n\n` in this chunk toward the next event
/// (rather than discarding them as the naive "add full chunk, then reset"
/// would). Detects boundaries that span chunks (prev ends '\n', this
/// starts '\n').
fn account_event_bytes(
    chunk: &[u8],
    mut count: u64,
    prev_ended_with_lf: bool,
    max_bytes: u64,
) -> Result<(u64, bool), u64> {
    // Handle the cross-chunk boundary case first: if the previous chunk ended
    // with '\n' and this chunk begins with '\n', the byte at index 0 closes
    // the previous event. Count that one byte toward the previous event (no
    // cap re-check needed — we already approved the previous chunk), then
    // reset and scan the rest.
    let (mut idx, count_after_cross_boundary) =
        if prev_ended_with_lf && chunk.first() == Some(&b'\n') {
            (1usize, 0u64) // event closed at byte 0; skip past it, reset counter
        } else {
            (0usize, count)
        };
    count = count_after_cross_boundary;

    // Scan for intra-chunk "\n\n" delimiters. Between delimiters, accumulate
    // per-event bytes; check the cap whenever the counter advances.
    while idx < chunk.len() {
        match memchr2(&chunk[idx..], b'\n') {
            None => {
                let advance = (chunk.len() - idx) as u64;
                count = count.saturating_add(advance);
                if count > max_bytes {
                    return Err(count);
                }
                idx = chunk.len();
            }
            Some(pos) => {
                // Advance up to and including the '\n' at relative `pos`.
                let advance = (pos + 1) as u64;
                count = count.saturating_add(advance);
                if count > max_bytes {
                    return Err(count);
                }
                idx += pos + 1;
                // Look at the next byte (in this chunk) to detect "\n\n".
                if chunk.get(idx) == Some(&b'\n') {
                    count = 0; // event closed at this byte
                    idx += 1; // skip the second '\n'
                }
            }
        }
    }

    let prev_ended_with_lf = chunk.last() == Some(&b'\n');
    Ok((count, prev_ended_with_lf))
}

/// Find the first occurrence of `needle` in `haystack`. Inlined to avoid
/// a `memchr` crate dep — the haystack is per-chunk so this is bounded.
fn memchr2(haystack: &[u8], needle: u8) -> Option<usize> {
    haystack.iter().position(|b| *b == needle)
}

/// Legacy helper kept for the chunk_contains_event_boundary tests in
/// docs and review evidence. The new `account_event_bytes` function
/// supersedes it for the streaming path.
#[cfg(test)]
fn chunk_contains_event_boundary(chunk: &[u8], prev_ended_with_lf: bool) -> bool {
    if prev_ended_with_lf && chunk.first() == Some(&b'\n') {
        return true;
    }
    chunk.windows(2).any(|w| w == b"\n\n")
}

impl StreamableHttpClient for BodyCappedHttpClient {
    type Error = reqwest::Error;

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        last_event_id: Option<String>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        let mut request_builder = self
            .inner
            .get(uri.as_ref())
            .header(ACCEPT, [EVENT_STREAM_MIME_TYPE, JSON_MIME_TYPE].join(", "))
            .header(HEADER_SESSION_ID, session_id.as_ref());
        if let Some(last_event_id) = last_event_id {
            request_builder = request_builder.header(HEADER_LAST_EVENT_ID, last_event_id);
        }
        if let Some(auth_header) = auth_token {
            request_builder = request_builder.bearer_auth(auth_header);
        }
        request_builder = apply_custom_headers(request_builder, custom_headers)?;
        let response = request_builder
            .send()
            .await
            .map_err(StreamableHttpError::Client)?;
        if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            return Err(StreamableHttpError::ServerDoesNotSupportSse);
        }
        let response = response
            .error_for_status()
            .map_err(StreamableHttpError::Client)?;
        match response.headers().get(reqwest::header::CONTENT_TYPE) {
            Some(ct) => {
                if !ct.as_bytes().starts_with(EVENT_STREAM_MIME_TYPE.as_bytes())
                    && !ct.as_bytes().starts_with(JSON_MIME_TYPE.as_bytes())
                {
                    return Err(StreamableHttpError::UnexpectedContentType(Some(
                        String::from_utf8_lossy(ct.as_bytes()).to_string(),
                    )));
                }
            }
            None => {
                return Err(StreamableHttpError::UnexpectedContentType(None));
            }
        }
        let capped = per_event_capped_byte_stream(response.bytes_stream(), self.max_bytes);
        Ok(SseStream::from_byte_stream(capped).boxed())
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session: Arc<str>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        let mut request_builder = self.inner.delete(uri.as_ref());
        if let Some(auth_header) = auth_token {
            request_builder = request_builder.bearer_auth(auth_header);
        }
        request_builder = request_builder.header(HEADER_SESSION_ID, session.as_ref());
        request_builder = apply_custom_headers(request_builder, custom_headers)?;
        let response = request_builder
            .send()
            .await
            .map_err(StreamableHttpError::Client)?;
        if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            tracing::debug!("this server doesn't support deleting session");
            return Ok(());
        }
        let _response = response
            .error_for_status()
            .map_err(StreamableHttpError::Client)?;
        Ok(())
    }

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let mut request = self
            .inner
            .post(uri.as_ref())
            .header(ACCEPT, [EVENT_STREAM_MIME_TYPE, JSON_MIME_TYPE].join(", "));
        if let Some(auth_header) = auth_token {
            request = request.bearer_auth(auth_header);
        }
        request = apply_custom_headers(request, custom_headers)?;
        let session_was_attached = session_id.is_some();
        if let Some(session_id) = session_id {
            request = request.header(HEADER_SESSION_ID, session_id.as_ref());
        }
        let response = request
            .json(&message)
            .send()
            .await
            .map_err(StreamableHttpError::Client)?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            && let Some(header) = response.headers().get(WWW_AUTHENTICATE)
        {
            let header = header
                .to_str()
                .map_err(|_| {
                    StreamableHttpError::UnexpectedServerResponse(Cow::from(
                        "invalid www-authenticate header value",
                    ))
                })?
                .to_string();
            return Err(StreamableHttpError::AuthRequired(AuthRequiredError::new(
                header,
            )));
        }
        if response.status() == reqwest::StatusCode::FORBIDDEN
            && let Some(header) = response.headers().get(WWW_AUTHENTICATE)
        {
            let header_str = header.to_str().map_err(|_| {
                StreamableHttpError::UnexpectedServerResponse(Cow::from(
                    "invalid www-authenticate header value",
                ))
            })?;
            let scope = extract_scope_from_header(header_str);
            return Err(StreamableHttpError::InsufficientScope(
                InsufficientScopeError::new(header_str.to_string(), scope),
            ));
        }
        let status = response.status();
        if matches!(
            status,
            reqwest::StatusCode::ACCEPTED | reqwest::StatusCode::NO_CONTENT
        ) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if status == reqwest::StatusCode::NOT_FOUND && session_was_attached {
            return Err(StreamableHttpError::SessionExpired);
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .map(|ct| String::from_utf8_lossy(ct.as_bytes()).to_string());
        let session_id_resp = response
            .headers()
            .get(HEADER_SESSION_ID)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        // Non-success: read body with cap so a hostile error response can't OOM.
        if !status.is_success() {
            let body_bytes = read_body_capped(response, self.max_bytes).await?;
            let body = String::from_utf8_lossy(&body_bytes).to_string();
            if content_type
                .as_deref()
                .is_some_and(|ct| ct.as_bytes().starts_with(JSON_MIME_TYPE.as_bytes()))
                && let Some(message) = parse_json_rpc_error(&body)
            {
                return Ok(StreamableHttpPostResponse::Json(message, session_id_resp));
            }
            return Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
                format!("HTTP {status}: {body}"),
            )));
        }
        match content_type.as_deref() {
            Some(ct) if ct.as_bytes().starts_with(EVENT_STREAM_MIME_TYPE.as_bytes()) => {
                let capped = per_event_capped_byte_stream(response.bytes_stream(), self.max_bytes);
                Ok(StreamableHttpPostResponse::Sse(
                    SseStream::from_byte_stream(capped).boxed(),
                    session_id_resp,
                ))
            }
            Some(ct) if ct.as_bytes().starts_with(JSON_MIME_TYPE.as_bytes()) => {
                let body_bytes = read_body_capped(response, self.max_bytes).await?;
                match serde_json::from_slice::<ServerJsonRpcMessage>(&body_bytes) {
                    Ok(message) => Ok(StreamableHttpPostResponse::Json(message, session_id_resp)),
                    Err(e) => {
                        tracing::warn!(
                            "could not parse JSON response as ServerJsonRpcMessage, treating as accepted: {e}"
                        );
                        Ok(StreamableHttpPostResponse::Accepted)
                    }
                }
            }
            _ => {
                tracing::error!("unexpected content type: {:?}", content_type);
                Err(StreamableHttpError::UnexpectedContentType(content_type))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn build(max_bytes: usize) -> BodyCappedHttpClient {
        let inner = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .expect("client");
        BodyCappedHttpClient::new(inner, max_bytes)
    }

    fn jsonrpc_request() -> ClientJsonRpcMessage {
        serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#)
            .expect("valid jsonrpc")
    }

    #[tokio::test]
    async fn allows_response_under_cap() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#.as_bytes().to_vec(),
                "application/json",
            ))
            .mount(&server)
            .await;

        let client = build(10 * 1024 * 1024);
        let uri: Arc<str> = format!("{}/mcp", server.uri()).into();
        let result = client
            .post_message(uri, jsonrpc_request(), None, None, HashMap::new())
            .await;
        assert!(result.is_ok(), "small response should succeed: {result:?}");
    }

    #[tokio::test]
    async fn rejects_oversized_response_body() {
        let server = MockServer::start().await;
        let big = "x".repeat(5 * 1024 * 1024);
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!(r#"{{"jsonrpc":"2.0","id":1,"result":"{big}"}}"#).into_bytes(),
                "application/json",
            ))
            .mount(&server)
            .await;

        let client = build(1024 * 1024); // 1 MB cap
        let uri: Arc<str> = format!("{}/mcp", server.uri()).into();
        let result = client
            .post_message(uri, jsonrpc_request(), None, None, HashMap::new())
            .await;
        let err = result.expect_err("must reject oversized body");
        let s = format!("{err:?}");
        assert!(
            s.contains("response_too_large"),
            "expected response_too_large, got: {s}"
        );
    }

    #[test]
    fn capped_stream_error_display_contains_token() {
        let e = CappedStreamError::TooLarge {
            event_bytes: 12345,
            max_bytes: 1024,
        };
        let msg = format!("{e}");
        assert!(msg.contains("response_too_large"), "got: {msg}");
        assert!(msg.contains("12345"));
        assert!(msg.contains("1024"));
    }

    #[test]
    fn chunk_contains_event_boundary_intra_chunk() {
        // "\n\n" entirely within one chunk
        assert!(chunk_contains_event_boundary(b"abc\n\ndef", false));
        assert!(!chunk_contains_event_boundary(b"abc\ndef", false));
        assert!(!chunk_contains_event_boundary(b"", false));
    }

    #[test]
    fn account_event_bytes_single_event_under_cap() {
        // 6-byte event in one chunk, no delimiter inside.
        let (c, lf) = account_event_bytes(b"abcdef", 0, false, 100).unwrap();
        assert_eq!(c, 6);
        assert!(!lf);
    }

    #[test]
    fn account_event_bytes_intra_chunk_boundary_resets() {
        // First event "abc\n\n" (5 bytes accounted), then "def" starts next event.
        let (c, lf) = account_event_bytes(b"abc\n\ndef", 0, false, 100).unwrap();
        // After the "\n\n" the counter resets, then 3 bytes of next event.
        assert_eq!(c, 3, "counter must track bytes AFTER the \\n\\n");
        assert!(!lf);
    }

    #[test]
    fn account_event_bytes_cross_chunk_boundary_resets() {
        // Previous chunk ended with '\n' and we already saw 4 bytes of an
        // event; this chunk starts with '\n', closing the event. Then
        // "next_event" accumulates from scratch.
        let (c, lf) = account_event_bytes(b"\nnext", 4, true, 100).unwrap();
        // After the cross-chunk "\n\n" the counter resets, then 4 bytes
        // of "next" accumulate.
        assert_eq!(c, 4);
        assert!(!lf);
    }

    #[test]
    fn account_event_bytes_caps_oversized_event() {
        // Cap = 5 bytes. Chunk = "abcdefg" with no delimiter — should error.
        let err = account_event_bytes(b"abcdefg", 0, false, 5).unwrap_err();
        assert!(err > 5, "error must include exceeded byte count: got {err}");
    }

    #[test]
    fn account_event_bytes_no_false_positive_on_multi_event_chunk() {
        // Three small events in one chunk; cap larger than any single
        // event but smaller than total. Naive "add chunk.len() then reset"
        // would falsely flag. account_event_bytes resets per-event so the
        // chunk passes cleanly.
        let chunk = b"event1\n\nevent2\n\nevent3";
        // Cap = 10 bytes — each event is 6, total chunk is 22.
        let (c, lf) = account_event_bytes(chunk, 0, false, 10).unwrap();
        // After the trailing "event3" (no closing "\n\n"), counter = 6.
        assert_eq!(c, 6);
        assert!(!lf);
    }

    #[test]
    fn account_event_bytes_tracks_trailing_lf() {
        // Chunk ends with '\n' — next chunk must be told to look for cross
        // boundary.
        let (_c, lf) = account_event_bytes(b"abc\n", 0, false, 100).unwrap();
        assert!(lf, "must report trailing '\\n' for cross-chunk detection");
    }

    /// SSE happy path through the full pipeline: server returns an
    /// `text/event-stream` response with multiple small events under the
    /// per-event cap. `post_message` must return `Sse(stream, _)` and the
    /// stream must yield at least one event without erroring.
    ///
    /// This guards against regressions in the per_event_capped_byte_stream
    /// state machine (scan + chunk_contains_event_boundary) when refactored.
    #[tokio::test]
    async fn sse_happy_path_yields_events_under_cap() {
        use futures::StreamExt;
        use rmcp::transport::streamable_http_client::StreamableHttpPostResponse as Resp;

        let server = MockServer::start().await;
        // 3 small SSE events well under the cap.
        let body = "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":1}\n\n\
                    data: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":2}\n\n\
                    data: {\"jsonrpc\":\"2.0\",\"id\":3,\"result\":3}\n\n";
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(body.as_bytes().to_vec(), "text/event-stream"),
            )
            .mount(&server)
            .await;

        let client = build(10 * 1024 * 1024);
        let uri: Arc<str> = format!("{}/mcp", server.uri()).into();
        let result = client
            .post_message(uri, jsonrpc_request(), None, None, HashMap::new())
            .await
            .expect("sse post_message must succeed");

        let mut stream = match result {
            Resp::Sse(s, _) => s,
            other => panic!("expected Sse variant, got: {other:?}"),
        };
        let mut event_count = 0usize;
        while let Some(item) = stream.next().await {
            let _sse = item.expect("each SSE event must parse cleanly under cap");
            event_count += 1;
            if event_count >= 3 {
                break;
            }
        }
        assert!(event_count >= 1, "must yield at least one SSE event");
    }

    #[test]
    fn chunk_contains_event_boundary_cross_chunk() {
        // Previous chunk ended with '\n' and this chunk starts with '\n'.
        // Without the prev-state flag the windowed scan would miss this.
        assert!(chunk_contains_event_boundary(b"\nrest", true));
        // Prev '\n' but next chunk doesn't start with '\n': no boundary.
        assert!(!chunk_contains_event_boundary(b"rest", true));
        // No prev '\n', chunk starts with '\n' but no in-chunk "\n\n": OK.
        assert!(!chunk_contains_event_boundary(b"\nrest", false));
    }
}
