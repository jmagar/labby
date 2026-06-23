//! Tests for `ts_signatures` (TypeScript type generation from JSON Schema).
#![cfg(test)]

use serde_json::json;

#[test]
fn json_schema_to_type_handles_refs_unions_arrays_and_required_properties() {
    let schema = json!({
        "$defs": {
            "Issue": {
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "number": { "type": "integer" }
                },
                "required": ["title"]
            }
        },
        "type": "object",
        "properties": {
            "items": {
                "type": "array",
                "items": { "$ref": "#/$defs/Issue" }
            },
            "state": {
                "enum": ["open", "closed"]
            },
            "cursor": {
                "anyOf": [{ "type": "string" }, { "type": "null" }]
            }
        },
        "required": ["items"]
    });

    let ts = super::ts_signatures::json_schema_to_type(Some(&schema));

    assert!(ts.contains("items: Array<{"));
    assert!(ts.contains("title: string;"));
    assert!(ts.contains("number?: number;"));
    assert!(ts.contains("state?: \"open\" | \"closed\";"));
    assert!(ts.contains("cursor?: string | null;"));
}

#[test]
fn json_schema_to_type_matches_cloudflare_edge_cases() {
    let schema = json!({
        "type": "object",
        "properties": {
            "tuple": {
                "type": "array",
                "items": [{ "type": "string" }, { "type": "integer" }]
            },
            "exact": {
                "type": "object",
                "additionalProperties": false
            },
            "when": {
                "type": "string",
                "format": "date-time",
                "description": "Timestamp"
            },
            "anything": true,
            "nothing": false
        }
    });

    let ts = super::ts_signatures::json_schema_to_type(Some(&schema));

    assert!(ts.contains("tuple?: [string, number];"), "{ts}");
    assert!(ts.contains("exact?: Record<string, never>;"), "{ts}");
    assert!(ts.contains("* Timestamp"), "{ts}");
    assert!(ts.contains("* @format date-time"), "{ts}");
    assert!(ts.contains("anything?: unknown;"), "{ts}");
    assert!(ts.contains("nothing?: never;"), "{ts}");
}

#[test]
fn json_schema_to_type_maps_binary_strings_to_runtime_buffer_types() {
    let schema = json!({
        "type": "object",
        "properties": {
            "payload": {
                "type": "string",
                "format": "binary"
            }
        },
        "required": ["payload"]
    });

    let ts = super::ts_signatures::json_schema_to_type(Some(&schema));

    assert!(ts.contains("payload: Uint8Array | ArrayBuffer;"), "{ts}");
}

#[test]
fn json_schema_to_type_does_not_emit_conflicting_index_signatures() {
    let schema = json!({
        "type": "object",
        "properties": {
            "id": { "type": "string" },
            "count": { "type": "integer" }
        },
        "required": ["id"],
        "additionalProperties": { "type": "number" }
    });

    let ts = super::ts_signatures::json_schema_to_type(Some(&schema));

    assert!(ts.contains("id: string;"), "{ts}");
    assert!(ts.contains("count?: number;"), "{ts}");
    assert!(
        ts.contains("* Additional properties match: number"),
        "additional properties should preserve the schema type in documentation: {ts}"
    );
    assert!(
        ts.contains("[key: string]: number | string | undefined;"),
        "additional properties should be widened to avoid conflicting with explicit properties: {ts}"
    );
    assert!(!ts.contains("[key: string]: number;"), "{ts}");
}

#[test]
fn generate_tool_types_emits_composable_codemode_declarations() {
    let first = super::ts_signatures::generate_tool_types(
        "github",
        "list_tags",
        "List tags",
        Some(&json!({"type": "object"})),
        None,
    );
    let second = super::ts_signatures::generate_tool_types(
        "github",
        "create_issue",
        "Create issue",
        Some(&json!({"type": "object"})),
        None,
    );
    let combined = format!("{}\n{}", first.dts, second.dts);

    assert!(!combined.contains("declare const codemode"), "{combined}");
    assert_eq!(combined.matches("declare var codemode").count(), 2);
    assert!(combined.contains("interface CodemodeGithubTools"));
    assert!(combined.contains("list_tags(params:"), "{combined}");
    assert!(combined.contains("create_issue(params:"), "{combined}");
}

#[test]
fn generate_tool_types_quotes_sanitized_namespace_and_method_names() {
    let types = super::ts_signatures::generate_tool_types(
        "github chat",
        "list tags",
        "List tags",
        Some(&json!({"type": "object"})),
        None,
    );

    assert!(
        types.signature.contains("codemode.github_chat.list_tags"),
        "{types:?}"
    );
    assert!(types.dts.contains("github_chat"), "{types:?}");
    assert!(types.dts.contains("list_tags(params:"), "{types:?}");
    assert!(!types.dts.contains("github chat: {"), "{types:?}");
    assert!(!types.dts.contains("list tags(params:"), "{types:?}");
}

#[test]
fn generate_tool_types_sanitizes_reserved_digits_empty_dollar_and_collision_adjacent_names() {
    let cases = [
        ("await", "delete", "codemode.await_.delete_"),
        ("9lives", "2fa setup", "codemode._9lives._2fa_setup"),
        ("", "", "codemode._._"),
        ("cash$box", "$charge", "codemode.cash$box.$charge"),
        (
            "movie.search",
            "list-tags",
            "codemode.movie_search.list_tags",
        ),
        (
            "movie_search",
            "list.tags",
            "codemode.movie_search.list_tags",
        ),
    ];

    for (namespace, tool, expected) in cases {
        let types = super::ts_signatures::generate_tool_types(
            namespace,
            tool,
            "Description",
            Some(&json!({"type": "object"})),
            None,
        );

        assert!(
            types.signature.contains(expected),
            "expected {expected} in {:?}",
            types.signature
        );
        assert!(!types.dts.contains(".."), "{types:?}");
        assert!(!types.dts.contains("  : {"), "{types:?}");
        assert!(!types.dts.contains(" (params:"), "{types:?}");
    }
}
