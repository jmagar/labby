use serde_json::json;

use super::{MetricsWindow, ToolCallQuery, agent_detail, aggregate, tool_calls, tool_detail};
use crate::dispatch::logs::types::LogEvent;

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
