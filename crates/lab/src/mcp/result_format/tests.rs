//! Tests for result/envelope formatting + error-info extraction + token
//! estimation. Distributed from `server.rs` (bead `lab-kvji.24.1.6`).

use super::{
    estimate_tokens, estimate_tokens_args, estimate_tokens_value, extract_error_info,
    tool_error_envelope,
};
use crate::dispatch::error::ToolError;
use crate::mcp::error::{DispatchError, canonical_kind};
use serde_json::Value;

#[test]
fn estimate_tokens_uses_chars_div_four_heuristic() {
    assert_eq!(estimate_tokens(""), 0);
    // 4 chars → 1 token.
    assert_eq!(estimate_tokens("abcd"), 1);
    // 5 chars → 2 tokens (ceiling).
    assert_eq!(estimate_tokens("abcde"), 2);
    assert_eq!(estimate_tokens("hello world"), 3);
}

#[test]
fn estimate_tokens_value_serializes_first() {
    // Value's serialized form is `{"a":1}` (7 chars) → 2 tokens.
    let v = serde_json::json!({"a": 1});
    assert_eq!(estimate_tokens_value(&v), 2);
}

#[test]
fn estimate_tokens_args_handles_empty_and_populated_maps() {
    let empty: serde_json::Map<String, Value> = serde_json::Map::new();
    // "{}" → 2 chars → 1 token.
    assert_eq!(estimate_tokens_args(&empty), 1);

    let mut populated = serde_json::Map::new();
    populated.insert("name".into(), Value::String("code_mode".into()));
    // `{"name":"code_mode"}` is 20 chars → 5 tokens.
    assert_eq!(estimate_tokens_args(&populated), 5);
}

#[tokio::test]
async fn extract_error_info_preserves_unknown_action_from_real_dispatch_downcast() {
    let err = crate::dispatch::lab_admin::dispatch("definitely.unknown", serde_json::json!({}))
        .await
        .expect_err("unknown lab_admin action should fail");
    let dispatch_error = DispatchError::from(err);
    let anyhow_error = anyhow::Error::from(dispatch_error);

    let (kind, message, extra) = extract_error_info(&anyhow_error);

    assert_eq!(kind, "unknown_action");
    assert_eq!(message, "unknown action `lab_admin.definitely.unknown`");
    let extra = extra.expect("unknown_action should preserve valid action extras");
    assert_eq!(extra["valid"][0], "help");
    assert_eq!(extra["param"], Value::Null);
    assert_eq!(extra["hint"], Value::Null);
}

#[test]
fn extract_error_info_preserves_unknown_action_from_json_fallback() {
    let serialized = serde_json::json!({
        "kind": "unknown_action",
        "message": "unknown action `movie.serch` for service `radarr`",
        "valid": ["movie.search", "movie.add"],
        "hint": "movie.search"
    })
    .to_string();
    let anyhow_error = anyhow::anyhow!(serialized);

    let (kind, message, extra) = extract_error_info(&anyhow_error);

    assert_eq!(kind, "unknown_action");
    assert_eq!(message, "unknown action `movie.serch` for service `radarr`");
    let extra = extra.expect("json fallback should preserve structured extras");
    assert_eq!(
        extra["valid"],
        serde_json::json!(["movie.search", "movie.add"])
    );
    assert_eq!(extra["param"], Value::Null);
    assert_eq!(extra["hint"], serde_json::json!("movie.search"));
}

/// Every kind that `ToolError::kind()` can return must have an explicit arm
/// in `canonical_kind()`.  If a new variant or SDK kind is added to `ToolError`
/// without a matching arm here, this test will catch the silent downgrade to
/// `"internal_error"`.
#[test]
fn canonical_kind_round_trips_all_tool_error_kinds() {
    // Fixed-variant kinds — produced by the named ToolError variants.
    let fixed_variants: &[ToolError] = &[
        ToolError::UnknownAction {
            message: String::new(),
            valid: vec![],
            hint: None,
        },
        ToolError::MissingParam {
            message: String::new(),
            param: "p".into(),
        },
        ToolError::InvalidParam {
            message: String::new(),
            param: "p".into(),
        },
        ToolError::UnknownInstance {
            message: String::new(),
            valid: vec![],
        },
    ];

    for err in fixed_variants {
        let kind = err.kind();
        assert_eq!(
            canonical_kind(kind),
            kind,
            "canonical_kind({kind:?}) should round-trip but returns \"{}\"",
            canonical_kind(kind),
        );
    }

    // SDK-promoted kinds — every stable kind tag that `ApiError::kind()` can
    // return and that `ToolError::Sdk` promotes to the top-level `kind` field.
    let sdk_kinds: &[&str] = &[
        "unknown_action",
        "unknown_subaction",
        "missing_param",
        "invalid_param",
        "unknown_instance",
        "auth_failed",
        "not_found",
        "rate_limited",
        "validation_failed",
        "network_error",
        "server_error",
        "decode_error",
        "confirmation_required",
    ];

    for &sdk_kind in sdk_kinds {
        let err = ToolError::Sdk {
            sdk_kind: sdk_kind.to_string(),
            message: String::new(),
        };
        let kind = err.kind();
        assert_eq!(
            canonical_kind(kind),
            kind,
            "canonical_kind({kind:?}) should round-trip but returns \"{}\"",
            canonical_kind(kind),
        );
    }
}

#[test]
fn tool_error_envelope_preserves_structured_extras() {
    let err = ToolError::MissingParam {
        message: "query is required".to_string(),
        param: "query".to_string(),
    };

    let envelope = tool_error_envelope("code_search", "call_tool", &err);

    assert_eq!(
        envelope.pointer("/error/kind"),
        Some(&Value::from("missing_param"))
    );
    assert_eq!(
        envelope.pointer("/error/param"),
        Some(&Value::from("query"))
    );
}
