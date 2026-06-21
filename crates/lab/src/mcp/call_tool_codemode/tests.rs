//! Tests for the Code Mode gateway meta-tool helpers. Distributed from
//! `server.rs` (bead `lab-kvji.24.1.6`).

use super::{
    CODE_MODE_DESCRIPTION, code_arg, code_mode_execute_trace, route_scoped_capability_filter,
    string_array_arg,
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
    assert!(CODE_MODE_DESCRIPTION.contains("Retry-safe"));
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
fn code_mode_description_documents_scope_gates_and_write_artifact() {
    // lab-g1tvp: MCP-only scope gates must be stated so a `lab`-scoped agent
    // understands a `forbidden`/`route_scope_denied` result instead of looping.
    assert!(
        CODE_MODE_DESCRIPTION.contains("requires `lab:admin`"),
        "snippet scope gate (codemode.run needs lab:admin) must be documented"
    );
    assert!(
        CODE_MODE_DESCRIPTION.contains("route_scope_denied"),
        "route-scope denial must be documented and distinguished from forbidden"
    );
    // lab-g1tvp: writeArtifact is the purpose-built escape hatch and must be
    // discoverable from the static description.
    assert!(
        CODE_MODE_DESCRIPTION.contains("writeArtifact(path, content, options?)"),
        "writeArtifact must be documented with its signature"
    );
    // lab-g1tvp: the live truncation-marker fields the agent will see.
    assert!(
        CODE_MODE_DESCRIPTION.contains("max_size_bytes")
            && CODE_MODE_DESCRIPTION.contains("max_tokens"),
        "description must mention the live budget fields carried by the marker"
    );
    // lab-g1tvp: confirmation_required is never minted by Code Mode itself.
    assert!(
        CODE_MODE_DESCRIPTION
            .contains("`confirmation_required` is never produced by Code Mode itself"),
        "confirmation_required must be labelled upstream-only / never host-minted"
    );
}

#[test]
fn code_mode_description_lists_sandbox_globals() {
    // lab-8bndv: the available/forbidden globals list must be statically present
    // so an agent that catches per-call errors (the documented JSON.parse idiom)
    // still has the globals list in context.
    assert!(
        CODE_MODE_DESCRIPTION.contains("Sandbox globals:"),
        "a static Sandbox globals stanza must exist"
    );
    for available in [
        "codemode.run",
        "codemode.search",
        "codemode.describe",
        "codemode.step",
        "callTool",
        "writeArtifact",
    ] {
        assert!(
            CODE_MODE_DESCRIPTION.contains(available),
            "available global `{available}` must be listed"
        );
    }
    for forbidden in ["require", "process", "fs", "fetch", "Bun"] {
        assert!(
            CODE_MODE_DESCRIPTION.contains(forbidden),
            "forbidden Node/Deno global `{forbidden}` must be named as unavailable"
        );
    }
}

/// lab-g1tvp validation criterion: the documented error-kind set must be a
/// superset of the kinds Code Mode actually mints. This guards against the
/// description drifting from emitted reality (the original bug).
///
/// The host-minted kinds below are cross-checked against the `sdk_kind: "..."`
/// / `kind: "..."` literals emitted across the `code_mode` module
/// (call_tool_codemode.rs, execute.rs, runner.rs, runner_io.rs,
/// runner_drive.rs, schema.rs, preamble.rs, util.rs, artifacts.rs). The two
/// cross-coordinated kinds (`call_budget_exceeded`, `result_too_large`) are
/// documented ahead of their emitters by design.
#[test]
fn code_mode_description_documents_every_host_minted_kind() {
    const HOST_MINTED_KINDS: &[&str] = &[
        // top-level / per-run
        "forbidden",
        "route_scope_denied",
        "unknown_tool",
        "invalid_param",
        "invalid_code_mode_id",
        "missing_param",
        "timeout",
        "server_error",
        "internal_error",
        "gateway_unavailable",
        "unknown_execution",
        // per-call / snippets / discovery
        "not_found",
        "upstream_error",
        "ambiguous_target",
        "bad_snippet_name",
        "snippet_recursion_limit",
        "snippet_depth_exceeded",
        "snippet_resolve_limit",
        "snippet_budget_exceeded",
        "invalid_snippet_resolution",
        // cross-coordinated (documented ahead of emitters)
        "call_budget_exceeded",
        "result_too_large",
    ];
    for kind in HOST_MINTED_KINDS {
        assert!(
            CODE_MODE_DESCRIPTION.contains(kind),
            "host-minted kind `{kind}` is emitted by the code but absent from CODE_MODE_DESCRIPTION"
        );
    }
}

#[test]
fn codemode_input_schema_is_code_only() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": { "code": { "type": "string", "minLength": 1 } },
        "required": ["code"]
    });
    let props = schema["properties"].as_object().expect("properties object");
    let prop_names: std::collections::BTreeSet<&str> = props.keys().map(String::as_str).collect();
    assert_eq!(prop_names, std::collections::BTreeSet::from(["code"]));
    assert_eq!(schema["properties"]["code"]["minLength"], json!(1));
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
