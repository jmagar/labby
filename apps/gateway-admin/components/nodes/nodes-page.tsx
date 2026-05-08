'use client'

import { LayoutGrid, RefreshCw, ScrollText, Table2, TriangleAlert } from 'lucide-react'
import Link from 'next/link'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'

import { AppHeader } from '@/components/app-header'
import { NodeLogStream } from '@/components/nodes/node-log-stream'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import {
  AURORA_DISPLAY_1,
  AURORA_DISPLAY_2,
  AURORA_MUTED_LABEL,
  AURORA_PAGE_FRAME,
  AURORA_PAGE_SHELL,
} from '@/components/aurora/tokens'
import { fetchFleetDevices, fetchFleetNode, type FleetDevice, type FleetNodeDetail } from '@/lib/api/device-client'

type ViewMode = 'cards' | 'table'

interface FleetNodeCardData extends FleetNodeDetail {
  detailError?: string | null
}

function toPercent(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value)) return '—'
  return `${value.toFixed(1)}%`
}

function toGB(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value)) return '—'
  return `${(value / 1_073_741_824).toFixed(1)} GB`
}

function uptimeLabel(s: number | null | undefined): string {
  if (s == null || !Number.isFinite(s) || s <= 0) return '—'
  const m = Math.floor(s / 60)
  const h = Math.floor(m / 60)
  const d = Math.floor(h / 24)
  if (d > 0) return `${d}d ${h % 24}h`
  if (h > 0) return `${h}h ${m % 60}m`
  return `${m % 60}m`
}

function primaryIp(ips: string[] | undefined): string {
  if (!ips?.length) return '—'
  return ips.find((ip) => ip.startsWith('100.')) ?? ips[0]
}

function statusTone(node: FleetNodeCardData): 'healthy' | 'warning' | 'error' | 'unknown' {
  const s = node.status
  if (!s?.health) return 'unknown'
  if (!s.connected) return 'error'
  if (s.health === 'healthy') return 'healthy'
  if (s.health === 'needs_attention') return 'warning'
  return 'unknown'
}

function statusLabel(tone: ReturnType<typeof statusTone>): string {
  return { healthy: 'Healthy', warning: 'Attention', error: 'Offline', unknown: 'Unknown' }[tone]
}

function toneClasses(tone: ReturnType<typeof statusTone>) {
  return {
    healthy: 'bg-aurora-accent-primary/15 text-aurora-accent-strong border-aurora-accent-primary/30',
    warning: 'bg-aurora-warn/12 text-aurora-warn border-aurora-warn/30',
    error: 'bg-aurora-error/14 text-aurora-error border-aurora-error/30',
    unknown: 'bg-aurora-border-strong text-aurora-text-muted border-aurora-border-default',
  }[tone]
}

function StatusDot({ tone }: { tone: ReturnType<typeof statusTone> }) {
  return (
    <span className={`inline-block size-2 shrink-0 rounded-full ${
      { healthy: 'bg-aurora-accent-strong', warning: 'bg-aurora-warn', error: 'bg-aurora-error', unknown: 'bg-aurora-text-muted' }[tone]
    }`} />
  )
}

function LogsDialog({ nodeId, open, onOpenChange }: { nodeId: string; open: boolean; onOpenChange: (v: boolean) => void }) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex h-[80vh] max-w-5xl flex-col overflow-hidden p-0">
        <DialogHeader className="shrink-0 px-4 py-3 border-b border-aurora-border-strong">
          <DialogTitle>{nodeId} — Logs</DialogTitle>
        </DialogHeader>
        <div className="min-h-0 flex-1">
          {open ? <NodeLogStream nodeId={nodeId} /> : null}
        </div>
      </DialogContent>
    </Dialog>
  )
}

// ── Card view ────────────────────────────────────────────────────────────────

function MetricTile({ label, value, warn }: { label: string; value: string; warn?: boolean }) {
  return (
    <div className="rounded border border-aurora-border-strong/60 bg-aurora-control-surface/50 px-2 py-1.5">
      <p className="text-[9px] uppercase tracking-[0.12em] text-aurora-text-muted">{label}</p>
      <p className={`mt-0.5 text-sm font-semibold tabular-nums ${warn ? 'text-aurora-warn' : 'text-aurora-text-primary'}`}>{value}</p>
    </div>
  )
}

function NodeCard({ node }: { node: FleetNodeCardData }) {
  const [logsOpen, setLogsOpen] = useState(false)
  const tone = statusTone(node)
  const s = node.status ?? null
  const version = s?.version ?? node.version ?? null
  const temp = s?.cpu_temp_c
  const hasIssues = (s?.doctor_issues?.length ?? 0) > 0

  return (
    <div className={`flex flex-col rounded-aurora-2 border bg-aurora-panel-strong ${hasIssues || node.detailError ? 'border-aurora-warn/40' : 'border-aurora-border-strong'} shadow-[var(--aurora-shadow-small)]`}>
      {/* Header */}
      <div className="flex items-center justify-between gap-2 border-b border-aurora-border-strong/60 px-3 py-2">
        <div className="flex min-w-0 items-center gap-2">
          <StatusDot tone={tone} />
          <span className="truncate text-sm font-semibold text-aurora-text-primary">{node.node_id}</span>
          <span className={`shrink-0 rounded-full border px-1.5 py-px text-[9px] uppercase tracking-[0.14em] ${toneClasses(tone)}`}>
            {statusLabel(tone)}
          </span>
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          {hasIssues ? <TriangleAlert className="size-3 text-aurora-warn" /> : null}
          {version ? <span className="text-[10px] text-aurora-text-muted">v{version}</span> : null}
        </div>
      </div>

      {/* Metrics grid */}
      <div className="grid grid-cols-4 gap-1.5 p-2.5">
        <MetricTile label="CPU" value={toPercent(s?.cpu_percent)} />
        <MetricTile label="Memory" value={toGB(s?.memory_used_bytes)} />
        <MetricTile label="Disk" value={toGB(s?.storage_used_bytes)} />
        <MetricTile label="Uptime" value={uptimeLabel(s?.uptime_seconds)} />
        {temp != null ? <MetricTile label="Temp" value={`${temp.toFixed(0)}°C`} warn={temp > 80} /> : null}
        {s?.cores != null ? <MetricTile label="Cores" value={String(s.cores)} /> : null}
      </div>

      {/* Footer */}
      <div className="mt-auto flex items-center justify-between border-t border-aurora-border-strong/60 px-3 py-1.5">
        <span className="font-mono text-[10px] text-aurora-text-muted">{primaryIp(s?.ips)}</span>
        <div className="flex items-center gap-2">
          {s?.os ? <span className="text-[10px] text-aurora-text-muted">{s.os}</span> : null}
          <Button variant="ghost" size="sm" className="h-6 gap-1 px-2 text-[11px]" onClick={() => setLogsOpen(true)}>
            <ScrollText className="size-3" />Logs
          </Button>
        </div>
      </div>

      <LogsDialog nodeId={node.node_id} open={logsOpen} onOpenChange={setLogsOpen} />
    </div>
  )
}

// ── Table view ───────────────────────────────────────────────────────────────

function NodesTable({ nodes }: { nodes: FleetNodeCardData[] }) {
  const [logsNode, setLogsNode] = useState<string | null>(null)

  return (
    <>
      <div className="overflow-x-auto rounded-aurora-2 border border-aurora-border-strong">
        <table className="w-full text-xs">
          <thead>
            <tr className="border-b border-aurora-border-strong bg-aurora-panel-medium text-left">
              {['Node', 'Status', 'CPU', 'Memory', 'Disk', 'Uptime', 'Temp', 'Cores', 'IP', 'Version', ''].map((h) => (
                <th key={h} className="px-3 py-2 text-[10px] font-medium uppercase tracking-[0.12em] text-aurora-text-muted whitespace-nowrap">{h}</th>
              ))}
            </tr>
          </thead>
          <tbody>
            {nodes.map((node) => {
              const tone = statusTone(node)
              const s = node.status ?? null
              const temp = s?.cpu_temp_c
              return (
                <tr key={node.node_id} className="border-b border-aurora-border-strong/50 last:border-0 hover:bg-aurora-control-surface/40 transition-colors">
                  <td className="px-3 py-2 font-medium text-aurora-text-primary whitespace-nowrap">
                    <div className="flex items-center gap-2">
                      <StatusDot tone={tone} />
                      {node.node_id}
                    </div>
                  </td>
                  <td className="px-3 py-2">
                    <span className={`rounded-full border px-1.5 py-px text-[9px] uppercase tracking-[0.14em] ${toneClasses(tone)}`}>
                      {statusLabel(tone)}
                    </span>
                  </td>
                  <td className="px-3 py-2 tabular-nums text-aurora-text-primary">{toPercent(s?.cpu_percent)}</td>
                  <td className="px-3 py-2 tabular-nums text-aurora-text-primary">{toGB(s?.memory_used_bytes)}</td>
                  <td className="px-3 py-2 tabular-nums text-aurora-text-primary">{toGB(s?.storage_used_bytes)}</td>
                  <td className="px-3 py-2 tabular-nums text-aurora-text-primary">{uptimeLabel(s?.uptime_seconds)}</td>
                  <td className={`px-3 py-2 tabular-nums ${temp != null && temp > 80 ? 'text-aurora-warn' : 'text-aurora-text-primary'}`}>
                    {temp != null ? `${temp.toFixed(0)}°C` : '—'}
                  </td>
                  <td className="px-3 py-2 tabular-nums text-aurora-text-primary">{s?.cores ?? '—'}</td>
                  <td className="px-3 py-2 font-mono text-aurora-text-muted whitespace-nowrap">{primaryIp(s?.ips)}</td>
                  <td className="px-3 py-2 text-aurora-text-muted whitespace-nowrap">
                    {s?.version ? `v${s.version}` : '—'}{s?.os ? ` · ${s.os}` : ''}
                  </td>
                  <td className="px-3 py-2">
                    <Button variant="ghost" size="sm" className="h-6 gap-1 px-2 text-[11px]" onClick={() => setLogsNode(node.node_id)}>
                      <ScrollText className="size-3" />Logs
                    </Button>
                  </td>
                </tr>
              )
            })}
          </tbody>
        </table>
      </div>

      {logsNode ? (
        <LogsDialog nodeId={logsNode} open={true} onOpenChange={(v) => { if (!v) setLogsNode(null) }} />
      ) : null}
    </>
  )
}

// ── Page ─────────────────────────────────────────────────────────────────────

export function NodesPage() {
  const [nodes, setNodes] = useState<FleetNodeCardData[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [refreshing, setRefreshing] = useState(false)
  const [view, setView] = useState<ViewMode>('cards')
  const loadedRef = useRef(false)
  const activeController = useRef<AbortController | null>(null)

  const loadNodes = useCallback(async () => {
    activeController.current?.abort()
    const controller = new AbortController()
    activeController.current = controller

    if (!loadedRef.current) setLoading(true)
    setError(null)
    setRefreshing(true)

    try {
      const list = await fetchFleetDevices(controller.signal)
      const details = await Promise.all(
        list.map(async (node: FleetDevice) => {
          try {
            return await fetchFleetNode(node.node_id, controller.signal)
          } catch (err) {
            return {
              ...node,
              status: node.status ?? null,
              detailError: err instanceof Error ? err.message : 'Failed to load node details',
            } as FleetNodeCardData
          }
        }),
      )
      if (!controller.signal.aborted) {
        setNodes(details)
        loadedRef.current = true
      }
    } catch (err) {
      if (!(err instanceof DOMException && err.name === 'AbortError')) {
        setError(err instanceof Error ? err.message : 'Failed to load nodes')
      }
    } finally {
      if (!controller.signal.aborted) {
        setLoading(false)
        setRefreshing(false)
      }
    }
  }, [])

  useEffect(() => {
    let cancelled = false
    void loadNodes()
    const interval = setInterval(() => { if (!cancelled) void loadNodes() }, 60_000)
    return () => {
      cancelled = true
      activeController.current?.abort()
      clearInterval(interval)
    }
  }, [loadNodes])

  const summary = useMemo(() => {
    const healthy = nodes.filter((n) => statusTone(n) === 'healthy').length
    const needsAttention = nodes.filter((n) => statusTone(n) === 'warning').length
    const offline = nodes.filter((n) => statusTone(n) === 'error').length
    const activeClaude = nodes.reduce((s, n) => s + (n.status?.active_claude_sessions ?? 0), 0)
    const activeCodex = nodes.reduce((s, n) => s + (n.status?.active_codex_sessions ?? 0), 0)
    return { total: nodes.length, healthy, needsAttention, offline, activeClaude, activeCodex }
  }, [nodes])

  return (
    <>
      <AppHeader
        breadcrumbs={[{ label: 'Nodes' }]}
        actions={(
          <div className="flex items-center gap-1 sm:gap-2">
            {/* View toggle */}
            <div className="flex rounded-aurora-1 border border-aurora-border-strong overflow-hidden">
              <button
                type="button"
                title="Card view"
                aria-label="Card view"
                onClick={() => setView('cards')}
                className={`flex items-center gap-1.5 px-2 py-1.5 text-xs transition-colors sm:px-2.5 ${view === 'cards' ? 'bg-aurora-control-surface text-aurora-text-primary' : 'text-aurora-text-muted hover:text-aurora-text-primary'}`}
              >
                <LayoutGrid className="size-3.5" />
                <span className="hidden sm:inline">Cards</span>
              </button>
              <button
                type="button"
                title="Table view"
                aria-label="Table view"
                onClick={() => setView('table')}
                className={`flex items-center gap-1.5 border-l border-aurora-border-strong px-2 py-1.5 text-xs transition-colors sm:px-2.5 ${view === 'table' ? 'bg-aurora-control-surface text-aurora-text-primary' : 'text-aurora-text-muted hover:text-aurora-text-primary'}`}
              >
                <Table2 className="size-3.5" />
                <span className="hidden sm:inline">Table</span>
              </button>
            </div>
            <Button
              variant="outline"
              size="sm"
              className="gap-2 px-2 sm:px-3"
              disabled={refreshing}
              onClick={() => void loadNodes()}
              aria-label={refreshing ? 'Refreshing nodes' : 'Refresh nodes'}
            >
              <RefreshCw className="size-4" />
              <span className="hidden sm:inline">{refreshing ? 'Refreshing…' : 'Refresh'}</span>
            </Button>
            <Button asChild size="sm" variant="outline" className="px-2 sm:px-3">
              <Link href="/logs" aria-label="Global logs">
                <ScrollText className="size-4 sm:hidden" />
                <span className="hidden sm:inline">Global logs</span>
              </Link>
            </Button>
          </div>
        )}
      />

      <div className={`min-h-[calc(100vh-3.5rem)] px-4 py-6 ${AURORA_PAGE_SHELL} ${AURORA_PAGE_FRAME}`}>
        {/* Header */}
        <div className="rounded-aurora-3 border border-aurora-border-strong bg-aurora-panel-strong p-5 shadow-[var(--aurora-shadow-strong),var(--aurora-highlight-strong)]">
          <p className={AURORA_MUTED_LABEL}>Device Fleet</p>
          <h1 className={`${AURORA_DISPLAY_1} text-aurora-text-primary`}>Nodes</h1>
          <p className="mt-1 text-sm text-aurora-text-muted">
            Hub for every enrolled binary device: health, resources, sessions, and live node logs.
          </p>
        </div>

        {/* Summary bar */}
        <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-5">
          {[
            { label: 'Total nodes', value: summary.total, cls: 'text-aurora-text-primary' },
            { label: 'Healthy', value: summary.healthy, cls: 'text-aurora-accent-strong' },
            { label: 'Needs attention', value: summary.needsAttention, cls: 'text-aurora-warn' },
            { label: 'Offline', value: summary.offline, cls: 'text-aurora-error' },
            { label: 'Active sessions', value: summary.activeClaude + summary.activeCodex, cls: 'text-aurora-text-primary', sub: `Claude ${summary.activeClaude} · Codex ${summary.activeCodex}` },
          ].map(({ label, value, cls, sub }) => (
            <div key={label} className="rounded-aurora-2 border border-aurora-border-strong bg-aurora-panel-medium p-4">
              <p className={AURORA_MUTED_LABEL}>{label}</p>
              <p className={`${AURORA_DISPLAY_2} mt-2 ${cls}`}>{value}</p>
              {sub ? <p className="mt-1 text-xs text-aurora-text-muted">{sub}</p> : null}
            </div>
          ))}
        </div>

        {/* Node list */}
        {loading ? (
          <div className="space-y-2">
            {[0, 1, 2].map((i) => <div key={i} className="h-10 animate-pulse rounded-aurora-2 border border-aurora-border-strong bg-aurora-control-surface" />)}
          </div>
        ) : error ? (
          <div className="rounded-aurora-2 border border-aurora-error/35 bg-aurora-error/10 px-4 py-4 text-aurora-error">{error}</div>
        ) : nodes.length === 0 ? (
          <div className="rounded-aurora-2 border border-aurora-border-strong bg-aurora-control-surface px-4 py-12 text-center">
            <p className="text-sm text-aurora-text-muted">No nodes found</p>
            <Button className="mt-3" size="sm" asChild><Link href="/gateway">Check gateway configuration</Link></Button>
          </div>
        ) : view === 'cards' ? (
          <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3 2xl:grid-cols-4">
            {nodes.map((node) => <NodeCard key={node.node_id} node={node} />)}
          </div>
        ) : (
          <NodesTable nodes={nodes} />
        )}
      </div>
    </>
  )
}
