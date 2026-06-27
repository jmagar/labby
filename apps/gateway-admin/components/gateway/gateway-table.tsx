'use client'

import { type ReactNode, useMemo, useState } from 'react'
import Link from 'next/link'
import {
  ArrowDown,
  ArrowUp,
  Check,
  ChevronDown,
  Copy,
  MoreHorizontal,
  Eye,
  Pencil,
  Play,
  Power,
  RefreshCw,
  Search,
  TriangleAlert,
  Trash2,
  FileText,
  MessageSquare,
  Wrench,
} from 'lucide-react'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Badge } from '@/components/ui/badge'
import { cn } from '@/lib/utils'
import { ActionConfirmationDialog } from '@/components/action-confirmation-dialog'
import { TransportBadge } from './transport-badge'
import { WarningsPill } from './warnings-pill'
import type { Gateway } from '@/lib/types/gateway'
import { gatewayDetailHref } from '@/lib/api/gateway-config'
import { buildGatewayEndpointPreview } from '@/lib/api/gateway-mobile'
import { SurfaceRatio } from './surface-ratio'
import type { TransportType } from '@/lib/types/gateway'
import {
  AURORA_MUTED_LABEL,
} from '@/components/aurora/tokens'
import {
  AURORA_GATEWAY_DISABLED_ROW,
  AURORA_GATEWAY_ROW,
  gatewayActionTone,
  gatewayStatusTone,
} from './gateway-theme'

type SortKey = 'name' | 'transport' | 'tools' | 'resources' | 'prompts'
type SortDirection = 'asc' | 'desc'

const AURORA_GATEWAY_TABLE_SHELL =
  'border border-aurora-border-strong bg-aurora-panel-strong shadow-[var(--aurora-shadow-strong),var(--aurora-highlight-strong)] rounded-aurora-1'

const GATEWAY_TABLE_BADGE =
  'inline-flex h-6 items-center rounded-full px-2 text-[10px] font-semibold uppercase tracking-[0.12em]'

const GATEWAY_TABLE_ACTION =
  'size-8 rounded-aurora-1 hover:bg-aurora-hover-bg hover:text-aurora-text-primary'

const rowToneClass = (index: number) => (index % 2 === 0 ? 'gateway-row-tone-a' : 'gateway-row-tone-b')

function isStaleVirtualServer(gateway: Gateway): boolean {
  return gateway.source === 'in_process' && gateway.warnings.some((warning) => warning.code === 'unknown_service')
}

function canRemoveGateway(gateway: Gateway): boolean {
  return gateway.source !== 'in_process' || isStaleVirtualServer(gateway)
}

interface GatewayTableProps {
  gateways: Gateway[]
  density: 'comfortable' | 'condensed'
  cleanupSummaryByGatewayId?: Record<
    string,
    { preview?: { label: string; occurredAt: string }; cleanup?: { label: string; occurredAt: string } }
  >
  onEdit: (gateway: Gateway) => void
  onTest: (gateway: Gateway) => void
  onReload: (gateway: Gateway) => void
  onCleanup: (gateway: Gateway, aggressive: boolean, dryRun: boolean) => void
  onClearCleanupHistory: (gateway: Gateway) => void
  onToggleEnabled: (gateway: Gateway) => void
  onDelete: (gateway: Gateway) => void
}

export function GatewayTable({
  gateways,
  density,
  cleanupSummaryByGatewayId = {},
  onEdit,
  onTest,
  onReload,
  onCleanup,
  onClearCleanupHistory,
  onToggleEnabled,
  onDelete,
}: GatewayTableProps) {
  const [loadingAction, setLoadingAction] = useState<{ id: string; action: string } | null>(null)
  const [sortKey, setSortKey] = useState<SortKey>('name')
  const [sortDirection, setSortDirection] = useState<SortDirection>('asc')
  const [copiedGatewayId, setCopiedGatewayId] = useState<string | null>(null)
  const [expandedMobileGatewayId, setExpandedMobileGatewayId] = useState<string | null>(null)
  const [disableConfirmationGatewayId, setDisableConfirmationGatewayId] = useState<string | null>(null)
  const disableConfirmationGateway = disableConfirmationGatewayId
    ? gateways.find((gateway) => gateway.id === disableConfirmationGatewayId) ?? null
    : null

  const requestToggleEnabled = (gateway: Gateway) => {
    if (gateway.enabled ?? true) {
      setDisableConfirmationGatewayId(gateway.id)
      return
    }
    onToggleEnabled(gateway)
  }

  const confirmDisableGateway = () => {
    const gateway = disableConfirmationGateway
    setDisableConfirmationGatewayId(null)
    if (!gateway) return
    onToggleEnabled(gateway)
  }

  const handleAction = async (
    gateway: Gateway,
    action: 'test' | 'reload',
    handler: (gateway: Gateway) => void | Promise<void>,
  ) => {
    setLoadingAction({ id: gateway.id, action })
    try {
      await handler(gateway)
    } finally {
      setLoadingAction(null)
    }
  }

  const isLoading = (id: string, action: string) => loadingAction?.id === id && loadingAction?.action === action

  const copyCommand = async (gateway: Gateway, value: string) => {
    try {
      await navigator.clipboard.writeText(value)
      setCopiedGatewayId(gateway.id)
      window.setTimeout(() => setCopiedGatewayId((current) => (current === gateway.id ? null : current)), 1200)
    } catch {
      // Clipboard failures should not block table use.
    }
  }

  const sortedGateways = useMemo(() => {
    const transportLabel = (transport: TransportType) => {
      switch (transport) {
        case 'in_process':
          return 'lab'
        case 'stdio':
          return 'stdio'
        case 'http':
          return 'http'
      }
    }

    const sorted = [...gateways].sort((left, right) => {
      let result = 0

      switch (sortKey) {
        case 'name':
          result = left.name.localeCompare(right.name, undefined, { sensitivity: 'base' })
          break
        case 'transport':
          result = transportLabel(left.transport).localeCompare(transportLabel(right.transport))
          break
        case 'tools':
          result = left.status.exposed_tool_count - right.status.exposed_tool_count
          break
        case 'resources':
          result = left.status.exposed_resource_count - right.status.exposed_resource_count
          break
        case 'prompts':
          result = left.status.exposed_prompt_count - right.status.exposed_prompt_count
          break
      }

      if (result === 0) {
        result = left.name.localeCompare(right.name)
      }

      return sortDirection === 'asc' ? result : -result
    })

    return sorted
  }, [gateways, sortDirection, sortKey])

  const handleSort = (nextKey: SortKey) => {
    if (sortKey === nextKey) {
      setSortDirection((current) => (current === 'asc' ? 'desc' : 'asc'))
      return
    }

    setSortKey(nextKey)
    setSortDirection(nextKey === 'name' || nextKey === 'transport' ? 'asc' : 'desc')
  }

  const renderSortIcon = (key: SortKey) => {
    if (sortKey !== key) return null

    return sortDirection === 'asc'
      ? <ArrowUp className="size-3.5" />
      : <ArrowDown className="size-3.5" />
  }

  const activeSortLabel = `${sortKey === 'name' ? 'Server' : sortKey[0]!.toUpperCase() + sortKey.slice(1)} ${sortDirection === 'asc' ? '↑' : '↓'}`

  const SortHeader = ({ label, sort }: { label: string; sort: SortKey }) => (
    <button
      type="button"
      onClick={() => handleSort(sort)}
      className="inline-flex items-center gap-1.5 transition-colors hover:text-aurora-text-primary"
      aria-label={`Sort by ${label.toLowerCase()}`}
      aria-sort={sortKey === sort ? (sortDirection === 'asc' ? 'ascending' : 'descending') : 'none'}
    >
      <span>{label}</span>
      {renderSortIcon(sort)}
    </button>
  )

  const formatRuntimeAge = (ageSeconds?: number) => {
    if (!ageSeconds || ageSeconds < 0) return null
    if (ageSeconds < 60) return `${ageSeconds}s old`
    if (ageSeconds < 3600) return `${Math.floor(ageSeconds / 60)}m old`
    if (ageSeconds < 86400) return `${Math.floor(ageSeconds / 3600)}h old`
    return `${Math.floor(ageSeconds / 86400)}d old`
  }

  const runtimeAgeLabel = (gateway: Gateway) => formatRuntimeAge(gateway.status.age_seconds)

  const formatHistoryTime = (occurredAt: string) => {
    const date = new Date(occurredAt)
    if (Number.isNaN(date.getTime())) return null
    return date.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })
  }

  const cleanupBadgeLabel = (
    entry: { label: string; occurredAt: string } | undefined,
    prefix: string,
  ) => {
    if (!entry) return null
    const time = formatHistoryTime(entry.occurredAt)
    return time ? `${prefix} ${time}` : prefix
  }

  const runtimeDetailsTitle = (gateway: Gateway) => {
    const owner = gateway.status.owner
    const lines = [
      owner ? `Owner surface: ${owner.surface}` : null,
      owner?.client_name ? `Owner client: ${owner.client_name}` : null,
      owner?.subject ? `Owner subject: ${owner.subject}` : null,
      owner?.request_id ? `Owner request: ${owner.request_id}` : null,
      owner?.session_id ? `Owner session: ${owner.session_id}` : null,
      gateway.status.origin ? `Origin: ${gateway.status.origin}` : null,
      gateway.status.runtime_state_path ? `Runtime snapshot: ${gateway.status.runtime_state_path}` : null,
      gateway.status.reconciled_at ? `Reconciled: ${gateway.status.reconciled_at}` : null,
    ].filter(Boolean)

    return lines.length > 0 ? lines.join('\n') : undefined
  }

  const runtimeBadges = (gateway: Gateway) => {
    const badges: ReactNode[] = []
    const detailsTitle = runtimeDetailsTitle(gateway)

    if ((gateway.status.likely_stale_count ?? 0) > 0) {
      badges.push(
        <Badge
          key="stale"
          title={detailsTitle}
          className={cn(GATEWAY_TABLE_BADGE, 'border border-aurora-warn/30 bg-[color-mix(in_srgb,var(--aurora-warn)_12%,transparent)] text-aurora-warn')}
        >
          {gateway.status.likely_stale_count} stale
        </Badge>,
      )
    }

    if (gateway.status.pid) {
      badges.push(
        <Badge
          key="pid"
          title={detailsTitle}
          className={cn(GATEWAY_TABLE_BADGE, 'border border-aurora-border-strong bg-[rgba(7,17,26,0.48)] font-mono text-aurora-text-muted')}
        >
          pid {gateway.status.pid}
        </Badge>,
      )
    }

    if (gateway.status.pgid && gateway.status.pgid !== gateway.status.pid) {
      badges.push(
        <Badge
          key="pgid"
          title={detailsTitle}
          className={cn(GATEWAY_TABLE_BADGE, 'border border-aurora-border-strong bg-[rgba(7,17,26,0.48)] font-mono text-aurora-text-muted')}
        >
          pgid {gateway.status.pgid}
        </Badge>,
      )
    }

    const age = runtimeAgeLabel(gateway)
    if (age) {
      badges.push(
        <Badge
          key="age"
          title={detailsTitle}
          className={cn(GATEWAY_TABLE_BADGE, 'border border-aurora-border-strong bg-[rgba(7,17,26,0.48)] text-aurora-text-muted')}
        >
          {age}
        </Badge>,
      )
    }

    return badges
  }

  const statusRailClass = (gateway: Gateway) => {
    if (!(gateway.enabled ?? true)) return 'bg-aurora-neutral'
    if (gateway.status.healthy && gateway.status.connected && gateway.warnings.length === 0) return 'bg-aurora-accent-strong'
    if (!gateway.status.connected) return 'bg-aurora-error'
    return 'bg-aurora-warn'
  }

  const commandParts = (gateway: Gateway, preview: string) => {
    if (gateway.transport !== 'stdio') {
      return { command: preview, args: '' }
    }
    const command = gateway.config.command?.trim()
    if (!command) return { command: preview, args: '' }
    const args = (gateway.config.args ?? []).join(' ')
    return { command, args }
  }

  const CommandPreview = ({
    gateway,
    preview,
    compact = false,
  }: {
    gateway: Gateway
    preview: string
    compact?: boolean
  }) => {
    const isCommand = gateway.transport === 'stdio'
    const parts = commandParts(gateway, preview)

    return (
      <span
        className={cn(
          'min-w-0 max-w-full font-mono text-aurora-text-muted transition-colors group-hover:text-aurora-text-primary/82',
          compact ? 'text-[9px] leading-3' : 'text-[11px] leading-5',
          isCommand ? 'whitespace-normal break-all' : 'truncate',
        )}
        title={preview}
      >
        {isCommand && parts.args ? (
          <>
            <span className="font-semibold text-aurora-text-primary/86">{parts.command}</span>
            <span className="text-aurora-text-muted"> {parts.args}</span>
          </>
        ) : (
          preview
        )}
      </span>
    )
  }

  return (
    <>
      <div className={cn(AURORA_GATEWAY_TABLE_SHELL, 'overflow-hidden md:hidden')}>
        <div className="grid grid-cols-[minmax(0,1fr)_82px_24px] gap-2 border-b border-aurora-border-strong px-2.5 py-2">
          <div className={AURORA_MUTED_LABEL}>Server</div>
          <div className={cn(AURORA_MUTED_LABEL, 'text-right')}>State</div>
          <div />
        </div>
        <div className="divide-y divide-aurora-border-strong/70">
          {sortedGateways.map((gateway, index) => {
            const supportsProbeControls = gateway.source !== 'in_process'
            const canRemoveGatewayRow = canRemoveGateway(gateway)
            const isDisabled = !(gateway.enabled ?? true)
            const statusTone = gatewayStatusTone(gateway.status.healthy, gateway.status.connected)
            const endpointPreview = buildGatewayEndpointPreview(gateway)
            const showsCommandLine = gateway.transport === 'stdio'
            const isExpanded = expandedMobileGatewayId === gateway.id
            const envCount = Object.keys(gateway.config.env ?? {}).length
            const runtimeLabel = runtimeAgeLabel(gateway) ?? 'live'
            const cleanupSummary = cleanupSummaryByGatewayId[gateway.id]
            const cleanupSummaryLabel =
              cleanupBadgeLabel(cleanupSummary?.cleanup, 'cleaned') ??
              cleanupBadgeLabel(cleanupSummary?.preview, 'preview')
            const rowTone = index % 2 === 0 ? 'gateway-row-tone-a' : 'gateway-row-tone-b'

            return (
              <div
                key={gateway.id}
                className={cn(
                  'relative overflow-hidden',
                  rowTone,
                  AURORA_GATEWAY_ROW,
                  isDisabled && AURORA_GATEWAY_DISABLED_ROW,
                )}
              >
                <span className={cn('absolute inset-y-0 left-0 w-1', statusRailClass(gateway))} aria-hidden="true" />
                <div className={cn('grid grid-cols-[minmax(0,1fr)_82px_24px] gap-2 px-2.5', density === 'condensed' ? 'py-1.5' : 'py-2')}>
                  <div className="min-w-0 space-y-1 pl-2">
                    <div className="flex min-w-0 items-center gap-2">
                      <span className={cn('size-2 rounded-full', statusTone.dot)} aria-label={statusTone.label} title={statusTone.label} />
                      <Link href={gatewayDetailHref(gateway.id)} className="truncate text-[12px] font-semibold text-aurora-text-primary hover:text-aurora-accent-strong">
                        {gateway.name}
                      </Link>
                      {isDisabled ? (
                        <span className="rounded-full border border-aurora-border-strong px-1.5 py-0.5 text-[9px] uppercase tracking-[0.12em] text-aurora-text-muted">
                          Off
                        </span>
                      ) : null}
                    </div>
                    <button
                      type="button"
                      className={cn(
                        'group/command flex w-full min-w-0 items-start gap-1.5 text-left',
                        showsCommandLine && 'rounded-aurora-1 border border-transparent hover:border-aurora-border-strong/70 hover:bg-aurora-control-surface/45',
                      )}
                      onClick={() => {
                        if (showsCommandLine) {
                          setExpandedMobileGatewayId((current) => (current === gateway.id ? null : gateway.id))
                        }
                      }}
                      aria-expanded={showsCommandLine ? isExpanded : undefined}
                      aria-label={showsCommandLine ? `${isExpanded ? 'Collapse' : 'Expand'} ${gateway.name} command` : undefined}
                      title={endpointPreview}
                    >
                      <CommandPreview gateway={gateway} preview={endpointPreview} compact />
                      {showsCommandLine ? (
                        <ChevronDown
                          className={cn(
                            'mt-0.5 size-3 shrink-0 text-aurora-text-muted transition-transform',
                            isExpanded && 'rotate-180',
                          )}
                          aria-hidden="true"
                        />
                      ) : null}
                    </button>
                    {showsCommandLine && isExpanded ? (
                      <div className="rounded-aurora-1 border border-aurora-border-strong bg-aurora-control-surface/70 p-2 text-[9px] leading-4 shadow-[var(--aurora-highlight-medium)]">
                        <div className="flex items-start justify-between gap-2">
                          <code className="font-mono text-aurora-text-primary break-all">{endpointPreview}</code>
                          <button
                            type="button"
                            onClick={() => copyCommand(gateway, endpointPreview)}
                            className="inline-flex size-6 shrink-0 items-center justify-center rounded-aurora-1 border border-aurora-border-strong bg-aurora-panel-medium text-aurora-text-muted"
                            aria-label={`Copy ${gateway.name} command`}
                          >
                            {copiedGatewayId === gateway.id ? <Check className="size-3" /> : <Copy className="size-3" />}
                          </button>
                        </div>
                        <div className="mt-1 text-[8px] uppercase tracking-[0.12em] text-aurora-text-muted">
                          {envCount > 0 ? `${envCount} env vars` : 'No env vars'}
                        </div>
                      </div>
                    ) : null}
                    <div className="flex flex-wrap items-center gap-x-2.5 gap-y-1 text-[9px] text-aurora-text-muted">
                      <span data-mobile-metric="tools" className="inline-flex items-center gap-1 whitespace-nowrap" title="Tools">
                        <Wrench className="size-3 text-aurora-text-muted" aria-hidden="true" />
                        <span className="sr-only">Tools:</span>
                        <strong className="text-[10px] font-semibold text-aurora-text-primary">{gateway.status.exposed_tool_count}</strong>
                      </span>
                      <span data-mobile-metric="resources" className="inline-flex items-center gap-1 whitespace-nowrap" title="Resources">
                        <FileText className="size-3 text-aurora-text-muted" aria-hidden="true" />
                        <span className="sr-only">Resources:</span>
                        <strong className="text-[10px] font-semibold text-aurora-text-primary">{gateway.status.exposed_resource_count}</strong>
                      </span>
                      <span data-mobile-metric="prompts" className="inline-flex items-center gap-1 whitespace-nowrap" title="Prompts">
                        <MessageSquare className="size-3 text-aurora-text-muted" aria-hidden="true" />
                        <span className="sr-only">Prompts:</span>
                        <strong className="text-[10px] font-semibold text-aurora-text-primary">{gateway.status.exposed_prompt_count}</strong>
                      </span>
                      <span data-mobile-metric="runtime" className="inline-flex items-center gap-1 whitespace-nowrap" title="Runtime age">
                        <RefreshCw className="size-3 text-aurora-text-muted" aria-hidden="true" />
                        <span className="sr-only">Runtime age:</span>
                        <strong className="text-[10px] font-semibold text-aurora-text-primary">{runtimeLabel}</strong>
                      </span>
                    </div>
                  </div>

                  <div className="space-y-0.5 pt-0.5 text-right">
                    <div className="inline-flex items-center justify-end gap-1 text-[10px] font-semibold text-aurora-text-primary">
                      <span className={cn('size-1.5 rounded-full', statusTone.dot)} />
                      <span>{statusTone.label}</span>
                    </div>
                    <div className="text-[8px] uppercase tracking-[0.12em] text-aurora-text-muted">
                      {cleanupSummaryLabel ?? (isDisabled ? 'disabled' : gateway.warnings.length > 0 ? `${gateway.warnings.length} warn` : 'clean')}
                    </div>
                  </div>

                  <DropdownMenu>
                    <DropdownMenuTrigger asChild>
                      <Button
                        variant="outline"
                        size="icon"
                        className={cn(gatewayActionTone(), 'size-6 shrink-0 rounded-full hover:bg-aurora-hover-bg hover:text-aurora-text-primary')}
                      >
                        <MoreHorizontal className="size-3" />
                        <span className="sr-only">More actions</span>
                      </Button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="end">
                      <DropdownMenuItem asChild>
                        <Link href={gatewayDetailHref(gateway.id)}>
                          <Eye className="size-4 mr-2" />
                          View details
                        </Link>
                      </DropdownMenuItem>
                      <DropdownMenuItem onClick={() => onEdit(gateway)}>
                        <Pencil className="size-4 mr-2" />
                        Edit server
                      </DropdownMenuItem>
                      <DropdownMenuItem onClick={() => requestToggleEnabled(gateway)}>
                        {gateway.enabled ?? true ? (
                          <>
                            <Trash2 className="size-4 mr-2" />
                            Disable server
                          </>
                        ) : (
                          <>
                            <Play className="size-4 mr-2" />
                            Enable server
                          </>
                        )}
                      </DropdownMenuItem>
                      {supportsProbeControls ? (
                        <>
                          <DropdownMenuSeparator />
                          <DropdownMenuItem onClick={() => onTest(gateway)}>
                            <Play className="size-4 mr-2" />
                            Test connection
                          </DropdownMenuItem>
                          <DropdownMenuItem onClick={() => onReload(gateway)}>
                            <RefreshCw className="size-4 mr-2" />
                            Reload server
                          </DropdownMenuItem>
                          <DropdownMenuItem onClick={() => onCleanup(gateway, false, true)}>
                            <Search className="size-4 mr-2" />
                            Preview cleanup
                          </DropdownMenuItem>
                          <DropdownMenuItem onClick={() => onCleanup(gateway, false, false)}>
                            <Wrench className="size-4 mr-2" />
                            Cleanup runtime
                          </DropdownMenuItem>
                          <DropdownMenuItem onClick={() => onCleanup(gateway, true, true)}>
                            <Search className="size-4 mr-2" />
                            Preview aggressive cleanup
                          </DropdownMenuItem>
                          <DropdownMenuItem onClick={() => onCleanup(gateway, true, false)}>
                            <TriangleAlert className="size-4 mr-2" />
                            Aggressive cleanup
                          </DropdownMenuItem>
                          {cleanupSummary ? (
                            <>
                              <DropdownMenuSeparator />
                              <DropdownMenuItem onClick={() => onClearCleanupHistory(gateway)}>
                                <Trash2 className="size-4 mr-2" />
                                Clear cleanup history
                              </DropdownMenuItem>
                            </>
                          ) : null}
                        </>
                      ) : null}
                      {canRemoveGatewayRow ? (
                        <>
                          <DropdownMenuSeparator />
                          <DropdownMenuItem onClick={() => onDelete(gateway)} className="text-aurora-error focus:text-aurora-error">
                            <Trash2 className="size-4 mr-2" />
                            {gateway.source === 'in_process' ? 'Remove stale service' : 'Remove server'}
                          </DropdownMenuItem>
                        </>
                      ) : null}
                    </DropdownMenuContent>
                  </DropdownMenu>
                </div>
              </div>
            )
          })}
        </div>
      </div>

      <div className={cn(AURORA_GATEWAY_TABLE_SHELL, 'hidden overflow-hidden md:block')}>
        <div className="flex items-center justify-between border-b border-aurora-border-strong/80 bg-[repeating-linear-gradient(90deg,rgba(41,182,246,0.045)_0,rgba(41,182,246,0.045)_1px,transparent_1px,transparent_18px),linear-gradient(180deg,rgba(7,17,26,0.56),rgba(7,17,26,0.38))] px-5 py-2">
          <span className="text-[10px] font-bold uppercase tracking-[0.18em] text-aurora-text-muted">
            Server inventory
          </span>
          <span className="inline-flex h-6 items-center rounded-full border border-aurora-accent-primary/24 bg-aurora-accent-primary/10 px-2.5 text-[10px] font-semibold uppercase tracking-[0.12em] text-aurora-accent-strong">
            Sorted by {activeSortLabel}
          </span>
        </div>
        <Table className="min-w-[920px] table-fixed">
          <TableHeader>
            <TableRow className="border-b border-aurora-border-strong bg-[rgba(7,17,26,0.48)] hover:bg-[rgba(7,17,26,0.48)]">
              <TableHead className={cn(AURORA_MUTED_LABEL, 'w-[50%] px-5 py-3')}>
                <SortHeader label="Server" sort="name" />
              </TableHead>
              <TableHead className={cn(AURORA_MUTED_LABEL, 'w-[8%] px-2 py-3 text-center')}>
                <SortHeader label="Transport" sort="transport" />
              </TableHead>
              <TableHead className={cn(AURORA_MUTED_LABEL, 'w-[9%] px-2 py-3 text-center')}>
                <SortHeader label="Tools" sort="tools" />
              </TableHead>
              <TableHead className={cn(AURORA_MUTED_LABEL, 'w-[10%] px-2 py-3 text-center')}>
                <SortHeader label="Resources" sort="resources" />
              </TableHead>
              <TableHead className={cn(AURORA_MUTED_LABEL, 'w-[9%] px-2 py-3 text-center')}>
                <SortHeader label="Prompts" sort="prompts" />
              </TableHead>
              <TableHead className={cn(AURORA_MUTED_LABEL, 'w-[14%] px-4 py-3 text-right')}>Actions</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {sortedGateways.map((gateway, index) => {
              const supportsProbeControls = gateway.source !== 'in_process'
              const canRemoveGatewayRow = canRemoveGateway(gateway)
              const endpointPreview = buildGatewayEndpointPreview(gateway)
              const showsCommandLine = gateway.transport === 'stdio'
              const isDisabled = !(gateway.enabled ?? true)
              const statusTone = gatewayStatusTone(gateway.status.healthy, gateway.status.connected)
              const runtimeChips = runtimeBadges(gateway)
              const cleanupSummary = cleanupSummaryByGatewayId[gateway.id]
              const cleanupBadge = cleanupBadgeLabel(cleanupSummary?.cleanup, 'cleaned')
              const previewBadge = cleanupBadgeLabel(cleanupSummary?.preview, 'preview')
              const rowTone = rowToneClass(index)

              return (
                <TableRow
                  key={gateway.id}
                  className={cn(
                    'group transition-[background-color,box-shadow,transform] duration-150 hover:translate-x-px hover:shadow-[inset_3px_0_0_color-mix(in_srgb,var(--aurora-accent-primary)_42%,transparent)]',
                    rowTone,
                    isDisabled ? AURORA_GATEWAY_DISABLED_ROW : AURORA_GATEWAY_ROW,
                  )}
                >
                  <TableCell className={cn('relative px-5 align-middle whitespace-normal', density === 'condensed' ? 'py-2.5' : 'py-3.5')}>
                    <span className={cn('absolute inset-y-0 left-0 w-1', statusRailClass(gateway))} aria-hidden="true" />
                    <div className="min-w-0 space-y-1">
                      <div className="flex min-w-0 flex-wrap items-center gap-2">
                        <span className={cn('size-2 rounded-full', statusTone.dot)} aria-label={statusTone.label} title={statusTone.label} />
                        <Link
                          href={gatewayDetailHref(gateway.id)}
                          className={cn(
                            'min-w-0 max-w-full break-words font-display text-[15px] leading-[1.16] font-bold text-aurora-text-primary hover:text-aurora-accent-strong hover:underline underline-offset-4',
                            density === 'condensed' && 'text-[14px]',
                          )}
                        >
                          {gateway.name}
                        </Link>
                        {isDisabled ? (
                          <Badge className={cn(GATEWAY_TABLE_BADGE, 'border border-aurora-border-strong bg-[rgba(7,17,26,0.48)] text-aurora-text-muted')}>
                            Disabled
                          </Badge>
                        ) : null}
                        <WarningsPill warnings={gateway.warnings} />
                        {runtimeChips}
                        {cleanupSummary?.cleanup && cleanupBadge ? (
                          <Badge
                            className={cn(GATEWAY_TABLE_BADGE, 'border border-aurora-success/30 bg-[color-mix(in_srgb,var(--aurora-success)_12%,transparent)] text-aurora-success')}
                            title={`${cleanupSummary.cleanup.label}\n${cleanupSummary.cleanup.occurredAt}`}
                          >
                            {cleanupBadge}
                          </Badge>
                        ) : null}
                        {cleanupSummary?.preview && previewBadge ? (
                          <Badge
                            className={cn(GATEWAY_TABLE_BADGE, 'border border-aurora-accent-primary/30 bg-aurora-accent-primary/10 text-aurora-accent-strong')}
                            title={`${cleanupSummary.preview.label}\n${cleanupSummary.preview.occurredAt}`}
                          >
                            {previewBadge}
                          </Badge>
                        ) : null}
                        {density === 'condensed' ? (
                          <span className="min-w-0 truncate font-mono text-[11px] leading-4 text-aurora-text-muted" title={endpointPreview}>
                            {endpointPreview}
                          </span>
                        ) : null}
                      </div>
                      {density === 'comfortable' ? (
                        <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-[12px] leading-5 text-aurora-text-muted">
                          <div className="group/command flex min-w-0 max-w-full items-start gap-1.5 rounded-aurora-1 border border-transparent pr-1 transition-[background-color,border-color] hover:border-aurora-border-strong/70 hover:bg-aurora-control-surface/45">
                            <CommandPreview
                              gateway={gateway}
                              preview={endpointPreview}
                            />
                            <button
                              type="button"
                              onClick={() => copyCommand(gateway, endpointPreview)}
                              className={cn(
                                'mt-0.5 inline-flex size-5 shrink-0 items-center justify-center rounded-aurora-1 border border-aurora-border-strong bg-aurora-panel-medium text-aurora-text-muted opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100 group-hover/command:opacity-100',
                                copiedGatewayId === gateway.id && 'border-aurora-accent-primary/35 text-aurora-accent-strong opacity-100',
                              )}
                              aria-label={`Copy ${gateway.name} ${showsCommandLine ? 'command' : 'endpoint'}`}
                              title={`Copy ${showsCommandLine ? 'command' : 'endpoint'}`}
                            >
                              {copiedGatewayId === gateway.id ? <Check className="size-3" /> : <Copy className="size-3" />}
                            </button>
                          </div>
                        </div>
                      ) : null}
                    </div>
                  </TableCell>
                  <TableCell className={cn('px-2 align-middle', density === 'condensed' ? 'py-2.5' : 'py-3.5')}>
                    <div className="flex items-center justify-center">
                      <TransportBadge transport={gateway.transport} iconOnly />
                    </div>
                  </TableCell>
                  <TableCell className={cn('px-2 align-middle', density === 'condensed' ? 'py-2.5' : 'py-3.5')}>
                    <div className="flex items-center justify-center">
                      <SurfaceRatio icon={Wrench} label="Tools" exposed={gateway.status.exposed_tool_count} total={gateway.status.discovered_tool_count} />
                    </div>
                  </TableCell>
                  <TableCell className={cn('px-2 align-middle', density === 'condensed' ? 'py-2.5' : 'py-3.5')}>
                    <div className="flex items-center justify-center">
                      <SurfaceRatio icon={FileText} label="Resources" exposed={gateway.status.exposed_resource_count} total={gateway.status.discovered_resource_count} />
                    </div>
                  </TableCell>
                  <TableCell className={cn('px-2 align-middle', density === 'condensed' ? 'py-2.5' : 'py-3.5')}>
                    <div className="flex items-center justify-center">
                      <SurfaceRatio icon={MessageSquare} label="Prompts" exposed={gateway.status.exposed_prompt_count} total={gateway.status.discovered_prompt_count} />
                    </div>
                  </TableCell>
                  <TableCell className={cn('px-4 text-right align-middle', density === 'condensed' ? 'py-2.5' : 'py-3.5')}>
                    <div className="flex items-center justify-end gap-1">
                      {density === 'comfortable' ? (
                        <Button
                          variant="outline"
                          size="icon"
                          className={cn(gatewayActionTone(), GATEWAY_TABLE_ACTION, 'opacity-100 transition-opacity md:opacity-0 md:focus-visible:opacity-100 md:group-hover:opacity-100')}
                          onClick={() => requestToggleEnabled(gateway)}
                        >
                          <Power className="size-3.5" />
                          <span className="sr-only">{gateway.enabled ?? true ? 'Disable server' : 'Enable server'}</span>
                        </Button>
                      ) : null}
                      {supportsProbeControls && density === 'comfortable' ? (
                        <Button
                          variant="outline"
                          size="icon"
                          className={cn(gatewayActionTone(), GATEWAY_TABLE_ACTION, 'opacity-100 transition-opacity md:opacity-0 md:focus-visible:opacity-100 md:group-hover:opacity-100')}
                          onClick={() => handleAction(gateway, 'test', onTest)}
                          disabled={isLoading(gateway.id, 'test')}
                        >
                          <Play className={`size-3.5 ${isLoading(gateway.id, 'test') ? 'animate-pulse' : ''}`} />
                          <span className="sr-only">Test connection</span>
                        </Button>
                      ) : null}
                      {supportsProbeControls && density === 'comfortable' ? (
                        <Button
                          variant="outline"
                          size="icon"
                          className={cn(gatewayActionTone(), GATEWAY_TABLE_ACTION, 'opacity-100 transition-opacity md:opacity-0 md:focus-visible:opacity-100 md:group-hover:opacity-100')}
                          onClick={() => handleAction(gateway, 'reload', onReload)}
                          disabled={isLoading(gateway.id, 'reload')}
                        >
                          <RefreshCw className={`size-3.5 ${isLoading(gateway.id, 'reload') ? 'animate-spin' : ''}`} />
                          <span className="sr-only">Reload server</span>
                        </Button>
                      ) : null}
                      {isStaleVirtualServer(gateway) && density === 'comfortable' ? (
                        <Button
                          variant="outline"
                          size="icon"
                          className={cn(gatewayActionTone(), GATEWAY_TABLE_ACTION, 'opacity-100 text-destructive transition-opacity hover:text-destructive md:opacity-0 md:focus-visible:opacity-100 md:group-hover:opacity-100')}
                          onClick={() => onDelete(gateway)}
                        >
                          <Trash2 className="size-3.5" />
                          <span className="sr-only">Remove stale service</span>
                        </Button>
                      ) : null}
                      <DropdownMenu>
                        <DropdownMenuTrigger asChild>
                          <Button variant="outline" size="icon" className={cn(gatewayActionTone(), GATEWAY_TABLE_ACTION)}>
                            <MoreHorizontal className="size-3.5" />
                            <span className="sr-only">More actions</span>
                          </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent align="end">
                          <DropdownMenuItem asChild>
                            <Link href={gatewayDetailHref(gateway.id)}>
                              <Eye className="mr-2 size-4" />
                              View details
                            </Link>
                          </DropdownMenuItem>
                          <DropdownMenuItem onClick={() => onEdit(gateway)}>
                            <Pencil className="mr-2 size-4" />
                            Edit server
                          </DropdownMenuItem>
                          <DropdownMenuItem onClick={() => requestToggleEnabled(gateway)}>
                            {gateway.enabled ?? true ? (
                              <>
                                <Trash2 className="mr-2 size-4" />
                                Disable server
                              </>
                            ) : (
                              <>
                                <Play className="mr-2 size-4" />
                                Enable server
                              </>
                            )}
                          </DropdownMenuItem>
                          {supportsProbeControls ? (
                            <>
                              <DropdownMenuSeparator />
                              <DropdownMenuItem onClick={() => onTest(gateway)}>
                                <Play className="mr-2 size-4" />
                                Test connection
                              </DropdownMenuItem>
                              <DropdownMenuItem onClick={() => onReload(gateway)}>
                                <RefreshCw className="mr-2 size-4" />
                                Reload server
                              </DropdownMenuItem>
                              <DropdownMenuItem onClick={() => onCleanup(gateway, false, true)}>
                                <Search className="mr-2 size-4" />
                                Preview cleanup
                              </DropdownMenuItem>
                              <DropdownMenuItem onClick={() => onCleanup(gateway, false, false)}>
                                <Wrench className="mr-2 size-4" />
                                Cleanup runtime
                              </DropdownMenuItem>
                              <DropdownMenuItem onClick={() => onCleanup(gateway, true, true)}>
                                <Search className="mr-2 size-4" />
                                Preview aggressive cleanup
                              </DropdownMenuItem>
                              <DropdownMenuItem onClick={() => onCleanup(gateway, true, false)}>
                                <TriangleAlert className="mr-2 size-4" />
                                Aggressive cleanup
                              </DropdownMenuItem>
                              {cleanupSummary ? (
                                <>
                                  <DropdownMenuSeparator />
                                  <DropdownMenuItem onClick={() => onClearCleanupHistory(gateway)}>
                                    <Trash2 className="mr-2 size-4" />
                                    Clear cleanup history
                                  </DropdownMenuItem>
                                </>
                              ) : null}
                            </>
                          ) : null}
                          {canRemoveGatewayRow ? (
                            <>
                              <DropdownMenuSeparator />
                              <DropdownMenuItem onClick={() => onDelete(gateway)} className="text-destructive focus:text-destructive">
                                <Trash2 className="mr-2 size-4" />
                                {gateway.source === 'in_process' ? 'Remove stale service' : 'Remove server'}
                              </DropdownMenuItem>
                            </>
                          ) : null}
                        </DropdownMenuContent>
                      </DropdownMenu>
                    </div>
                  </TableCell>
                </TableRow>
              )
            })}
          </TableBody>
        </Table>
      </div>
      <ActionConfirmationDialog
        open={disableConfirmationGatewayId !== null}
        title="Disable server?"
        description="Connected clients should no longer have access to this server. Existing sessions may fail until the gateway is enabled again."
        confirmLabel="Disable server"
        onOpenChange={(open) => {
          if (!open) setDisableConfirmationGatewayId(null)
        }}
        onConfirm={confirmDisableGateway}
      />
    </>
  )
}
