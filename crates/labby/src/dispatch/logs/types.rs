//! Wire types and runtime façade for the local-master log subsystem.
//!
//! All types here are serialized to JSON at the HTTP/MCP boundary and MUST
//! preserve the snake_case + lowercase casing pinned by the gateway-admin
//! TypeScript consumer in `apps/gateway-admin/lib/types/logs.ts`.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::dispatch::error::ToolError;

// ── Enums ────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
#[clap(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }

    /// Parse a case-insensitive level name. Recognizes standard aliases
    /// (`warning`, `err`, `information`).
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "trace" => Self::Trace,
            "debug" => Self::Debug,
            "info" | "information" => Self::Info,
            "warn" | "warning" => Self::Warn,
            "error" | "err" => Self::Error,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
#[clap(rename_all = "snake_case")]
pub enum Subsystem {
    Gateway,
    McpServer,
    McpClient,
    Api,
    Web,
    OauthRelay,
    AuthWebui,
    AuthMcp,
    AuthUpstream,
    CoreRuntime,
    Syslog,
}

impl Subsystem {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gateway => "gateway",
            Self::McpServer => "mcp_server",
            Self::McpClient => "mcp_client",
            Self::Api => "api",
            Self::Web => "web",
            Self::OauthRelay => "oauth_relay",
            Self::AuthWebui => "auth_webui",
            Self::AuthMcp => "auth_mcp",
            Self::AuthUpstream => "auth_upstream",
            Self::CoreRuntime => "core_runtime",
            Self::Syslog => "syslog",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "gateway" => Self::Gateway,
            "mcp_server" => Self::McpServer,
            "mcp_client" => Self::McpClient,
            "api" => Self::Api,
            "web" => Self::Web,
            "oauth_relay" => Self::OauthRelay,
            "auth_webui" => Self::AuthWebui,
            "auth_mcp" => Self::AuthMcp,
            "auth_upstream" => Self::AuthUpstream,
            "core_runtime" => Self::CoreRuntime,
            "syslog" => Self::Syslog,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
#[clap(rename_all = "snake_case")]
pub enum Surface {
    Cli,
    Mcp,
    Api,
    Web,
    Acp,
    Dispatch,
    Node,
    CoreRuntime,
}

impl Surface {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Mcp => "mcp",
            Self::Api => "api",
            Self::Web => "web",
            Self::Acp => "acp",
            Self::Dispatch => "dispatch",
            Self::Node => "node",
            Self::CoreRuntime => "core_runtime",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "cli" => Self::Cli,
            "mcp" => Self::Mcp,
            "api" => Self::Api,
            "web" => Self::Web,
            "acp" => Self::Acp,
            "dispatch" => Self::Dispatch,
            "node" => Self::Node,
            "core_runtime" => Self::CoreRuntime,
            _ => return None,
        })
    }
}

// ── Events ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogEvent {
    pub event_id: String,
    pub ts: i64,
    pub level: LogLevel,
    pub subsystem: Subsystem,
    pub surface: Surface,
    #[serde(default)]
    pub action: Option<String>,
    pub message: String,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub correlation_id: Option<String>,
    #[serde(default)]
    pub trace_id: Option<String>,
    #[serde(default)]
    pub span_id: Option<String>,
    #[serde(default)]
    pub instance: Option<String>,
    #[serde(default)]
    pub auth_flow: Option<String>,
    #[serde(default)]
    pub outcome_kind: Option<String>,
    #[serde(default)]
    pub fields_json: serde_json::Value,
    #[serde(default)]
    pub source_kind: Option<String>,
    #[serde(default)]
    pub source_node_id: Option<String>,
    #[serde(default)]
    pub source_device_id: Option<String>,
    #[serde(default)]
    pub actor_key: Option<String>,
    #[serde(default)]
    pub ingest_path: Option<String>,
    #[serde(default)]
    pub upstream_event_id: Option<String>,
}

impl LogEvent {
    #[must_use]
    #[doc(hidden)]
    #[allow(dead_code)]
    pub fn fixture() -> Self {
        Self {
            event_id: "evt-fixture".to_string(),
            ts: 1_713_225_600_000,
            level: LogLevel::Info,
            subsystem: Subsystem::CoreRuntime,
            surface: Surface::CoreRuntime,
            action: Some("fixture.event".to_string()),
            message: "fixture log event".to_string(),
            request_id: Some("req-fixture".to_string()),
            session_id: Some("sess-fixture".to_string()),
            correlation_id: Some("corr-fixture".to_string()),
            trace_id: Some("trace-fixture".to_string()),
            span_id: Some("span-fixture".to_string()),
            instance: Some("default".to_string()),
            auth_flow: None,
            outcome_kind: Some("ok".to_string()),
            fields_json: serde_json::json!({}),
            source_kind: Some("local".to_string()),
            source_node_id: Some("node-local".to_string()),
            source_device_id: Some("device-local".to_string()),
            actor_key: Some("actor-fixture".to_string()),
            ingest_path: Some("tracing".to_string()),
            upstream_event_id: None,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RawLogEvent {
    #[serde(default)]
    pub ts: Option<i64>,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub subsystem: Option<String>,
    #[serde(default)]
    pub surface: Option<String>,
    #[serde(default)]
    pub action: Option<String>,
    pub message: String,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub correlation_id: Option<String>,
    #[serde(default)]
    pub trace_id: Option<String>,
    #[serde(default)]
    pub span_id: Option<String>,
    #[serde(default)]
    pub instance: Option<String>,
    #[serde(default)]
    pub auth_flow: Option<String>,
    #[serde(default)]
    pub outcome_kind: Option<String>,
    #[serde(default)]
    pub fields_json: serde_json::Value,
    #[serde(default)]
    pub source_kind: Option<String>,
    #[serde(default)]
    pub source_node_id: Option<String>,
    #[serde(default)]
    pub source_device_id: Option<String>,
    #[serde(default)]
    pub actor_key: Option<String>,
    #[serde(default)]
    pub ingest_path: Option<String>,
    #[serde(default)]
    pub upstream_event_id: Option<String>,
}

// ── Queries ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LogQuery {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub after_ts: Option<i64>,
    #[serde(default)]
    pub before_ts: Option<i64>,
    #[serde(default)]
    pub levels: Vec<LogLevel>,
    #[serde(default)]
    pub subsystems: Vec<Subsystem>,
    #[serde(default)]
    pub surfaces: Vec<Surface>,
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub correlation_id: Option<String>,
    #[serde(default)]
    pub source_node_ids: Vec<String>,
    #[serde(default)]
    pub source_kinds: Vec<String>,
    #[serde(default)]
    pub actor_key: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LogTailRequest {
    #[serde(default)]
    pub after_ts: Option<i64>,
    #[serde(default)]
    pub since_event_id: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

// ── Results ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogSearchResult {
    pub events: Vec<LogEvent>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogTailResult {
    pub events: Vec<LogEvent>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct LogRetention {
    pub max_age_days: u64,
    pub max_bytes: u64,
}

impl Default for LogRetention {
    fn default() -> Self {
        Self {
            max_age_days: 7,
            max_bytes: 256 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogStoreStats {
    pub on_disk_bytes: u64,
    pub oldest_retained_ts: Option<i64>,
    pub newest_retained_ts: Option<i64>,
    pub total_event_count: u64,
    pub dropped_event_count: u64,
    pub retention: LogRetention,
}

// ── Peer ingest ──────────────────────────────────────────────────────────────

/// Batch of raw events forwarded from a peer node via `POST /v1/logs/ingest`.
///
/// The `node_id` field overrides `source_node_id` on every event so the master
/// can trust the node identity from the request, not from self-reported event fields.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PeerIngestRequest {
    pub node_id: String,
    pub events: Vec<RawLogEvent>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PeerIngestResponse {
    pub accepted: usize,
    pub dropped: usize,
}

// ── Stream ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StreamSubscription {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub levels: Vec<LogLevel>,
    #[serde(default)]
    pub subsystems: Vec<Subsystem>,
    #[serde(default)]
    pub surfaces: Vec<Surface>,
}

pub struct LogStreamReceiver {
    inner: broadcast::Receiver<LogEvent>,
    filter: StreamSubscription,
}

impl LogStreamReceiver {
    pub(super) fn new(inner: broadcast::Receiver<LogEvent>, filter: StreamSubscription) -> Self {
        Self { inner, filter }
    }

    pub async fn recv(&mut self) -> Result<LogEvent, broadcast::error::RecvError> {
        loop {
            let event = self.inner.recv().await?;
            if self.matches(&event) {
                return Ok(event);
            }
        }
    }

    fn matches(&self, event: &LogEvent) -> bool {
        let f = &self.filter;
        if !f.levels.is_empty() && !f.levels.contains(&event.level) {
            return false;
        }
        if !f.subsystems.is_empty() && !f.subsystems.contains(&event.subsystem) {
            return false;
        }
        if !f.surfaces.is_empty() && !f.surfaces.contains(&event.surface) {
            return false;
        }
        if let Some(needle) = &f.text {
            if !event.message.contains(needle.as_str()) {
                return false;
            }
        }
        true
    }
}

// ── Runtime façade ───────────────────────────────────────────────────────────

pub struct LogSystem {
    pub(super) store: Arc<super::store::LogStore>,
    pub(super) hub: Arc<super::stream::StreamHub>,
    pub(super) ingest: super::ingest::IngestHandle,
    pub(super) counters: Arc<super::ingest::IngestCounters>,
    pub(super) maintenance_task: tokio::task::JoinHandle<()>,
}

impl Drop for LogSystem {
    fn drop(&mut self) {
        self.maintenance_task.abort();
    }
}

impl LogSystem {
    #[doc(hidden)]
    #[allow(dead_code)]
    pub async fn ingest(&self, raw: RawLogEvent) -> Result<(), ToolError> {
        self.ingest.submit(raw).await
    }

    pub fn try_ingest(&self, raw: RawLogEvent) -> Result<(), ToolError> {
        self.ingest.try_submit(raw)
    }

    pub async fn search(&self, query: LogQuery) -> Result<LogSearchResult, ToolError> {
        self.store.search(query).await
    }

    pub async fn tail(&self, req: LogTailRequest) -> Result<LogTailResult, ToolError> {
        self.store.tail(req).await
    }

    pub async fn stats(&self) -> Result<LogStoreStats, ToolError> {
        let mut stats = self.store.stats().await?;
        stats.dropped_event_count = self.counters.dropped();
        Ok(stats)
    }

    /// Fetch all dispatch-completion events in a rolling window. Returns
    /// `(now_ms, events)`. Shared by all dashboard aggregation entry points.
    async fn fetch_window(
        &self,
        window: super::metrics::MetricsWindow,
    ) -> Result<(i64, Vec<LogEvent>), ToolError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let events = self
            .store
            .completion_events(Some(now - window.ms()), Some(now))
            .await?;
        Ok((now, events))
    }

    async fn fetch_previous_actor_ids(
        &self,
        before_ts: i64,
    ) -> Result<BTreeSet<String>, ToolError> {
        self.store.previous_completion_actor_ids(before_ts).await
    }

    /// Aggregate dispatch-completion events over a rolling window into the
    /// dashboard usage-metrics shape (see `super::metrics`).
    pub async fn metrics(
        &self,
        window: super::metrics::MetricsWindow,
    ) -> Result<super::metrics::DashboardMetrics, ToolError> {
        let (now, events) = self.fetch_window(window).await?;
        let previous = self.fetch_previous_actor_ids(now - window.ms()).await?;
        Ok(super::metrics::aggregate_with_previous(
            &events, window, now, &previous,
        ))
    }

    /// Single-tool drill-down over the window.
    pub async fn tool_detail(
        &self,
        tool: String,
        window: super::metrics::MetricsWindow,
    ) -> Result<super::metrics::ToolDetail, ToolError> {
        let (now, events) = self.fetch_window(window).await?;
        Ok(super::metrics::tool_detail(&events, &tool, window, now))
    }

    /// Single-agent/device drill-down over the window.
    pub async fn agent_detail(
        &self,
        agent: String,
        window: super::metrics::MetricsWindow,
    ) -> Result<super::metrics::AgentDetail, ToolError> {
        let (now, events) = self.fetch_window(window).await?;
        Ok(super::metrics::agent_detail(&events, &agent, window, now))
    }

    /// Filterable, paginated tool-call log for the explorer.
    pub async fn tool_calls(
        &self,
        query: super::metrics::ToolCallQuery,
    ) -> Result<super::metrics::ToolCallPage, ToolError> {
        let window = super::metrics::MetricsWindow::parse(&query.window)
            .unwrap_or(super::metrics::MetricsWindow::H24);
        let (_now, events) = self.fetch_window(window).await?;
        Ok(super::metrics::tool_calls(&events, &query))
    }

    pub async fn subscribe(&self, sub: StreamSubscription) -> Result<LogStreamReceiver, ToolError> {
        Ok(self.hub.subscribe(sub))
    }
}
