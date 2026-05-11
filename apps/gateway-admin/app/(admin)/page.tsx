'use client'

import Link from 'next/link'
import { Cable, Wrench, Eye, AlertTriangle, ArrowRight, Activity, Clock3 } from 'lucide-react'
import { AppHeader } from '@/components/app-header'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import { gatewayDetailHref } from '@/lib/api/gateway-config'
import { useGateways } from '@/lib/hooks/use-gateways'
import { Skeleton } from '@/components/ui/skeleton'
import { StatusBadge } from '@/components/gateway/status-badge'
import { TransportBadge } from '@/components/gateway/transport-badge'
import { formatUiDate } from '@/lib/format-ui-time'
import { cn } from '@/lib/utils'
import {
  AURORA_DISPLAY_1,
  AURORA_DISPLAY_NUMBER,
  AURORA_MUTED_LABEL,
  AURORA_PAGE_FRAME,
  AURORA_PAGE_SHELL,
  AURORA_STRONG_PANEL,
} from '@/components/aurora/tokens'

/** Icon container tone classes — no raw hex, uses Aurora semantic tokens. */
const statIconTone = {
  default: 'bg-aurora-accent-primary/15 text-aurora-accent-primary',
  success: 'bg-aurora-accent-strong/15 text-aurora-accent-strong',
  warning: 'bg-aurora-warn/15 text-aurora-warn',
  info: 'bg-aurora-accent-deep/20 text-aurora-accent-strong',
} as const

function StatCard({
  label,
  value,
  detail,
  icon: Icon,
  variant = 'default',
  loading = false,
}: {
  label: string
  value: number | string
  detail: string
  icon: React.ElementType
  variant?: keyof typeof statIconTone
  loading?: boolean
}) {
  return (
    <div className="flex items-center gap-4 p-5">
      <div className={cn('flex size-10 shrink-0 items-center justify-center rounded-lg', statIconTone[variant])}>
        <Icon className="size-5" />
      </div>
      <div className="min-w-0">
        {loading ? (
          <>
            <Skeleton className="mb-1 h-7 w-12" />
            <Skeleton className="h-4 w-32" />
          </>
        ) : (
          <>
            <p className={cn(AURORA_DISPLAY_NUMBER, 'text-[22px]')}>{value}</p>
            <p className="text-sm font-medium text-aurora-text-primary">{label}</p>
            <p className="text-xs text-aurora-text-muted">{detail}</p>
          </>
        )}
      </div>
    </div>
  )
}

export default function OverviewPage() {
  const { data: gateways, isLoading } = useGateways()

  const stats = {
    totalGateways: gateways?.length ?? 0,
    healthyGateways: gateways?.filter(g => g.status.healthy && g.status.connected).length ?? 0,
    totalTools: gateways?.reduce((sum, g) => sum + g.status.discovered_tool_count, 0) ?? 0,
    exposedTools: gateways?.reduce((sum, g) => sum + g.status.exposed_tool_count, 0) ?? 0,
    totalWarnings: gateways?.reduce((sum, g) => sum + g.warnings.length, 0) ?? 0,
  }

  const recentGateways = gateways?.slice(0, 5) ?? []

  return (
    <>
      <AppHeader
        breadcrumbs={[{ label: 'Overview' }]}
      />

      <div className={cn(AURORA_PAGE_FRAME, AURORA_PAGE_SHELL)}>
        {/* Header */}
        <div className={cn(AURORA_STRONG_PANEL, 'px-6 py-5')}>
          <div className="flex flex-col gap-5 lg:flex-row lg:items-end lg:justify-between">
            <div className="max-w-2xl space-y-2">
              <p className={AURORA_MUTED_LABEL}>Server Fleet</p>
              <h1 className={cn(AURORA_DISPLAY_1, 'text-aurora-text-primary')}>Operational overview</h1>
              <p className="text-sm text-aurora-text-muted sm:text-base">
                Keep an eye on reachability, exposure, and recent server changes before clients start depending on them.
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

        {/* Stats Grid */}
        <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
          <Card variant="medium">
            <CardContent className="p-0">
              <StatCard
                label="Total Servers"
                value={stats.totalGateways}
                detail="Managed upstream MCP connections."
                icon={Cable}
                loading={isLoading}
              />
            </CardContent>
          </Card>
          <Card variant="medium">
            <CardContent className="p-0">
              <StatCard
                label="Healthy Connections"
                value={stats.healthyGateways}
                detail="Connected and probing successfully."
                icon={Activity}
                variant="success"
                loading={isLoading}
              />
            </CardContent>
          </Card>
          <Card variant="medium">
            <CardContent className="p-0">
              <StatCard
                label="Discovered Tools"
                value={stats.totalTools}
                detail="Capabilities currently visible upstream."
                icon={Wrench}
                variant="info"
                loading={isLoading}
              />
            </CardContent>
          </Card>
          <Card variant="medium">
            <CardContent className="p-0">
              <StatCard
                label="Exposed Downstream"
                value={stats.exposedTools}
                detail="Tools currently re-published to clients."
                icon={Eye}
                variant="success"
                loading={isLoading}
              />
            </CardContent>
          </Card>
        </div>

        {/* Warnings Banner */}
        {!isLoading && stats.totalWarnings > 0 && (
          <div className="rounded-aurora-2 border border-aurora-warn/30 bg-aurora-warn/8 p-4">
            <div className="flex flex-col gap-3 sm:flex-row sm:items-center">
              <div className="flex min-w-0 items-start gap-3">
                <div className="rounded-full border border-aurora-warn/30 bg-aurora-warn/12 p-2">
                  <AlertTriangle className="size-5 text-aurora-warn" />
                </div>
                <div className="min-w-0 flex-1">
                  <p className="font-semibold text-aurora-warn">
                    {stats.totalWarnings} warning{stats.totalWarnings !== 1 ? 's' : ''} across servers
                  </p>
                  <p className="text-sm text-aurora-text-muted">
                    Review unhealthy or overexposed servers before publishing more downstream tools.
                  </p>
                </div>
              </div>
              <Button variant="outline" size="sm" className="w-full sm:w-auto" asChild>
                <Link href="/gateways">View servers</Link>
              </Button>
            </div>
          </div>
        )}

        {/* Recent Servers */}
        <div>
          <div className="mb-4 flex items-center justify-between">
            <h2 className="text-lg font-semibold text-aurora-text-primary">Recent Servers</h2>
            <Button variant="ghost" size="sm" asChild>
              <Link href="/gateways">
                View all
                <ArrowRight className="ml-1 size-4" />
              </Link>
            </Button>
          </div>

          {isLoading ? (
            <div className="space-y-2">
              {[1, 2, 3].map((i) => (
                <div key={i} className="flex items-center gap-4 rounded-aurora-2 border border-aurora-border-strong bg-aurora-panel-medium p-4">
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
            <div className={cn(AURORA_STRONG_PANEL, 'p-10 text-center')}>
              <div className="mx-auto mb-4 flex size-14 items-center justify-center rounded-full border border-aurora-border-strong bg-aurora-control-surface shadow-[0_8px_16px_rgba(0,0,0,0.16)]">
                <Cable className="size-7 text-aurora-accent-strong" />
              </div>
              <p className="text-lg font-semibold text-aurora-text-primary">No servers configured</p>
              <p className="mt-1 text-sm text-aurora-text-muted">
                Add your first MCP server to get started
              </p>
              <Button className="mt-5" asChild>
                <Link href="/gateways">Add Server</Link>
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
                  <div className={cn(
                    'flex size-10 shrink-0 items-center justify-center rounded-lg transition-colors',
                    gateway.status.healthy && gateway.status.connected
                      ? 'bg-aurora-accent-strong/15 text-aurora-accent-strong'
                      : 'bg-aurora-error/15 text-aurora-error',
                  )}>
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
                      {gateway.status.discovered_tool_count} discovered tools, {gateway.status.exposed_tool_count} exposed downstream
                    </p>
                    <div className="flex flex-wrap items-center gap-3 text-xs text-aurora-text-muted">
                      <span className="inline-flex items-center gap-1">
                        <Clock3 className="size-3.5" />
                        {formatUiDate(gateway.updated_at)}
                      </span>
                      {gateway.warnings.length > 0 && (
                        <span className="text-aurora-warn">{gateway.warnings.length} warning{gateway.warnings.length === 1 ? '' : 's'}</span>
                      )}
                    </div>
                  </div>
                  <div className="text-sm text-aurora-text-muted sm:text-right">
                    <span className={cn(AURORA_DISPLAY_NUMBER, 'block text-[20px] text-aurora-text-primary')}>
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
    </>
  )
}
