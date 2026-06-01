//! Result/envelope formatting + error-info extraction + token estimation.
//!
//! Free functions extracted from `server.rs` (bead `lab-kvji.24.1.1`).
//! No behavior change — relocation + `pub(crate)` visibility only.
//!
//! `normalize_upstream_result` intentionally does NOT live here — it is
//! consolidated into `upstream.rs` (its semantic home) in bead `.5`.

use rmcp::model::{CallToolResult, Content, LoggingLevel};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::dispatch::error::ToolError as DispatchToolError;
use crate::mcp::envelope::{build_error, build_error_extra, build_success};
use crate::mcp::error::DispatchError;
use crate::mcp::error::canonical_kind;
use crate::mcp::logging::DispatchLogOutcome;

pub(crate) fn tool_error_envelope(service: &str, action: &str, err: &DispatchToolError) -> Value {
    let Ok(Value::Object(mut serialized)) = serde_json::to_value(err) else {
        return build_error(service, action, err.kind(), &err.to_string());
    };
    let kind = serialized
        .remove("kind")
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| err.kind().to_string());
    let message = serialized
        .remove("message")
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| err.to_string());
    if serialized.is_empty() {
        build_error(service, action, &kind, &message)
    } else {
        build_error_extra(service, action, &kind, &message, &Value::Object(serialized))
    }
}

pub(crate) fn hash_arguments(arguments: &Value) -> String {
    let bytes = serde_json::to_vec(arguments).unwrap_or_default();
    hex::encode(Sha256::digest(bytes))
}

/// Rough char-based token estimator for gateway telemetry logs.
///
/// Uses the conventional ~4 chars-per-token heuristic. Cheap, dependency-free,
/// and accurate enough for capacity tracking; do NOT use for LLM budget
/// enforcement — pull in `tiktoken-rs` if exact counts are required.
pub(crate) fn estimate_tokens(s: &str) -> usize {
    s.len().div_ceil(4)
}

/// Token count of a JSON value, computed against its serialized form.
#[cfg(test)]
pub(crate) fn estimate_tokens_value(value: &Value) -> usize {
    estimate_tokens(&serde_json::to_string(value).unwrap_or_default())
}

/// Token count of an MCP arguments map (`request.arguments.unwrap_or_default()`).
pub(crate) fn estimate_tokens_args(arguments: &serde_json::Map<String, Value>) -> usize {
    estimate_tokens(&serde_json::to_string(arguments).unwrap_or_default())
}

/// Format the result of a dispatch operation into an MCP `CallToolResult`.
pub(crate) fn format_dispatch_result(
    result: Result<Value, anyhow::Error>,
    service: &str,
    action: &str,
    elapsed_ms: u128,
    subject: &str,
    actor_key: Option<&str>,
) -> (CallToolResult, DispatchLogOutcome) {
    match result {
        Ok(v) => {
            tracing::info!(
                surface = "mcp",
                service,
                action,
                subject,
                actor_key,
                tool = %service,
                elapsed_ms,
                "dispatch ok"
            );
            let envelope = build_success(service, action, &v);
            (
                CallToolResult::success(vec![Content::text(envelope.to_string())]),
                DispatchLogOutcome::Success,
            )
        }
        Err(e) => {
            let (kind, message, extra) = extract_error_info(&e);
            let is_fatal = matches!(kind, "internal_error" | "server_error" | "decode_error");
            if is_fatal {
                tracing::error!(
                    surface = "mcp",
                    service,
                    action,
                    subject,
                    actor_key,
                    tool = %service,
                    elapsed_ms,
                    kind,
                    "dispatch error"
                );
            } else {
                tracing::warn!(
                    surface = "mcp",
                    service,
                    action,
                    subject,
                    actor_key,
                    tool = %service,
                    elapsed_ms,
                    kind,
                    "dispatch error"
                );
            }
            let envelope = extra.map_or_else(
                || build_error(service, action, kind, &message),
                |ref extra| build_error_extra(service, action, kind, &message, extra),
            );
            (
                CallToolResult::error(vec![Content::text(envelope.to_string())]),
                DispatchLogOutcome::Failure {
                    level: if is_fatal {
                        LoggingLevel::Error
                    } else {
                        LoggingLevel::Warning
                    },
                    kind,
                },
            )
        }
    }
}

/// Recover a stable kind tag and message from an `anyhow::Error`.
///
/// Priority:
/// 1. Downcast to [`DispatchError`] — gives structured kind + optional extras.
/// 2. Parse `e.to_string()` as JSON `{ "kind": "…" }` — covers `ToolError`
///    errors that were serialized to string before entering anyhow (radarr).
/// 3. Fall back to `"internal_error"`.
pub(crate) fn extract_error_info(e: &anyhow::Error) -> (&'static str, String, Option<Value>) {
    // 1. Structured DispatchError
    if let Some(de) = e.downcast_ref::<DispatchError>() {
        let extra = if de.valid.is_some() || de.param.is_some() || de.hint.is_some() {
            Some(serde_json::json!({
                "valid": de.valid,
                "param": de.param,
                "hint":  de.hint,
            }))
        } else {
            None
        };
        return (de.kind, de.message.clone(), extra);
    }
    // 2. ToolError serialized as JSON string (legacy radarr path)
    let msg = e.to_string();
    if let Ok(v) = serde_json::from_str::<Value>(&msg)
        && let Some(kind_str) = v.get("kind").and_then(|k| k.as_str())
    {
        let kind: &'static str = canonical_kind(kind_str);
        let message = v["message"].as_str().unwrap_or(&msg).to_string();
        // Preserve structured extras (valid list, param name, hint) if present.
        let has_valid = v.get("valid").is_some_and(|v| !v.is_null());
        let has_param = v.get("param").is_some_and(|v| !v.is_null());
        let has_hint = v.get("hint").is_some_and(|v| !v.is_null());
        let extra = if has_valid || has_param || has_hint {
            Some(serde_json::json!({
                "valid": v.get("valid"),
                "param": v.get("param"),
                "hint":  v.get("hint"),
            }))
        } else {
            None
        };
        return (kind, message, extra);
    }
    // 3. Generic fallback
    ("internal_error", msg, None)
}
