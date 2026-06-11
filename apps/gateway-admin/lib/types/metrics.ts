/**
 * Gateway usage-metrics contract.
 *
 * The dashboard composes three sources:
 *   - servers / tools   → `useGateways()`        (existing)
 *   - devices / nodes   → `fetchFleetDevices()`  (existing)
 *   - activity metrics  → `fetchDashboardMetrics()` (this module's shape)
 *
 * Activity metrics are aggregated server-side from the persisted log store
 * over a bounded time window. This file is the single source of truth for the
 * JSON shape; the Rust `logs.metrics` action mirrors it once the backend lands.
 */

/** Rolling activity window. `7d` is the log-store retention ceiling. */
export const METRICS_WINDOWS = ['1h', '24h', '7d'] as const
export type MetricsWindow = (typeof METRICS_WINDOWS)[number]

/** A tool ranked by call volume within the window. */
export interface ToolUsageEntry {
  /** Service / tool name as dispatched (e.g. `radarr`, `code_execute`). */
  name: string
  /** Total dispatched calls in the window. */
  calls: number
  /** Calls that returned a non-`ok` outcome. */
  failed: number
}

/** An agent / device ranked by activity within the window. */
export interface AgentUsageEntry {
  /** Stable identity — subject, session, or source node id. */
  id: string
  /** Human label (falls back to a truncated id). */
  label: string
  /** Origin surface for the activity. */
  kind: 'agent' | 'device'
  /** Total dispatched calls attributed to this identity. */
  calls: number
}

/** Facets the "most active" list can be grouped by. Agents, devices, and source
 * IPs are distinct populations — never mix their counts in one ranking. */
export type ActorKind = 'agent' | 'device' | 'ip'

/** One ranked actor within a single facet. */
export interface ActorUsageEntry {
  id: string
  label: string
  kind: ActorKind
  calls: number
}

/** A ranked population for one actor facet. */
export interface ActorFacet {
  /** Distinct actors of this kind active in the window. */
  active: number
  /** Busiest actors, sorted by `calls` desc. */
  top: ActorUsageEntry[]
}

/** One time bucket of the call-volume series. */
export interface MetricsBucket {
  /** Bucket start, epoch ms. */
  ts: number
  /** Total calls in the bucket. */
  calls: number
  /** Failed calls in the bucket. */
  failed: number
}

export interface DashboardMetrics {
  window: MetricsWindow
  /** Window bounds, epoch ms. */
  since_ms: number
  until_ms: number

  tool_calls: {
    total: number
    failed: number
    succeeded: number
  }

  tools: {
    /** Most-used tools, sorted by `calls` desc. */
    top: ToolUsageEntry[]
    /** Least-used tools (non-zero), sorted by `calls` asc. */
    least: ToolUsageEntry[]
    /** Distinct tools dispatched in the window. */
    distinct: number
  }

  tokens: {
    input: number
    output: number
    total: number
    /** `total / tool_calls.total`, rounded. */
    avg_per_call: number
  }

  /** "Most active" split into distinct populations — agents, devices, and
   * source IPs are ranked separately, never compared in one count. */
  actors: {
    agent: ActorFacet
    device: ActorFacet
    ip: ActorFacet
  }

  /** Code Mode `execute` fan-out — runs that dispatch >=1 upstream callTool(). */
  fan_out: {
    runs: number
    total_calls: number
    avg_calls_per_run: number
    max_calls_in_run: number
    /** Share of execute runs that hit the wall-clock timeout (0–1). */
    timeout_rate: number
    /** Share of runs whose result was truncated to the envelope budget (0–1). */
    truncation_rate: number
    /** Artifacts written by execute runs in the window. */
    artifact_writes: number
  }

  /** Dispatch latency, ms. Percentiles over every call in the window. */
  latency: {
    p50: number
    p95: number
    p99: number
    avg: number
    /** Slowest tools by average latency. */
    slowest: LatencyStat[]
  }

  /** Failure breakdown by stable error kind. */
  errors: {
    total: number
    by_kind: ErrorKindCount[]
  }

  /** Call volume by origin surface (MCP / API / CLI / Web). */
  surfaces: SurfaceCount[]

  /** Token spend grouped by tool — cost attribution. */
  tokens_by_tool: TokenByTool[]

  /** Call volume grouped by upstream server. */
  upstreams: UpstreamUsage[]

  /** Throughput summary. */
  throughput: {
    peak_per_min: number
    avg_per_min: number
    /** Hour of day (0–23, local) with the most calls. */
    busiest_hour: number
  }

  /** Calls by hour of day (0–23) — activity rhythm / heat strip. */
  hourly: HourBucket[]

  /** Distinct agents first seen in the window vs. seen before. */
  agents_seen: {
    new: number
    returning: number
  }

  /** Call-volume series for the window, oldest bucket first. */
  timeseries: MetricsBucket[]
}

export interface LatencyStat {
  name: string
  avg_ms: number
}

export interface ErrorKindCount {
  kind: string
  count: number
}

export interface SurfaceCount {
  surface: CallSurface
  calls: number
}

export interface TokenByTool {
  name: string
  tokens: number
}

export interface UpstreamUsage {
  name: string
  calls: number
  failed: number
}

export interface HourBucket {
  /** Hour of day, 0–23. */
  hour: number
  calls: number
}

// ── Drill-down ─────────────────────────────────────────────────────────────

export type CallOutcome = 'ok' | 'failed'
export type CallSurface = 'mcp' | 'api' | 'cli' | 'web'

/** A single dispatched tool call — the atom every drill-down rolls up from. */
export interface ToolCallRecord {
  id: string
  ts: number
  tool: string
  action: string | null
  agent_id: string
  agent_label: string
  agent_kind: 'agent' | 'device'
  /** Source IP the call originated from. */
  ip: string
  surface: CallSurface
  outcome: CallOutcome
  error_kind: string | null
  input_tokens: number
  output_tokens: number
  elapsed_ms: number
}

/** Single-tool drill-down (drawer). */
export interface ToolDetail {
  name: string
  window: MetricsWindow
  calls: number
  failed: number
  total_tokens: number
  avg_tokens: number
  avg_elapsed_ms: number
  timeseries: MetricsBucket[]
  top_callers: AgentUsageEntry[]
  recent: ToolCallRecord[]
}

/** Single-agent/device drill-down (drawer). */
export interface AgentDetail {
  id: string
  label: string
  kind: 'agent' | 'device'
  window: MetricsWindow
  calls: number
  failed: number
  total_tokens: number
  tools_used: ToolUsageEntry[]
  timeseries: MetricsBucket[]
  recent: ToolCallRecord[]
}

/** Tool-call explorer filters. */
export interface ToolCallQuery {
  window: MetricsWindow
  tool?: string
  agent?: string
  ip?: string
  outcome?: CallOutcome
  surface?: CallSurface
  search?: string
  limit?: number
  offset?: number
}

/** A page of explorer results. `total` is all calls in the window; `filtered` is after filters. */
export interface ToolCallPage {
  calls: ToolCallRecord[]
  total: number
  filtered: number
}
