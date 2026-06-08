//! Redacted Code Mode trace helpers.
//!
//! Raw tool-call params are only available at the broker boundary. Everything
//! this module returns is safe to place in public response structs, history,
//! structured content, resources, UI state, and tests.

use serde_json::{Map, Value, json};

use super::types::{CodeModeExecutionResponse, split_code_mode_call_id};

const REDACTED: &str = "[redacted]";
const TRUNCATED_STRING: &str = "[truncated]";
const MAX_DEPTH: usize = 16;
const MAX_COLLECTION_ITEMS: usize = 64;
const MAX_STRING_CHARS: usize = 512;
const DEFAULT_PARAM_BYTES: usize = 4096;

#[must_use]
pub(in crate::dispatch::gateway::code_mode) fn redact_trace_params(
    params: &Value,
    enabled: bool,
) -> Option<Value> {
    if !enabled {
        return None;
    }
    Some(redact_trace_value(params, DEFAULT_PARAM_BYTES))
}

#[must_use]
pub(in crate::dispatch::gateway::code_mode) fn redact_trace_value(
    value: &Value,
    max_bytes: usize,
) -> Value {
    let redacted = redact_value(value, 0);
    let size = serde_json::to_vec(&redacted)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX);
    if size <= max_bytes {
        return redacted;
    }

    json!({
        "truncated": true,
        "reason": "redacted_params_exceeded_cap",
        "original_size_bytes": size,
        "max_size_bytes": max_bytes,
    })
}

#[must_use]
pub(crate) fn code_mode_search_trace(response: &Value, elapsed_ms: u128) -> Value {
    let matches = response
        .as_array()
        .map(|items| {
            items
                .iter()
                .take(50)
                .filter_map(search_match_summary)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let match_count = response.as_array().map_or(0, Vec::len);
    let displayed_count = matches.len();
    json!({
        "kind": "code_mode_search_trace",
        "query_kind": "catalog_filter",
        "elapsed_ms": elapsed_ms,
        "match_count": match_count,
        "displayed_count": displayed_count,
        "truncated": match_count > displayed_count,
        "matches": matches,
        "result_shape": compact_result_shape(response),
    })
}

fn search_match_summary(value: &Value) -> Option<Value> {
    let object = value.as_object()?;
    Some(json!({
        "id": object.get("id").and_then(Value::as_str).unwrap_or_default(),
        "upstream": object.get("upstream").and_then(Value::as_str).unwrap_or_default(),
        "tool": object.get("name").and_then(Value::as_str).unwrap_or_default(),
        "description": object
            .get("description")
            .and_then(Value::as_str)
            .map(|description| truncate_trace_string(description, 240))
            .unwrap_or_default(),
        "has_schema": object.get("schema").is_some_and(|value| !value.is_null()),
        "has_output_schema": object.get("output_schema").is_some_and(|value| !value.is_null()),
    }))
}

fn truncate_trace_string(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let prefix = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}...")
    } else {
        value.to_string()
    }
}

#[must_use]
pub(crate) fn code_mode_execute_trace(response: &CodeModeExecutionResponse) -> Value {
    let calls = response
        .calls
        .iter()
        .map(|call| {
            let (upstream, tool) = split_code_mode_call_id(&call.id);
            json!({
                "id": call.id,
                "upstream": upstream,
                "tool": tool,
                "ok": call.ok,
                "elapsed_ms": call.elapsed_ms,
                "params": call.params,
                "error_kind": call.error_kind,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "kind": "code_mode_execute_trace",
        "call_count": response.calls.len(),
        "calls": calls,
        "result_shape": response
            .result
            .as_ref()
            .map(compact_result_shape)
            .unwrap_or_else(|| json!({ "type": "undefined" })),
        "logs_count": response.logs.len(),
    })
}

#[must_use]
pub(crate) fn compact_result_shape(value: &Value) -> Value {
    let size_bytes = serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX);
    match value {
        Value::Null => json!({ "type": "null", "size_bytes": size_bytes }),
        Value::Bool(_) => json!({ "type": "boolean", "size_bytes": size_bytes }),
        Value::Number(_) => json!({ "type": "number", "size_bytes": size_bytes }),
        Value::String(s) => json!({
            "type": "string",
            "size_bytes": size_bytes,
            "length": s.chars().count(),
        }),
        Value::Array(items) => json!({
            "type": "array",
            "size_bytes": size_bytes,
            "length": items.len(),
            "item_types": compact_item_types(items),
        }),
        Value::Object(object) => {
            let mut keys = object.keys().take(16).cloned().collect::<Vec<_>>();
            keys.sort();
            json!({
                "type": "object",
                "size_bytes": size_bytes,
                "keys": keys,
                "key_count": object.len(),
                "truncated": object.get("truncated").and_then(Value::as_bool).unwrap_or(false),
                "content_block_kinds": content_block_kinds(value),
            })
        }
    }
}

fn compact_item_types(items: &[Value]) -> Vec<&'static str> {
    let mut types = items.iter().take(16).map(value_type).collect::<Vec<_>>();
    types.sort_unstable();
    types.dedup();
    types
}

fn value_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn content_block_kinds(value: &Value) -> Vec<String> {
    value
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(16)
        .filter_map(|block| {
            block
                .get("type")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .collect()
}

fn redact_value(value: &Value, depth: usize) -> Value {
    if depth >= MAX_DEPTH {
        return json!({
            "truncated": true,
            "reason": "max_depth_exceeded",
        });
    }

    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
        Value::String(s) => Value::String(redact_string(s)),
        Value::Array(items) => {
            let mut out = items
                .iter()
                .take(MAX_COLLECTION_ITEMS)
                .map(|item| redact_value(item, depth + 1))
                .collect::<Vec<_>>();
            if items.len() > MAX_COLLECTION_ITEMS {
                out.push(json!({
                    "truncated": true,
                    "reason": "array_item_limit_exceeded",
                    "omitted": items.len() - MAX_COLLECTION_ITEMS,
                }));
            }
            Value::Array(out)
        }
        Value::Object(map) => {
            let mut out = Map::new();
            for (idx, (key, child)) in map.iter().enumerate() {
                if idx >= MAX_COLLECTION_ITEMS {
                    out.insert(
                        "_truncated".to_string(),
                        json!({
                            "reason": "object_key_limit_exceeded",
                            "omitted": map.len() - MAX_COLLECTION_ITEMS,
                        }),
                    );
                    break;
                }
                if crate::dispatch::redact::is_sensitive_key(key) {
                    out.insert(key.clone(), Value::String(REDACTED.to_string()));
                } else {
                    out.insert(key.clone(), redact_value(child, depth + 1));
                }
            }
            Value::Object(out)
        }
    }
}

fn redact_string(value: &str) -> String {
    if looks_sensitive_value(value) {
        return REDACTED.to_string();
    }

    let url_redacted = redact_url_like(value);
    truncate_string(&url_redacted)
}

fn redact_url_like(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        return crate::dispatch::redact::redact_url(value);
    }
    value.to_string()
}

fn truncate_string(value: &str) -> String {
    let mut chars = value.chars();
    let prefix = chars.by_ref().take(MAX_STRING_CHARS).collect::<String>();
    if chars.next().is_some() {
        format!(
            "{prefix}{TRUNCATED_STRING} ({} chars)",
            value.chars().count()
        )
    } else {
        value.to_string()
    }
}

fn looks_sensitive_value(value: &str) -> bool {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();

    lower.starts_with("bearer ")
        || lower.starts_with("basic ")
        || lower.contains("-----begin ")
        || lower.contains("authorization:")
        || lower.contains("cookie:")
        || looks_like_jwt(trimmed)
        || looks_like_sensitive_assignment(trimmed)
        || looks_like_base64_blob(trimmed)
}

fn looks_like_sensitive_assignment(value: &str) -> bool {
    value.lines().any(|line| {
        let trimmed = line.trim();
        let Some((key, _)) = trimmed.split_once('=') else {
            return false;
        };
        crate::dispatch::redact::is_sensitive_key(key.trim_start_matches("--"))
    })
}

fn looks_like_jwt(value: &str) -> bool {
    let parts = value.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| part.len() >= 10 && part.chars().all(is_base64url_char))
}

fn looks_like_base64_blob(value: &str) -> bool {
    value.len() >= 160 && value.chars().all(is_base64ish_char)
}

fn is_base64url_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_')
}

fn is_base64ish_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '=' | '-' | '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redacts_nested_sensitive_keys_and_values() {
        let raw = json!({
            "query": "matrix",
            "nested": {
                "authorization": "Bearer secret-token",
                "items": [
                    {"api_key": "sk-secret"},
                    "https://user:pass@example.com/path?token=secret&page=2",
                    "OPENAI_API_KEY=sk-secret"
                ]
            }
        });

        let redacted = redact_trace_value(&raw, 4096);
        let serialized = redacted.to_string();

        assert_eq!(redacted["query"], json!("matrix"));
        assert_eq!(redacted["nested"]["authorization"], json!(REDACTED));
        assert_eq!(redacted["nested"]["items"][0]["api_key"], json!(REDACTED));
        assert!(
            serialized.contains("token=[redacted]"),
            "credential URL query token must be redacted: {serialized}"
        );
        assert!(!serialized.contains("secret-token"));
        assert!(!serialized.contains("sk-secret"));
        assert!(!serialized.contains("user:pass"));
    }

    #[test]
    fn redacts_sensitive_key_variants() {
        let raw = json!({
            "token": "a",
            "secret": "b",
            "authorization": "c",
            "password": "d",
            "apikey": "e",
            "api_key": "f",
            "service-key": "g",
            "cookie": "h"
        });

        let redacted = redact_trace_value(&raw, 4096);
        for key in [
            "token",
            "secret",
            "authorization",
            "password",
            "apikey",
            "api_key",
            "service-key",
            "cookie",
        ] {
            assert_eq!(redacted[key], json!(REDACTED), "{key} must be redacted");
        }
    }

    #[test]
    fn caps_long_strings_and_large_objects_deterministically() {
        let long = "x".repeat(MAX_STRING_CHARS + 100);
        let raw = json!({
            "safe": long,
            "many": (0..200).map(|i| json!({ "idx": i })).collect::<Vec<_>>()
        });

        let redacted = redact_trace_value(&raw, 512);
        let serialized = redacted.to_string();
        assert!(
            serialized.len() <= 512,
            "redacted params must respect byte cap, got {} bytes: {serialized}",
            serialized.len()
        );
        assert!(serialized.contains("redacted_params_exceeded_cap"));

        let string_capped = redact_trace_value(
            &json!({"safe": "safe words ".repeat(MAX_STRING_CHARS / 5)}),
            4096,
        );
        assert!(
            string_capped["safe"]
                .as_str()
                .expect("string")
                .contains(TRUNCATED_STRING)
        );
    }

    #[test]
    fn trace_params_can_be_disabled() {
        assert_eq!(
            redact_trace_params(&json!({"token": "secret"}), false),
            None
        );
    }
}
