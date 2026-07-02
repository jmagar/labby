use serde_json::json;
use std::collections::BTreeSet;

use super::{
    MetricsWindow, ToolCallQuery, agent_detail, aggregate, aggregate_with_previous, tool_calls,
    tool_detail,
};
use crate::dispatch::logs::types::{LogEvent, LogRetention};

/// Build a dispatch-completion `LogEvent` via serde (LogEvent is a serde type).
fn call_event(
    ts: i64,
    surface: &str,
    service: &str,
    ok: bool,
    elapsed: u64,
    input: u64,
    output: u64,
) -> LogEvent {
    serde_json::from_value(json!({
        "event_id": format!("e-{ts}-{service}"),
        "ts": ts,
        "level": if ok { "info" } else { "warn" },
        "subsystem": "mcp_server",
        "surface": surface,
        "action": "call_tool",
        "message": if ok { "dispatch ok" } else { "dispatch error" },
        "request_id": null,
        "session_id": null,
        "correlation_id": null,
        "trace_id": null,
        "span_id": null,
        "instance": null,
        "auth_flow": null,
        "outcome_kind": null,
        "fields_json": {
            "service": service,
            "elapsed_ms": elapsed,
            "input_tokens": input,
            "output_tokens": output,
            "subject": "claude-code",
            "kind": if ok { serde_json::Value::Null } else { json!("server_error") },
        },
        "source_kind": null,
        "source_node_id": null,
        "source_device_id": null,
        "actor_key": "claude-code",
        "ingest_path": "tracing",
        "upstream_event_id": null,
    }))
    .expect("valid LogEvent fixture")
}

/// A "start" event (only input_tokens) must NOT be counted as a tool call.
fn start_event(ts: i64) -> LogEvent {
    serde_json::from_value(json!({
        "event_id": format!("s-{ts}"),
        "ts": ts, "level": "info", "subsystem": "mcp_server", "surface": "mcp",
        "action": "call_tool", "message": "dispatch start",
        "request_id": null, "session_id": null, "correlation_id": null,
        "trace_id": null, "span_id": null, "instance": null, "auth_flow": null,
        "outcome_kind": null,
        "fields_json": { "service": "radarr", "input_tokens": 50 },
        "source_kind": null, "source_node_id": null, "source_device_id": null,
        "actor_key": "claude-code", "ingest_path": "tracing", "upstream_event_id": null,
    }))
    .expect("valid LogEvent fixture")
}

fn code_mode_event(ts: i64, calls: serde_json::Value) -> LogEvent {
    let mut event = call_event(ts, "mcp", "code_mode", true, 900, 90, 300);
    event.fields_json["call_count"] = calls
        .as_array()
        .map_or(json!(0), |items| json!(items.len()));
    event.fields_json["code_mode_calls"] = calls;
    event
}

fn sample(now: i64) -> Vec<LogEvent> {
    vec![
        call_event(now - 1000, "mcp", "radarr", true, 120, 80, 400),
        call_event(now - 2000, "mcp", "radarr", true, 90, 70, 300),
        call_event(now - 3000, "api", "sonarr", true, 200, 60, 500),
        call_event(now - 4000, "mcp", "cortex", false, 50, 90, 0),
        start_event(now - 1500),
    ]
}

#[test]
fn counts_completion_events_and_ignores_start() {
    let now = 1_000_000_000_000;
    let m = aggregate(&sample(now), MetricsWindow::H24, now);
    assert_eq!(m.tool_calls.total, 4); // start event excluded
    assert_eq!(m.tool_calls.failed, 1);
    assert_eq!(m.tool_calls.succeeded, 3);
    assert_eq!(m.window, "24h");
}

#[test]
fn surfaces_hourly_reconcile_to_total() {
    let now = 1_000_000_000_000;
    let m = aggregate(&sample(now), MetricsWindow::H24, now);
    let total = m.tool_calls.total;
    assert_eq!(m.surfaces.iter().map(|s| s.calls).sum::<u64>(), total);
    assert_eq!(m.hourly.iter().map(|h| h.calls).sum::<u64>(), total);
    assert_eq!(m.timeseries.iter().map(|b| b.calls).sum::<u64>(), total);
    assert_eq!(m.hourly.len(), 24);
}

#[test]
fn tokens_tools_and_errors_aggregate() {
    let now = 1_000_000_000_000;
    let m = aggregate(&sample(now), MetricsWindow::H24, now);
    assert_eq!(m.tokens.input, 80 + 70 + 60 + 90);
    assert_eq!(m.tokens.output, 400 + 300 + 500);
    assert_eq!(m.tokens.total, m.tokens.input + m.tokens.output);
    assert_eq!(m.tools.distinct, 3); // radarr, sonarr, cortex
    assert_eq!(m.tools.top.first().map(|t| t.name.as_str()), Some("radarr")); // 2 calls
    assert_eq!(m.errors.total, 1);
    assert_eq!(
        m.errors.by_kind.first().map(|e| e.kind.as_str()),
        Some("server_error")
    );
}

#[test]
fn latency_percentiles_are_monotonic() {
    let now = 1_000_000_000_000;
    let m = aggregate(&sample(now), MetricsWindow::H24, now);
    assert!(m.latency.p50 <= m.latency.p95);
    assert!(m.latency.p95 <= m.latency.p99);
    assert!(m.latency.avg > 0);
    assert_eq!(m.actors.agent.active, 1); // one distinct actor
    assert_eq!(
        m.actors.agent.top.first().map(|a| a.id.as_str()),
        Some("claude-code")
    );
}

#[test]
fn empty_window_is_all_zero() {
    let now = 1_000_000_000_000;
    let m = aggregate(&[], MetricsWindow::H1, now);
    assert_eq!(m.tool_calls.total, 0);
    assert_eq!(m.tokens.avg_per_call, 0);
    assert_eq!(m.latency.p50, 0);
    assert!(m.surfaces.is_empty());
}

#[test]
fn tool_detail_filters_and_ranks_callers() {
    let now = 1_000_000_000_000;
    let d = tool_detail(&sample(now), "radarr", MetricsWindow::H24, now);
    assert_eq!(d.name, "radarr");
    assert_eq!(d.calls, 2);
    assert_eq!(d.failed, 0);
    assert_eq!(d.total_tokens, (80 + 400) + (70 + 300));
    assert_eq!(
        d.top_callers.first().map(|c| c.id.as_str()),
        Some("claude-code")
    );
    assert_eq!(d.recent.len(), 2);
    assert!(d.recent.iter().all(|r| r.tool == "radarr"));
}

#[test]
fn agent_detail_lists_tools_used() {
    let now = 1_000_000_000_000;
    let d = agent_detail(&sample(now), "claude-code", MetricsWindow::H24, now);
    assert_eq!(d.calls, 4);
    assert_eq!(d.failed, 1);
    assert_eq!(d.tools_used.len(), 3);
    assert_eq!(
        d.tools_used.first().map(|t| t.name.as_str()),
        Some("radarr")
    ); // 2 calls
}

fn query(tool: Option<&str>, outcome: Option<&str>, limit: Option<usize>) -> ToolCallQuery {
    ToolCallQuery {
        window: "24h".to_string(),
        tool: tool.map(ToOwned::to_owned),
        agent: None,
        ip: None,
        outcome: outcome.map(ToOwned::to_owned),
        surface: None,
        search: None,
        limit,
        offset: None,
    }
}

#[test]
fn tool_calls_filters_and_paginates() {
    let now = 1_000_000_000_000;
    let page = tool_calls(&sample(now), &query(Some("radarr"), None, Some(1)));
    assert_eq!(page.total, 4); // all completion events
    assert_eq!(page.filtered, 2); // radarr only
    assert_eq!(page.calls.len(), 1); // limit 1
    assert!(page.calls.iter().all(|c| c.tool == "radarr"));

    let failed = tool_calls(&sample(now), &query(None, Some("failed"), None));
    assert_eq!(failed.filtered, 1);
    assert!(failed.calls.iter().all(|c| c.outcome == "failed"));
}

#[test]
fn tool_call_facets_cover_full_unpaginated_input_and_skip_empty_ips() {
    let now = 1_000_000_000_000;
    let mut events = sample(now);
    let mut rare = call_event(now - 10_000, "api", "rare_tool", true, 42, 1, 2);
    rare.fields_json["ip"] = json!("");
    events.push(rare);

    let page = tool_calls(&events, &query(None, None, Some(1)));

    assert_eq!(page.calls.len(), 1);
    assert_eq!(page.total, 5);
    assert!(page.facets.tools.iter().any(|tool| tool == "rare_tool"));
    assert!(!page.facets.ips.iter().any(|ip| ip.is_empty()));
}

#[test]
fn historical_actor_labels_do_not_render_email_or_subject_values() {
    let now = 1_000_000_000_000;
    let mut event = call_event(now - 1000, "api", "radarr", true, 42, 1, 2);
    event.actor_key = Some("actor-hash".to_string());
    event.fields_json["subject"] = json!("person@example.com");
    event.fields_json["actor_label"] = json!("person@example.com");

    let page = tool_calls(&[event.clone()], &query(None, None, None));
    assert_eq!(page.calls[0].agent_id, "actor-hash");
    assert_eq!(page.calls[0].agent_label, "actor-hash");

    let metrics = aggregate(&[event], MetricsWindow::H24, now);
    assert_eq!(metrics.actors.agent.top[0].label, "actor-hash");
}

#[test]
fn historical_actor_without_actor_key_uses_opaque_legacy_identity() {
    let now = 1_000_000_000_000;
    let mut event = call_event(now - 1000, "api", "radarr", true, 42, 1, 2);
    event.actor_key = None;
    event.fields_json["subject"] = json!("person@example.com");
    event.fields_json["actor_label"] = json!("person@example.com");

    let page = tool_calls(&[event.clone()], &query(None, None, None));
    let record = &page.calls[0];
    assert!(record.agent_id.starts_with("legacy-actor-"));
    assert!(!record.agent_id.contains("person@example.com"));
    assert_eq!(record.agent_label, record.agent_id);
    assert_eq!(page.facets.agents[0].id, record.agent_id);
    assert_eq!(page.facets.agents[0].label, record.agent_id);

    let metrics = aggregate(&[event], MetricsWindow::H24, now);
    let top = &metrics.actors.agent.top[0];
    assert_eq!(top.id, record.agent_id);
    assert_eq!(top.label, record.agent_id);
}

#[tokio::test]
async fn completion_event_window_is_not_capped_at_ten_thousand_raw_rows() {
    let store = crate::dispatch::logs::store::open_store_for_test(LogRetention::default())
        .await
        .expect("open store");
    let now = 1_000_000_000_000;
    let old_completion = call_event(now - 86_000_000, "mcp", "old_tool", true, 10, 1, 1);
    store
        .insert(&old_completion)
        .await
        .expect("insert old call");

    for i in 0..10_050 {
        store
            .insert(&start_event(now - i))
            .await
            .expect("insert raw start event");
    }

    let events = store
        .completion_events(Some(now - MetricsWindow::D7.ms()), Some(now))
        .await
        .expect("fetch completion events");

    assert!(
        events
            .iter()
            .any(|event| event.event_id == old_completion.event_id),
        "older completion event must remain visible even when newer raw rows exceed 10k"
    );
}

#[tokio::test]
async fn completion_kind_excludes_start_events_from_store_queries() {
    let store = crate::dispatch::logs::store::open_store_for_test(LogRetention::default())
        .await
        .expect("open store");
    let now = 1_000_000_000_000;

    let mut completion = call_event(now - 4_000_000, "mcp", "radarr", true, 10, 1, 1);
    completion.actor_key = Some("completion-agent".to_string());
    store.insert(&completion).await.expect("insert completion");

    let mut start = start_event(now - 4_000_001);
    start.actor_key = Some("start-only-agent".to_string());
    store.insert(&start).await.expect("insert start");

    let events = store
        .completion_events(Some(now - MetricsWindow::D7.ms()), Some(now))
        .await
        .expect("completion events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_id, completion.event_id);

    let actors = store
        .previous_completion_actor_ids(now - MetricsWindow::H1.ms())
        .await
        .expect("previous actors");
    assert_eq!(actors, BTreeSet::from(["completion-agent".to_string()]));
}

#[tokio::test]
async fn previous_completion_actor_ids_uses_distinct_actor_keys() {
    let store = crate::dispatch::logs::store::open_store_for_test(LogRetention::default())
        .await
        .expect("open store");
    let now = 1_000_000_000_000;

    let mut older = call_event(now - 4_000_000, "mcp", "radarr", true, 10, 1, 1);
    older.actor_key = Some("returning-agent".to_string());
    store.insert(&older).await.expect("insert older call");

    let mut duplicate = call_event(now - 3_900_000, "mcp", "sonarr", true, 10, 1, 1);
    duplicate.event_id = "duplicate-returning-agent".to_string();
    duplicate.actor_key = Some("returning-agent".to_string());
    store
        .insert(&duplicate)
        .await
        .expect("insert duplicate call");

    let mut current_window = call_event(now - 1_000, "mcp", "cortex", true, 10, 1, 1);
    current_window.actor_key = Some("current-window-agent".to_string());
    store
        .insert(&current_window)
        .await
        .expect("insert current-window call");

    let actors = store
        .previous_completion_actor_ids(now - MetricsWindow::H1.ms())
        .await
        .expect("previous actors");

    assert_eq!(actors, BTreeSet::from(["returning-agent".to_string()]));
}

#[test]
fn tier_two_fields_populate_actor_ip_and_code_mode_metrics() {
    let now = 1_000_000_000_000;
    let mut device_call = call_event(now - 1000, "api", "codemode", true, 120, 80, 400);
    device_call.actor_key = Some("device-1".to_string());
    device_call.fields_json["actor_label"] = json!("Pixel browser");
    device_call.fields_json["agent_kind"] = json!("device");
    device_call.fields_json["ip"] = json!("10.0.0.8");
    device_call.fields_json["call_count"] = json!(3);
    device_call.fields_json["code_mode_calls"] = json!([
        {
            "id": "github::get_me",
            "namespace": "github",
            "tool": "get_me",
            "ok": true,
            "elapsed_ms": 30
        },
        {
            "id": "cortex::cortex",
            "namespace": "cortex",
            "tool": "cortex",
            "ok": true,
            "elapsed_ms": 40
        },
        {
            "id": "cortex::cortex",
            "namespace": "cortex",
            "tool": "cortex",
            "ok": true,
            "elapsed_ms": 50
        }
    ]);
    device_call.fields_json["artifact_writes"] = json!(2);
    device_call.fields_json["truncated"] = json!(true);

    let mut returning_agent = call_event(now - 2000, "mcp", "radarr", true, 90, 70, 300);
    returning_agent.actor_key = Some("agent-1".to_string());
    returning_agent.fields_json["actor_label"] = json!("Codex");
    returning_agent.fields_json["agent_kind"] = json!("agent");

    let previous = BTreeSet::from(["agent-1".to_string()]);
    let m = aggregate_with_previous(
        &[device_call, returning_agent],
        MetricsWindow::H24,
        now,
        &previous,
    );

    assert_eq!(m.actors.device.active, 1);
    assert_eq!(m.actors.device.top[0].label, "Pixel browser");
    assert_eq!(m.actors.agent.active, 1);
    assert_eq!(m.actors.agent.top[0].label, "Codex");
    assert_eq!(m.actors.ip.active, 1);
    assert_eq!(m.actors.ip.top[0].id, "10.0.0.8");
    assert_eq!(m.fan_out.runs, 1);
    assert_eq!(m.fan_out.total_calls, 3);
    assert!((m.fan_out.truncation_rate - 1.0).abs() < f64::EPSILON);
    assert_eq!(m.fan_out.artifact_writes, 2);
    assert_eq!(m.agents_seen.new, 1);
    assert_eq!(m.agents_seen.returning, 1);
}

#[test]
fn dashboard_tool_usage_expands_code_mode_children_and_excludes_wrappers() {
    let now = 1_000_000_000_000;
    let code_mode = code_mode_event(
        now - 1000,
        json!([
            {
                "id": "github::get_me",
                "namespace": "github",
                "tool": "get_me",
                "ok": true,
                "elapsed_ms": 20
            },
            {
                "id": "cortex::cortex",
                "namespace": "cortex",
                "tool": "cortex",
                "ok": false,
                "elapsed_ms": 50,
                "error_kind": "upstream_error"
            },
            {
                "id": "cortex::cortex",
                "namespace": "cortex",
                "tool": "cortex",
                "ok": true,
                "elapsed_ms": 30
            }
        ]),
    );
    let gateway = call_event(now - 2000, "api", "gateway", true, 5, 10, 20);
    let logs = call_event(now - 3000, "api", "logs", true, 5, 10, 20);

    let m = aggregate(&[code_mode, gateway, logs], MetricsWindow::H24, now);

    assert_eq!(m.tool_calls.total, 3);
    assert_eq!(m.tool_calls.failed, 1);
    assert_eq!(m.fan_out.runs, 1);
    assert_eq!(m.fan_out.total_calls, 3);
    assert_eq!(m.tools.distinct, 2);
    assert_eq!(
        m.tools
            .top
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        vec!["cortex::cortex", "github::get_me"]
    );
    assert!(
        m.tools
            .top
            .iter()
            .chain(m.tools.least.iter())
            .all(|tool| !matches!(
                tool.name.as_str(),
                "code_mode" | "codemode" | "gateway" | "logs"
            ))
    );
    assert_eq!(m.upstreams[0].name, "cortex");
}

#[test]
fn tool_call_explorer_lists_code_mode_child_calls() {
    let now = 1_000_000_000_000;
    let code_mode = code_mode_event(
        now - 1000,
        json!([
            {
                "id": "github::get_me",
                "namespace": "github",
                "tool": "get_me",
                "ok": true,
                "elapsed_ms": 20
            },
            {
                "id": "cortex::cortex",
                "namespace": "cortex",
                "tool": "cortex",
                "ok": true,
                "elapsed_ms": 30
            }
        ]),
    );
    let logs = call_event(now - 3000, "api", "logs", true, 5, 10, 20);

    let page = tool_calls(&[code_mode, logs], &query(None, None, None));

    assert_eq!(page.total, 2);
    assert_eq!(page.filtered, 2);
    assert_eq!(
        page.facets.tools,
        vec!["cortex::cortex".to_string(), "github::get_me".to_string()]
    );
    assert!(page.calls.iter().all(|call| call.tool != "logs"));
}
