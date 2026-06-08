//! Tests: response-budget truncation, wasm runner smoke, token estimate.
#![cfg(test)]

use serde_json::{Value, json};
use tempfile::TempDir;

use super::artifacts::{
    CodeModeArtifactReceipt, CodeModeArtifactWrite, code_mode_artifact_root,
    write_code_mode_artifact,
};
use super::protocol::{CodeModeRunnerOutput, CodeModeRunnerResult};
use super::*;

#[test]
fn code_mode_runner_wrapper_exposes_write_artifact() {
    let wrapped = runner::wrap_code_mode_for_test("async () => 'ok'", "var codemode = {};");

    assert!(wrapped.contains("globalThis.writeArtifact"));
    assert!(wrapped.contains("__labEmitArtifactWrite"));
    assert!(wrapped.contains("writeArtifact path must be a non-empty string"));
    assert!(wrapped.contains("writeArtifact content must be a string"));
}

#[test]
fn code_mode_artifact_root_uses_run_id_under_lab_home() {
    let root = code_mode_artifact_root("01JTEST");
    let text = root.display().to_string();

    assert!(
        text.ends_with(".lab/code-mode-artifacts/01JTEST")
            || text.ends_with("lab/code-mode-artifacts/01JTEST")
    );
}

#[test]
fn truncates_code_execute_final_result_when_oversized() {
    // calls[] carry lightweight metadata only — truncation caps the FINAL
    // result. An oversized final result is replaced with a truncation marker;
    // the calls metadata is preserved untouched.
    let response = CodeModeExecutionResponse {
        result: Some(json!({"payload": "x".repeat(5000)})),
        calls: vec![
            CodeModeExecutedCall {
                id: "github::search_issues".to_string(),
                ok: true,
                elapsed_ms: 12,
                params: None,
                error_kind: None,
            },
            CodeModeExecutedCall {
                id: "github::list_issues".to_string(),
                ok: false,
                elapsed_ms: 7,
                params: None,
                error_kind: Some("rate_limited".to_string()),
            },
        ],
        logs: Vec::new(),
        artifacts: vec![],
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
            id: "github::search_issues".to_string(),
            ok: true,
            elapsed_ms: 3,
            params: None,
            error_kind: None,
        }],
        logs: Vec::new(),
        artifacts: vec![],
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
            id: "test::ping".to_string(),
            ok: true,
            elapsed_ms: 2,
            params: None,
            error_kind: None,
        }],
        logs: (0..50)
            .map(|i| format!("log line {i}: {}", "y".repeat(200)))
            .collect(),
        artifacts: vec![],
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
                id: format!("test::tool_{i}"),
                ok: true,
                elapsed_ms: 1,
                params: None,
                error_kind: None,
            })
            .collect(),
        logs: (0..20).map(|i| format!("line {i}")).collect(),
        artifacts: vec![],
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
fn apply_log_caps_byte_count_reflects_only_kept_bytes() {
    // Three 10-byte lines against a 25-byte cap: the first two (20 bytes) fit,
    // the third would push to 30 > 25 and is dropped. The sentinel must report
    // 20 kept bytes — NOT 30 (the over-cap total) and NOT 25 (the cap).
    let logs = vec!["a".repeat(10), "b".repeat(10), "c".repeat(10)];
    let capped = apply_log_caps(logs, 1000, 25);

    // Two original lines kept + one sentinel.
    assert_eq!(
        capped.len(),
        3,
        "expected 2 kept lines + sentinel: {capped:?}"
    );
    let sentinel = capped.last().expect("sentinel present");
    assert_eq!(
        sentinel, "[log output truncated at 2 lines / 20 bytes]",
        "sentinel must report kept bytes (20), got: {sentinel}"
    );
}

#[test]
fn apply_log_caps_entry_cap_reports_kept_bytes() {
    // Entry cap trips at 2 entries; the byte count must equal the sum of the two
    // kept lines, independent of the dropped remainder.
    let logs = vec!["x".repeat(5), "y".repeat(7), "z".repeat(100)];
    let capped = apply_log_caps(logs, 2, 1_000_000);
    let sentinel = capped.last().expect("sentinel present");
    assert_eq!(sentinel, "[log output truncated at 2 lines / 12 bytes]");
}

#[test]
fn wasm_runner_returns_42() {
    let result = wasm_runner::run_wasm_i32_export_for_smoke(
        r#"
        (module
          (func (export "run") (result i32)
            i32.const 42))
        "#,
        "run",
        wasm_runner::DEFAULT_SEARCH_FUEL,
    )
    .expect("wasm smoke runs");

    assert_eq!(result, 42);
}

#[test]
fn wasm_runner_reuses_cached_modules() {
    // Use a WAT unique to this test so the assertion is robust against the
    // module cache being shared across parallel tests: compiling the same WAT
    // twice must hand back the SAME Arc (pointer-equal), proving reuse without
    // depending on the absolute global cache size.
    let wat = r#"
        (module
          (func (export "run") (result i32)
            i32.const 7331))
        "#;
    let first = wasm_runner::cached_module_arc_for_tests(wat);
    let count_after_first = wasm_runner::cached_module_count_for_tests();
    let second = wasm_runner::cached_module_arc_for_tests(wat);
    let count_after_second = wasm_runner::cached_module_count_for_tests();

    assert!(
        std::sync::Arc::ptr_eq(&first, &second),
        "same WAT must return the same cached module Arc"
    );
    assert_eq!(
        count_after_second, count_after_first,
        "re-fetching the same WAT must not grow the cache"
    );
}

#[test]
fn wasm_runner_reports_fuel_exhaustion_kind() {
    let err = wasm_runner::run_wasm_i32_export_for_smoke(
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
        wasm_runner::trap_kind(&err),
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
            id: "test::large".to_string(),
            ok: true,
            elapsed_ms: 1,
            params: None,
            error_kind: None,
        }],
        logs: Vec::new(),
        artifacts: vec![],
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

#[test]
fn code_mode_history_bounds_entries_and_keeps_redacted_params_only() {
    let mut history = CodeModeHistory::new(2, 100_000);
    for idx in 0..3 {
        history.push(CodeModeHistoryEntry {
            seq: 0,
            kind: CodeModeHistoryKind::Execute,
            ok: true,
            elapsed_ms: idx,
            error_kind: None,
            calls: vec![CodeModeExecutedCall {
                id: format!("test::tool_{idx}"),
                ok: true,
                elapsed_ms: 1,
                params: trace::redact_trace_params(
                    &json!({"query": idx, "token": "raw-secret-token"}),
                    true,
                ),
                error_kind: None,
            }],
            match_count: None,
        });
    }

    let snapshot = history.snapshot();
    assert_eq!(snapshot.len(), 2);
    assert_eq!(snapshot[0].seq, 2);
    assert_eq!(snapshot[1].seq, 3);
    let serialized = serde_json::to_string(&snapshot).unwrap();
    assert!(serialized.contains("[redacted]"));
    assert!(!serialized.contains("raw-secret-token"));
}

#[test]
fn code_mode_history_bounds_by_bytes() {
    let mut history = CodeModeHistory::new(50, 1300);
    for idx in 0..10 {
        history.push(CodeModeHistoryEntry {
            seq: 0,
            kind: CodeModeHistoryKind::Search,
            ok: true,
            elapsed_ms: idx,
            error_kind: None,
            calls: vec![CodeModeExecutedCall {
                id: format!("test::tool_{idx}"),
                ok: true,
                elapsed_ms: 1,
                params: Some(json!({"safe": "x".repeat(250)})),
                error_kind: None,
            }],
            match_count: Some(idx as usize),
        });
    }

    let snapshot = history.snapshot();
    assert!(
        serde_json::to_vec(&snapshot).unwrap().len() <= 1300 || snapshot.len() == 1,
        "history should drop oldest entries until under byte budget or one entry remains"
    );
    assert!(
        snapshot.len() < 10,
        "byte budget should have dropped old entries"
    );
}

#[test]
fn code_mode_history_replaces_single_oversized_entry_with_bounded_sentinel() {
    let mut history = CodeModeHistory::new(50, 1300);
    history.push(CodeModeHistoryEntry {
        seq: 0,
        kind: CodeModeHistoryKind::Execute,
        ok: false,
        elapsed_ms: 99,
        error_kind: Some("server_error".to_string()),
        calls: vec![CodeModeExecutedCall {
            id: "test::oversized".to_string(),
            ok: false,
            elapsed_ms: 1,
            params: Some(json!({"safe": "x".repeat(20_000)})),
            error_kind: Some("server_error".to_string()),
        }],
        match_count: None,
    });

    let snapshot = history.snapshot();
    let serialized = serde_json::to_vec(&snapshot).unwrap();
    assert!(
        serialized.len() <= 1300,
        "single oversized history entry must be replaced to honor byte budget, got {} bytes",
        serialized.len()
    );
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].seq, 1);
    assert_eq!(
        snapshot[0].error_kind.as_deref(),
        Some("history_entry_too_large")
    );
    assert!(snapshot[0].calls.is_empty());
}

#[test]
fn code_mode_call_history_serializes_upstream_and_tool_fields() {
    let call = CodeModeExecutedCall {
        id: "github::search_issues".to_string(),
        ok: true,
        elapsed_ms: 12,
        params: Some(json!({"query": "bug"})),
        error_kind: None,
    };

    let serialized = serde_json::to_value(&call).unwrap();
    assert_eq!(serialized["id"], json!("github::search_issues"));
    assert_eq!(serialized["upstream"], json!("github"));
    assert_eq!(serialized["tool"], json!("search_issues"));
    assert_eq!(serialized["params"]["query"], json!("bug"));
}

#[test]
fn runner_protocol_preserves_null_distinct_from_undefined() {
    let null_output = CodeModeRunnerOutput::Done {
        result: CodeModeRunnerResult::Json(Value::Null),
        logs: Vec::new(),
    };
    let undefined_output = CodeModeRunnerOutput::Done {
        result: CodeModeRunnerResult::Undefined,
        logs: Vec::new(),
    };

    let null_round_trip: CodeModeRunnerOutput =
        serde_json::from_value(serde_json::to_value(null_output).unwrap()).unwrap();
    let undefined_round_trip: CodeModeRunnerOutput =
        serde_json::from_value(serde_json::to_value(undefined_output).unwrap()).unwrap();

    assert_eq!(
        null_round_trip.result_for_response(),
        Some(Value::Null),
        "explicit null must survive protocol round trip"
    );
    assert_eq!(
        undefined_round_trip.result_for_response(),
        None,
        "undefined must remain absent"
    );

    let explicit_null = serde_json::to_value(CodeModeExecutionResponse {
        result: Some(Value::Null),
        calls: Vec::new(),
        logs: Vec::new(),
        artifacts: vec![],
    })
    .unwrap();
    let undefined = serde_json::to_value(CodeModeExecutionResponse {
        result: None,
        calls: Vec::new(),
        logs: Vec::new(),
        artifacts: vec![],
    })
    .unwrap();
    assert!(
        explicit_null.get("result").is_some_and(Value::is_null),
        "explicit null must serialize as a present null result"
    );
    assert!(
        undefined.get("result").is_none(),
        "undefined must omit the result field"
    );
}

#[test]
fn code_mode_execution_error_carries_partial_calls() {
    let call = CodeModeExecutedCall {
        id: "github::search_issues".to_string(),
        ok: true,
        elapsed_ms: 12,
        params: Some(json!({"query": "bug"})),
        error_kind: None,
    };
    let err = CodeModeExecutionError::with_trace(
        ToolError::Sdk {
            sdk_kind: "server_error".to_string(),
            message: "boom".to_string(),
        },
        vec![call.clone()],
    );

    assert_eq!(err.kind(), "server_error");
    assert_eq!(err.calls(), &[call]);
}

#[test]
fn truncation_preserves_artifact_receipts() {
    let response = CodeModeExecutionResponse {
        result: Some(serde_json::json!({
            "markdown": "x".repeat(10_000),
            "artifact": {
                "path": "code-mode-artifacts/run/brief.md"
            }
        })),
        calls: vec![],
        logs: vec![],
        artifacts: vec![CodeModeArtifactReceipt {
            path: "brief.md".to_string(),
            absolute_path: "~/.lab/code-mode-artifacts/run/brief.md".to_string(),
            content_type: "text/markdown".to_string(),
            bytes: 10_000,
            sha256: "a".repeat(64),
        }],
    };

    let truncated = truncate_execution_response(response, 1400, 6000, 4);

    assert_eq!(truncated.artifacts.len(), 1);
    assert_eq!(truncated.artifacts[0].path, "brief.md");
    let result = truncated.result.expect("truncated marker result");
    assert_eq!(result["truncated"], true);
    assert_eq!(result["artifacts"][0]["path"], "brief.md");
}
#[tokio::test]
async fn write_code_mode_artifact_rejects_absolute_paths() {
    let root = TempDir::new().expect("temp root");
    let request = CodeModeArtifactWrite {
        path: "/tmp/escape.md".to_string(),
        content: "# nope".to_string(),
        content_type: Some("text/markdown".to_string()),
    };

    let err = write_code_mode_artifact(root.path(), &request)
        .await
        .expect_err("absolute artifact path must be rejected");

    assert_eq!(err.kind(), "invalid_param");
    assert!(
        err.to_string().contains("relative path"),
        "error should explain relative path requirement: {err}"
    );
}

#[tokio::test]
async fn write_code_mode_artifact_rejects_parent_dir_paths() {
    let root = TempDir::new().expect("temp root");
    let request = CodeModeArtifactWrite {
        path: "../escape.md".to_string(),
        content: "# nope".to_string(),
        content_type: Some("text/markdown".to_string()),
    };

    let err = write_code_mode_artifact(root.path(), &request)
        .await
        .expect_err("parent dir artifact path must be rejected");

    assert_eq!(err.kind(), "invalid_param");
    assert!(
        err.to_string().contains("path traversal"),
        "error should mention traversal: {err}"
    );
}

#[tokio::test]
async fn write_code_mode_artifact_persists_content_and_returns_receipt() {
    let root = TempDir::new().expect("temp root");
    let request = CodeModeArtifactWrite {
        path: "axon/brief.md".to_string(),
        content: "# Brief\n\nUseful output.\n".to_string(),
        content_type: Some("text/markdown".to_string()),
    };

    let receipt = write_code_mode_artifact(root.path(), &request)
        .await
        .expect("artifact write succeeds");

    assert_eq!(receipt.path, "axon/brief.md");
    assert_eq!(receipt.content_type, "text/markdown");
    assert_eq!(receipt.bytes, 24);
    assert_eq!(receipt.sha256.len(), 64);

    let written = tokio::fs::read_to_string(root.path().join("axon/brief.md"))
        .await
        .expect("artifact file exists");
    assert_eq!(written, "# Brief\n\nUseful output.\n");
}
