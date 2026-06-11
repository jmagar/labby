'use client'

import type { ReactNode } from 'react'
import {
  Clock3,
  Gauge,
  Layers,
  Server,
  Timer,
  TriangleAlert,
  Zap,
} from 'lucide-react'
import { DashboardPanel } from './panel'
import { MetricBarList, type MetricBarItem } from './metric-bars'
import { DASH_METRIC_SM } from './ui'
import { formatCompactNumber, formatDuration } from '@/lib/dashboard/dashboard-metrics'
import type {
  CallSurface,
  DashboardMetrics,
  ErrorKindCount,
  SurfaceCount,
  TokenByTool,
  UpstreamUsage,
} from '@/lib/types/metrics'
import { cn } from '@/lib/utils'

function StatCell({ value, label }: { value: ReactNode; label: string }) {
  return (
    <div>
      <p className={cn(DASH_METRIC_SM, 'text-aurora-text-primary')}>{value}</p>
      <p className="mt-1 text-xs text-aurora-text-muted">{label}</p>
    </div>
  )
}

const SURFACE_LABEL: Record<CallSurface, string> = { mcp: 'MCP', api: 'API', cli: 'CLI', web: 'Web' }

function hourLabel(hour: number): string {
  const h = hour % 12 === 0 ? 12 : hour % 12
  return `${h}${hour < 12 ? 'a' : 'p'}`
}

// ── Latency ──────────────────────────────────────────────────────────────

export function LatencyPanel({ latency }: { latency: DashboardMetrics['latency'] }) {
  return (
    <DashboardPanel title="Latency" icon={<Timer className="size-4" />} meta={`avg ${formatDuration(latency.avg)}`}>
      <div className="grid grid-cols-3 gap-3">
        <StatCell value={formatDuration(latency.p50)} label="p50" />
        <StatCell value={formatDuration(latency.p95)} label="p95" />
        <StatCell value={formatDuration(latency.p99)} label="p99" />
      </div>
      {latency.slowest.length > 0 ? (
        <div className="flex flex-col gap-1.5 border-t border-aurora-border-default/60 pt-3">
          <p className="text-xs text-aurora-text-muted">Slowest tools</p>
          {latency.slowest.map((tool) => (
            <div key={tool.name} className="flex items-center justify-between gap-3">
              <span className="truncate font-mono text-[13px] text-aurora-text-primary">{tool.name}</span>
              <span className="shrink-0 text-sm font-semibold tabular-nums text-aurora-text-muted">
                {formatDuration(tool.avg_ms)}
              </span>
            </div>
          ))}
        </div>
      ) : null}
    </DashboardPanel>
  )
}

// ── Failures by kind ─────────────────────────────────────────────────────

export function FailuresPanel({ errors }: { errors: DashboardMetrics['errors'] }) {
  const items: MetricBarItem[] = errors.by_kind.map((e: ErrorKindCount) => ({
    key: e.kind,
    label: e.kind,
    value: e.count,
    display: formatCompactNumber(e.count),
  }))
  return (
    <DashboardPanel
      title="Failures by kind"
      icon={<TriangleAlert className="size-4" />}
      meta={`${formatCompactNumber(errors.total)} failed`}
    >
      <MetricBarList items={items} tone="error" mono empty="No failures in this window." />
    </DashboardPanel>
  )
}

// ── Calls by surface ─────────────────────────────────────────────────────

export function SurfacesPanel({ surfaces }: { surfaces: SurfaceCount[] }) {
  const items: MetricBarItem[] = surfaces.map((s) => ({
    key: s.surface,
    label: SURFACE_LABEL[s.surface],
    value: s.calls,
    display: formatCompactNumber(s.calls),
  }))
  return (
    <DashboardPanel title="By surface" icon={<Layers className="size-4" />}>
      <MetricBarList items={items} />
    </DashboardPanel>
  )
}

// ── Tokens by tool ───────────────────────────────────────────────────────

export function TokensByToolPanel({
  tokens,
  onSelect,
}: {
  tokens: TokenByTool[]
  onSelect?: (tool: string) => void
}) {
  const items: MetricBarItem[] = tokens.map((t) => ({
    key: t.name,
    label: t.name,
    value: t.tokens,
    display: formatCompactNumber(t.tokens),
    onSelect: onSelect ? () => onSelect(t.name) : undefined,
  }))
  return (
    <DashboardPanel title="Tokens by tool" icon={<Zap className="size-4" />}>
      <MetricBarList items={items} tone="strong" mono />
    </DashboardPanel>
  )
}

// ── Upstreams ────────────────────────────────────────────────────────────

export function UpstreamsPanel({ upstreams }: { upstreams: UpstreamUsage[] }) {
  const items: MetricBarItem[] = upstreams.map((u) => ({
    key: u.name,
    label: u.name,
    value: u.calls,
    display: u.failed > 0 ? `${formatCompactNumber(u.calls)} · ${u.failed} err` : formatCompactNumber(u.calls),
  }))
  return (
    <DashboardPanel title="Top upstreams" icon={<Server className="size-4" />} meta={`${upstreams.length} servers`}>
      <MetricBarList items={items} mono />
    </DashboardPanel>
  )
}

// ── Throughput ───────────────────────────────────────────────────────────

export function ThroughputPanel({
  throughput,
  agentsSeen,
}: {
  throughput: DashboardMetrics['throughput']
  agentsSeen: DashboardMetrics['agents_seen']
}) {
  return (
    <DashboardPanel title="Throughput" icon={<Gauge className="size-4" />}>
      <div className="grid grid-cols-3 gap-3">
        <StatCell value={formatCompactNumber(throughput.peak_per_min)} label="peak / min" />
        <StatCell value={throughput.avg_per_min} label="avg / min" />
        <StatCell value={hourLabel(throughput.busiest_hour)} label="busiest hour" />
      </div>
      <div className="flex items-center justify-between gap-3 border-t border-aurora-border-default/60 pt-3 text-sm">
        <span className="text-aurora-text-muted">Agents</span>
        <span className="tabular-nums text-aurora-text-primary">
          <span className="text-aurora-accent-strong">{agentsSeen.new} new</span>
          <span className="text-aurora-text-muted"> · {agentsSeen.returning} returning</span>
        </span>
      </div>
    </DashboardPanel>
  )
}

// ── Hourly heat strip ────────────────────────────────────────────────────

export function HourlyHeatPanel({
  hourly,
  busiestHour,
}: {
  hourly: DashboardMetrics['hourly']
  busiestHour: number
}) {
  const max = Math.max(1, ...hourly.map((h) => h.calls))
  return (
    <DashboardPanel
      title="Activity by hour"
      icon={<Clock3 className="size-4" />}
      meta={`busiest ${hourLabel(busiestHour)}`}
    >
      <div className="flex items-end gap-[3px]">
        {hourly.map((h) => {
          const intensity = h.calls / max
          const pctMix = Math.round(10 + intensity * 90)
          return (
            <div
              key={h.hour}
              title={`${hourLabel(h.hour)} — ${h.calls} calls`}
              className="h-9 flex-1 rounded-sm border border-aurora-border-default/40"
              style={{
                background: `color-mix(in srgb, var(--aurora-accent-primary) ${pctMix}%, var(--aurora-control-surface))`,
              }}
            />
          )
        })}
      </div>
      <div className="flex justify-between text-[10px] text-aurora-text-muted">
        <span>12a</span>
        <span>6a</span>
        <span>12p</span>
        <span>6p</span>
        <span>11p</span>
      </div>
    </DashboardPanel>
  )
}
