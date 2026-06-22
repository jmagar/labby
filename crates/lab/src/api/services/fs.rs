//! HTTP route group for the `fs` workspace filesystem browser service.
//!
//! Exposes:
//! - `GET /v1/fs/list`    — directory enumeration (delegated to MCP-parallel dispatch)
//! - `GET /v1/fs/preview` — capped byte streaming from a workspace file.
//!   HTTP-only; the MCP surface refuses this action (see
//!   `crate::mcp::services::fs` for rationale).
//!
//! The `/v1` subtree already sits behind `authenticate_request` (see
//! `api/router.rs::build_router`), so no per-route auth wiring is needed.

use axum::{
    Json, Router,
    body::Body,
    extract::{Query, State},
    http::{HeaderName, HeaderValue, StatusCode, header},
    response::Response,
    routing::get,
};
use serde::Deserialize;
use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio_util::io::ReaderStream;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::api::error::ApiError;
use crate::api::state::AppState;
use crate::dispatch::error::ToolError;

/// Build the `/v1/fs` subrouter.
///
/// Applies security headers (`X-Content-Type-Options`, `X-Frame-Options`,
/// `Content-Security-Policy`) via a `SetResponseHeaderLayer` so that both
/// success (200) and error responses (404/403/422/500) produced by
/// `ToolError::into_response()` carry them. Inline header setting on the
/// `handle_preview` 200 path does NOT cover error responses — the layer does.
///
/// `Cache-Control: no-store` remains response-specific (only preview needs
/// it) and is set inline on the 200 response.
pub fn routes(_state: AppState) -> Router<AppState> {
    Router::new()
        .route("/list", get(handle_list))
        .route("/preview", get(handle_preview))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'none'; sandbox"),
        ))
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    /// Workspace-relative path. Empty or omitted = workspace root.
    pub path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PreviewQuery {
    /// Workspace-relative file path. Required.
    pub path: String,
    /// Caller-suggested byte cap; server cap of 2 MiB always wins.
    pub max_bytes: Option<u64>,
}

async fn handle_list(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>, ApiError> {
    let root = state
        .workspace_root
        .as_ref()
        .ok_or_else(crate::dispatch::fs::not_configured_error)?;

    let params = match query.path {
        Some(p) => serde_json::json!({ "path": p }),
        None => serde_json::json!({}),
    };

    let start = std::time::Instant::now();
    let result = crate::dispatch::fs::dispatch_with_root(root.as_path(), "fs.list", params).await;
    let elapsed_ms = start.elapsed().as_millis();
    match &result {
        Ok(_) => log_ok("fs.list", elapsed_ms, None, None, None),
        Err(err) => log_err("fs.list", elapsed_ms, err, None),
    }
    result.map(Json).map_err(ApiError)
}

/// `GET /v1/fs/preview` handler.
///
/// Streams file bytes with:
/// - `Content-Type` from the safe-MIME whitelist or `application/octet-stream`.
/// - `Content-Disposition: attachment; filename="…"` for non-inline MIMEs
///   (SVG, HTML, scripts, unknown).
/// - `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`,
///   `Content-Security-Policy: default-src 'none'; sandbox`.
///
/// The byte cap is `min(caller_max_bytes, 2 MiB)` — enforced via
/// `AsyncReadExt::take`, not `read_to_end` (which would allocate the whole
/// file before streaming).
async fn handle_preview(
    State(state): State<AppState>,
    Query(query): Query<PreviewQuery>,
) -> Result<Response, ApiError> {
    let root = state
        .workspace_root
        .as_ref()
        .ok_or_else(crate::dispatch::fs::not_configured_error)?;

    let mut params = serde_json::Map::new();
    params.insert("path".into(), Value::String(query.path.clone()));
    if let Some(n) = query.max_bytes {
        params.insert("max_bytes".into(), Value::Number(n.into()));
    }
    let params = Value::Object(params);

    let start = std::time::Instant::now();
    let preview_result = crate::dispatch::fs::open_for_preview(root.as_path(), params).await;
    let elapsed_ms = start.elapsed().as_millis();
    match &preview_result {
        // Success logs omit `path` to avoid enumerating the workspace
        // structure via log aggregation (symmetric with the deny-list
        // redaction in log_err). Callers correlate by request_id.
        Ok(p) => log_ok(
            "fs.preview",
            elapsed_ms,
            Some(p.content_type),
            Some(crate::dispatch::fs::client::is_inline_mime(p.content_type)),
            Some(p.max_bytes),
        ),
        Err(err) => log_err("fs.preview", elapsed_ms, err, Some(query.path.as_str())),
    }
    let preview = preview_result?;

    // `take(max_bytes)` applied at the AsyncRead layer so we never buffer
    // the full file. ReaderStream chunks it and `Body::from_stream` hands
    // that to hyper.
    let limited = preview.file.take(preview.max_bytes);
    let stream = ReaderStream::new(limited);
    let body = Body::from_stream(stream);

    let inline = crate::dispatch::fs::client::is_inline_mime(preview.content_type);
    let dtype = if inline { "inline" } else { "attachment" };
    // RFC 6266: `filename=` must be a quoted-string of ASCII chars only.
    // Substitute non-ASCII bytes with `_` for the legacy parameter and use
    // RFC 5987 `filename*` to carry the original UTF-8 name. Without this
    // substitution, HeaderValue parsing fails on any non-ASCII basename.
    let ascii_fallback: String = preview
        .disposition_filename
        .chars()
        .map(|c| if c.is_ascii() { c } else { '_' })
        .collect();
    let disposition = if ascii_fallback == preview.disposition_filename {
        format!("{dtype}; filename=\"{}\"", preview.disposition_filename)
    } else {
        let encoded = rfc5987_encode(&preview.disposition_filename);
        format!("{dtype}; filename=\"{ascii_fallback}\"; filename*=UTF-8''{encoded}")
    };

    // Security headers (nosniff, X-Frame-Options, CSP) are applied via
    // `SetResponseHeaderLayer` in `routes()` so they cover both success and
    // error responses. Only response-specific headers are set inline here.
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, preview.content_type)
        .header(header::CONTENT_DISPOSITION, disposition)
        .header(header::CACHE_CONTROL, "private, no-store")
        .body(body)
        .map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("response build failed: {e}"),
        })?;

    // Explicitly clear any upstream content-length — the stream is
    // capped but unknown-length at construction time.
    response.headers_mut().remove(header::CONTENT_LENGTH);
    Ok(response)
}

fn log_ok(
    action: &'static str,
    elapsed_ms: u128,
    mime: Option<&'static str>,
    inline: Option<bool>,
    max_bytes: Option<u64>,
) {
    tracing::info!(
        surface = "api",
        service = "fs",
        action,
        elapsed_ms,
        mime,
        inline,
        max_bytes,
        "dispatch ok"
    );
}

fn log_err(action: &'static str, elapsed_ms: u128, err: &ToolError, path: Option<&str>) {
    let kind = err.kind();
    // Omit `path` for error kinds where it is either an exfiltration oracle
    // (`not_found` aliases the deny-list — logging the probed path defeats
    // the ambiguity) or a low-value raw-input dump that may carry an attack
    // payload (`invalid_param`, `missing_param`).
    let logged_path = match kind {
        "not_found" | "invalid_param" | "missing_param" => None,
        _ => path,
    };
    if err.is_internal() {
        tracing::error!(
            surface = "api",
            service = "fs",
            action,
            elapsed_ms,
            kind,
            path = logged_path,
            "dispatch error"
        );
    } else {
        tracing::warn!(
            surface = "api",
            service = "fs",
            action,
            elapsed_ms,
            kind,
            path = logged_path,
            "dispatch error"
        );
    }
}

/// RFC 5987 attribute-char percent-encoding for `filename*=UTF-8''…`.
/// Encodes any byte that isn't `attr-char` per RFC 5987 §3.2.1.
fn rfc5987_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        let safe = b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'!' | b'#' | b'$' | b'&' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
            );
        if safe {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
