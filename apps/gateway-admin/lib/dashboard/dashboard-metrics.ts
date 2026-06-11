import type { Gateway } from '@/lib/types/gateway'
import type { FleetDevice } from '@/lib/api/device-client'
import type { MetricsWindow } from '@/lib/types/metrics'

/** Human label for each rolling window. */
export const WINDOW_LABELS: Record<MetricsWindow, string> = {
  '1h': 'Last hour',
  '24h': 'Last 24 hours',
  '7d': 'Last 7 days',
}

/** Live (point-in-time) fleet counts derived from gateways + devices. */
export interface LiveFleetStats {
  totalServers: number
  connectedServers: number
  offlineServers: number
  discoveredTools: number
  exposedTools: number
  totalDevices: number
  connectedDevices: number
  warnings: number
}

export function buildLiveFleetStats(
  gateways: Gateway[],
  devices: FleetDevice[],
): LiveFleetStats {
  const connectedServers = gateways.filter(
    (g) => g.status.connected && g.status.healthy,
  ).length

  return {
    totalServers: gateways.length,
    connectedServers,
    offlineServers: gateways.length - connectedServers,
    discoveredTools: gateways.reduce(
      (sum, g) => sum + g.status.discovered_tool_count,
      0,
    ),
    exposedTools: gateways.reduce(
      (sum, g) => sum + g.status.exposed_tool_count,
      0,
    ),
    totalDevices: devices.length,
    connectedDevices: devices.filter((d) => d.connected).length,
    warnings: gateways.reduce((sum, g) => sum + g.warnings.length, 0),
  }
}

/**
 * Compact number formatting for dashboard tiles: `980`, `1.2k`, `3.4M`.
 * Negatives are clamped to 0 — counts are never negative.
 */
export function formatCompactNumber(value: number): string {
  const n = Number.isFinite(value) ? Math.max(0, value) : 0
  if (n < 1000) return String(Math.round(n))
  if (n < 1_000_000) return `${trimZero(n / 1000)}k`
  if (n < 1_000_000_000) return `${trimZero(n / 1_000_000)}M`
  return `${trimZero(n / 1_000_000_000)}B`
}

function trimZero(value: number): string {
  // One decimal, but drop a trailing `.0` (1.0k → 1k).
  const rounded = Math.round(value * 10) / 10
  return Number.isInteger(rounded) ? String(rounded) : rounded.toFixed(1)
}

/**
 * Success rate as a whole-number percent, or `null` when there is no traffic
 * to rate. `successRatePercent(100, 8) === 92`.
 */
export function successRatePercent(total: number, failed: number): number | null {
  if (total <= 0) return null
  const ok = Math.max(0, total - Math.max(0, failed))
  return Math.round((ok / total) * 100)
}

/**
 * Stable identity of the current server-warning set. Changes whenever a
 * warning is added or removed, so a dismissed banner re-surfaces on new
 * warnings rather than staying hidden forever.
 */
export function warningsSignature(gateways: Gateway[]): string {
  return gateways
    .flatMap((gateway) => gateway.warnings.map((warning) => `${gateway.id}:${warning.code}`))
    .sort()
    .join('|')
}

/** Compact relative time: `now`, `45s`, `12m`, `3h`, `5d` ago. */
export function formatRelativeTime(ts: number, now: number = Date.now()): string {
  const diff = Math.max(0, now - ts)
  const secs = Math.floor(diff / 1000)
  if (secs < 5) return 'now'
  if (secs < 60) return `${secs}s ago`
  const mins = Math.floor(secs / 60)
  if (mins < 60) return `${mins}m ago`
  const hours = Math.floor(mins / 60)
  if (hours < 24) return `${hours}h ago`
  return `${Math.floor(hours / 24)}d ago`
}

/** Dispatch duration: `120ms`, `1.2s`, `45s`. */
export function formatDuration(ms: number): string {
  const n = Number.isFinite(ms) ? Math.max(0, ms) : 0
  if (n < 1000) return `${Math.round(n)}ms`
  const secs = n / 1000
  if (secs < 60) return `${trimZero(secs)}s`
  return `${Math.round(secs)}s`
}
