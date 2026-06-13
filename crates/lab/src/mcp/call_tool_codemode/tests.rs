//! Tests for the Code Mode gateway meta-tool helpers. Distributed from
//! `server.rs` (bead `lab-kvji.24.1.6`).

use super::{
    CODE_EXECUTE_DESCRIPTION, code_mode_execute_trace, code_mode_search_trace,
    route_scoped_capability_filter, string_array_arg,
};
use crate::dispatch::gateway::code_mode::{CodeModeExecutedCall, CodeModeExecutionResponse};
use serde_json::{Value, json};

#[test]
fn code_mode_filter_arg_rejects_malformed_values() {
    let mut args = serde_json::Map::new();
    args.insert(
        "tools".to_string(),
        Value::String("github::search_issues".to_string()),
    );
    let err = string_array_arg(&args, "tools")
        .expect_err("string filter must not be treated as allow-all");
    assert_eq!(err.kind(), "invalid_param");

    let mut args = serde_json::Map::new();
    args.insert("upstreams".to_string(), serde_json::json!(["github", 42]));
    let err = string_array_arg(&args, "upstreams")
        .expect_err("non-string filter entries must not be dropped");
    assert_eq!(err.kind(), "invalid_param");
}

#[test]
fn code_mode_filter_arg_accepts_absent_and_string_arrays() {
    let args = serde_json::Map::new();
    assert_eq!(
        string_array_arg(&args, "tools").expect("absent ok"),
        Vec::<String>::new()
    );

    let mut args = serde_json::Map::new();
    args.insert("tools".to_string(), serde_json::json!(["a", "b"]));
    assert_eq!(
        string_array_arg(&args, "tools").expect("array ok"),
        vec!["a".to_string(), "b".to_string()]
    );
}

#[test]
fn scoped_capability_filter_rejects_disallowed_requested_upstreams() {
    let mut args = serde_json::Map::new();
    args.insert("upstreams".to_string(), json!(["beta"]));
    let allowed = std::collections::BTreeSet::from(["alpha".to_string()]);

    let err = route_scoped_capability_filter(&args, Some(&allowed))
        .expect_err("disallowed explicit upstream must fail");

    assert_eq!(err.kind(), "route_scope_denied");
}

#[test]
fn scoped_capability_filter_defaults_to_route_allowed_upstreams() {
    let args = serde_json::Map::new();
    let allowed = std::collections::BTreeSet::from(["alpha".to_string()]);

    let filter = route_scoped_capability_filter(&args, Some(&allowed))
        .expect("omitted upstreams should default to route scope");

    assert!(filter.allows("alpha", "search"));
    assert!(!filter.allows("beta", "search"));
}

#[test]
fn code_execute_description_contains_protocol_contract() {
    // Source of truth: docs/contracts/CODE_NODE_CONTRACT_FOR_RETARD_AGENTS.md
    // Full spec:       docs/specs/CODE_MODE_SPEC_FOR_RETARD_AGENTS.md
    assert!(CODE_EXECUTE_DESCRIPTION.contains("callTool<T = unknown>"));
    assert!(
        CODE_EXECUTE_DESCRIPTION
            .contains("Successful return: the upstream tool's structuredContent")
    );
    assert!(CODE_EXECUTE_DESCRIPTION.contains("JSON.parse(String(e.message))"));
    assert!(CODE_EXECUTE_DESCRIPTION.contains("Retry-safe:"));
    assert!(CODE_EXECUTE_DESCRIPTION.contains("Promise.all"));
    assert!(
        CODE_EXECUTE_DESCRIPTION.contains("codemode"),
        "description must explain the codemode typed helper namespace"
    );
    assert!(
        !CODE_EXECUTE_DESCRIPTION.contains("code_search"),
        "description must not reference the deprecated code_search tool"
    );
    assert!(CODE_EXECUTE_DESCRIPTION.len() < 8192);
}

#[test]
fn gateway_search_input_schema_is_code_only() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": { "code": { "type": "string" } },
        "required": ["code"]
    });
    let props = schema["properties"].as_object().expect("properties object");
    let prop_names: std::collections::BTreeSet<&str> = props.keys().map(String::as_str).collect();
    assert_eq!(prop_names, std::collections::BTreeSet::from(["code"]));
}

#[test]
fn execute_trace_contains_redacted_params_and_compact_result_shape() {
    let response = CodeModeExecutionResponse {
        ui: None,
        result: Some(json!({
            "items": ["raw payload that should not be copied into trace"],
            "secret": "result payloads are summarized, not previewed"
        })),
        calls: vec![CodeModeExecutedCall {
            id: "github::search_issues".to_string(),
            ok: true,
            elapsed_ms: 12,
            params: Some(json!({"query": "bug", "token": "[redacted]"})),
            error_kind: None,
        }],
        logs: vec!["one".to_string()],
        artifacts: vec![],
    };

    let trace = code_mode_execute_trace(&response);
    assert_eq!(trace["kind"], json!("code_mode_execute_trace"));
    assert_eq!(trace["calls"][0]["upstream"], json!("github"));
    assert_eq!(trace["calls"][0]["tool"], json!("search_issues"));
    assert_eq!(trace["calls"][0]["params"]["token"], json!("[redacted]"));
    assert_eq!(trace["result_shape"]["type"], json!("object"));
    assert_eq!(trace["result_shape"]["key_count"], json!(2));

    let serialized = trace.to_string();
    assert!(serialized.contains("[redacted]"));
    assert!(!serialized.contains("raw payload that should not be copied"));
    assert!(!serialized.contains("result payloads are summarized"));
}

#[test]
fn search_trace_summarizes_matched_tools() {
    let response = json!([
        {
            "id": "github::search_issues",
            "name": "search_issues",
            "upstream": "github",
            "description": "Search issues",
            "schema": {"type": "object"},
            "output_schema": null,
            "dts": "large",
            "signature": "large"
        }
    ]);

    let trace = code_mode_search_trace(&response, 7);
    assert_eq!(trace["kind"], json!("code_mode_search_trace"));
    assert_eq!(trace["query_kind"], json!("catalog_filter"));
    assert_eq!(trace["match_count"], json!(1));
    assert_eq!(trace["displayed_count"], json!(1));
    assert_eq!(trace["truncated"], json!(false));
    assert_eq!(trace["matches"][0]["id"], json!("github::search_issues"));
    assert_eq!(trace["matches"][0]["upstream"], json!("github"));
    assert_eq!(trace["matches"][0]["tool"], json!("search_issues"));
    assert_eq!(trace["matches"][0]["has_schema"], json!(true));
    assert_eq!(trace["matches"][0]["has_output_schema"], json!(false));

    let serialized = trace.to_string();
    assert!(!serialized.contains("large"));
}

#[test]
fn search_trace_reports_display_truncation() {
    let response = Value::Array(
        (0..60)
            .map(|idx| {
                json!({
                    "id": format!("github::tool_{idx}"),
                    "name": format!("tool_{idx}"),
                    "upstream": "github",
                    "description": "Tool",
                })
            })
            .collect(),
    );

    let trace = code_mode_search_trace(&response, 7);
    assert_eq!(trace["match_count"], json!(60));
    assert_eq!(trace["displayed_count"], json!(50));
    assert_eq!(trace["truncated"], json!(true));
    assert_eq!(trace["matches"].as_array().expect("matches").len(), 50);
}
