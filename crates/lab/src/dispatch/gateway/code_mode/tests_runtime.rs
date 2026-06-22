//! Tests: response-budget truncation, wasm runner smoke, token estimate.
#![cfg(test)]
#![allow(clippy::panic)]

use std::collections::HashSet;

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::config::CodeModeResultShapePolicy;

use super::artifacts::{
    CodeModeArtifactReceipt, CodeModeArtifactWrite, code_mode_artifact_root,
    write_code_mode_artifact,
};
use super::protocol::{CodeModeRunnerInput, CodeModeRunnerOutput, CodeModeRunnerResult};
use super::*;

/// Generous content cap for artifact tests whose subject is not the size limit
/// (path safety, content-type defaulting, persistence, I/O failure). The
/// dedicated size test passes its own explicit cap.
const TEST_MAX_BYTES: usize = 8 * 1024 * 1024;

#[test]
fn shape_policy_off_preserves_result_exactly() {
    let input = Some(json!({"ok": true, "items": [1, 2]}));
    let shaped = shape_final_result(input.clone(), CodeModeResultShapePolicy::Off, 100, 100, 4);
    assert_eq!(shaped.result, input);
    assert_eq!(shaped.metadata.policy, CodeModeResultShapePolicy::Off);
    assert!(!shaped.metadata.changed);
}

#[test]
fn shape_policy_truncate_preserves_small_json() {
    let input = Some(json!({"ok": true}));
    let shaped = shape_final_result(
        input.clone(),
        CodeModeResultShapePolicy::Truncate,
        4096,
        1000,
        4,
    );
    assert_eq!(shaped.result, input);
    assert_eq!(
        shaped.metadata.policy,
        CodeModeResultShapePolicy::Truncate
    );
    assert!(!shaped.metadata.changed);
}

#[test]
fn shape_policy_truncate_stringifies_large_object() {
    let input = Some(json!({"payload": "x".repeat(5000)}));
    let shaped = shape_final_result(input, CodeModeResultShapePolicy::Truncate, 1400, 6000, 4);
    let result = shaped.result.unwrap();
    let text = result.as_str().expect("large shaped result is a marker string");
    assert!(text.contains("[code mode result truncated]"), "{text}");
    assert!(text.contains("\"payload\""), "{text}");
    assert!(shaped.metadata.changed);
    assert!(shaped.metadata.truncated);
    assert!(shaped.metadata.original_size_bytes > shaped.metadata.shaped_size_bytes);
}

#[test]
fn shape_policy_preserves_none_and_null_distinction() {
    let none = shape_final_result(None, CodeModeResultShapePolicy::Truncate, 100, 100, 4);
    assert!(none.result.is_none());

    let null = shape_final_result(
        Some(Value::Null),
        CodeModeResultShapePolicy::Truncate,
        100,
        100,
        4,
    );
    assert_eq!(null.result, Some(Value::Null));
}

#[test]
fn snippet_runner_protocol_round_trips() {
    let output = CodeModeRunnerOutput::SnippetResolve {
        seq: 7,
        name: "demo".to_string(),
        input: json!({"x": 2}),
    };
    let encoded = serde_json::to_string(&output).unwrap();
    assert_eq!(
        serde_json::from_str::<CodeModeRunnerOutput>(&encoded).unwrap(),
        output
    );

    let input = CodeModeRunnerInput::SnippetResolved {
        seq: 7,
        code: "async (input) => input.x * 2".to_string(),
        input: json!({"x": 2}),
    };
    let encoded = serde_json::to_string(&input).unwrap();
    assert_eq!(
        serde_json::from_str::<CodeModeRunnerInput>(&encoded).unwrap(),
        input
    );
}

#[test]
fn code_mode_reference_error_hint_names_available_globals() {
    let message = runner::add_code_mode_hint_for_test(
        "ReferenceError",
        "ReferenceError: require is not defined",
    );
    assert!(message.contains("Available globals"));
    assert!(message.contains("codemode.run"));
    assert!(message.contains("require, process, fs, fetch"));
}

#[test]
fn code_mode_helper_type_error_hint_points_to_search_describe() {
    let message = runner::add_code_mode_hint_for_test(
        "server_error",
        "TypeError: codemode.github.nope is not a function",
    );
    assert!(message.contains("codemode.search"));
    assert!(message.contains("codemode.describe"));
}

#[test]
fn code_mode_source_store_resolves_same_route_admin_source() {
    let mut store = CodeModeSourceStore::new(4, 4096);
    store.push(CodeModeExecutionSource {
        execution_id: "01JTEST".to_string(),
        created_at_ms: 1,
        actor_key: Some("actor-a".to_string()),
        is_admin: true,
        route_scope: "root".to_string(),
        surface: CodeModeSurface::Mcp,
        capability_filter_fingerprint: CodeModeCapabilityFilter::default().fingerprint(),
        code: "async () => ({ ok: true })".to_string(),
    });

    let source = store
        .resolve(
            "01JTEST",
            &CodeModeSourceLookup {
                actor_key: Some("actor-a".to_string()),
                is_admin: true,
                route_scope: "root".to_string(),
                capability_filter_fingerprint: CodeModeCapabilityFilter::default().fingerprint(),
            },
        )
        .unwrap();
    assert_eq!(source.code, "async () => ({ ok: true })");
}

#[test]
fn code_mode_source_store_rejects_unknown_and_cross_route() {
    let mut store = CodeModeSourceStore::new(4, 4096);
    store.push(CodeModeExecutionSource {
        execution_id: "01JTEST".to_string(),
        created_at_ms: 1,
        actor_key: None,
        is_admin: true,
        route_scope: "root".to_string(),
        surface: CodeModeSurface::Mcp,
        capability_filter_fingerprint: CodeModeCapabilityFilter::default().fingerprint(),
        code: "async () => ({ ok: true })".to_string(),
    });

    let lookup = CodeModeSourceLookup {
        actor_key: None,
        is_admin: true,
        route_scope: "protected".to_string(),
        capability_filter_fingerprint: CodeModeCapabilityFilter::default().fingerprint(),
    };
    assert_eq!(
        store.resolve("missing", &lookup).unwrap_err().kind(),
        "unknown_execution"
    );
    assert_eq!(
        store.resolve("01JTEST", &lookup).unwrap_err().kind(),
        "forbidden"
    );
}

#[test]
fn code_mode_source_store_allows_narrower_capability_filter() {
    let mut store = CodeModeSourceStore::new(4, 4096);
    store.push(CodeModeExecutionSource {
        execution_id: "01JTEST".to_string(),
        created_at_ms: 1,
        actor_key: Some("actor-a".to_string()),
        is_admin: true,
        route_scope: "protected".to_string(),
        surface: CodeModeSurface::Mcp,
        capability_filter_fingerprint: "upstreams=github;tools=github::search_issues".to_string(),
        code: "async () => ({ ok: true })".to_string(),
    });

    let source = store
        .resolve(
            "01JTEST",
            &CodeModeSourceLookup {
                actor_key: Some("actor-a".to_string()),
                is_admin: true,
                route_scope: "protected".to_string(),
                capability_filter_fingerprint: "upstreams=github;tools=".to_string(),
            },
        )
        .unwrap();
    assert_eq!(source.execution_id, "01JTEST");
}

#[test]
fn code_mode_source_store_rejects_wider_source_capability_filter() {
    let mut store = CodeModeSourceStore::new(4, 4096);
    store.push(CodeModeExecutionSource {
        execution_id: "01JTEST".to_string(),
        created_at_ms: 1,
        actor_key: Some("actor-a".to_string()),
        is_admin: true,
        route_scope: "protected".to_string(),
        surface: CodeModeSurface::Mcp,
        capability_filter_fingerprint: CodeModeCapabilityFilter::default().fingerprint(),
        code: "async () => ({ ok: true })".to_string(),
    });

    let err = store
        .resolve(
            "01JTEST",
            &CodeModeSourceLookup {
                actor_key: Some("actor-a".to_string()),
                is_admin: true,
                route_scope: "protected".to_string(),
                capability_filter_fingerprint: "upstreams=github;tools=".to_string(),
            },
        )
        .unwrap_err();
    assert_eq!(err.kind(), "forbidden");
}

#[test]
fn code_mode_runner_wrapper_exposes_write_artifact() {
    let wrapped = runner::wrap_code_mode_for_test("async () => 'ok'", "var codemode = {};");

    assert!(wrapped.contains("globalThis.writeArtifact"));
    assert!(wrapped.contains("__labEmitArtifactWrite"));
    assert!(wrapped.contains("writeArtifact path must be a non-empty string"));
    assert!(wrapped.contains("writeArtifact content must be a string"));
    assert!(wrapped.contains("writeArtifact options.contentType must be a string"));
}

#[test]
fn code_mode_artifact_root_uses_run_id_under_lab_home() {
    let root = code_mode_artifact_root("01JTEST");

    // Separator-agnostic: `Path::ends_with` matches whole components, so this
    // holds on both `/`- and `\`-separated platforms.
    assert!(
        root.ends_with(std::path::Path::new("code-mode-artifacts").join("01JTEST")),
        "got {}",
        root.display()
    );
    // The parent component must be a `.lab` / `lab` home dir.
    let parent = root
        .parent()
        .and_then(std::path::Path::parent)
        .and_then(std::path::Path::file_name)
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    assert!(
        parent == ".lab" || parent == "lab",
        "expected .lab/lab home parent, got {}",
        root.display()
    );
}

#[test]
fn truncates_codemode_final_result_when_oversized() {
    // calls[] carry lightweight metadata only — truncation caps the FINAL
    // result. An oversized final result is replaced with a truncation marker;
    // the calls metadata is preserved untouched.
    let response = CodeModeExecutionResponse {
        execution_id: None,
        ui: None,
        result: Some(json!({"payload": "x".repeat(5000)})),
        result_shape: None,
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
        execution_id: None,
        ui: None,
        result: Some(json!({"items": ["small"]})),
        result_shape: None,
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
        execution_id: None,
        ui: None,
        result: Some(json!({"ok": true})),
        result_shape: None,
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
        execution_id: None,
        ui: None,
        result: Some(json!({"ok": true})),
        result_shape: None,
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
        execution_id: None,
        ui: None,
        result: Some(json!({"payload": payload.clone()})),
        result_shape: None,
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
            execution_id: None,
            seq: 0,
            route_scope: "root".to_string(),
            kind: CodeModeHistoryKind::Execute,
            ok: true,
            elapsed_ms: idx,
            input_tokens: Some(idx as usize + 1),
            output_tokens: Some(idx as usize + 2),
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
    assert!(serialized.contains("input_tokens"));
    assert!(serialized.contains("output_tokens"));
}

#[test]
fn code_mode_history_bounds_by_bytes() {
    let mut history = CodeModeHistory::new(50, 1300);
    for idx in 0..10 {
        history.push(CodeModeHistoryEntry {
            execution_id: None,
            seq: 0,
            route_scope: "root".to_string(),
            kind: CodeModeHistoryKind::Execute,
            ok: true,
            elapsed_ms: idx,
            input_tokens: Some(idx as usize + 1),
            output_tokens: Some(idx as usize + 2),
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
        execution_id: None,
        seq: 0,
        route_scope: "root".to_string(),
        kind: CodeModeHistoryKind::Execute,
        ok: false,
        elapsed_ms: 99,
        input_tokens: Some(99),
        output_tokens: Some(0),
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
        execution_id: None,
        ui: None,
        result: Some(Value::Null),
        result_shape: None,
        calls: Vec::new(),
        logs: Vec::new(),
        artifacts: vec![],
    })
    .unwrap();
    let undefined = serde_json::to_value(CodeModeExecutionResponse {
        execution_id: None,
        ui: None,
        result: None,
        result_shape: None,
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
        execution_id: None,
        ui: None,
        result: Some(serde_json::json!({
            "markdown": "x".repeat(10_000),
            "artifact": {
                "path": "code-mode-artifacts/run/brief.md"
            }
        })),
        result_shape: None,
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

#[test]
fn execute_trace_surfaces_artifacts_when_present() {
    let response = CodeModeExecutionResponse {
        execution_id: None,
        ui: None,
        result: Some(json!({ "ok": true })),
        result_shape: None,
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

    let trace = code_mode_execute_trace(&response);
    // A structured-content-only client must see artifact receipts so it can
    // follow the "write large payload to an artifact, read it back" path.
    assert_eq!(trace["artifacts"][0]["path"], json!("brief.md"));
    assert_eq!(trace["artifacts"][0]["bytes"], json!(10_000));
    assert_eq!(trace["result"]["ok"], json!(true));
}

#[test]
fn execute_trace_omits_artifacts_when_empty() {
    let response = CodeModeExecutionResponse {
        execution_id: None,
        ui: None,
        result: Some(json!({ "ok": true })),
        result_shape: None,
        calls: vec![],
        logs: vec![],
        artifacts: vec![],
    };

    let trace = code_mode_execute_trace(&response);
    assert!(
        trace.get("artifacts").is_none(),
        "no artifacts → field omitted"
    );
}

#[tokio::test]
async fn write_code_mode_artifact_rejects_absolute_paths() {
    let root = TempDir::new().expect("temp root");
    let request = CodeModeArtifactWrite {
        path: "/tmp/escape.md".to_string(),
        content: "# nope".to_string(),
        content_type: Some("text/markdown".to_string()),
    };

    let err = write_code_mode_artifact(root.path(), &request, TEST_MAX_BYTES)
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

    let err = write_code_mode_artifact(root.path(), &request, TEST_MAX_BYTES)
        .await
        .expect_err("parent dir artifact path must be rejected");

    assert_eq!(err.kind(), "path_traversal");
    assert!(
        err.to_string().contains("path traversal"),
        "error should mention traversal: {err}"
    );
}

#[tokio::test]
async fn write_code_mode_artifact_rejects_backslash_relative_traversal() {
    // Regression: on Linux a backslash is an ordinary filename byte, so the
    // lexical guard must run AFTER `\`->`/` normalization. `a\..\..\escape.md`
    // must be rejected, and nothing may be written outside `root`.
    let root = TempDir::new().expect("temp root");
    let outside = TempDir::new().expect("outside dir");
    let request = CodeModeArtifactWrite {
        path: format!(
            "a\\..\\..\\..\\{}\\pwned.md",
            outside.path().file_name().unwrap().to_str().unwrap()
        ),
        content: "# escaped".to_string(),
        content_type: None,
    };

    let err = write_code_mode_artifact(root.path(), &request, TEST_MAX_BYTES)
        .await
        .expect_err("backslash traversal must be rejected");

    assert_eq!(err.kind(), "path_traversal");
    assert!(
        err.to_string().contains("path traversal"),
        "error should mention traversal: {err}"
    );
}

#[tokio::test]
async fn write_code_mode_artifact_rejects_backslash_absolute_path() {
    // Regression: `\etc\evil` is NOT absolute on Linux (no leading `/`), so the
    // pre-normalization guard missed it; after `\`->`/` it becomes `/etc/evil`
    // and `root.join` would discard the base entirely. Must be rejected.
    let root = TempDir::new().expect("temp root");
    let request = CodeModeArtifactWrite {
        path: "\\etc\\cron.d\\evil".to_string(),
        content: "# escaped".to_string(),
        content_type: None,
    };

    let err = write_code_mode_artifact(root.path(), &request, TEST_MAX_BYTES)
        .await
        .expect_err("backslash absolute path must be rejected");

    assert_eq!(err.kind(), "invalid_param");
    assert!(
        err.to_string().contains("relative path"),
        "error should explain relative path requirement: {err}"
    );
}

#[tokio::test]
async fn write_code_mode_artifact_rejects_windows_drive_paths() {
    // Regression: a Windows drive prefix (`C:/foo` drive-absolute or `C:foo`
    // drive-relative) is NOT caught by `Path::is_absolute()` on Unix, and its
    // `C:` segment parses as a plain `Normal` component there — so without the
    // explicit `has_windows_drive_prefix` guard it would slip past the jail and
    // only later (if at all) be mislabeled by the traversal check. Both forms
    // must be rejected as `invalid_param` on every platform.
    let root = TempDir::new().expect("temp root");
    for raw in ["C:/Windows/evil", "C:foo"] {
        let request = CodeModeArtifactWrite {
            path: raw.to_string(),
            content: "# nope".to_string(),
            content_type: None,
        };
        let err = write_code_mode_artifact(root.path(), &request, TEST_MAX_BYTES)
            .await
            .expect_err("drive-prefixed path must be rejected");
        assert_eq!(err.kind(), "invalid_param", "path `{raw}`");
        assert!(
            err.to_string().contains("relative path"),
            "error should explain relative path requirement for `{raw}`: {err}"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn write_code_mode_artifact_rejects_symlinked_ancestor() {
    // Defense-in-depth: a pre-existing symlinked directory under the root must
    // not be used to redirect the write outside the jail.
    let root = TempDir::new().expect("temp root");
    let outside = TempDir::new().expect("outside dir");
    std::os::unix::fs::symlink(outside.path(), root.path().join("link")).expect("create symlink");
    let request = CodeModeArtifactWrite {
        path: "link/escape.md".to_string(),
        content: "# escaped".to_string(),
        content_type: None,
    };

    let err = write_code_mode_artifact(root.path(), &request, TEST_MAX_BYTES)
        .await
        .expect_err("symlinked ancestor must be rejected");

    assert_eq!(err.kind(), "symlink_rejected");
    assert!(
        !outside.path().join("escape.md").exists(),
        "write must not land outside the jail through the symlink"
    );
}

#[tokio::test]
async fn write_code_mode_artifact_enforces_the_passed_content_cap() {
    let root = TempDir::new().expect("temp root");
    // The cap is whatever the caller passes (resolved from
    // LAB_CODE_MODE_ARTIFACT_MAX_MIB). Drive it with an explicit 1 MiB cap so the
    // boundary is exercised cheaply: exactly at-cap succeeds, one byte over is a
    // clean invalid_param.
    let cap = 1024 * 1024;
    let at_cap = "a".repeat(cap);
    let over_cap = "a".repeat(cap + 1);

    let ok = write_code_mode_artifact(
        root.path(),
        &CodeModeArtifactWrite {
            path: "ok.md".to_string(),
            content: at_cap,
            content_type: None,
        },
        cap,
    )
    .await
    .expect("exactly at-cap must be accepted");
    assert_eq!(ok.bytes, cap);

    let err = write_code_mode_artifact(
        root.path(),
        &CodeModeArtifactWrite {
            path: "too-big.md".to_string(),
            content: over_cap,
            content_type: Some("text/markdown".to_string()),
        },
        cap,
    )
    .await
    .expect_err("over cap must be rejected");
    assert_eq!(err.kind(), "invalid_param");
    assert!(err.to_string().contains("maximum"), "error: {err}");
}

#[test]
fn artifact_max_bytes_parses_env_and_falls_back() {
    use crate::dispatch::helpers::with_env_override;
    use std::collections::HashMap;

    // Absent → 8 MiB default.
    let default = artifacts::artifact_max_bytes();
    assert_eq!(
        default,
        8 * 1024 * 1024,
        "absent env must yield the 8 MiB default"
    );

    // Valid MiB value is honored and converted to bytes.
    let valid = with_env_override(
        HashMap::from([(
            "LAB_CODE_MODE_ARTIFACT_MAX_MIB".to_string(),
            "16".to_string(),
        )]),
        artifacts::artifact_max_bytes,
    );
    assert_eq!(valid, 16 * 1024 * 1024, "a valid MiB value must be honored");

    // `0` and garbage both fall back to the default (a 0-byte cap rejects all).
    for bad in ["0", "5O"] {
        let got = with_env_override(
            HashMap::from([(
                "LAB_CODE_MODE_ARTIFACT_MAX_MIB".to_string(),
                bad.to_string(),
            )]),
            artifacts::artifact_max_bytes,
        );
        assert_eq!(
            got,
            8 * 1024 * 1024,
            "{bad:?} must fall back to the default"
        );
    }
}

#[test]
fn artifact_max_store_bytes_parses_env_with_zero_disabling() {
    use crate::dispatch::helpers::with_env_override;
    use std::collections::HashMap;

    // Absent → 4 GiB default.
    assert_eq!(
        artifacts::artifact_max_store_bytes(),
        4096 * 1024 * 1024,
        "absent env must yield the 4 GiB default"
    );

    let cases = [
        ("8192", 8192u64 * 1024 * 1024), // valid MiB honored
        ("0", 0),                        // 0 is meaningful here: disable byte pruning
        ("5O", 4096 * 1024 * 1024),      // garbage → default
    ];
    for (raw, expected) in cases {
        let got = with_env_override(
            HashMap::from([(
                "LAB_CODE_MODE_ARTIFACT_MAX_STORE_MIB".to_string(),
                raw.to_string(),
            )]),
            artifacts::artifact_max_store_bytes,
        );
        assert_eq!(got, expected, "{raw:?} must resolve to {expected} bytes");
    }
}

#[tokio::test]
async fn write_code_mode_artifact_accepts_and_trims_common_content_types() {
    let root = TempDir::new().expect("temp root");
    for (idx, (content_type, expected)) in [
        ("text/plain", "text/plain"),
        ("text/markdown", "text/markdown"),
        ("application/json", "application/json"),
        (" application/vnd.lab+json ", "application/vnd.lab+json"),
    ]
    .into_iter()
    .enumerate()
    {
        let receipt = write_code_mode_artifact(
            root.path(),
            &CodeModeArtifactWrite {
                path: format!("valid-{idx}.txt"),
                content: "body".to_string(),
                content_type: Some(content_type.to_string()),
            },
            TEST_MAX_BYTES,
        )
        .await
        .expect("write succeeds");
        assert_eq!(receipt.content_type, expected);
    }
}

#[tokio::test]
async fn write_code_mode_artifact_rejects_invalid_content_types_without_writing() {
    // content_type rides the receipt into the model's context, so it carries its
    // own grammar and 256-byte cap independent of the (large) content cap.
    let root = TempDir::new().expect("temp root");
    for (idx, content_type) in [
        "text/html\nnope".to_string(),
        "text/html\r\nContent-Length: 0".to_string(),
        "text /plain".to_string(),
        "text/plain charset=utf-8".to_string(),
        "textplain".to_string(),
        "/plain".to_string(),
        "text/".to_string(),
        "text/plain/html".to_string(),
        "text/pláin".to_string(),
        "\u{00a0}text/plain\u{00a0}".to_string(),
        format!("text/{}", "a".repeat(252)),
    ]
    .into_iter()
    .enumerate()
    {
        let path = format!("invalid-{idx}.txt");
        let err = write_code_mode_artifact(
            root.path(),
            &CodeModeArtifactWrite {
                path: path.clone(),
                content: "body".to_string(),
                content_type: Some(content_type),
            },
            TEST_MAX_BYTES,
        )
        .await
        .expect_err("invalid content_type must be rejected");
        assert_eq!(err.kind(), "invalid_param");
        assert!(
            err.to_string().contains("content_type"),
            "error should name the offending param: {err}"
        );
        assert!(
            !root.path().join(path).exists(),
            "rejected content_type must not write a file"
        );
    }
}

#[tokio::test]
async fn write_code_mode_artifact_defaults_content_type_to_text_plain() {
    let root = TempDir::new().expect("temp root");
    for (idx, content_type) in [None, Some(String::new()), Some("   ".to_string())]
        .into_iter()
        .enumerate()
    {
        let receipt = write_code_mode_artifact(
            root.path(),
            &CodeModeArtifactWrite {
                path: format!("note-{idx}.txt"),
                content: "body".to_string(),
                content_type,
            },
            TEST_MAX_BYTES,
        )
        .await
        .expect("write succeeds");
        assert_eq!(
            receipt.content_type, "text/plain",
            "absent/blank content type must default to text/plain"
        );
    }
}

#[tokio::test]
async fn write_code_mode_artifact_maps_io_failure_to_internal_error() {
    let root = TempDir::new().expect("temp root");
    // Pre-create `a` as a regular file so `create_dir_all(root/a)` fails when
    // writing `a/b.md`. The I/O failure must surface as a server-side kind.
    tokio::fs::write(root.path().join("a"), b"blocker")
        .await
        .expect("seed blocking file");
    let err = write_code_mode_artifact(
        root.path(),
        &CodeModeArtifactWrite {
            path: "a/b.md".to_string(),
            content: "# nope".to_string(),
            content_type: None,
        },
        TEST_MAX_BYTES,
    )
    .await
    .expect_err("write under a file must fail");

    assert_eq!(err.kind(), "internal_error");
}

#[tokio::test]
async fn prune_artifact_runs_keeps_newest_and_ignores_non_ulid_entries() {
    let store = TempDir::new().expect("store root");
    // Three valid ULID run dirs (sortable chronologically) plus an operator's
    // stray non-ULID directory that must never be collected.
    let mut run_ids: Vec<String> = (0..3).map(|_| ulid::Ulid::new().to_string()).collect();
    run_ids.sort(); // ascending: oldest first
    for id in &run_ids {
        tokio::fs::create_dir_all(store.path().join(id))
            .await
            .unwrap();
    }
    tokio::fs::create_dir_all(store.path().join("operator-notes"))
        .await
        .unwrap();

    artifacts::prune_artifact_runs_in(store.path(), 1, 0, &HashSet::new()).await;

    // Oldest two run dirs pruned; newest retained.
    assert!(!store.path().join(&run_ids[0]).exists());
    assert!(!store.path().join(&run_ids[1]).exists());
    assert!(store.path().join(&run_ids[2]).exists());
    // Non-ULID directory is untouched.
    assert!(store.path().join("operator-notes").exists());
}

#[tokio::test]
async fn prune_artifact_runs_never_deletes_active_run_dir() {
    // Concurrency guard: a still-executing run's directory must survive pruning
    // even when it is the oldest and `retain` would otherwise collect it. This
    // is the proper fix for the cross-run prune race — not "keep retention high".
    let store = TempDir::new().expect("store root");
    let mut ids: Vec<String> = (0..3).map(|_| ulid::Ulid::new().to_string()).collect();
    ids.sort(); // ascending: oldest first
    for id in &ids {
        tokio::fs::create_dir_all(store.path().join(id))
            .await
            .unwrap();
    }

    // Mark the OLDEST run active; with retain=1 it would normally be deleted.
    let active = HashSet::from([ids[0].clone()]);
    artifacts::prune_artifact_runs_in(store.path(), 1, 0, &active).await;

    // Active oldest dir survives despite retain=1; the inactive middle dir is
    // pruned; the newest dir is retained by the cap.
    assert!(
        store.path().join(&ids[0]).exists(),
        "active run dir must never be pruned, even as the oldest under retain=1"
    );
    assert!(
        !store.path().join(&ids[1]).exists(),
        "inactive old run dir must still be pruned"
    );
    assert!(
        store.path().join(&ids[2]).exists(),
        "newest run dir retained"
    );
}

#[tokio::test]
async fn prune_artifact_runs_enforces_byte_budget() {
    // With the count cap effectively off (retain high), the byte budget alone
    // must drop the oldest runs until the store fits. Three 1000-byte runs
    // against a 2000-byte budget keep the newest two and prune the oldest.
    let store = TempDir::new().expect("store root");
    let mut ids: Vec<String> = (0..3).map(|_| ulid::Ulid::new().to_string()).collect();
    ids.sort(); // ascending: oldest first
    for id in &ids {
        let dir = store.path().join(id);
        // Nest the file so the recursive size walk is exercised.
        tokio::fs::create_dir_all(dir.join("sub")).await.unwrap();
        tokio::fs::write(dir.join("sub/a.md"), vec![b'x'; 1000])
            .await
            .unwrap();
    }

    artifacts::prune_artifact_runs_in(store.path(), 100, 2000, &HashSet::new()).await;

    assert!(
        !store.path().join(&ids[0]).exists(),
        "oldest run must be pruned to honor the byte budget"
    );
    assert!(
        store.path().join(&ids[1]).exists(),
        "second-newest fits the budget"
    );
    assert!(
        store.path().join(&ids[2]).exists(),
        "newest fits the budget"
    );
}

#[test]
fn active_artifact_run_guard_registers_and_clears() {
    // Unique id so the assertion is robust against parallel tests sharing the
    // process-global active set.
    let id = ulid::Ulid::new().to_string();
    assert!(!artifacts::active_artifact_runs_snapshot().contains(&id));
    {
        let _guard = artifacts::ActiveArtifactRun::register(&id);
        assert!(
            artifacts::active_artifact_runs_snapshot().contains(&id),
            "register must mark the run active"
        );
    }
    assert!(
        !artifacts::active_artifact_runs_snapshot().contains(&id),
        "dropping the guard must clear the run id"
    );
}

#[tokio::test]
async fn prune_artifact_runs_disabled_when_retain_zero() {
    let store = TempDir::new().expect("store root");
    let id = ulid::Ulid::new().to_string();
    tokio::fs::create_dir_all(store.path().join(&id))
        .await
        .unwrap();

    artifacts::prune_artifact_runs_in(store.path(), 0, 0, &HashSet::new()).await;

    assert!(
        store.path().join(&id).exists(),
        "retain=0 and store-bytes=0 disables all pruning"
    );
}

#[tokio::test]
async fn prune_artifact_runs_noop_when_retain_at_or_above_count() {
    let store = TempDir::new().expect("store root");
    let ids: Vec<String> = (0..2).map(|_| ulid::Ulid::new().to_string()).collect();
    for id in &ids {
        tokio::fs::create_dir_all(store.path().join(id))
            .await
            .unwrap();
    }

    // retain == count and retain > count must both keep every run dir.
    artifacts::prune_artifact_runs_in(store.path(), 2, 0, &HashSet::new()).await;
    artifacts::prune_artifact_runs_in(store.path(), 9, 0, &HashSet::new()).await;

    for id in &ids {
        assert!(store.path().join(id).exists(), "{id} must be retained");
    }
}

#[test]
fn artifact_retention_runs_parses_env_and_falls_back_on_garbage() {
    use crate::dispatch::helpers::with_env_override;
    use std::collections::HashMap;

    let valid = with_env_override(
        HashMap::from([(
            "LAB_CODE_MODE_ARTIFACT_RETENTION_RUNS".to_string(),
            "50".to_string(),
        )]),
        artifacts::artifact_retention_runs,
    );
    assert_eq!(valid, 50, "a valid numeric value must be honored");

    // A present-but-unparseable value falls back to the default (200), not to
    // the garbage. It also emits a warn (not asserted here).
    let garbage = with_env_override(
        HashMap::from([(
            "LAB_CODE_MODE_ARTIFACT_RETENTION_RUNS".to_string(),
            "5O".to_string(),
        )]),
        artifacts::artifact_retention_runs,
    );
    assert_eq!(garbage, 200, "garbage must fall back to the default");
}

// ---------------------------------------------------------------------------
// T4: runner sandbox invariant tests (guards the Q-H2 refactor)
// ---------------------------------------------------------------------------

/// T4-a: Verify that the runner child is spawned with `env_clear` — i.e. a
/// sentinel variable explicitly present in one spawn is ABSENT when the same
/// command is respawned with `env_clear()`.
///
/// Design: we spawn two children of `/bin/sh -c 'cat /proc/$$/environ'` (which
/// prints its own env to stdout):
///   1. With `.env(sentinel_key, sentinel_val)` — must contain the sentinel.
///   2. With `.env_clear()` — must NOT contain the sentinel.
///
/// This avoids any `set_var`/`remove_var` call (which became `unsafe` in Rust
/// 1.94) by injecting the sentinel exclusively through the `Command` builder.
#[test]
#[cfg(target_os = "linux")]
fn runner_child_env_is_cleared() {
    let sentinel_key = "LAB_TEST_ENV_SENTINEL_SHOULD_NOT_LEAK";
    let sentinel_val = "THIS_MUST_NOT_APPEAR_IN_RUNNER_ENV";

    // ---- Positive control: sentinel present when NOT cleared ----
    let output = std::process::Command::new("/bin/sh")
        .args(["-c", "cat /proc/$$/environ"])
        .env(sentinel_key, sentinel_val)
        .output();
    let output = match output {
        Ok(o) => o,
        Err(_) => return, // /bin/sh unavailable — skip
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(sentinel_key),
        "positive control: sentinel must appear in child env when set via .env()"
    );

    // ---- Negative control: sentinel absent when env is cleared ----
    let output = std::process::Command::new("/bin/sh")
        .args(["-c", "cat /proc/$$/environ"])
        .env_clear()
        .output()
        .expect("/bin/sh must spawn for env_clear check");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains(sentinel_key),
        "runner child must not inherit sentinel env var `{sentinel_key}` — \
         env_clear must be in effect"
    );
}

/// T4-b: Verify that after the runner child is killed via its process group,
/// the direct child is fully reaped — i.e. it does not remain as a zombie.
///
/// This is a **compile-time guard** for the `process_group(0)` + killpg path
/// in `runner_drive.rs`. It does NOT require a live `labby` binary — it uses
/// `/bin/sleep` as a stand-in subprocess so the test is hermetic and fast.
///
/// The `nix` crate (`signal` + `process` features) is an existing direct
/// dependency and provides the `killpg` / `getpgid` / `waitpid` surface.
#[test]
#[cfg(unix)]
fn runner_process_group_is_reaped_after_kill() {
    use nix::sys::signal::{Signal, killpg};
    use nix::sys::wait::{WaitStatus, waitpid};
    use nix::unistd::Pid;
    use std::os::unix::process::CommandExt as _;
    use std::process::Stdio;

    // Spawn `/bin/sleep 60` in its own process group (pgid = pid), mirroring
    // the `process_group(0)` call in `runner_drive.rs`.
    // `process_group(0)` is a safe method on `std::process::Command` (stable
    // since Rust 1.64) — no `pre_exec` or `unsafe` needed.
    let mut child = std::process::Command::new("/bin/sleep")
        .arg("60")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("/bin/sleep must spawn");
    let child_pid = Pid::from_raw(child.id() as i32);

    // Give the child a brief moment to call setpgid before we query its pgid.
    std::thread::sleep(std::time::Duration::from_millis(20));

    // The child's pgid equals its own pid after setpgid(0,0).
    let pgid = nix::unistd::getpgid(Some(child_pid)).unwrap_or(child_pid);

    // Kill the entire process group (mirrors `terminate_code_mode_runner`).
    let _ = killpg(pgid, Signal::SIGKILL);

    // Reap the direct child. A `WaitStatus::Signaled` result confirms the
    // process group kill reached it; `WaitStatus::StillAlive` would indicate
    // a bug in the killpg path.
    let wait_result = waitpid(child_pid, None);
    match wait_result {
        Ok(WaitStatus::Signaled(pid, sig, _)) => {
            assert_eq!(pid, child_pid, "reaped the correct pid");
            assert_eq!(sig, Signal::SIGKILL, "killed by SIGKILL");
        }
        // `Exited` is also acceptable (race between setpgid and kill on fast hosts).
        Ok(WaitStatus::Exited(pid, _)) => {
            assert_eq!(pid, child_pid, "reaped the correct pid");
        }
        other => {
            panic!("unexpected waitpid result after killpg: {other:?}");
        }
    }
    drop(child.wait());

    // On Linux, verify the process is gone from /proc as an extra check.
    #[cfg(target_os = "linux")]
    {
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            !std::path::Path::new(&format!("/proc/{}", child_pid.as_raw())).exists(),
            "runner pid {} must not appear in /proc after killpg + waitpid",
            child_pid.as_raw()
        );
    }
}

/// P-M5: binary-search log truncation produces valid output for a
/// representative case. Asserts that:
///  1. The truncated response is within budget.
///  2. A sentinel is present.
///  3. The drop boundary is tight: the truncated set fits, and adding back
///     one more original line would exceed the budget.
#[test]
fn binary_search_log_truncation_boundary_is_tight() {
    // 50 log lines of ~80 bytes each → ~4 KB of logs.
    // The envelope (result, calls, artifacts) adds another ~200 bytes.
    // Use a 2 KB budget so some lines are dropped but not all.
    let line_count = 50usize;
    let line_payload = "B".repeat(70);
    let make_response = || CodeModeExecutionResponse {
        execution_id: None,
        ui: None,
        result: Some(json!({"ok": true})),
        result_shape: None,
        calls: vec![CodeModeExecutedCall {
            id: "test::ping".to_string(),
            ok: true,
            elapsed_ms: 1,
            params: None,
            error_kind: None,
        }],
        logs: (0..line_count)
            .map(|i| format!("[line {i:02}] {line_payload}"))
            .collect(),
        artifacts: vec![],
    };
    let original = make_response();

    // Pick a budget above the bare envelope (so some logs can fit) but well
    // below the total log volume.
    let bare_envelope_size = {
        let mut bare = make_response();
        bare.logs = Vec::new();
        serde_json::to_vec(&bare).unwrap().len()
    };
    // Budget = bare envelope + sentinel + 4 log lines worth.
    let one_line_bytes = format!("[line 00] {line_payload}").len();
    let budget = bare_envelope_size + 80 /* sentinel */ + 4 * one_line_bytes;

    let truncated = truncate_execution_response(original.clone(), budget, 100_000, 4);

    // 1. Within budget.
    let serialized_len = serde_json::to_vec(&truncated).unwrap().len();
    assert!(
        serialized_len <= budget,
        "truncated response ({serialized_len} bytes) must be within the {budget}-byte budget"
    );

    // 2. Sentinel present (we dropped some lines).
    let sentinel_present = truncated
        .logs
        .iter()
        .any(|l| l.contains("logs truncated to fit response budget"));
    assert!(
        sentinel_present,
        "sentinel must be present when lines were dropped; logs: {:?}",
        truncated.logs
    );

    // 3. Boundary is tight: adding one more original line would exceed budget.
    // The truncated logs are: [sentinel, kept_0, kept_1, …].
    // kept_lines = everything after the sentinel.
    if truncated.logs.len() >= 2 {
        let kept_lines = &truncated.logs[1..];
        let kept_count = kept_lines.len();
        let drop_count = line_count - kept_count;
        // The original lines that were kept start at index `drop_count`.
        if drop_count > 0 {
            let next_original_idx = drop_count - 1;
            let next_original = &original.logs[next_original_idx];
            let mut one_more = truncated.clone();
            let mut new_logs = vec![truncated.logs[0].clone()]; // sentinel
            new_logs.push(next_original.clone());
            new_logs.extend_from_slice(kept_lines);
            one_more.logs = new_logs;
            let one_more_len = serde_json::to_vec(&one_more).unwrap().len();
            assert!(
                one_more_len > budget,
                "adding one more line ({one_more_len} bytes) must exceed budget ({budget}); \
                 boundary must be tight"
            );
        }
    }
}

/// M2 regression: a structured tool error whose `message` CONTAINS A NEWLINE
/// must still classify to its real `kind`. This is the exact case that silently
/// downgraded `upstream_error` → `server_error` under the old `"Error: "` prefix
/// + first-line parse. The bridge now wraps the JSON payload in sentinel markers
/// and the host scans the whole message, so an embedded newline (and any
/// QuickJS-appended stack trace) no longer perturbs recovery.
#[test]
fn classify_rejection_preserves_kind_with_multiline_message() {
    // Build the exact wire shape the JS bridge throws, then simulate QuickJS's
    // `Error.toString()`: an `Error: ` prefix plus an appended stack trace on a
    // following line. The multi-line tool message is the load-bearing part.
    let multiline_message = "upstream failed:\nline two of the detail\nline three";
    let thrown = runner::structured_error_message_for_test("upstream_error", multiline_message);
    let stringified = format!("Error: {thrown}\n    at <anonymous>:1:1\n    at callTool");

    assert_eq!(
        runner::classify_rejection_kind_for_test(&stringified),
        "upstream_error",
        "a structured kind must survive a multi-line message + appended stack trace"
    );

    // Control: a bare runtime throw with no sentinel payload stays server_error.
    assert_eq!(
        runner::classify_rejection_kind_for_test("Error: x is not a function\n    at foo"),
        "server_error",
    );

    // Control: the non-serializable result path still maps to invalid_param.
    assert_eq!(
        runner::classify_rejection_kind_for_test("Code Mode result must be JSON-serializable: ..."),
        "invalid_param",
    );

    // A single-line structured rate_limited error is preserved too.
    let rate_limited = runner::structured_error_message_for_test("rate_limited", "slow down");
    assert_eq!(
        runner::classify_rejection_kind_for_test(&format!("Error: {rate_limited}")),
        "rate_limited",
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

    let receipt = write_code_mode_artifact(root.path(), &request, TEST_MAX_BYTES)
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
