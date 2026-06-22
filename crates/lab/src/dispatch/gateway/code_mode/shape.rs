use serde::Serialize;
use serde_json::Value;

use crate::config::CodeModeResultShapePolicy;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CodeModeResultShapeMetadata {
    pub policy: CodeModeResultShapePolicy,
    pub changed: bool,
    pub truncated: bool,
    pub original_size_bytes: usize,
    pub shaped_size_bytes: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShapedResult {
    pub result: Option<Value>,
    pub metadata: CodeModeResultShapeMetadata,
}

pub(in crate::dispatch::gateway::code_mode) fn shape_final_result(
    result: Option<Value>,
    policy: CodeModeResultShapePolicy,
    max_response_bytes: usize,
    max_response_tokens: usize,
    token_estimate_divisor: u32,
) -> ShapedResult {
    let original_size = result
        .as_ref()
        .and_then(|value| serde_json::to_vec(value).ok())
        .map(|bytes| bytes.len())
        .unwrap_or(0);

    match (policy, result) {
        (CodeModeResultShapePolicy::Off, result) => unchanged(result, policy, original_size),
        (CodeModeResultShapePolicy::Truncate, None) => unchanged(None, policy, original_size),
        (CodeModeResultShapePolicy::Truncate, Some(value)) => shape_truncate(
            value,
            policy,
            original_size,
            max_response_bytes,
            max_response_tokens,
            token_estimate_divisor,
        ),
    }
}

fn unchanged(
    result: Option<Value>,
    policy: CodeModeResultShapePolicy,
    original_size_bytes: usize,
) -> ShapedResult {
    ShapedResult {
        result,
        metadata: CodeModeResultShapeMetadata {
            policy,
            changed: false,
            truncated: false,
            original_size_bytes,
            shaped_size_bytes: original_size_bytes,
        },
    }
}

fn shape_truncate(
    value: Value,
    policy: CodeModeResultShapePolicy,
    original_size_bytes: usize,
    max_response_bytes: usize,
    max_response_tokens: usize,
    token_estimate_divisor: u32,
) -> ShapedResult {
    let token_budget_bytes = max_response_tokens
        .max(1)
        .saturating_mul(token_estimate_divisor.max(1) as usize);
    let budget = max_response_bytes.min(token_budget_bytes).max(256);
    if original_size_bytes <= budget {
        return unchanged(Some(value), policy, original_size_bytes);
    }

    let serialized = match &value {
        Value::String(text) => text.clone(),
        _ => serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()),
    };
    let marker_prefix =
        format!("[code mode result truncated]\noriginal_size_bytes={original_size_bytes}, max_size_bytes={budget}\n");
    let room = budget.saturating_sub(marker_prefix.len());
    let preview = serialized.chars().take(room).collect::<String>();
    let marker = format!("{marker_prefix}{preview}");
    let shaped_size_bytes = marker.len();

    ShapedResult {
        result: Some(Value::String(marker)),
        metadata: CodeModeResultShapeMetadata {
            policy,
            changed: true,
            truncated: true,
            original_size_bytes,
            shaped_size_bytes,
        },
    }
}
