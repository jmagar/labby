//! Usage-metrics aggregation for the gateway dashboard.
//!
//! Rolls up dispatch-completion log events (one per tool call) into the
//! `DashboardMetrics` shape consumed by `apps/gateway-admin`
//! (`lib/types/metrics.ts`). Aggregation runs in Rust over a fetched window of
//! `LogEvent`s — percentiles, grouping, and bucketing are trivial here and the
//! same machinery serves the eventual SQL path.
//!
//! A tool call is a completion event: its `fields_json` carries both
//! `input_tokens` and `output_tokens` (dispatch/Code-Mode start events log
//! only `input_tokens`, so the pair reliably excludes them). Success vs failure
//! is read from the message suffix (`… ok` / `… error` / `… failed`).

// Averages / rates / percentiles cast counts to f64. Event counts and token
// totals never approach 2^52, so the precision loss is immaterial here.
#![allow(clippy::cast_precision_loss)]

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::types::LogEvent;

/// Rolling activity window. `7d` is the log-store retention ceiling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetricsWindow {
    H1,
    H24,
    D7,
}

impl MetricsWindow {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "1h" => Some(Self::H1),
            "24h" => Some(Self::H24),
            "7d" => Some(Self::D7),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::H1 => "1h",
            Self::H24 => "24h",
            Self::D7 => "7d",
        }
    }

    pub(crate) fn ms(self) -> i64 {
        match self {
            Self::H1 => 60 * 60 * 1000,
            Self::H24 => 24 * 60 * 60 * 1000,
            Self::D7 => 7 * 24 * 60 * 60 * 1000,
        }
    }

    fn buckets(self) -> usize {
        match self {
            Self::H1 => 12,
            Self::H24 => 24,
            Self::D7 => 14,
        }
    }
}

/// Tool → upstream server, mirroring the frontend mock map.
fn tool_upstream(tool: &str) -> &str {
    if let Some((namespace, _)) = tool.split_once("::") {
        return namespace;
    }
    match tool {
        "radarr" | "sonarr" => "media-stack",
        "qbittorrent" => "downloads",
        "codemode" => "code-mode",
        other => other,
    }
}

// ── Output contract (serializes to lib/types/metrics.ts) ───────────────────

#[derive(Serialize)]
pub struct DashboardMetrics {
    pub window: String,
    pub since_ms: i64,
    pub until_ms: i64,
    pub tool_calls: ToolCalls,
    pub tools: Tools,
    pub tokens: Tokens,
    pub actors: Actors,
    pub fan_out: FanOut,
    pub latency: Latency,
    pub errors: Errors,
    pub surfaces: Vec<SurfaceCount>,
    pub tokens_by_tool: Vec<TokenByTool>,
    pub upstreams: Vec<UpstreamUsage>,
    pub throughput: Throughput,
    pub hourly: Vec<HourBucket>,
    pub agents_seen: AgentsSeen,
    pub timeseries: Vec<MetricsBucket>,
}

#[derive(Serialize)]
pub struct ToolCalls {
    pub total: u64,
    pub failed: u64,
    pub succeeded: u64,
}

#[derive(Serialize)]
pub struct ToolUsageEntry {
    pub name: String,
    pub calls: u64,
    pub failed: u64,
}

#[derive(Serialize)]
pub struct Tools {
    pub top: Vec<ToolUsageEntry>,
    pub least: Vec<ToolUsageEntry>,
    pub distinct: usize,
}

#[derive(Serialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub total: u64,
    pub avg_per_call: u64,
}

#[derive(Serialize)]
pub struct ActorUsageEntry {
    pub id: String,
    pub label: String,
    pub kind: &'static str,
    pub calls: u64,
}

#[derive(Serialize)]
pub struct ActorFacet {
    pub active: usize,
    pub top: Vec<ActorUsageEntry>,
}

#[derive(Serialize)]
pub struct Actors {
    pub agent: ActorFacet,
    pub device: ActorFacet,
    pub ip: ActorFacet,
}

#[derive(Serialize)]
pub struct FanOut {
    pub runs: u64,
    pub total_calls: u64,
    pub avg_calls_per_run: f64,
    pub max_calls_in_run: u64,
    pub timeout_rate: f64,
    pub truncation_rate: f64,
    pub artifact_writes: u64,
}

#[derive(Serialize)]
pub struct LatencyStat {
    pub name: String,
    pub avg_ms: u64,
}

#[derive(Serialize)]
pub struct Latency {
    pub p50: u64,
    pub p95: u64,
    pub p99: u64,
    pub avg: u64,
    pub slowest: Vec<LatencyStat>,
}

#[derive(Serialize)]
pub struct ErrorKindCount {
    pub kind: String,
    pub count: u64,
}

#[derive(Serialize)]
pub struct Errors {
    pub total: u64,
    pub by_kind: Vec<ErrorKindCount>,
}

#[derive(Serialize)]
pub struct SurfaceCount {
    pub surface: String,
    pub calls: u64,
}

#[derive(Serialize)]
pub struct TokenByTool {
    pub name: String,
    pub tokens: u64,
}

#[derive(Serialize)]
pub struct UpstreamUsage {
    pub name: String,
    pub calls: u64,
    pub failed: u64,
}

#[derive(Serialize)]
pub struct Throughput {
    pub peak_per_min: u64,
    pub avg_per_min: f64,
    pub busiest_hour: u32,
}

#[derive(Serialize)]
pub struct HourBucket {
    pub hour: u32,
    pub calls: u64,
}

#[derive(Serialize)]
pub struct AgentsSeen {
    pub new: u64,
    pub returning: u64,
}

#[derive(Serialize)]
pub struct MetricsBucket {
    pub ts: i64,
    pub calls: u64,
    pub failed: u64,
}

// ── Extraction ─────────────────────────────────────────────────────────────

/// One dispatched tool call, distilled from a completion `LogEvent`.
struct Call {
    ts: i64,
    service: String,
    ok: bool,
    error_kind: Option<String>,
    input_tokens: u64,
    output_tokens: u64,
    elapsed_ms: f64,
    surface: String,
    actor: Option<String>,
    actor_label: Option<String>,
    agent_kind: String,
    ip: Option<String>,
    call_count: Option<u64>,
    truncated: bool,
    artifact_writes: u64,
}

fn field<'a>(event: &'a LogEvent, key: &str) -> Option<&'a Value> {
    event.fields_json.get(key)
}

/// Numeric field that may be stored as a JSON number or a stringified number
/// (`u128` tracing values are recorded via `Debug` → string).
fn num_u64(v: Option<&Value>) -> u64 {
    v.and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    })
    .unwrap_or(0)
}

fn num_f64(v: Option<&Value>) -> f64 {
    v.and_then(|v| {
        v.as_f64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    })
    .unwrap_or(0.0)
}

fn str_field(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(ToOwned::to_owned)
}

fn is_code_mode_service(service: &str) -> bool {
    matches!(service, "code_mode" | "codemode")
}

fn is_usage_wrapper_service(service: &str) -> bool {
    matches!(service, "gateway" | "logs") || is_code_mode_service(service)
}

fn looks_sensitive_actor_label(value: &str) -> bool {
    value.contains('@')
}

fn legacy_subject_actor_id(subject: &str) -> String {
    let digest = Sha256::digest(subject.as_bytes());
    let hex = hex::encode(digest);
    format!("legacy-actor-{}", &hex[..12])
}

fn actor_identity(event: &LogEvent) -> Option<String> {
    event.actor_key.clone().or_else(|| {
        str_field(field(event, "subject")).map(|subject| legacy_subject_actor_id(&subject))
    })
}

fn safe_actor_label(actor: &str, candidate: Option<String>, subject: Option<String>) -> String {
    if let Some(label) = candidate
        && subject.as_deref() != Some(label.as_str())
        && !looks_sensitive_actor_label(&label)
    {
        return label;
    }
    if looks_sensitive_actor_label(actor) {
        "unknown actor".to_string()
    } else {
        actor.to_string()
    }
}

/// Is this a completed tool-call event? Completion logs carry both token
/// fields; start events carry only `input_tokens`.
fn extract_call(event: &LogEvent) -> Option<Call> {
    let input = field(event, "input_tokens")?;
    let output = field(event, "output_tokens")?;
    let service = str_field(field(event, "service")).or_else(|| str_field(field(event, "tool")))?;
    let ok = event.message.ends_with(" ok");
    let actor = actor_identity(event);
    let actor_label = actor.as_ref().map(|actor| {
        safe_actor_label(
            actor,
            str_field(field(event, "actor_label")),
            str_field(field(event, "subject")),
        )
    });
    Some(Call {
        ts: event.ts,
        service,
        ok,
        error_kind: if ok {
            None
        } else {
            str_field(field(event, "kind"))
        },
        input_tokens: num_u64(Some(input)),
        output_tokens: num_u64(Some(output)),
        elapsed_ms: num_f64(field(event, "elapsed_ms")),
        surface: event.surface.as_str().to_string(),
        actor,
        actor_label,
        agent_kind: str_field(field(event, "agent_kind"))
            .or_else(|| {
                event
                    .source_device_id
                    .as_ref()
                    .map(|_| "device".to_string())
            })
            .unwrap_or_else(|| "agent".to_string()),
        ip: str_field(field(event, "ip")),
        call_count: field(event, "call_count").map(|v| num_u64(Some(v))),
        truncated: field(event, "truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        artifact_writes: num_u64(field(event, "artifact_writes")),
    })
}

fn parsed_code_mode_calls(event: &LogEvent) -> Vec<Value> {
    let Some(raw) = field(event, "code_mode_calls").or_else(|| field(event, "calls")) else {
        return Vec::new();
    };
    match raw {
        Value::Array(calls) => calls.clone(),
        Value::String(s) => serde_json::from_str::<Vec<Value>>(s).unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn split_evenly(total: u64, count: usize, index: usize) -> u64 {
    if count == 0 {
        return 0;
    }
    let base = total / count as u64;
    let remainder = total % count as u64;
    base + u64::from((index as u64) < remainder)
}

fn code_mode_child_service(call: &Value) -> Option<String> {
    if let Some(id) = call.get("id").and_then(Value::as_str)
        && id.contains("::")
    {
        return Some(id.to_string());
    }
    let namespace = call.get("namespace").and_then(Value::as_str).unwrap_or("");
    let tool = call.get("tool").and_then(Value::as_str)?;
    if namespace.is_empty() {
        Some(tool.to_string())
    } else {
        Some(format!("{namespace}::{tool}"))
    }
}

/// Usage calls are the semantic tools the dashboard should rank.
///
/// Code Mode completion events are wrapper rows; their `code_mode_calls` field
/// carries the actual upstream calls made inside the run. Built-in Lab wrapper
/// services such as `gateway` and `logs` are intentionally excluded from tool
/// usage rankings.
fn extract_usage_calls(event: &LogEvent) -> Vec<Call> {
    let Some(parent) = extract_call(event) else {
        return Vec::new();
    };
    if is_code_mode_service(&parent.service) {
        let children = parsed_code_mode_calls(event);
        let count = children.len();
        return children
            .into_iter()
            .enumerate()
            .filter_map(|(index, child)| {
                let service = code_mode_child_service(&child)?;
                let ok = child.get("ok").and_then(Value::as_bool).unwrap_or(true);
                Some(Call {
                    ts: parent.ts,
                    service,
                    ok,
                    error_kind: if ok {
                        None
                    } else {
                        str_field(child.get("error_kind"))
                    },
                    input_tokens: split_evenly(parent.input_tokens, count, index),
                    output_tokens: split_evenly(parent.output_tokens, count, index),
                    elapsed_ms: num_f64(child.get("elapsed_ms")),
                    surface: parent.surface.clone(),
                    actor: parent.actor.clone(),
                    actor_label: parent.actor_label.clone(),
                    agent_kind: parent.agent_kind.clone(),
                    ip: parent.ip.clone(),
                    call_count: None,
                    truncated: false,
                    artifact_writes: 0,
                })
            })
            .collect();
    }
    if is_usage_wrapper_service(&parent.service) {
        Vec::new()
    } else {
        vec![parent]
    }
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = (((p / 100.0) * sorted.len() as f64).floor() as usize).min(sorted.len() - 1);
    sorted[idx]
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// Sort a label→count map into a ranked `(name, count)` vec, desc by count.
fn ranked(map: BTreeMap<String, (u64, u64)>) -> Vec<(String, u64, u64)> {
    let mut v: Vec<(String, u64, u64)> = map.into_iter().map(|(k, (c, f))| (k, c, f)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

/// Aggregate a window of log events into dashboard metrics.
#[allow(dead_code)]
pub fn aggregate(events: &[LogEvent], window: MetricsWindow, now_ms: i64) -> DashboardMetrics {
    aggregate_with_previous(events, window, now_ms, &BTreeSet::new())
}

/// Aggregate a window, with actor IDs seen before the window supplied for
/// first-seen/returning classification.
pub fn aggregate_with_previous(
    events: &[LogEvent],
    window: MetricsWindow,
    now_ms: i64,
    previous_actors: &BTreeSet<String>,
) -> DashboardMetrics {
    let completion_calls: Vec<Call> = events.iter().filter_map(extract_call).collect();
    let calls: Vec<Call> = events.iter().flat_map(extract_usage_calls).collect();
    let total = calls.len() as u64;
    let failed = calls.iter().filter(|c| !c.ok).count() as u64;
    let since = now_ms - window.ms();

    // Per-tool tallies (calls, failed) + tokens + latency.
    let mut tool_tally: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut tool_tokens: BTreeMap<String, u64> = BTreeMap::new();
    let mut tool_latency: BTreeMap<String, (f64, u64)> = BTreeMap::new();
    let mut surface_tally: BTreeMap<String, u64> = BTreeMap::new();
    let mut kind_tally: BTreeMap<String, u64> = BTreeMap::new();
    let mut upstream_tally: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut agent_tally: BTreeMap<String, (String, u64)> = BTreeMap::new();
    let mut device_tally: BTreeMap<String, (String, u64)> = BTreeMap::new();
    let mut ip_tally: BTreeMap<String, u64> = BTreeMap::new();
    let mut window_actors: BTreeSet<String> = BTreeSet::new();
    let mut latencies: Vec<u64> = Vec::with_capacity(calls.len());
    let mut input_sum: u64 = 0;
    let mut output_sum: u64 = 0;

    for c in &calls {
        let t = tool_tally.entry(c.service.clone()).or_default();
        t.0 += 1;
        if !c.ok {
            t.1 += 1;
        }
        *tool_tokens.entry(c.service.clone()).or_default() += c.input_tokens + c.output_tokens;
        let l = tool_latency.entry(c.service.clone()).or_insert((0.0, 0));
        l.0 += c.elapsed_ms;
        l.1 += 1;
        *surface_tally.entry(c.surface.clone()).or_default() += 1;
        if let Some(kind) = &c.error_kind {
            *kind_tally.entry(kind.clone()).or_default() += 1;
        }
        let up = upstream_tally
            .entry(tool_upstream(&c.service).to_string())
            .or_default();
        up.0 += 1;
        if !c.ok {
            up.1 += 1;
        }
        if let Some(actor) = &c.actor {
            window_actors.insert(actor.clone());
            let label = c.actor_label.clone().unwrap_or_else(|| actor.clone());
            let tally = if c.agent_kind == "device" {
                &mut device_tally
            } else {
                &mut agent_tally
            };
            let entry = tally.entry(actor.clone()).or_insert((label, 0));
            entry.1 += 1;
        }
        if let Some(ip) = &c.ip {
            *ip_tally.entry(ip.clone()).or_default() += 1;
        }
        latencies.push(c.elapsed_ms.round() as u64);
        input_sum += c.input_tokens;
        output_sum += c.output_tokens;
    }
    latencies.sort_unstable();

    let tools_ranked = ranked(tool_tally);
    let top: Vec<ToolUsageEntry> = tools_ranked
        .iter()
        .take(5)
        .map(|(n, c, f)| ToolUsageEntry {
            name: n.clone(),
            calls: *c,
            failed: *f,
        })
        .collect();
    let least: Vec<ToolUsageEntry> = tools_ranked
        .iter()
        .rev()
        .take(3)
        .map(|(n, c, f)| ToolUsageEntry {
            name: n.clone(),
            calls: *c,
            failed: *f,
        })
        .collect();

    let mut slowest: Vec<LatencyStat> = tool_latency
        .into_iter()
        .map(|(name, (sum, n))| LatencyStat {
            name,
            avg_ms: if n > 0 {
                (sum / n as f64).round() as u64
            } else {
                0
            },
        })
        .collect();
    slowest.sort_by(|a, b| b.avg_ms.cmp(&a.avg_ms).then_with(|| a.name.cmp(&b.name)));
    slowest.truncate(3);

    let total_tokens = input_sum + output_sum;
    let avg_latency = if latencies.is_empty() {
        0
    } else {
        (latencies.iter().sum::<u64>() as f64 / latencies.len() as f64).round() as u64
    };

    // Throughput + hourly rhythm (UTC hour-of-day).
    let mut minute_buckets: BTreeMap<i64, u64> = BTreeMap::new();
    let mut hourly_counts = [0u64; 24];
    for c in &calls {
        *minute_buckets.entry(c.ts / 60_000).or_default() += 1;
        let hour = (((c.ts / 3_600_000) % 24 + 24) % 24) as usize;
        hourly_counts[hour] += 1;
    }
    let peak_per_min = minute_buckets.values().copied().max().unwrap_or(0);
    let window_minutes = (window.ms() / 60_000).max(1) as f64;
    let busiest_hour = (0..24u32)
        .max_by_key(|&h| hourly_counts[h as usize])
        .unwrap_or(0);
    let hourly: Vec<HourBucket> = (0..24u32)
        .map(|hour| HourBucket {
            hour,
            calls: hourly_counts[hour as usize],
        })
        .collect();

    // Time series (oldest bucket first).
    let buckets = window.buckets();
    let bucket_ms = window.ms() / buckets as i64;
    let mut series: Vec<MetricsBucket> = (0..buckets)
        .map(|i| MetricsBucket {
            ts: since + bucket_ms * i as i64,
            calls: 0,
            failed: 0,
        })
        .collect();
    for c in &calls {
        let idx = (((c.ts - since) / bucket_ms).max(0) as usize).min(buckets - 1);
        series[idx].calls += 1;
        if !c.ok {
            series[idx].failed += 1;
        }
    }

    // Code Mode fan-out (events with service == codemode carry call_count).
    let code_runs: Vec<&Call> = completion_calls
        .iter()
        .filter(|c| is_code_mode_service(&c.service))
        .collect();
    let runs = code_runs.len() as u64;
    let fan_total: u64 = code_runs.iter().filter_map(|c| c.call_count).sum();
    let timeouts = code_runs
        .iter()
        .filter(|c| c.error_kind.as_deref() == Some("timeout"))
        .count() as u64;
    let max_calls_in_run = code_runs
        .iter()
        .filter_map(|c| c.call_count)
        .max()
        .unwrap_or(0);

    let distinct_agents = agent_tally.len();
    let distinct_devices = device_tally.len();
    let agent_top = ranked_actor_entries(agent_tally, "agent", 8);
    let device_top = ranked_actor_entries(device_tally, "device", 8);
    let ip_top = ranked(ip_tally.into_iter().map(|(k, c)| (k, (c, 0))).collect())
        .into_iter()
        .take(8)
        .map(|(id, calls, _)| ActorUsageEntry {
            id: id.clone(),
            label: id,
            kind: "ip",
            calls,
        })
        .collect::<Vec<_>>();
    let distinct_ips = ip_top.len();
    let returning = window_actors
        .iter()
        .filter(|actor| previous_actors.contains(*actor))
        .count() as u64;
    let new = window_actors.len() as u64 - returning;

    DashboardMetrics {
        window: window.label().to_string(),
        since_ms: since,
        until_ms: now_ms,
        tool_calls: ToolCalls {
            total,
            failed,
            succeeded: total - failed,
        },
        tools: Tools {
            top,
            least,
            distinct: tools_ranked.len(),
        },
        tokens: Tokens {
            input: input_sum,
            output: output_sum,
            total: total_tokens,
            avg_per_call: if total > 0 { total_tokens / total } else { 0 },
        },
        actors: Actors {
            agent: ActorFacet {
                active: distinct_agents,
                top: agent_top,
            },
            device: ActorFacet {
                active: distinct_devices,
                top: device_top,
            },
            ip: ActorFacet {
                active: distinct_ips,
                top: ip_top,
            },
        },
        fan_out: FanOut {
            runs,
            total_calls: fan_total,
            avg_calls_per_run: if runs > 0 {
                round1(fan_total as f64 / runs as f64)
            } else {
                0.0
            },
            max_calls_in_run,
            timeout_rate: if runs > 0 {
                round1(timeouts as f64 / runs as f64)
            } else {
                0.0
            },
            truncation_rate: if runs > 0 {
                round1(code_runs.iter().filter(|c| c.truncated).count() as f64 / runs as f64)
            } else {
                0.0
            },
            artifact_writes: code_runs.iter().map(|c| c.artifact_writes).sum(),
        },
        latency: Latency {
            p50: percentile(&latencies, 50.0),
            p95: percentile(&latencies, 95.0),
            p99: percentile(&latencies, 99.0),
            avg: avg_latency,
            slowest,
        },
        errors: Errors {
            total: failed,
            by_kind: ranked(kind_tally.into_iter().map(|(k, c)| (k, (c, 0))).collect())
                .into_iter()
                .map(|(kind, count, _)| ErrorKindCount { kind, count })
                .collect(),
        },
        surfaces: {
            let mut s: Vec<SurfaceCount> = surface_tally
                .into_iter()
                .map(|(surface, calls)| SurfaceCount { surface, calls })
                .collect();
            s.sort_by(|a, b| b.calls.cmp(&a.calls));
            s
        },
        tokens_by_tool: {
            let mut t: Vec<TokenByTool> = tool_tokens
                .into_iter()
                .map(|(name, tokens)| TokenByTool { name, tokens })
                .collect();
            t.sort_by(|a, b| b.tokens.cmp(&a.tokens));
            t.truncate(5);
            t
        },
        upstreams: ranked(upstream_tally)
            .into_iter()
            .map(|(name, calls, failed)| UpstreamUsage {
                name,
                calls,
                failed,
            })
            .collect(),
        throughput: Throughput {
            peak_per_min,
            avg_per_min: round1(total as f64 / window_minutes),
            busiest_hour,
        },
        hourly,
        agents_seen: AgentsSeen { new, returning },
        timeseries: series,
    }
}

fn ranked_actor_entries(
    tally: BTreeMap<String, (String, u64)>,
    kind: &'static str,
    limit: usize,
) -> Vec<ActorUsageEntry> {
    let mut entries: Vec<ActorUsageEntry> = tally
        .into_iter()
        .map(|(id, (label, calls))| ActorUsageEntry {
            id,
            label,
            kind,
            calls,
        })
        .collect();
    entries.sort_by(|a, b| b.calls.cmp(&a.calls).then_with(|| a.id.cmp(&b.id)));
    entries.truncate(limit);
    entries
}

// ── Drill-down ─────────────────────────────────────────────────────────────

/// One dispatched call, serialized for the explorer + recent-call lists.
#[derive(Serialize)]
pub struct ToolCallRecord {
    pub id: String,
    pub ts: i64,
    pub tool: String,
    pub action: Option<String>,
    pub agent_id: String,
    pub agent_label: String,
    pub agent_kind: &'static str,
    pub ip: String,
    pub surface: String,
    pub outcome: &'static str,
    pub error_kind: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub elapsed_ms: u64,
}

#[derive(Serialize)]
pub struct ToolDetail {
    pub name: String,
    pub window: String,
    pub calls: u64,
    pub failed: u64,
    pub total_tokens: u64,
    pub avg_tokens: u64,
    pub avg_elapsed_ms: u64,
    pub timeseries: Vec<MetricsBucket>,
    pub top_callers: Vec<ActorUsageEntry>,
    pub recent: Vec<ToolCallRecord>,
}

#[derive(Serialize)]
pub struct AgentDetail {
    pub id: String,
    pub label: String,
    pub kind: &'static str,
    pub window: String,
    pub calls: u64,
    pub failed: u64,
    pub total_tokens: u64,
    pub tools_used: Vec<ToolUsageEntry>,
    pub timeseries: Vec<MetricsBucket>,
    pub recent: Vec<ToolCallRecord>,
}

#[derive(Deserialize)]
pub struct ToolCallQuery {
    pub window: String,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub ip: Option<String>,
    #[serde(default)]
    pub outcome: Option<String>,
    #[serde(default)]
    pub surface: Option<String>,
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}

#[derive(Serialize)]
pub struct ToolCallPage {
    pub calls: Vec<ToolCallRecord>,
    pub total: usize,
    pub filtered: usize,
    pub facets: ToolCallFacets,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ToolCallFacets {
    pub tools: Vec<String>,
    pub agents: Vec<ToolCallAgentFacet>,
    pub ips: Vec<String>,
    pub surfaces: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolCallAgentFacet {
    pub id: String,
    pub label: String,
}

fn record_from_call(event: &LogEvent, call: &Call, id: String) -> ToolCallRecord {
    let actor = call.actor.clone().unwrap_or_else(|| "unknown".to_string());
    let agent_kind = if call.agent_kind == "device" {
        "device"
    } else {
        "agent"
    };
    ToolCallRecord {
        id,
        ts: event.ts,
        tool: call.service.clone(),
        action: event.action.clone(),
        agent_id: actor.clone(),
        agent_label: call
            .actor_label
            .clone()
            .unwrap_or_else(|| safe_actor_label(&actor, None, str_field(field(event, "subject")))),
        agent_kind,
        ip: call.ip.clone().unwrap_or_default(),
        surface: call.surface.clone(),
        outcome: if call.ok { "ok" } else { "failed" },
        error_kind: call.error_kind.clone(),
        input_tokens: call.input_tokens,
        output_tokens: call.output_tokens,
        elapsed_ms: call.elapsed_ms.round() as u64,
    }
}

fn extract_usage_records(event: &LogEvent) -> Vec<ToolCallRecord> {
    let calls = extract_usage_calls(event);
    if calls.len() == 1 {
        return calls
            .first()
            .map(|call| record_from_call(event, call, event.event_id.clone()))
            .into_iter()
            .collect();
    }
    calls
        .iter()
        .enumerate()
        .map(|(index, call)| record_from_call(event, call, format!("{}:{index}", event.event_id)))
        .collect()
}

/// Bucket records into the window's call-volume series (oldest first).
fn build_series(records: &[ToolCallRecord], window: MetricsWindow, now: i64) -> Vec<MetricsBucket> {
    let since = now - window.ms();
    let buckets = window.buckets();
    let bucket_ms = window.ms() / buckets as i64;
    let mut series: Vec<MetricsBucket> = (0..buckets)
        .map(|i| MetricsBucket {
            ts: since + bucket_ms * i as i64,
            calls: 0,
            failed: 0,
        })
        .collect();
    for r in records {
        let idx = (((r.ts - since) / bucket_ms).max(0) as usize).min(buckets - 1);
        series[idx].calls += 1;
        if r.outcome == "failed" {
            series[idx].failed += 1;
        }
    }
    series
}

fn recent_sorted(mut records: Vec<ToolCallRecord>, limit: usize) -> Vec<ToolCallRecord> {
    records.sort_by(|a, b| b.ts.cmp(&a.ts));
    records.truncate(limit);
    records
}

/// Single-tool drill-down.
pub fn tool_detail(events: &[LogEvent], name: &str, window: MetricsWindow, now: i64) -> ToolDetail {
    let records: Vec<ToolCallRecord> = events
        .iter()
        .flat_map(extract_usage_records)
        .filter(|r| r.tool == name)
        .collect();
    let calls = records.len() as u64;
    let failed = records.iter().filter(|r| r.outcome == "failed").count() as u64;
    let total_tokens: u64 = records
        .iter()
        .map(|r| r.input_tokens + r.output_tokens)
        .sum();
    let total_elapsed: u64 = records.iter().map(|r| r.elapsed_ms).sum();

    let mut callers: BTreeMap<String, u64> = BTreeMap::new();
    for r in &records {
        *callers.entry(r.agent_id.clone()).or_default() += 1;
    }
    let mut top_callers: Vec<ActorUsageEntry> = callers
        .into_iter()
        .map(|(id, c)| ActorUsageEntry {
            id: id.clone(),
            label: records
                .iter()
                .find(|r| r.agent_id == id)
                .map(|r| r.agent_label.clone())
                .unwrap_or_else(|| id.clone()),
            kind: records
                .iter()
                .find(|r| r.agent_id == id)
                .map_or("agent", |r| r.agent_kind),
            calls: c,
        })
        .collect();
    top_callers.sort_by(|a, b| b.calls.cmp(&a.calls).then_with(|| a.id.cmp(&b.id)));
    top_callers.truncate(5);

    let timeseries = build_series(&records, window, now);
    ToolDetail {
        name: name.to_string(),
        window: window.label().to_string(),
        calls,
        failed,
        total_tokens,
        avg_tokens: if calls > 0 { total_tokens / calls } else { 0 },
        avg_elapsed_ms: if calls > 0 { total_elapsed / calls } else { 0 },
        timeseries,
        top_callers,
        recent: recent_sorted(records, 25),
    }
}

/// Single-agent/device drill-down.
pub fn agent_detail(events: &[LogEvent], id: &str, window: MetricsWindow, now: i64) -> AgentDetail {
    let records: Vec<ToolCallRecord> = events
        .iter()
        .flat_map(extract_usage_records)
        .filter(|r| r.agent_id == id)
        .collect();
    let calls = records.len() as u64;
    let failed = records.iter().filter(|r| r.outcome == "failed").count() as u64;
    let total_tokens: u64 = records
        .iter()
        .map(|r| r.input_tokens + r.output_tokens)
        .sum();
    let label = records
        .first()
        .map_or_else(|| id.to_string(), |r| r.agent_label.clone());

    let mut tally: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for r in &records {
        let t = tally.entry(r.tool.clone()).or_default();
        t.0 += 1;
        if r.outcome == "failed" {
            t.1 += 1;
        }
    }
    let tools_used: Vec<ToolUsageEntry> = ranked(tally)
        .into_iter()
        .map(|(name, calls, failed)| ToolUsageEntry {
            name,
            calls,
            failed,
        })
        .collect();

    let timeseries = build_series(&records, window, now);
    AgentDetail {
        id: id.to_string(),
        label,
        kind: records.first().map_or("agent", |r| r.agent_kind),
        window: window.label().to_string(),
        calls,
        failed,
        total_tokens,
        tools_used,
        timeseries,
        recent: recent_sorted(records, 25),
    }
}

/// Filterable, paginated tool-call log for the explorer.
pub fn tool_calls(events: &[LogEvent], query: &ToolCallQuery) -> ToolCallPage {
    let all: Vec<ToolCallRecord> = events.iter().flat_map(extract_usage_records).collect();
    let total = all.len();
    let facets = build_facets(&all);
    let search = query.search.as_deref().map(str::to_lowercase);

    let mut filtered: Vec<ToolCallRecord> = all
        .into_iter()
        .filter(|r| {
            if query.tool.as_deref().is_some_and(|t| r.tool != t) {
                return false;
            }
            if query.agent.as_deref().is_some_and(|a| r.agent_id != a) {
                return false;
            }
            if query.ip.as_deref().is_some_and(|ip| r.ip != ip) {
                return false;
            }
            if query.outcome.as_deref().is_some_and(|o| r.outcome != o) {
                return false;
            }
            if query.surface.as_deref().is_some_and(|s| r.surface != s) {
                return false;
            }
            if let Some(q) = &search {
                let hay = format!(
                    "{} {} {} {} {}",
                    r.tool,
                    r.action.as_deref().unwrap_or(""),
                    r.agent_label,
                    r.ip,
                    r.error_kind.as_deref().unwrap_or(""),
                )
                .to_lowercase();
                if !hay.contains(q) {
                    return false;
                }
            }
            true
        })
        .collect();
    filtered.sort_by(|a, b| b.ts.cmp(&a.ts));

    let filtered_count = filtered.len();
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(50);
    let calls = filtered.into_iter().skip(offset).take(limit).collect();
    ToolCallPage {
        calls,
        total,
        filtered: filtered_count,
        facets,
    }
}

fn build_facets(records: &[ToolCallRecord]) -> ToolCallFacets {
    let mut tools = BTreeSet::new();
    let mut agents: BTreeMap<String, String> = BTreeMap::new();
    let mut ips = BTreeSet::new();
    let mut surfaces = BTreeSet::new();

    for record in records {
        if !record.tool.is_empty() {
            tools.insert(record.tool.clone());
        }
        if !record.agent_id.is_empty() {
            agents
                .entry(record.agent_id.clone())
                .or_insert_with(|| record.agent_label.clone());
        }
        if !record.ip.trim().is_empty() {
            ips.insert(record.ip.clone());
        }
        if !record.surface.is_empty() {
            surfaces.insert(record.surface.clone());
        }
    }

    let mut agents: Vec<ToolCallAgentFacet> = agents
        .into_iter()
        .map(|(id, label)| ToolCallAgentFacet { id, label })
        .collect();
    agents.sort_by(|a, b| a.label.cmp(&b.label).then_with(|| a.id.cmp(&b.id)));

    ToolCallFacets {
        tools: tools.into_iter().collect(),
        agents,
        ips: ips.into_iter().collect(),
        surfaces: surfaces.into_iter().collect(),
    }
}

#[cfg(test)]
mod tests;
