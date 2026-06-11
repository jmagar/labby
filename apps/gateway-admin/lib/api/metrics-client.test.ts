import test from 'node:test'
import assert from 'node:assert/strict'

// Mock mode must be on before the module captures `USE_MOCK_DATA` at load.
// Set it here (runs before any test callback) and import lazily inside tests
// to avoid a top-level await (rejected by the project's TS module target).
process.env.NEXT_PUBLIC_MOCK_DATA = 'true'

let cached: typeof import('./metrics-client.ts') | undefined
async function client() {
  cached ??= await import('./metrics-client.ts')
  return cached
}

const sum = (xs: number[]) => xs.reduce((a, b) => a + b, 0)

test('surfaces, hourly, and upstreams each reconcile to total calls', async () => {
  const { fetchDashboardMetrics } = await client()
  const m = await fetchDashboardMetrics('24h')
  const total = m.tool_calls.total
  assert.ok(total > 0)
  assert.equal(sum(m.surfaces.map((s) => s.calls)), total)
  assert.equal(sum(m.hourly.map((h) => h.calls)), total)
  assert.equal(sum(m.upstreams.map((u) => u.calls)), total)
  assert.equal(m.hourly.length, 24)
})

test('failures reconcile: errors.total === failed === sum(by_kind)', async () => {
  const { fetchDashboardMetrics } = await client()
  const m = await fetchDashboardMetrics('24h')
  assert.equal(m.errors.total, m.tool_calls.failed)
  assert.equal(sum(m.errors.by_kind.map((e) => e.count)), m.tool_calls.failed)
})

test('latency percentiles are monotonic and slowest is sorted desc', async () => {
  const { fetchDashboardMetrics } = await client()
  const m = await fetchDashboardMetrics('24h')
  assert.ok(m.latency.p50 <= m.latency.p95)
  assert.ok(m.latency.p95 <= m.latency.p99)
  assert.ok(m.latency.avg > 0)
  for (let i = 1; i < m.latency.slowest.length; i += 1) {
    assert.ok(m.latency.slowest[i - 1].avg_ms >= m.latency.slowest[i].avg_ms)
  }
})

test('actor facets are kind-pure and counts match top length cap', async () => {
  const { fetchDashboardMetrics } = await client()
  const m = await fetchDashboardMetrics('24h')
  assert.ok(m.actors.agent.top.every((a) => a.kind === 'agent'))
  assert.ok(m.actors.device.top.every((a) => a.kind === 'device'))
  assert.ok(m.actors.ip.top.every((a) => a.kind === 'ip'))
  assert.ok(m.actors.agent.active >= m.actors.agent.top.length)
})

test('tokens_by_tool sorted desc; fan-out rates and busiest hour bounded', async () => {
  const { fetchDashboardMetrics } = await client()
  const m = await fetchDashboardMetrics('24h')
  for (let i = 1; i < m.tokens_by_tool.length; i += 1) {
    assert.ok(m.tokens_by_tool[i - 1].tokens >= m.tokens_by_tool[i].tokens)
  }
  assert.ok(m.fan_out.timeout_rate >= 0 && m.fan_out.timeout_rate <= 1)
  assert.ok(m.fan_out.truncation_rate >= 0 && m.fan_out.truncation_rate <= 1)
  assert.ok(m.throughput.busiest_hour >= 0 && m.throughput.busiest_hour <= 23)
})

test('mock is deterministic per window; longer windows carry more traffic', async () => {
  const { fetchDashboardMetrics } = await client()
  const a = await fetchDashboardMetrics('1h')
  const b = await fetchDashboardMetrics('1h')
  assert.equal(a.tool_calls.total, b.tool_calls.total)
  const week = await fetchDashboardMetrics('7d')
  assert.ok(week.tool_calls.total > a.tool_calls.total)
})

test('fetchToolCalls honors tool, ip, and outcome filters', async () => {
  const { fetchToolCalls } = await client()
  const all = await fetchToolCalls({ window: '24h', limit: 5000 })
  assert.ok(all.calls.length > 0)
  assert.equal(all.filtered, all.total)

  const tool = all.calls[0].tool
  const byTool = await fetchToolCalls({ window: '24h', tool, limit: 5000 })
  assert.ok(byTool.calls.every((c) => c.tool === tool))

  const ip = all.calls[0].ip
  const byIp = await fetchToolCalls({ window: '24h', ip, limit: 5000 })
  assert.ok(byIp.calls.every((c) => c.ip === ip))

  const failed = await fetchToolCalls({ window: '24h', outcome: 'failed', limit: 5000 })
  assert.ok(failed.calls.every((c) => c.outcome === 'failed'))
})
