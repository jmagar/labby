'use client'

import { useState } from 'react'
import Link from 'next/link'
import useSWR from 'swr'
import {
  Activity,
  AlertTriangle,
  ArrowRight,
  Cable,
  Clock3,
  Coins,
  Gauge,
  HardDrive,
  PlugZap,
  Wrench,
} from 'lucide-react'
import { AppHeader } from '@/components/app-header'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { StatusBadge } from '@/components/gateway/status-badge'
import { TransportBadge } from '@/components/gateway/transport-badge'
import { StatTile } from '@/components/dashboard/stat-tile'
import { WindowSelector } from '@/components/dashboard/window-selector'
import { ToolVolumeChart } from '@/components/dashboard/tool-volume-chart'
import { TopToolsChart } from '@/components/dashboard/top-tools-chart'
import {
  FanOutPanel,
  LeastUsedPanel,
  MostActivePanel,
} from '@/components/dashboard/activity-insight-panels'
import { AnalysisSection } from '@/components/dashboard/analysis-section'
import { ErrorNotice } from '@/components/dashboard/error-notice'
import { ToolDetailDrawer } from '@/components/dashboard/tool-detail-drawer'
import { AgentDetailDrawer } from '@/components/dashboard/agent-detail-drawer'
import { WarningsBanner } from '@/components/dashboard/warnings-banner'
import { DASH_METRIC_SM, DASH_SURFACE } from '@/components/dashboard/ui'
import type { DrillTarget } from '@/components/dashboard/drill'
import { gatewayDetailHref } from '@/lib/api/gateway-config'
import { useGateways } from '@/lib/hooks/use-gateways'
import { useDashboardMetrics } from '@/lib/hooks/use-dashboard-metrics'
import { fetchFleetDevices } from '@/lib/api/device-client'
import {
  WINDOW_LABELS,
  buildLiveFleetStats,
  formatCompactNumber,
  warningsSignature,
} from '@/lib/dashboard/dashboard-metrics'
import type { MetricsWindow } from '@/lib/types/metrics'
import { formatUiDate } from '@/lib/format-ui-time'
import { cn } from '@/lib/utils'
import {
  AURORA_DISPLAY_1,
  AURORA_MEDIUM_PANEL,
  AURORA_MUTED_LABEL,
  AURORA_PAGE_FRAME,
  AURORA_PAGE_SHELL,
  AURORA_STRONG_PANEL,
} from '@/components/aurora/tokens'

function PanelHeading({ title, hint }: { title: string; hint?: string }) {
  return (
    <div className="mb-3 flex items-baseline justify-between gap-3">
      <p className={AURORA_MUTED_LABEL}>{title}</p>
      {hint ? <p className="text-xs text-aurora-text-muted">{hint}</p> : null}
    </div>
  )
}

export default function OverviewPage() {
  const { data: gateways, isLoading: gatewaysLoading } = useGateways()
  const { data: devices, error: devicesError } = useSWR('/fleet-devices', () => fetchFleetDevices())
  const [activeWindow, setActiveWindow] = useState<MetricsWindow>('24h')
  const [drill, setDrill] = useState<DrillTarget | null>(null)
  const { data: metrics, error: metricsError, mutate: reloadMetrics } = useDashboardMetrics(activeWindow)

  const live = buildLiveFleetStats(gateways ?? [], devices ?? [])
  const warningsSig = warningsSignature(gateways ?? [])
  const metricsLoading = !metrics
  const recentGateways = gateways?.slice(0, 5) ?? []

  return (
    <>
      <AppHeader breadcrumbs={[{ label: 'Overview' }]} />

      <div className={cn(AURORA_PAGE_FRAME, AURORA_PAGE_SHELL)}>
        {/* Header */}
        <div className={cn(AURORA_STRONG_PANEL, DASH_SURFACE,'px-6 py-5')}>
          <div className="flex flex-col gap-5 lg:flex-row lg:items-end lg:justify-between">
            <div className="max-w-2xl space-y-2">
              <p className={AURORA_MUTED_LABEL}>Gateway control plane</p>
              <h1 className={cn(AURORA_DISPLAY_1, 'text-aurora-text-primary')}>
                Operational overview
              </h1>
              <p className="text-sm text-aurora-text-muted sm:text-base">
                Reachability, exposure, and usage across your connected MCP
                servers, agents, and devices.
              </p>
            </div>
            <div className="flex flex-wrap gap-3">
              <Button variant="outline" asChild>
                <Link href="/activity">Review activity</Link>
              </Button>
              <Button asChild>
                <Link href="/gateways">Manage servers</Link>
              </Button>
            </div>
          </div>
        </div>

        {/* Warnings banner (dismissable) */}
        {!gatewaysLoading && (
          <WarningsBanner count={live.warnings} signature={warningsSig} />
        )}

        {/* Window controls — govern the four usage tiles + charts below */}
        <div className="flex flex-wrap items-center justify-between gap-3">
          <p className="text-sm text-aurora-text-muted">
            Live fleet state · usage over the {WINDOW_LABELS[activeWindow].toLowerCase()}
          </p>
          <div className="flex items-center gap-2">
            <Button variant="outline" size="sm" asChild>
              <Link href={`/usage?window=${activeWindow}`}>Open explorer</Link>
            </Button>
            <WindowSelector value={activeWindow} onChange={setActiveWindow} />
          </div>
        </div>

        {metricsError ? (
          <ErrorNotice
            message="Couldn't load usage metrics for this window."
            onRetry={() => reloadMetrics()}
          />
        ) : null}

        {/* Unified stats — live fleet + windowed usage, one row */}
        <div className="grid grid-cols-2 gap-4 md:grid-cols-4 2xl:grid-cols-8">
          <StatTile
            label="Connected"
            value={live.connectedServers}
            icon={Cable}
            tone="success"
            loading={gatewaysLoading}
          />
          <StatTile
            label="Offline"
            value={live.offlineServers}
            icon={PlugZap}
            tone={live.offlineServers > 0 ? 'warning' : 'success'}
            loading={gatewaysLoading}
          />
          <StatTile
            label="Discovered tools"
            value={live.discoveredTools}
            icon={Wrench}
            tone="info"
            loading={gatewaysLoading}
          />
          <StatTile
            label="Devices"
            value={live.connectedDevices}
            icon={HardDrive}
            loading={!devices && !devicesError}
          />
          <StatTile
            label="Tool calls"
            value={metrics ? formatCompactNumber(metrics.tool_calls.total) : '—'}
            icon={Activity}
            loading={metricsLoading}
          />
          <StatTile
            label="Failed"
            value={metrics ? formatCompactNumber(metrics.tool_calls.failed) : '—'}
            icon={AlertTriangle}
            tone={metrics && metrics.tool_calls.failed > 0 ? 'error' : 'success'}
            loading={metricsLoading}
          />
          <StatTile
            label="Tokens"
            value={metrics ? formatCompactNumber(metrics.tokens.total) : '—'}
            icon={Coins}
            tone="info"
            loading={metricsLoading}
          />
          <StatTile
            label="Avg tokens"
            value={metrics ? formatCompactNumber(metrics.tokens.avg_per_call) : '—'}
            icon={Gauge}
            loading={metricsLoading}
          />
        </div>

        {/* Charts */}
        <div className="grid gap-4 lg:grid-cols-3">
          <div className={cn(AURORA_MEDIUM_PANEL, DASH_SURFACE, 'p-5 lg:col-span-2')}>
            <PanelHeading title="Tool-call volume" hint={WINDOW_LABELS[activeWindow]} />
            {metrics ? (
              <ToolVolumeChart data={metrics.timeseries} window={activeWindow} />
            ) : (
              <Skeleton className="h-[200px] w-full" />
            )}
          </div>
          <div className={cn(AURORA_MEDIUM_PANEL, DASH_SURFACE, 'p-5')}>
            <PanelHeading title="Top tools" />
            {metrics ? (
              <TopToolsChart
                tools={metrics.tools.top}
                onSelect={(name) => setDrill({ type: 'tool', name })}
              />
            ) : (
              <Skeleton className="h-[200px] w-full" />
            )}
          </div>
        </div>

        {/* Insight panels */}
        <div className="grid gap-4 lg:grid-cols-3">
          {metrics ? (
            <>
              <MostActivePanel
                actors={metrics.actors}
                window={activeWindow}
                onSelectActor={(entry) => setDrill({ type: 'agent', id: entry.id })}
              />
              <FanOutPanel fanOut={metrics.fan_out} />
              <LeastUsedPanel
                tools={metrics.tools.least}
                distinct={metrics.tools.distinct}
                onSelect={(name) => setDrill({ type: 'tool', name })}
              />
            </>
          ) : (
            [1, 2, 3].map((i) => (
              <Skeleton key={i} className="h-[176px] w-full rounded-aurora-3" />
            ))
          )}
        </div>

        {/* ── Performance, cost & rhythm ─────────────────────────────── */}
        {!metricsError ? (
          <AnalysisSection
            metrics={metrics}
            onSelectTool={(name) => setDrill({ type: 'tool', name })}
          />
        ) : null}

        {/* ── Recent servers ─────────────────────────────────────────── */}
        <div>
          <div className="mb-4 flex items-center justify-between">
            <h2 className="text-lg font-semibold text-aurora-text-primary">Recent servers</h2>
            <Button variant="ghost" size="sm" asChild>
              <Link href="/gateways">
                View all
                <ArrowRight className="ml-1 size-4" />
              </Link>
            </Button>
          </div>

          {gatewaysLoading ? (
            <div className="space-y-2">
              {[1, 2, 3].map((i) => (
                <div
                  key={i}
                  className="flex items-center gap-4 rounded-aurora-2 border border-aurora-border-strong bg-aurora-panel-medium p-4"
                >
                  <Skeleton className="size-10 rounded-lg" />
                  <div className="flex-1">
                    <Skeleton className="mb-1 h-5 w-32" />
                    <Skeleton className="h-4 w-24" />
                  </div>
                  <Skeleton className="h-5 w-16" />
                </div>
              ))}
            </div>
          ) : recentGateways.length === 0 ? (
            <div className={cn(AURORA_STRONG_PANEL, DASH_SURFACE,'p-10 text-center')}>
              <div className="mx-auto mb-4 flex size-14 items-center justify-center rounded-full border border-aurora-border-strong bg-aurora-control-surface shadow-[0_8px_16px_rgba(0,0,0,0.16)]">
                <Cable className="size-7 text-aurora-accent-strong" />
              </div>
              <p className="text-lg font-semibold text-aurora-text-primary">No servers configured</p>
              <p className="mt-1 text-sm text-aurora-text-muted">
                Add your first MCP server to get started
              </p>
              <Button className="mt-5" asChild>
                <Link href="/gateways">Add server</Link>
              </Button>
            </div>
          ) : (
            <div className="space-y-2">
              {recentGateways.map((gateway) => (
                <Link
                  key={gateway.id}
                  href={gatewayDetailHref(gateway.id)}
                  className={cn(
                    'group flex flex-col gap-4 rounded-aurora-2 border border-aurora-border-strong bg-aurora-panel-medium p-4 transition-colors',
                    'hover:border-aurora-accent-primary/30 hover:bg-aurora-panel-strong',
                    'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary/34 focus-visible:ring-offset-2 focus-visible:ring-offset-aurora-page-bg',
                    'sm:flex-row sm:items-start',
                  )}
                >
                  <div
                    className={cn(
                      'flex size-10 shrink-0 items-center justify-center rounded-aurora-1 transition-colors',
                      gateway.status.healthy && gateway.status.connected
                        ? 'bg-aurora-accent-strong/15 text-aurora-accent-strong'
                        : 'bg-aurora-error/15 text-aurora-error',
                    )}
                  >
                    <Cable className="size-5" />
                  </div>
                  <div className="min-w-0 flex-1 space-y-2">
                    <div className="flex flex-wrap items-center gap-2">
                      <p className="truncate font-semibold text-aurora-text-primary transition-colors group-hover:text-aurora-accent-strong">
                        {gateway.name}
                      </p>
                      <StatusBadge healthy={gateway.status.healthy} connected={gateway.status.connected} />
                      <TransportBadge transport={gateway.transport} />
                    </div>
                    <p className="text-sm text-aurora-text-muted">
                      {gateway.status.discovered_tool_count} discovered tools,{' '}
                      {gateway.status.exposed_tool_count} exposed downstream
                    </p>
                    <div className="flex flex-wrap items-center gap-3 text-xs text-aurora-text-muted">
                      <span className="inline-flex items-center gap-1">
                        <Clock3 className="size-3.5" />
                        {formatUiDate(gateway.updated_at)}
                      </span>
                      {gateway.warnings.length > 0 && (
                        <span className="text-aurora-warn">
                          {gateway.warnings.length} warning{gateway.warnings.length === 1 ? '' : 's'}
                        </span>
                      )}
                    </div>
                  </div>
                  <div className="text-sm text-aurora-text-muted sm:text-right">
                    <span className={cn(DASH_METRIC_SM, 'block text-aurora-text-primary')}>
                      {gateway.status.exposed_tool_count}
                    </span>
                    exposed
                  </div>
                </Link>
              ))}
            </div>
          )}
        </div>
      </div>

      <ToolDetailDrawer
        tool={drill?.type === 'tool' ? drill.name : null}
        window={activeWindow}
        onClose={() => setDrill(null)}
        onDrill={setDrill}
      />
      <AgentDetailDrawer
        agentId={drill?.type === 'agent' ? drill.id : null}
        window={activeWindow}
        onClose={() => setDrill(null)}
        onDrill={setDrill}
      />
    </>
  )
}
