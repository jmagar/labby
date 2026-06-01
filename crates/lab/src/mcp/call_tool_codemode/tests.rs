//! Tests for the Code Mode gateway meta-tool helpers. Distributed from
//! `server.rs` (bead `lab-kvji.24.1.6`).

use super::{CODE_EXECUTE_DESCRIPTION, string_array_arg};
use serde_json::Value;

#[test]
fn code_mode_filter_arg_rejects_malformed_values() {
    let mut args = serde_json::Map::new();
    args.insert(
        "tools".to_string(),
        Value::String("upstream::github::search_issues".to_string()),
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
    for schema in [serde_json::json!({
        "type": "object",
        "properties": { "code": { "type": "string" } },
        "required": ["code"]
    })] {
        let props = schema["properties"].as_object().expect("properties object");
        let prop_names: std::collections::BTreeSet<&str> =
            props.keys().map(String::as_str).collect();
        assert_eq!(prop_names, std::collections::BTreeSet::from(["code"]));
    }
}
