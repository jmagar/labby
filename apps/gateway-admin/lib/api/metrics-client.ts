import { normalizeGatewayApiBase } from './gateway-config.ts'
import { gatewayRequestInit } from './gateway-request.ts'
import type {
  ActorFacet,
  ActorKind,
  ActorUsageEntry,
  AgentDetail,
  AgentUsageEntry,
  CallSurface,
  DashboardMetrics,
  MetricsBucket,
  MetricsWindow,
  ToolCallPage,
  ToolCallQuery,
  ToolCallRecord,
  ToolDetail,
  ToolUsageEntry,
} from '../types/metrics.ts'

const USE_MOCK_DATA = process.env.NEXT_PUBLIC_MOCK_DATA === 'true'

export type MetricsRequestOptions = {
  baseUrl?: string
  token?: string
  signal?: AbortSignal
  standaloneBearerAuth?: boolean
}

const WINDOW_MS: Record<MetricsWindow, number> = {
  '1h': 60 * 60 * 1000,
  '24h': 24 * 60 * 60 * 1000,
  '7d': 7 * 24 * 60 * 60 * 1000,
}

const WINDOW_BUCKETS: Record<MetricsWindow, number> = {
  '1h': 12,
  '24h': 24,
  '7d': 14,
}

const WINDOW_SCALE: Record<MetricsWindow, number> = { '1h': 1, '24h': 14, '7d': 78 }

// ── Mock fixtures ──────────────────────────────────────────────────────────

const MOCK_TOOLS: Array<{ name: string; weight: number; failRate: number; actions: string[] }> = [
  { name: 'code_execute', weight: 26, failRate: 0.08, actions: ['call_tool'] },
  { name: 'radarr', weight: 19, failRate: 0.03, actions: ['movie.search', 'movie.add', 'queue.list'] },
  { name: 'sonarr', weight: 17, failRate: 0.04, actions: ['series.search', 'episode.list', 'queue.list'] },
  { name: 'cortex', weight: 14, failRate: 0.06, actions: ['logs.search', 'logs.stats'] },
  { name: 'code_search', weight: 12, failRate: 0.02, actions: ['call_tool'] },
  { name: 'tailscale', weight: 9, failRate: 0.05, actions: ['devices.list', 'device.get'] },
  { name: 'gotify', weight: 6, failRate: 0.01, actions: ['message.send'] },
  { name: 'unraid', weight: 4, failRate: 0.09, actions: ['array.status', 'docker.list'] },
  { name: 'qbittorrent', weight: 2, failRate: 0.12, actions: ['torrent.list', 'torrent.add'] },
]

const MOCK_AGENTS: Array<{
  id: string
  label: string
  kind: 'agent' | 'device'
  weight: number
  ips: string[]
}> = [
  { id: 'claude-code', label: 'Claude Code', kind: 'agent', weight: 34, ips: ['100.88.16.79', '100.74.16.82'] },
  { id: 'codex-cli', label: 'Codex CLI', kind: 'agent', weight: 22, ips: ['100.88.16.79'] },
  { id: 'dookie', label: 'dookie', kind: 'device', weight: 18, ips: ['100.88.16.79'] },
  { id: 'zed-acp', label: 'Zed ACP', kind: 'agent', weight: 14, ips: ['100.74.16.82'] },
  { id: 'steamy-wsl', label: 'steamy-wsl', kind: 'device', weight: 8, ips: ['100.74.16.82'] },
  { id: 'agent-os', label: 'agent-os', kind: 'device', weight: 4, ips: ['100.109.125.128'] },
]

const SURFACES_BY_TOOL: Record<string, CallSurface[]> = {
  code_execute: ['mcp'],
  code_search: ['mcp'],
}
const DEFAULT_SURFACES: CallSurface[] = ['mcp', 'api', 'cli', 'web']
const ERROR_KINDS = ['rate_limited', 'auth_failed', 'not_found', 'validation_failed', 'timeout', 'server_error']

/** Tool → upstream server, for per-server traffic rollups. */
const TOOL_UPSTREAM: Record<string, string> = {
  radarr: 'media-stack',
  sonarr: 'media-stack',
  qbittorrent: 'downloads',
  code_execute: 'code-mode',
  code_search: 'code-mode',
  cortex: 'cortex',
  tailscale: 'tailscale',
  gotify: 'gotify',
  unraid: 'unraid',
}

// ── Seeded RNG (stable per window → no flicker on revalidate) ───────────────

function mulberry32(seed: number): () => number {
  let a = seed >>> 0
  return () => {
    a += 0x6d2b79f5
    let t = a
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

function seedFromString(s: string): number {
  let h = 2166136261
  for (let i = 0; i < s.length; i += 1) {
    h ^= s.charCodeAt(i)
    h = Math.imul(h, 16777619)
  }
  return h >>> 0
}

function pickWeighted<T extends { weight: number }>(items: T[], r: number): T {
  const total = items.reduce((sum, item) => sum + item.weight, 0)
  let acc = r * total
  for (const item of items) {
    acc -= item.weight
    if (acc <= 0) return item
  }
  return items[items.length - 1]
}

// ── Call-stream generation (the single source for every drill-down) ─────────

const streamCache = new Map<string, ToolCallRecord[]>()

function buildCallStream(window: MetricsWindow, now: number): ToolCallRecord[] {
  const cacheKey = `${window}:${Math.floor(now / WINDOW_MS['1h'])}`
  const cached = streamCache.get(cacheKey)
  if (cached) return cached

  const rnd = mulberry32(seedFromString(`stream:${window}`))
  const target = Math.round(47 * WINDOW_SCALE[window])
  const windowMs = WINDOW_MS[window]
  const records: ToolCallRecord[] = []

  for (let i = 0; i < target; i += 1) {
    const tool = pickWeighted(MOCK_TOOLS, rnd())
    const agent = pickWeighted(MOCK_AGENTS, rnd())
    const ip = agent.ips[Math.floor(rnd() * agent.ips.length)] ?? '0.0.0.0'
    const failed = rnd() < tool.failRate
    // Triangular distribution → a believable mid-window traffic hump.
    const pos = (rnd() + rnd()) / 2
    const ts = now - windowMs + Math.floor(pos * windowMs)
    const surfaces = SURFACES_BY_TOOL[tool.name] ?? DEFAULT_SURFACES
    const isCode = tool.name.startsWith('code_')

    records.push({
      id: `call-${window}-${i}`,
      ts,
      tool: tool.name,
      action: tool.actions[Math.floor(rnd() * tool.actions.length)] ?? null,
      agent_id: agent.id,
      agent_label: agent.label,
      agent_kind: agent.kind,
      ip,
      surface: surfaces[Math.floor(rnd() * surfaces.length)] ?? 'mcp',
      outcome: failed ? 'failed' : 'ok',
      error_kind: failed ? (ERROR_KINDS[Math.floor(rnd() * ERROR_KINDS.length)] ?? 'server_error') : null,
      input_tokens: 80 + Math.floor(rnd() * 400),
      output_tokens: failed ? 20 + Math.floor(rnd() * 120) : 200 + Math.floor(rnd() * 1600),
      elapsed_ms: 30 + Math.floor(rnd() * (isCode ? 1800 : 600)),
    })
  }

  records.sort((a, b) => a.ts - b.ts)
  streamCache.set(cacheKey, records)
  return records
}

// ── Derivations ────────────────────────────────────────────────────────────

function bucketize(records: ToolCallRecord[], window: MetricsWindow, now: number): MetricsBucket[] {
  const buckets = WINDOW_BUCKETS[window]
  const windowMs = WINDOW_MS[window]
  const bucketMs = windowMs / buckets
  const start = now - windowMs
  const out: MetricsBucket[] = Array.from({ length: buckets }, (_, i) => ({
    ts: start + Math.round(bucketMs * i),
    calls: 0,
    failed: 0,
  }))
  for (const record of records) {
    const idx = Math.min(buckets - 1, Math.max(0, Math.floor((record.ts - start) / bucketMs)))
    out[idx].calls += 1
    if (record.outcome === 'failed') out[idx].failed += 1
  }
  return out
}

function rankTools(records: ToolCallRecord[]): ToolUsageEntry[] {
  const map = new Map<string, ToolUsageEntry>()
  for (const record of records) {
    const entry = map.get(record.tool) ?? { name: record.tool, calls: 0, failed: 0 }
    entry.calls += 1
    if (record.outcome === 'failed') entry.failed += 1
    map.set(record.tool, entry)
  }
  return [...map.values()].sort((a, b) => b.calls - a.calls)
}

function rankAgents(records: ToolCallRecord[]): AgentUsageEntry[] {
  const map = new Map<string, AgentUsageEntry>()
  for (const record of records) {
    const entry = map.get(record.agent_id) ?? {
      id: record.agent_id,
      label: record.agent_label,
      kind: record.agent_kind,
      calls: 0,
    }
    entry.calls += 1
    map.set(record.agent_id, entry)
  }
  return [...map.values()].sort((a, b) => b.calls - a.calls)
}

function rankActorBy(
  records: ToolCallRecord[],
  keyOf: (r: ToolCallRecord) => string,
  labelOf: (r: ToolCallRecord) => string,
  kind: ActorKind,
): ActorUsageEntry[] {
  const map = new Map<string, ActorUsageEntry>()
  for (const record of records) {
    const id = keyOf(record)
    const entry = map.get(id) ?? { id, label: labelOf(record), kind, calls: 0 }
    entry.calls += 1
    map.set(id, entry)
  }
  return [...map.values()].sort((a, b) => b.calls - a.calls)
}

function toFacet(entries: ActorUsageEntry[]): ActorFacet {
  return { active: entries.length, top: entries.slice(0, 8) }
}

/** Nearest-rank percentile over an already-sorted ascending array. */
function percentile(sorted: number[], p: number): number {
  if (sorted.length === 0) return 0
  const idx = Math.min(sorted.length - 1, Math.floor((p / 100) * sorted.length))
  return sorted[idx]
}

function groupSum<K>(
  records: ToolCallRecord[],
  keyOf: (r: ToolCallRecord) => K,
  valueOf: (r: ToolCallRecord) => number,
): Map<K, number> {
  const map = new Map<K, number>()
  for (const record of records) {
    const key = keyOf(record)
    map.set(key, (map.get(key) ?? 0) + valueOf(record))
  }
  return map
}

function sumTokens(records: ToolCallRecord[]): { input: number; output: number } {
  return records.reduce(
    (acc, r) => ({ input: acc.input + r.input_tokens, output: acc.output + r.output_tokens }),
    { input: 0, output: 0 },
  )
}

function recentCalls(records: ToolCallRecord[], limit = 25): ToolCallRecord[] {
  return [...records].sort((a, b) => b.ts - a.ts).slice(0, limit)
}

function aggregateDashboard(
  stream: ToolCallRecord[],
  window: MetricsWindow,
  now: number,
): DashboardMetrics {
  const tools = rankTools(stream)
  const tokens = sumTokens(stream)
  const total = stream.length
  const failedRecords = stream.filter((r) => r.outcome === 'failed')
  const failed = failedRecords.length
  const totalTokens = tokens.input + tokens.output

  const agentEntries = rankActorBy(
    stream.filter((r) => r.agent_kind === 'agent'),
    (r) => r.agent_id,
    (r) => r.agent_label,
    'agent',
  )
  const deviceEntries = rankActorBy(
    stream.filter((r) => r.agent_kind === 'device'),
    (r) => r.agent_id,
    (r) => r.agent_label,
    'device',
  )

  const codeRecords = stream.filter((r) => r.tool === 'code_execute')
  const codeRuns = codeRecords.length
  const fanTotal = Math.round(codeRuns * 3.4)
  const codeTimeouts = codeRecords.filter((r) => r.error_kind === 'timeout').length

  // Latency percentiles + slowest tools.
  const latencies = stream.map((r) => r.elapsed_ms).sort((a, b) => a - b)
  const avgLatency = latencies.length
    ? Math.round(latencies.reduce((s, v) => s + v, 0) / latencies.length)
    : 0
  const latencyByTool = new Map<string, { sum: number; n: number }>()
  for (const r of stream) {
    const e = latencyByTool.get(r.tool) ?? { sum: 0, n: 0 }
    e.sum += r.elapsed_ms
    e.n += 1
    latencyByTool.set(r.tool, e)
  }
  const slowest = [...latencyByTool.entries()]
    .map(([name, { sum, n }]) => ({ name, avg_ms: Math.round(sum / n) }))
    .sort((a, b) => b.avg_ms - a.avg_ms)
    .slice(0, 3)

  // Failures by kind.
  const byKind = [...groupSum(failedRecords, (r) => r.error_kind ?? 'unknown', () => 1).entries()]
    .map(([kind, count]) => ({ kind, count }))
    .sort((a, b) => b.count - a.count)

  // Surfaces.
  const surfaces = [...groupSum(stream, (r) => r.surface, () => 1).entries()]
    .map(([surface, calls]) => ({ surface, calls }))
    .sort((a, b) => b.calls - a.calls)

  // Tokens by tool.
  const tokensByTool = [...groupSum(stream, (r) => r.tool, (r) => r.input_tokens + r.output_tokens).entries()]
    .map(([name, t]) => ({ name, tokens: t }))
    .sort((a, b) => b.tokens - a.tokens)
    .slice(0, 5)

  // Upstreams.
  const upstreamMap = new Map<string, { calls: number; failed: number }>()
  for (const r of stream) {
    const name = TOOL_UPSTREAM[r.tool] ?? r.tool
    const e = upstreamMap.get(name) ?? { calls: 0, failed: 0 }
    e.calls += 1
    if (r.outcome === 'failed') e.failed += 1
    upstreamMap.set(name, e)
  }
  const upstreams = [...upstreamMap.entries()]
    .map(([name, v]) => ({ name, ...v }))
    .sort((a, b) => b.calls - a.calls)

  // Throughput + hourly rhythm.
  const minuteBuckets = groupSum(stream, (r) => Math.floor(r.ts / 60_000), () => 1)
  const peakPerMin = minuteBuckets.size ? Math.max(...minuteBuckets.values()) : 0
  const windowMinutes = WINDOW_MS[window] / 60_000
  const hourly = Array.from({ length: 24 }, (_, hour) => ({ hour, calls: 0 }))
  for (const r of stream) hourly[new Date(r.ts).getHours()].calls += 1
  let busiestHour = 0
  for (let h = 1; h < 24; h += 1) {
    if (hourly[h].calls > hourly[busiestHour].calls) busiestHour = h
  }

  // New vs returning agents (mocked split — deterministic).
  const distinctAgents = agentEntries.length + deviceEntries.length
  const newAgents = Math.min(distinctAgents, Math.max(1, Math.round(distinctAgents * 0.3)))

  return {
    window,
    since_ms: now - WINDOW_MS[window],
    until_ms: now,
    tool_calls: { total, failed, succeeded: total - failed },
    tools: { top: tools.slice(0, 5), least: tools.slice(-3).reverse(), distinct: tools.length },
    tokens: {
      input: tokens.input,
      output: tokens.output,
      total: totalTokens,
      avg_per_call: total > 0 ? Math.round(totalTokens / total) : 0,
    },
    actors: {
      agent: toFacet(agentEntries),
      device: toFacet(deviceEntries),
      ip: toFacet(rankActorBy(stream, (r) => r.ip, (r) => r.ip, 'ip')),
    },
    fan_out: {
      runs: codeRuns,
      total_calls: fanTotal,
      avg_calls_per_run: codeRuns > 0 ? Math.round((fanTotal / codeRuns) * 10) / 10 : 0,
      max_calls_in_run: Math.min(8, 4 + Math.round(WINDOW_SCALE[window] / 20)),
      timeout_rate: codeRuns > 0 ? Math.round((codeTimeouts / codeRuns) * 100) / 100 : 0,
      truncation_rate: codeRuns > 0 ? Math.round(0.18 * 100) / 100 : 0,
      artifact_writes: Math.round(codeRuns * 0.3),
    },
    latency: {
      p50: percentile(latencies, 50),
      p95: percentile(latencies, 95),
      p99: percentile(latencies, 99),
      avg: avgLatency,
      slowest,
    },
    errors: { total: failed, by_kind: byKind },
    surfaces,
    tokens_by_tool: tokensByTool,
    upstreams,
    throughput: {
      peak_per_min: peakPerMin,
      avg_per_min: Math.round((total / windowMinutes) * 100) / 100,
      busiest_hour: busiestHour,
    },
    hourly,
    agents_seen: { new: newAgents, returning: distinctAgents - newAgents },
    timeseries: bucketize(stream, window, now),
  }
}

// ── Real path (mirrors logs-client) ────────────────────────────────────────

function metricsActionUrl(baseUrl?: string) {
  return `${normalizeGatewayApiBase(baseUrl)}/logs`
}

async function parseJsonResponse<T>(response: Response): Promise<T> {
  const raw = await response.text()
  if (raw.length === 0) {
    throw new Error(`request failed with status ${response.status} ${response.statusText}`.trim())
  }
  let payload: unknown
  try {
    payload = JSON.parse(raw)
  } catch (error) {
    throw new Error(`invalid JSON response from gateway: ${response.status}`, { cause: error })
  }
  if (!response.ok) {
    const message =
      typeof (payload as { message?: unknown })?.message === 'string'
        ? (payload as { message: string }).message
        : `request failed with status ${response.status}`
    throw new Error(message)
  }
  return payload as T
}

async function postLogsAction<T>(
  action: string,
  params: object,
  options?: MetricsRequestOptions,
): Promise<T> {
  const response = await fetch(
    metricsActionUrl(options?.baseUrl),
    gatewayRequestInit(action, params, options?.token, options?.signal, options?.standaloneBearerAuth),
  )
  return parseJsonResponse<T>(response)
}

// ── Public fetchers ────────────────────────────────────────────────────────

export async function fetchDashboardMetrics(
  window: MetricsWindow,
  options?: MetricsRequestOptions,
): Promise<DashboardMetrics> {
  if (USE_MOCK_DATA) {
    options?.signal?.throwIfAborted?.()
    const now = Date.now()
    return aggregateDashboard(buildCallStream(window, now), window, now)
  }
  return postLogsAction<DashboardMetrics>('logs.metrics', { window }, options)
}

export async function fetchToolDetail(
  name: string,
  window: MetricsWindow,
  options?: MetricsRequestOptions,
): Promise<ToolDetail> {
  if (USE_MOCK_DATA) {
    options?.signal?.throwIfAborted?.()
    const now = Date.now()
    const records = buildCallStream(window, now).filter((r) => r.tool === name)
    const tokens = sumTokens(records)
    const calls = records.length
    const totalTokens = tokens.input + tokens.output
    return {
      name,
      window,
      calls,
      failed: records.filter((r) => r.outcome === 'failed').length,
      total_tokens: totalTokens,
      avg_tokens: calls > 0 ? Math.round(totalTokens / calls) : 0,
      avg_elapsed_ms:
        calls > 0 ? Math.round(records.reduce((s, r) => s + r.elapsed_ms, 0) / calls) : 0,
      timeseries: bucketize(records, window, now),
      top_callers: rankAgents(records).slice(0, 5),
      recent: recentCalls(records),
    }
  }
  return postLogsAction<ToolDetail>('logs.tool_detail', { tool: name, window }, options)
}

export async function fetchAgentDetail(
  id: string,
  window: MetricsWindow,
  options?: MetricsRequestOptions,
): Promise<AgentDetail> {
  if (USE_MOCK_DATA) {
    options?.signal?.throwIfAborted?.()
    const now = Date.now()
    const records = buildCallStream(window, now).filter((r) => r.agent_id === id)
    const tokens = sumTokens(records)
    const first = records[0]
    return {
      id,
      label: first?.agent_label ?? id,
      kind: first?.agent_kind ?? 'agent',
      window,
      calls: records.length,
      failed: records.filter((r) => r.outcome === 'failed').length,
      total_tokens: tokens.input + tokens.output,
      tools_used: rankTools(records),
      timeseries: bucketize(records, window, now),
      recent: recentCalls(records),
    }
  }
  return postLogsAction<AgentDetail>('logs.agent_detail', { agent: id, window }, options)
}

export async function fetchToolCalls(
  query: ToolCallQuery,
  options?: MetricsRequestOptions,
): Promise<ToolCallPage> {
  if (USE_MOCK_DATA) {
    options?.signal?.throwIfAborted?.()
    const now = Date.now()
    const stream = buildCallStream(query.window, now)
    const search = query.search?.trim().toLowerCase()
    const filtered = stream.filter((r) => {
      if (query.tool && r.tool !== query.tool) return false
      if (query.agent && r.agent_id !== query.agent) return false
      if (query.ip && r.ip !== query.ip) return false
      if (query.outcome && r.outcome !== query.outcome) return false
      if (query.surface && r.surface !== query.surface) return false
      if (search) {
        const hay = `${r.tool} ${r.action ?? ''} ${r.agent_label} ${r.ip} ${r.error_kind ?? ''}`.toLowerCase()
        if (!hay.includes(search)) return false
      }
      return true
    })
    const sorted = filtered.sort((a, b) => b.ts - a.ts)
    const offset = query.offset ?? 0
    const limit = query.limit ?? 50
    return {
      calls: sorted.slice(offset, offset + limit),
      total: stream.length,
      filtered: sorted.length,
    }
  }
  return postLogsAction<ToolCallPage>('logs.calls', { query }, options)
}
