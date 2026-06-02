//! Tests: response-budget truncation, wasm runner smoke, token estimate.
#![cfg(test)]

use serde_json::json;

use super::*;

#[test]
fn truncates_code_execute_final_result_when_oversized() {
    // calls[] carry lightweight metadata only — truncation caps the FINAL
    // result. An oversized final result is replaced with a truncation marker;
    // the calls metadata is preserved untouched.
    let response = CodeModeExecutionResponse {
        result: Some(json!({"payload": "x".repeat(5000)})),
        calls: vec![
            CodeModeExecutedCall {
                id: "upstream::github::search_issues".to_string(),
                ok: true,
                elapsed_ms: 12,
                error_kind: None,
            },
            CodeModeExecutedCall {
                id: "upstream::github::list_issues".to_string(),
                ok: false,
                elapsed_ms: 7,
                error_kind: Some("rate_limited".to_string()),
            },
        ],
        logs: Vec::new(),
    };

    let truncated = truncate_execution_response(response, 1400, 6000, 4);

    // Final result replaced with truncation marker.
    let result = truncated.result.as_ref().expect("result present");
    assert_eq!(result["truncated"], json!(true));
    assert!(result["original_size"].as_u64().unwrap() > 5000);
    assert!(result["next_action"].as_str().unwrap().contains("narrower"));
    // Calls metadata preserved unchanged (no result payloads to truncate).
    assert_eq!(truncated.calls.len(), 2);
    assert!(truncated.calls[0].ok);
    assert_eq!(
        truncated.calls[1].error_kind.as_deref(),
        Some("rate_limited")
    );
    // The marker replaces the multi-KB payload with a bounded preview, so the
    // serialized response is far smaller than the original (~5 KB) result.
    assert!(serde_json::to_vec(&truncated).unwrap().len() < 5000);
}

#[test]
fn does_not_truncate_when_final_result_within_budget() {
    let response = CodeModeExecutionResponse {
        result: Some(json!({"items": ["small"]})),
        calls: vec![CodeModeExecutedCall {
            id: "upstream::github::search_issues".to_string(),
            ok: true,
            elapsed_ms: 3,
            error_kind: None,
        }],
        logs: Vec::new(),
    };

    let out = truncate_execution_response(response, 1400, 6000, 4);
    assert_eq!(out.result, Some(json!({"items": ["small"]})));
}

#[test]
fn truncates_oversized_logs_after_result() {
    // Logs-dominant response: small result, small calls[], but many large log
    // lines push the envelope over budget. After capping the (small) result,
    // logs must be trimmed until within budget, leaving a sentinel.
    let response = CodeModeExecutionResponse {
        result: Some(json!({"ok": true})),
        calls: vec![CodeModeExecutedCall {
            id: "upstream::test::ping".to_string(),
            ok: true,
            elapsed_ms: 2,
            error_kind: None,
        }],
        logs: (0..50)
            .map(|i| format!("log line {i}: {}", "y".repeat(200)))
            .collect(),
    };

    // ~10 KB of logs against a 2 KB byte budget.
    let truncated = truncate_execution_response(response, 2048, 100_000, 4);

    // Within byte budget after trimming.
    assert!(
        serde_json::to_vec(&truncated).unwrap().len() <= 2048,
        "logs-dominant response must be trimmed within the byte budget"
    );
    // A sentinel records that logs were dropped.
    assert!(
        truncated
            .logs
            .iter()
            .any(|l| l.contains("logs truncated to fit response budget")),
        "a logs-truncation sentinel must be present, got: {:?}",
        truncated.logs
    );
    // Small result is preserved untouched (it was within budget on its own).
    assert_eq!(truncated.result, Some(json!({"ok": true})));
}

#[test]
fn log_trimming_terminates_when_budget_unreachable() {
    // calls[] metadata can dominate and is NOT trimmed, so the budget may be
    // unreachable. The log-trimming loop must still terminate (best-effort),
    // collapsing logs to a single sentinel rather than looping forever.
    let response = CodeModeExecutionResponse {
        result: Some(json!({"ok": true})),
        calls: (0..200)
            .map(|i| CodeModeExecutedCall {
                id: format!("upstream::test::tool_{i}"),
                ok: true,
                elapsed_ms: 1,
                error_kind: None,
            })
            .collect(),
        logs: (0..20).map(|i| format!("line {i}")).collect(),
    };

    // Tiny budget that calls[] alone exceeds — unreachable by log trimming.
    let truncated = truncate_execution_response(response, 64, 100_000, 4);

    // Terminated: logs collapsed to a single sentinel entry.
    assert_eq!(
        truncated.logs.len(),
        1,
        "logs must collapse to a single sentinel when budget is unreachable, got: {:?}",
        truncated.logs
    );
    assert!(
        truncated.logs[0].contains("logs truncated to fit response budget"),
        "the remaining entry must be the sentinel, got: {:?}",
        truncated.logs
    );
}

#[test]
fn wasm_runner_returns_42() {
    let result = super::wasm_runner::run_wasm_i32_export_for_smoke(
        r#"
        (module
          (func (export "run") (result i32)
            i32.const 42))
        "#,
        "run",
        super::wasm_runner::DEFAULT_SEARCH_FUEL,
    )
    .expect("wasm smoke runs");

    assert_eq!(result, 42);
}

#[test]
fn wasm_runner_reuses_cached_modules() {
    let wat = r#"
        (module
          (func (export "run") (result i32)
            i32.const 7))
        "#;
    super::wasm_runner::run_wasm_i32_export_for_smoke(
        wat,
        "run",
        super::wasm_runner::DEFAULT_SEARCH_FUEL,
    )
    .expect("first wasm smoke runs");
    let after_first = super::wasm_runner::cached_module_count_for_tests();
    super::wasm_runner::run_wasm_i32_export_for_smoke(
        wat,
        "run",
        super::wasm_runner::DEFAULT_SEARCH_FUEL,
    )
    .expect("second wasm smoke runs");
    let after_second = super::wasm_runner::cached_module_count_for_tests();

    assert_eq!(
        after_second, after_first,
        "same WAT should reuse cached module"
    );
}

#[test]
fn wasm_runner_reports_fuel_exhaustion_kind() {
    let err = super::wasm_runner::run_wasm_i32_export_for_smoke(
        r#"
        (module
          (func (export "run") (result i32)
            (loop br 0)
            i32.const 0))
        "#,
        "run",
        1,
    )
    .expect_err("fuel should be exhausted");

    assert_eq!(
        super::wasm_runner::trap_kind(&err),
        Some("code_mode_fuel_exhausted")
    );
}

// ── normalize_user_code ───────────────────────────────────────────────────

#[test]
fn token_estimate_divisor_affects_truncation_decision() {
    // A payload of ~4000 bytes.  With divisor=4 → ~1000 tokens (fits inside
    // max_response_tokens=2000).  With divisor=1 → ~4000 tokens (exceeds 2000).
    let payload = "x".repeat(4000);
    let make_response = || CodeModeExecutionResponse {
        result: Some(json!({"payload": payload.clone()})),
        calls: vec![CodeModeExecutedCall {
            id: "upstream::test::large".to_string(),
            ok: true,
            elapsed_ms: 1,
            error_kind: None,
        }],
        logs: Vec::new(),
    };

    // divisor=4: 4000 bytes / 4 = 1000 estimated tokens → within 2000 → NOT truncated
    let fits = truncate_execution_response(make_response(), usize::MAX, 2000, 4);
    // PRESENCE: final result is the original object, not a truncation marker
    let fits_result = fits.result.as_ref().expect("result present");
    assert!(
        fits_result.get("payload").is_some(),
        "divisor=4 must not truncate 4 kB payload against 2000-token limit"
    );
    // ABSENCE: no truncation marker
    assert!(
        fits_result.get("truncated").is_none(),
        "divisor=4 result must not carry a truncated flag"
    );

    // divisor=1: 4000 bytes / 1 = 4000 estimated tokens → exceeds 2000 → TRUNCATED
    let truncated = truncate_execution_response(make_response(), usize::MAX, 2000, 1);
    // PRESENCE: truncation marker is injected on the final result
    let truncated_result = truncated.result.as_ref().expect("result present");
    assert_eq!(
        truncated_result.get("truncated"),
        Some(&json!(true)),
        "divisor=1 must truncate 4 kB payload against 2000-token limit"
    );
    // ABSENCE: original payload content not preserved in the marker
    assert!(
        truncated_result.get("payload").is_none(),
        "truncation marker must not keep original payload key"
    );
}
