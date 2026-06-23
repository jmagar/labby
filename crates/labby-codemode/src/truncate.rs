//! Response-budget truncation for Code Mode execution responses and log caps.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use super::artifacts::CodeModeArtifactReceipt;
use super::types::CodeModeExecutionResponse;

static SECRET_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:sk-[A-Za-z0-9_-]{20,}|ghp_[A-Za-z0-9]{36}|github_pat_[A-Za-z0-9_]{82}|glpat-[A-Za-z0-9_-]{20}|xox[bp]-[A-Za-z0-9-]+|eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+)",
    )
    .expect("SECRET_REGEX is valid")
});

/// Sanitize one line of runner-captured log/output text before it is returned
/// to the caller: strips control / bidi-override characters and common
/// prompt-injection markers, redacts secret-like segments, and caps length.
///
/// Self-contained log hygiene so the kernel does not depend on the host's
/// projection helpers.
pub(crate) fn sanitize_log_text(input: &str, max_len: usize) -> String {
    let mut sanitized = input.to_string();
    sanitized.retain(|ch| {
        !matches!(
            ch,
            '\u{0000}'..='\u{001F}'
                | '\u{007F}'..='\u{009F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
        )
    });
    for marker in ["<system>", "[INST]", "###", "<<"] {
        sanitized = sanitized.replace(marker, "");
    }
    let redacted = redact_secret_like_segments(&sanitized);
    redacted.chars().take(max_len).collect()
}

fn redact_secret_like_segments(input: &str) -> String {
    let after_split = input
        .split_whitespace()
        .map(|segment| {
            let looks_secret = segment.starts_with("sk-")
                || segment.starts_with("ghp_")
                || segment.starts_with("github_pat_")
                || segment.starts_with("glpat-")
                || segment.starts_with("xoxb-")
                || segment.starts_with("xoxp-")
                || segment.starts_with("eyJ");
            if looks_secret {
                "[REDACTED]".to_string()
            } else {
                segment.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    SECRET_REGEX
        .replace_all(&after_split, "[REDACTED]")
        .into_owned()
}

pub(crate) fn truncate_execution_response(
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
        let marker = truncation_marker(result, token_estimate_divisor, &response.artifacts);
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
        let original = std::mem::take(&mut response.logs);
        let total = original.len();

        // Binary-search for the drop point: find the smallest `start` index
        // such that the response (with original[start..] as logs) is within
        // budget. This reduces the worst-case serialization work from O(n²)
        // to O(n log n) — for a 1 000-line log only ~10 budget checks instead
        // of up to 1 000.
        //
        // `start = 0` means keep all lines (we already know that's over budget).
        // `start = total` means drop everything (sentinel-only); that is the
        // fallback when even a single log line is too large.
        //
        // The predicate is monotone: once the response fits with `start` lines
        // dropped it also fits with more dropped.
        let drop_count = binary_search_drop_count(
            &original,
            &response,
            total,
            max_response_bytes,
            max_response_tokens,
            token_estimate_divisor,
        );

        let mut candidate = Vec::with_capacity(original.len() - drop_count + 1);
        if drop_count > 0 {
            candidate.push(format!(
                "[logs truncated to fit response budget — {drop_count} line(s) dropped]"
            ));
        }
        candidate.extend_from_slice(&original[drop_count..]);
        response.logs = candidate;
        debug_assert!(drop_count <= total);
    }

    response
}

/// Binary-search for the minimum number of oldest log lines to drop so that
/// the overall response fits within the byte/token budget.
///
/// Returns the drop count (0 = drop nothing, `total` = drop everything).
/// The caller is responsible for prepending a sentinel when `drop_count > 0`.
fn binary_search_drop_count(
    original: &[String],
    response: &CodeModeExecutionResponse,
    total: usize,
    max_response_bytes: usize,
    max_response_tokens: usize,
    token_estimate_divisor: u32,
) -> usize {
    // Fast path: dropping everything still over budget → return total.
    let fits_with_all_dropped = {
        let mut probe = response.clone();
        probe.logs = Vec::new();
        response_within_budget(
            &probe,
            max_response_bytes,
            max_response_tokens,
            token_estimate_divisor,
        )
    };
    if !fits_with_all_dropped {
        return total;
    }

    // Binary search: lo = 0 (known over-budget), hi = total (known fits).
    let mut lo = 0usize;
    let mut hi = total;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let mut probe = response.clone();
        probe.logs = original[mid..].to_vec();
        if response_within_budget(
            &probe,
            max_response_bytes,
            max_response_tokens,
            token_estimate_divisor,
        ) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    lo
}

pub(crate) fn response_within_budget(
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
        Err(e) => {
            tracing::warn!(
                error = %e,
                "response_within_budget: failed to serialize response; treating as over-budget"
            );
            false
        }
    }
}

fn estimated_tokens(byte_len: usize, divisor: u32) -> usize {
    byte_len.div_ceil(divisor.max(1) as usize).max(1)
}

fn truncation_marker(
    value: &Value,
    token_estimate_divisor: u32,
    artifacts: &[CodeModeArtifactReceipt],
) -> Value {
    let serialized = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
    let preview = serialized.chars().take(1024).collect::<String>();
    json!({
        "truncated": true,
        "original_size": serialized.len(),
        "original_tokens": estimated_tokens(serialized.len(), token_estimate_divisor),
        "preview": preview,
        "artifacts": artifacts,
        "next_action": "Use a narrower query, request fewer fields, or split the work across multiple codemode calls."
    })
}

/// Enforce `max_log_entries` and `max_log_bytes` caps on captured log lines.
///
/// Returns the capped list. If either cap trips, appends a single sentinel line
/// `"[log output truncated at N lines / M bytes]"` as the last entry.
pub(crate) fn apply_log_caps(
    mut logs: Vec<String>,
    max_entries: usize,
    max_bytes: usize,
) -> Vec<String> {
    let max_entries = max_entries.max(1);
    let max_bytes = max_bytes.max(1);

    let mut kept_bytes: usize = 0;
    let mut kept = 0;
    let mut truncated = false;

    for (i, line) in logs.iter().enumerate() {
        if i >= max_entries {
            truncated = true;
            break;
        }
        // Check the prospective total before counting the line so a line that
        // would push us over the cap is dropped without inflating the reported
        // byte count — the sentinel reflects only the bytes actually kept.
        if kept_bytes + line.len() > max_bytes {
            truncated = true;
            break;
        }
        kept_bytes += line.len();
        kept = i + 1;
    }

    if truncated {
        logs.truncate(kept);
        logs.push(format!(
            "[log output truncated at {kept} lines / {kept_bytes} bytes]"
        ));
    }

    logs
}
