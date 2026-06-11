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

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

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
    match tool {
        "radarr" | "sonarr" => "media-stack",
        "qbittorrent" => "downloads",
        "code_execute" | "code_search" => "code-mode",
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
    call_count: Option<u64>,
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

/// Is this a completed tool-call event? Completion logs carry both token
/// fields; start events carry only `input_tokens`.
fn extract_call(event: &LogEvent) -> Option<Call> {
    let input = field(event, "input_tokens")?;
    let output = field(event, "output_tokens")?;
    let service = str_field(field(event, "service")).or_else(|| str_field(field(event, "tool")))?;
    let ok = event.message.ends_with(" ok");
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
        actor: event
            .actor_key
            .clone()
            .or_else(|| str_field(field(event, "subject"))),
        call_count: field(event, "call_count").map(|v| num_u64(Some(v))),
    })
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
pub fn aggregate(events: &[LogEvent], window: MetricsWindow, now_ms: i64) -> DashboardMetrics {
    let calls: Vec<Call> = events.iter().filter_map(extract_call).collect();
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
    let mut actor_tally: BTreeMap<String, u64> = BTreeMap::new();
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
            *actor_tally.entry(actor.clone()).or_default() += 1;
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

    // Code Mode fan-out (events with service == code_execute carry call_count).
    let code_runs: Vec<&Call> = calls
        .iter()
        .filter(|c| c.service == "code_execute")
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

    // Actors — grouped by actor key. Device/IP classification is tier-2 work;
    // until the gateway records those dimensions, all actors land under `agent`.
    let actors_ranked = ranked(actor_tally.into_iter().map(|(k, c)| (k, (c, 0))).collect());
    let agent_top: Vec<ActorUsageEntry> = actors_ranked
        .iter()
        .take(8)
        .map(|(id, c, _)| ActorUsageEntry {
            id: id.clone(),
            label: id.clone(),
            kind: "agent",
            calls: *c,
        })
        .collect();
    let distinct_actors = actors_ranked.len();

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
                active: distinct_actors,
                top: agent_top,
            },
            device: ActorFacet {
                active: 0,
                top: Vec::new(),
            },
            ip: ActorFacet {
                active: 0,
                top: Vec::new(),
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
            // Truncation + artifact writes are not yet logged distinctly.
            truncation_rate: 0.0,
            artifact_writes: 0,
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
        // New-vs-returning needs first-seen history; report all as new for now.
        agents_seen: AgentsSeen {
            new: distinct_actors as u64,
            returning: 0,
        },
        timeseries: series,
    }
}

#[cfg(test)]
mod tests;
