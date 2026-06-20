#![allow(
    clippy::bool_assert_comparison,
    clippy::err_expect,
    clippy::field_reassign_with_default,
    clippy::float_cmp,
    clippy::len_zero,
    clippy::manual_string_new,
    clippy::needless_raw_string_hashes,
    clippy::single_char_pattern,
    clippy::unnested_or_patterns
)]
//! Architecture test: pins the `lab://gateway/*` URI scheme, the JSON
//! shape of the synthetic documents, and exposure-policy filtering.
//!
//! Any change to a top-level key here is a contract change — update
//! `docs/contracts/gateway-schema-resources.md` in the same PR.

#![cfg(feature = "gateway")]

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;

use labby::dispatch::upstream::pool::UpstreamPool;
use labby::dispatch::upstream::types::{
    ToolExposurePolicy, UpstreamEntry, UpstreamHealth, UpstreamTool,
};

fn make_tool(name: &'static str, upstream: &str) -> UpstreamTool {
    UpstreamTool {
        tool: rmcp::model::Tool::new(name, "desc", Arc::new(serde_json::Map::new())),
        input_schema: Some(json!({"type": "object", "properties": {}})),
        output_schema: None,
        upstream_name: Arc::from(upstream),
        destructive: false,
    }
}

fn make_entry(name: &str, tools: Vec<UpstreamTool>, policy: ToolExposurePolicy) -> UpstreamEntry {
    let mut map = HashMap::new();
    for t in tools {
        map.insert(t.tool.name.to_string(), t);
    }
    UpstreamEntry {
        name: Arc::from(name),
        tools: map,
        exposure_policy: policy,
        proxy_resources: true,
        prompt_count: 0,
        resource_count: 0,
        prompt_names: Vec::new(),
        resource_uris: Vec::new(),
        tool_health: UpstreamHealth::Healthy,
        prompt_health: UpstreamHealth::Healthy,
        resource_health: UpstreamHealth::Healthy,
        tool_unhealthy_since: None,
        prompt_unhealthy_since: None,
        resource_unhealthy_since: None,
        tool_last_error: None,
        prompt_last_error: None,
        resource_last_error: None,
    }
}

#[tokio::test]
async fn gateway_servers_doc_shape_is_contract_stable() {
    let pool = UpstreamPool::new();
    pool.insert_entry_for_test(
        "alpha",
        make_entry(
            "alpha",
            vec![make_tool("search", "alpha")],
            ToolExposurePolicy::All,
        ),
    )
    .await;

    let doc = pool.gateway_servers_doc().await;
    let servers = doc["servers"].as_array().expect("servers array");
    assert_eq!(servers.len(), 1);
    let s = &servers[0];

    for key in [
        "name",
        "tool_count",
        "prompt_count",
        "resource_count",
        "tool_health",
        "tool_last_error",
    ] {
        assert!(s.get(key).is_some(), "missing required field: {key}");
    }
    assert_eq!(s["tool_health"].as_str(), Some("healthy"));
    assert!(s["tool_last_error"].is_null());
}

#[tokio::test]
async fn gateway_server_schema_shape_is_contract_stable() {
    let policy = ToolExposurePolicy::from_patterns(vec!["github_*".into()]).expect("policy");
    let tools = vec![
        make_tool("github_create", "alpha"),
        make_tool("delete_repo", "alpha"),
    ];

    let pool = UpstreamPool::new();
    pool.insert_entry_for_test("alpha", make_entry("alpha", tools, policy))
        .await;

    let doc = pool.gateway_server_schema("alpha").await.expect("doc");

    for key in ["name", "tools", "health", "last_error"] {
        assert!(doc.get(key).is_some(), "missing top-level field: {key}");
    }
    let tools = doc["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 1, "exposure policy must filter hidden tools");

    let t = &tools[0];
    for key in ["name", "description", "input_schema", "meta"] {
        assert!(t.get(key).is_some(), "missing tool-entry field: {key}");
    }
    assert_eq!(t["name"], "github_create");
}

#[tokio::test]
async fn gateway_server_schema_unknown_upstream_returns_none() {
    let pool = UpstreamPool::new();
    assert!(pool.gateway_server_schema("nope").await.is_none());
}

#[tokio::test]
async fn gateway_synthetic_resources_uri_scheme_is_pinned() {
    let pool = UpstreamPool::new();
    pool.insert_entry_for_test(
        "alpha",
        make_entry("alpha", vec![], ToolExposurePolicy::All),
    )
    .await;

    let resources = pool.gateway_synthetic_resources().await;
    let uris: Vec<String> = resources.iter().map(|r| r.uri.clone()).collect();
    assert!(uris.contains(&"lab://gateway/servers".to_string()));
    assert!(uris.contains(&"lab://gateway/alpha/schema".to_string()));
    // Nothing should leak into the lab://upstream/ namespace from here.
    assert!(!uris.iter().any(|u| u.starts_with("lab://upstream/")));
}
