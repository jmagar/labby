//! Tests for the Code Mode gateway meta-tool helpers. Distributed from
//! `server.rs` (bead `lab-kvji.24.1.6`).

use super::{
    CODE_MODE_DESCRIPTION, code_arg, code_mode_execute_trace, route_scoped_capability_filter,
    string_array_arg,
};
use crate::config::CodeModeResultShapePolicy;
use labby_codemode::{
    CodeModeExecutedCall, CodeModeExecutionResponse, CodeModeResultShapeMetadata, MAX_SOURCE_BYTES,
};
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
fn code_arg_rejects_missing_or_blank_code() {
    let args = serde_json::Map::new();
    let err = code_arg(&args).expect_err("missing code must be invalid");
    assert_eq!(err.kind(), "invalid_param");

    let mut args = serde_json::Map::new();
    args.insert("code".to_string(), Value::String("  \n\t ".to_string()));
    let err = code_arg(&args).expect_err("blank code must be invalid");
    assert_eq!(err.kind(), "invalid_param");
}

#[test]
fn code_arg_source_limit_is_shared_const_boundary() {
    let mut args = serde_json::Map::new();
    args.insert(
        "code".to_string(),
        Value::String("a".repeat(MAX_SOURCE_BYTES)),
    );
    assert!(code_arg(&args).is_ok());

    let mut args = serde_json::Map::new();
    args.insert(
        "code".to_string(),
        Value::String("a".repeat(MAX_SOURCE_BYTES + 1)),
    );
    let err = code_arg(&args).expect_err("over-limit code must be invalid");
    assert_eq!(err.kind(), "invalid_param");
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
fn code_mode_description_contains_protocol_contract() {
    // Source of truth: docs/contracts/CODE_NODE_CONTRACT_FOR_RETARD_AGENTS.md
    // Full spec:       docs/specs/CODE_MODE_SPEC_FOR_RETARD_AGENTS.md
    assert!(CODE_MODE_DESCRIPTION.contains("callTool<T = unknown>"));
    assert!(
        CODE_MODE_DESCRIPTION.contains("Successful return: the upstream tool's structuredContent")
    );
    assert!(CODE_MODE_DESCRIPTION.contains("JSON.parse(String(e.message))"));
    assert!(CODE_MODE_DESCRIPTION.contains("Retry-safe:"));
    assert!(CODE_MODE_DESCRIPTION.contains("Promise.all"));
    assert!(
        CODE_MODE_DESCRIPTION.contains("codemode"),
        "description must explain the codemode typed helper namespace"
    );
    assert!(
        CODE_MODE_DESCRIPTION.contains("codemode.search()"),
        "description must make in-sandbox discovery primary"
    );
    assert!(
        !CODE_MODE_DESCRIPTION.contains("search.dts"),
        "description must not imply primary codemode discovery returns legacy dts"
    );
    assert!(
        !CODE_MODE_DESCRIPTION.contains("For Lab built-in actions use the `execute` tool"),
        "description must not point codemode callers at a removed execute tool"
    );
    assert!(CODE_MODE_DESCRIPTION.len() < 8192);
}

#[test]
fn codemode_input_schema_includes_optional_filters() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "code": { "type": "string", "minLength": 1 },
            "upstreams": { "type": "array", "items": { "type": "string" } },
            "tools": { "type": "array", "items": { "type": "string" } }
        },
        "required": ["code"]
    });
    let props = schema["properties"].as_object().expect("properties object");
    let prop_names: std::collections::BTreeSet<&str> = props.keys().map(String::as_str).collect();
    assert_eq!(
        prop_names,
        std::collections::BTreeSet::from(["code", "tools", "upstreams"])
    );
    assert_eq!(schema["properties"]["code"]["minLength"], json!(1));
    assert_eq!(
        schema["properties"]["upstreams"]["items"]["type"],
        json!("string")
    );
    assert_eq!(
        schema["properties"]["tools"]["items"]["type"],
        json!("string")
    );
}

#[test]
fn code_mode_execute_trace_includes_shape_metadata_and_shaped_result() {
    let response = CodeModeExecutionResponse {
        execution_id: None,
        result: Some(json!("[code mode result truncated]\n{}")),
        result_shaping: Some(CodeModeResultShapeMetadata {
            policy: CodeModeResultShapePolicy::Truncate,
            changed: true,
            truncated: true,
            original_size_bytes: 5000,
            shaped_size_bytes: 256,
        }),
        ui: None,
        calls: vec![],
        logs: vec![],
        artifacts: vec![],
    };

    let text_json = serde_json::to_value(&response).expect("response serializes");
    let structured_json = code_mode_execute_trace(&response);

    assert_eq!(
        text_json.get("result"),
        structured_json.get("result"),
        "MCP text JSON and structuredContent must use the same shaped result"
    );
    assert_eq!(
        text_json.get("result_shaping"),
        structured_json.get("result_shaping"),
        "MCP text JSON and structuredContent must expose the same shaping metadata"
    );
    assert!(text_json.get("result_shape").is_none());
    assert_eq!(structured_json["result_shape"]["type"], json!("string"));
    assert_eq!(
        structured_json["result_shaping"]["policy"],
        json!("truncate")
    );
    assert_eq!(structured_json["result_shaping"]["changed"], json!(true));
    assert_eq!(structured_json["result_shaping"]["truncated"], json!(true));
    assert_eq!(
        structured_json["result_shaping"]["original_size_bytes"],
        json!(5000)
    );
    assert_eq!(
        structured_json["result_shaping"]["shaped_size_bytes"],
        json!(256)
    );
    assert!(structured_json["result_shape"].get("policy").is_none());
    assert!(structured_json["result_shape"].get("truncated").is_none());
}

#[test]
fn execute_trace_embeds_result_and_redacts_call_params() {
    let response = CodeModeExecutionResponse {
        execution_id: None,
        ui: None,
        result: Some(json!({
            "answer": "the full research answer the model asked for",
            "items": ["a", "b", "c"]
        })),
        result_shaping: None,
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
    // Neutral vocabulary after the lab-codemode extraction: the call trace
    // namespace field is `namespace` (was `upstream`).
    assert_eq!(trace["calls"][0]["namespace"], json!("github"));
    assert_eq!(trace["calls"][0]["tool"], json!("search_issues"));
    // Per-call params remain redacted — that is the secret-bearing channel.
    assert_eq!(trace["calls"][0]["params"]["token"], json!("[redacted]"));

    // The real return value is now embedded verbatim so structured-content-only
    // clients (e.g. Claude Code) actually receive it, not just its shape. The
    // value is already response-budget-capped upstream by
    // `truncate_execution_response`, so it is not re-truncated here.
    assert_eq!(
        trace["result"]["answer"],
        json!("the full research answer the model asked for")
    );
    assert_eq!(trace["result"]["items"], json!(["a", "b", "c"]));

    // result_shape is retained for the inline UI app / quick inspection.
    assert_eq!(trace["result_shape"]["type"], json!("object"));
    assert_eq!(trace["result_shape"]["key_count"], json!(2));
}

#[test]
fn execute_trace_omits_result_when_function_returns_undefined() {
    let response = CodeModeExecutionResponse {
        execution_id: None,
        ui: None,
        result: None,
        result_shaping: None,
        calls: vec![],
        logs: vec![],
        artifacts: vec![],
    };

    let trace = code_mode_execute_trace(&response);
    // `undefined` return omits the field entirely (parity with the response
    // envelope), and the shape descriptor reports `"undefined"`.
    assert!(
        trace.get("result").is_none(),
        "an undefined return must omit `result`, not emit null"
    );
    assert_eq!(trace["result_shape"]["type"], json!("undefined"));
    assert_eq!(trace["logs_count"], json!(0));
}

#[test]
fn execute_trace_preserves_explicit_null_result() {
    let response = CodeModeExecutionResponse {
        execution_id: None,
        ui: None,
        result: Some(Value::Null),
        result_shaping: None,
        calls: vec![],
        logs: vec![],
        artifacts: vec![],
    };

    let trace = code_mode_execute_trace(&response);
    // Explicit JS `null` is distinct from `undefined`: the field is present and
    // null, matching the response envelope's null-vs-undefined contract.
    assert!(
        trace.get("result").is_some(),
        "explicit null must emit `result`, not omit it"
    );
    assert!(trace["result"].is_null());
    assert_eq!(trace["result_shape"]["type"], json!("null"));
}
