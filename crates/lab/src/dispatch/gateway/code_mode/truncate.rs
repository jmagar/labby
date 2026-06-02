//! Response-budget truncation for Code Mode execution responses and log caps.

use serde_json::{Value, json};

use super::types::CodeModeExecutionResponse;

pub(in crate::dispatch::gateway::code_mode) fn truncate_execution_response(
    mut response: CodeModeExecutionResponse,
    max_response_bytes: usize,
    max_response_tokens: usize,
    token_estimate_divisor: u32,
) -> CodeModeExecutionResponse {
    if response_within_budget(
        &response,
        max_response_bytes,
        max_response_tokens,
        token_estimate_divisor,
    ) {
        return response;
    }

    // calls[] carries lightweight metadata only (no result payloads), so there
    // is nothing per-call to truncate. Cap the FINAL result first — but only
    // when doing so actually shrinks the envelope. The marker has a ~1 KB
    // preview floor, so markering an already-small result (e.g. `{"ok":true}`)
    // would *grow* it; in a logs-dominant response the result is innocent and
    // must be left intact so log trimming can do the work.
    if let Some(result) = response.result.as_ref() {
        let original_len = serde_json::to_string(result).map(|s| s.len()).unwrap_or(0);
        let marker = truncation_marker(result, token_estimate_divisor);
        let marker_len = serde_json::to_string(&marker).map(|s| s.len()).unwrap_or(0);
        if marker_len < original_len {
            response.result = Some(marker);
        }
    }

    // The result marker has a fixed ~1 KB preview floor, so a logs-dominant
    // response can still exceed budget after capping the result. Trim `logs`
    // oldest-first until within budget, keeping the newest lines that fit and
    // prepending a sentinel that records how many were dropped. Best-effort:
    // `calls[]` metadata alone can dominate a high fan-out run and is not
    // trimmed here, so the loop terminates on logs-exhaustion rather than
    // guaranteeing budget (see report — residual is a follow-up).
    if !response.logs.is_empty()
        && !response_within_budget(
            &response,
            max_response_bytes,
            max_response_tokens,
            token_estimate_divisor,
        )
    {
        let original_len = response.logs.len();
        let mut dropped = 0usize;
        // Drop oldest lines one at a time, replacing the dropped prefix with a
        // single sentinel, until within budget or all original lines are gone.
        // Terminates: each iteration removes one line; the sentinel is a short
        // fixed string, so logs collapse to at most one entry.
        loop {
            let sentinel =
                format!("[logs truncated to fit response budget — {dropped} line(s) dropped]");
            let mut candidate = Vec::with_capacity(response.logs.len() + 1);
            if dropped > 0 {
                candidate.push(sentinel);
            }
            candidate.extend(response.logs.iter().cloned());
            let mut trial = response.clone();
            trial.logs = candidate;
            if response_within_budget(
                &trial,
                max_response_bytes,
                max_response_tokens,
                token_estimate_divisor,
            ) || response.logs.is_empty()
            {
                response.logs = trial.logs;
                break;
            }
            response.logs.remove(0);
            dropped += 1;
        }
        debug_assert!(dropped <= original_len);
    }

    response
}

pub(in crate::dispatch::gateway::code_mode) fn response_within_budget(
    response: &CodeModeExecutionResponse,
    max_response_bytes: usize,
    max_response_tokens: usize,
    token_estimate_divisor: u32,
) -> bool {
    match serde_json::to_vec(response) {
        Ok(bytes) => {
            bytes.len() <= max_response_bytes
                && estimated_tokens(bytes.len(), token_estimate_divisor)
                    <= max_response_tokens.max(1)
        }
        Err(_) => false,
    }
}

fn estimated_tokens(byte_len: usize, divisor: u32) -> usize {
    byte_len.div_ceil(divisor.max(1) as usize).max(1)
}

fn truncation_marker(value: &Value, token_estimate_divisor: u32) -> Value {
    let serialized = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
    let preview = serialized.chars().take(1024).collect::<String>();
    json!({
        "truncated": true,
        "original_size": serialized.len(),
        "original_tokens": estimated_tokens(serialized.len(), token_estimate_divisor),
        "preview": preview,
        "next_action": "Use a narrower query, request fewer fields, or split the work across multiple code_execute calls."
    })
}

/// Enforce `max_log_entries` and `max_log_bytes` caps on captured log lines.
///
/// Returns the capped list. If either cap trips, appends a single sentinel line
/// `"[log output truncated at N lines / M bytes]"` as the last entry.
pub(in crate::dispatch::gateway::code_mode) fn apply_log_caps(
    mut logs: Vec<String>,
    max_entries: usize,
    max_bytes: usize,
) -> Vec<String> {
    let max_entries = max_entries.max(1);
    let max_bytes = max_bytes.max(1);

    let mut total_bytes: usize = 0;
    let mut kept = 0;
    let mut truncated = false;

    for (i, line) in logs.iter().enumerate() {
        if i >= max_entries {
            truncated = true;
            break;
        }
        total_bytes += line.len();
        if total_bytes > max_bytes {
            truncated = true;
            break;
        }
        kept = i + 1;
    }

    if truncated {
        logs.truncate(kept);
        logs.push(format!(
            "[log output truncated at {} lines / {} bytes]",
            kept,
            total_bytes.min(max_bytes),
        ));
    }

    logs
}
