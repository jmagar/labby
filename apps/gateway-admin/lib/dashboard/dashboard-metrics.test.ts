import test from 'node:test'
import assert from 'node:assert/strict'

import type { Gateway } from '../types/gateway.ts'
import type { FleetDevice } from '../api/device-client.ts'
import {
  buildLiveFleetStats,
  formatCompactNumber,
  formatDuration,
  formatRelativeTime,
  successRatePercent,
  warningsSignature,
} from './dashboard-metrics.ts'

function gateway(overrides: {
  connected?: boolean
  healthy?: boolean
  discovered?: number
  exposed?: number
  warnings?: number
}): Gateway {
  return {
    id: 'gw',
    name: 'gw',
    transport: 'http',
    config: {},
    status: {
      healthy: overrides.healthy ?? true,
      connected: overrides.connected ?? true,
      discovered_tool_count: overrides.discovered ?? 0,
      exposed_tool_count: overrides.exposed ?? 0,
      discovered_resource_count: 0,
      exposed_resource_count: 0,
      discovered_prompt_count: 0,
      exposed_prompt_count: 0,
    },
    discovery: { tools: [], resources: [], prompts: [] },
    warnings: Array.from({ length: overrides.warnings ?? 0 }, (_, i) => ({
      code: `w${i}`,
      message: 'warn',
      timestamp: '',
    })),
  } as Gateway
}

function device(connected: boolean): FleetDevice {
  return { node_id: connected ? 'on' : 'off', connected, role: 'worker' }
}

test('buildLiveFleetStats counts connected vs offline servers', () => {
  const stats = buildLiveFleetStats(
    [
      gateway({ connected: true, healthy: true, discovered: 10, exposed: 6, warnings: 1 }),
      gateway({ connected: true, healthy: false, discovered: 4, exposed: 0 }),
      gateway({ connected: false, healthy: false, discovered: 0, exposed: 0, warnings: 2 }),
    ],
    [device(true), device(true), device(false)],
  )

  assert.equal(stats.totalServers, 3)
  assert.equal(stats.connectedServers, 1) // only connected AND healthy
  assert.equal(stats.offlineServers, 2)
  assert.equal(stats.discoveredTools, 14)
  assert.equal(stats.exposedTools, 6)
  assert.equal(stats.totalDevices, 3)
  assert.equal(stats.connectedDevices, 2)
  assert.equal(stats.warnings, 3)
})

test('buildLiveFleetStats handles empty fleet', () => {
  const stats = buildLiveFleetStats([], [])
  assert.equal(stats.totalServers, 0)
  assert.equal(stats.connectedServers, 0)
  assert.equal(stats.offlineServers, 0)
})

test('formatCompactNumber abbreviates and trims trailing zero', () => {
  assert.equal(formatCompactNumber(0), '0')
  assert.equal(formatCompactNumber(980), '980')
  assert.equal(formatCompactNumber(1000), '1k')
  assert.equal(formatCompactNumber(1240), '1.2k')
  assert.equal(formatCompactNumber(3_400_000), '3.4M')
  assert.equal(formatCompactNumber(-5), '0')
})

test('successRatePercent rates traffic and returns null when idle', () => {
  assert.equal(successRatePercent(100, 8), 92)
  assert.equal(successRatePercent(0, 0), null)
  assert.equal(successRatePercent(50, 50), 0)
})

test('formatRelativeTime buckets by unit', () => {
  const now = 10_000_000_000
  assert.equal(formatRelativeTime(now, now), 'now')
  assert.equal(formatRelativeTime(now - 30_000, now), '30s ago')
  assert.equal(formatRelativeTime(now - 5 * 60_000, now), '5m ago')
  assert.equal(formatRelativeTime(now - 3 * 3_600_000, now), '3h ago')
  assert.equal(formatRelativeTime(now - 2 * 86_400_000, now), '2d ago')
})

test('formatDuration scales ms to seconds', () => {
  assert.equal(formatDuration(120), '120ms')
  assert.equal(formatDuration(1200), '1.2s')
  assert.equal(formatDuration(45000), '45s')
})

test('warningsSignature is order-independent and tracks set changes', () => {
  const mk = (id: string, ...codes: string[]) =>
    ({ id, warnings: codes.map((code) => ({ code, message: '', timestamp: '' })) }) as unknown as Gateway

  const s1 = warningsSignature([mk('a', 'x', 'y'), mk('b', 'z')])
  const s2 = warningsSignature([mk('b', 'z'), mk('a', 'y', 'x')])
  assert.equal(s1, s2) // same warnings, any order → same signature

  assert.notEqual(s1, warningsSignature([mk('a', 'x', 'y')])) // removing a warning re-surfaces
  assert.equal(warningsSignature([]), '')
})
