//! Tests: tool-id parsing, tool scope, schema validation, tool descriptor.
#![cfg(test)]
#![allow(clippy::panic)]

use std::collections::BTreeMap;

use serde_json::json;

use crate::error::ToolError;
use crate::snippet::store::{SnippetInfo, SnippetInputSpec, SnippetInputType, SnippetSource};
use crate::types::{CodeModeToolId, CodeModeToolRef, ToolDescriptor, ToolScope};

use super::protocol::CodeModeRunnerOutput;
use super::schema::validate_code_mode_params_against_schema;

#[test]
fn local_provider_ids_are_detected_before_upstream_ids() {
    let state = crate::local_provider::try_parse_local_provider_call("state::readFile")
        .expect("parse succeeds")
        .expect("state provider detected");
    assert_eq!(state.provider.as_str(), "state");
    assert_eq!(state.method, "readFile");

    let git = crate::local_provider::try_parse_local_provider_call("git::status")
        .expect("parse succeeds")
        .expect("git provider detected");
    assert_eq!(git.provider.as_str(), "git");
    assert_eq!(git.method, "status");

    assert!(
        crate::local_provider::try_parse_local_provider_call("movie::search")
            .expect("ordinary upstream id is valid")
            .is_none()
    );
}

#[test]
fn local_provider_ids_reject_bad_methods() {
    let err = crate::local_provider::try_parse_local_provider_call("state::")
        .expect_err("empty local method is rejected");
    assert_eq!(err.kind(), "invalid_param");
}

#[test]
fn artifact_write_protocol_round_trips() {
    let output = CodeModeRunnerOutput::ArtifactWrite {
        seq: 7,
        path: "axon/brief.md".to_string(),
        content: "# Brief".to_string(),
        content_type: Some("text/markdown".to_string()),
    };

    let encoded = serde_json::to_string(&output).expect("serialize protocol");
    assert_eq!(
        encoded,
        r##"{"type":"artifact_write","seq":7,"path":"axon/brief.md","content":"# Brief","content_type":"text/markdown"}"##
    );

    let decoded: CodeModeRunnerOutput =
        serde_json::from_str(&encoded).expect("deserialize protocol");
    assert_eq!(decoded, output);
}

#[test]
fn snippet_catalog_entry_projects_to_codemode_run() {
    let info = SnippetInfo {
        name: "repo-summary".to_string(),
        description: Some("Summarize repo health".to_string()),
        tags: vec!["repo".to_string()],
        inputs: Default::default(),
        source: SnippetSource::User,
        path: "repo-summary.md".into(),
        shadowed: false,
    };
    let entry = ToolDescriptor::snippet(&info);
    assert_eq!(entry.kind, crate::types::CodeModeCatalogKind::Snippet);
    assert_eq!(entry.id, "snippet::repo-summary");
    assert_eq!(entry.namespace, "snippet");
    assert!(entry.signature.contains("codemode.run"));
    assert!(entry.dts.is_empty());

    let discovery = crate::types::CodeModeDiscoveryEntry::from_catalog(&entry);
    assert_eq!(discovery.kind, crate::types::CodeModeCatalogKind::Snippet);
    assert_eq!(discovery.path, "snippet.repo-summary");
    assert_eq!(discovery.helper, "codemode.run(\"repo-summary\", input)");
}

#[test]
fn snippet_catalog_json_input_schema_allows_any_json_value() {
    let mut inputs = BTreeMap::new();
    inputs.insert(
        "payload".to_string(),
        SnippetInputSpec {
            ty: SnippetInputType::Json,
            required: true,
            default: None,
            description: Some("Raw payload".to_string()),
        },
    );
    let info = SnippetInfo {
        name: "json-snippet".to_string(),
        description: None,
        tags: Vec::new(),
        inputs,
        source: SnippetSource::User,
        path: "json-snippet.md".into(),
        shadowed: false,
    };

    let entry = ToolDescriptor::snippet(&info);
    let schema = entry.schema.expect("snippet schema");
    let payload = &schema["properties"]["payload"];
    assert!(payload.get("type").is_none(), "{payload}");
    assert_eq!(payload["description"], "Raw payload");
    assert_eq!(schema["required"], json!(["payload"]));
}

#[test]
fn parse_rejects_lab_id() {
    let err =
        CodeModeToolId::parse("lab::radarr.movie.search").expect_err("lab:: ids are rejected");
    match err {
        ToolError::Sdk { sdk_kind, message } => {
            assert_eq!(sdk_kind, "unknown_tool");
            assert!(message.contains("lab::"));
            // Message points callers at the native Lab service tool, not back
            // through Code Mode.
            assert!(message.contains("native Lab service tool"));
            assert!(message.contains("radarr"));
        }
        other => panic!("expected unknown_tool, got {other:?}"),
    }
}

#[test]
fn parses_namespaced_tool_id() {
    let parsed = CodeModeToolId::parse("github::search_issues").unwrap();
    assert_eq!(
        parsed,
        CodeModeToolId {
            raw: "github::search_issues".to_string(),
            reference: CodeModeToolRef::Tool {
                namespace: "github".to_string(),
                tool: "search_issues".to_string(),
            },
        }
    );
}

#[test]
fn rejects_invalid_ids() {
    for id in [
        "",
        "a.a.schema",
        "lab::native",
        "github",
        "::tool",
        "ns::github::search_issues",
    ] {
        assert!(CodeModeToolId::parse(id).is_err(), "{id} should be invalid");
    }
}

#[test]
fn tool_scope_allows_only_selected_namespaces_and_tools() {
    let filter = ToolScope::new(
        vec!["github".to_string()],
        vec!["github::search_issues".to_string()],
    );

    assert!(filter.allows("github", "search_issues"));
    assert!(!filter.allows("github", "delete_repo"));
    assert!(!filter.allows("docker", "search_issues"));
}

#[test]
fn capability_filter_fingerprint_is_structured_and_collision_resistant() {
    let first = ToolScope::new(
        vec!["a,b".to_string(), "c".to_string()],
        vec!["x".to_string()],
    );
    let second = ToolScope::new(
        vec!["a".to_string(), "b,c".to_string()],
        vec!["x".to_string()],
    );

    assert_ne!(first.fingerprint(), second.fingerprint());
    assert!(serde_json::from_str::<serde_json::Value>(&first.fingerprint()).is_ok());
}

#[test]
fn scoped_tool_scope_with_empty_namespaces_denies_all_tool_calls() {
    let filter = ToolScope::scoped_namespaces(Vec::new(), Vec::new());

    assert!(!filter.allows("github", "search_issues"));
    assert!(!filter.allows("docker", "containers"));
}

#[test]
fn validates_code_mode_params_against_input_schema() {
    let schema = json!({
        "type": "object",
        "properties": {
            "query": { "type": "string" },
            "limit": { "type": "integer" }
        },
        "required": ["query"]
    });

    validate_code_mode_params_against_schema(&json!({"query": "rust", "limit": 10}), Some(&schema))
        .expect("valid params pass");

    let missing = validate_code_mode_params_against_schema(&json!({}), Some(&schema))
        .expect_err("missing required field fails");
    assert_eq!(missing.kind(), "missing_param");

    let invalid = validate_code_mode_params_against_schema(&json!({"query": 42}), Some(&schema))
        .expect_err("wrong field type fails");
    assert_eq!(invalid.kind(), "invalid_param");
}

#[test]
fn validates_code_mode_params_recursively_against_schema() {
    let schema = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "state": { "enum": ["open", "closed"] },
            "limit": { "type": ["integer", "null"], "minimum": 1, "maximum": 100 },
            "labels": { "type": "array", "items": { "type": "string" } },
            "owner": {
                "type": "object",
                "properties": { "login": { "type": "string" } },
                "required": ["login"],
                "additionalProperties": false
            }
        },
        "required": ["state", "owner"]
    });

    validate_code_mode_params_against_schema(
        &json!({
            "state": "open",
            "limit": null,
            "labels": ["bug"],
            "owner": {"login": "octo"}
        }),
        Some(&schema),
    )
    .expect("valid nested params pass");

    for params in [
        json!({"state": "merged", "owner": {"login": "octo"}}),
        json!({"state": "open", "owner": {"login": "octo", "extra": true}}),
        json!({"state": "open", "owner": {}, "labels": ["bug"]}),
        json!({"state": "open", "owner": {"login": "octo"}, "labels": [1]}),
        json!({"state": "open", "owner": {"login": "octo"}, "limit": 0}),
        json!({"state": "open", "owner": {"login": "octo"}, "extra": true}),
    ] {
        let err = validate_code_mode_params_against_schema(&params, Some(&schema))
            .expect_err("invalid nested params fail");
        assert_eq!(err.kind(), "invalid_param", "{params}");
    }
}

#[test]
fn validates_code_mode_params_through_local_refs_and_constraints() {
    let schema = json!({
        "$ref": "#/$defs/Params",
        "$defs": {
            "Params": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "minLength": 2,
                        "maxLength": 5,
                        "pattern": "^[a-z]+$"
                    },
                    "tags": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 2,
                        "uniqueItems": true,
                        "items": { "type": "string" }
                    },
                    "meta": {
                        "type": "object",
                        "properties": {
                            "known": { "type": "string" }
                        },
                        "additionalProperties": { "type": "integer" }
                    },
                    "flag": {
                        "oneOf": [
                            { "type": "string", "const": "on" },
                            { "type": "boolean" }
                        ]
                    },
                    "labels": {
                        "type": "object",
                        "patternProperties": {
                            "^x-": { "type": "string" }
                        },
                        "additionalProperties": false
                    }
                },
                "required": ["query", "tags", "flag"]
            }
        }
    });

    validate_code_mode_params_against_schema(
        &json!({
            "query": "abc",
            "tags": ["one", "two"],
            "meta": {"known": "ok", "count": 1},
            "flag": true,
            "labels": {"x-owner": "me"}
        }),
        Some(&schema),
    )
    .expect("valid params through local ref pass");

    for params in [
        json!({"tags": ["one"], "flag": true}),
        json!({"query": "a", "tags": ["one"], "flag": true}),
        json!({"query": "abcdef", "tags": ["one"], "flag": true}),
        json!({"query": "ABC", "tags": ["one"], "flag": true}),
        json!({"query": "abc", "tags": [], "flag": true}),
        json!({"query": "abc", "tags": ["one", "two", "three"], "flag": true}),
        json!({"query": "abc", "tags": ["one", "one"], "flag": true}),
        json!({"query": "abc", "tags": ["one"], "flag": 1}),
        json!({"query": "abc", "tags": ["one"], "flag": true, "meta": {"count": "one"}}),
        json!({"query": "abc", "tags": ["one"], "flag": true, "labels": {"owner": "me"}}),
    ] {
        let err = validate_code_mode_params_against_schema(&params, Some(&schema))
            .expect_err("invalid params fail through local ref");
        assert!(
            matches!(err.kind(), "missing_param" | "invalid_param"),
            "{params}: {err}"
        );
    }
}

#[test]
fn builds_catalog_entry_for_tool() {
    let candidate = ToolDescriptor::tool(
        "github",
        "search_issues",
        "Search issues",
        Some(json!({
            "type": "object",
            "properties": {
                "q": {
                    "type": "string",
                    "description": "Search query"
                }
            },
            "required": ["q"]
        })),
        Some(json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "title": { "type": "string" }
                        }
                    }
                }
            }
        })),
    );
    assert_eq!(candidate.id, "github::search_issues");
    assert_eq!(candidate.namespace, "github");
    assert_eq!(candidate.name, "search_issues");
    assert_eq!(
        candidate.output_schema,
        Some(json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "title": { "type": "string" }
                        }
                    }
                }
            }
        }))
    );
    assert!(
        candidate
            .signature
            .contains("codemode.github.search_issues")
    );
    assert!(candidate.signature.contains("GithubSearchIssuesInput"));
    assert!(candidate.signature.contains("GithubSearchIssuesOutput"));
    assert!(candidate.dts.contains("type GithubSearchIssuesInput"));
    assert!(candidate.dts.contains("/** Search query */"));
    assert!(candidate.dts.contains("q: string;"));
    assert!(candidate.dts.contains("title?: string;"));
    assert!(
        candidate
            .dts
            .contains("declare function callTool(id: \"github::search_issues\"")
    );
}
