'use client'

import { useEffect, useMemo, useState } from 'react'
import { useRouter } from 'next/navigation'
import {
  Play,
  RefreshCw,
  Pencil,
  Trash2,
  Copy,
  Check,
  AlertTriangle,
  Clock,
  FileText,
  MessageSquare,
  Loader2,
  Search,
  Wrench,
  Settings,
  Power,
  SlidersHorizontal,
} from 'lucide-react'
import { toast } from 'sonner'
import { AppHeader } from '@/components/app-header'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Switch } from '@/components/ui/switch'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Skeleton } from '@/components/ui/skeleton'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { TransportBadge } from './transport-badge'
import { ToolExposureTable } from './tool-exposure-table'
import { PrimitiveExposureTable } from './primitive-exposure-table'
import { GatewayFormDialog } from './gateway-form-dialog'
import { DisableGatewayDialog } from './disable-gateway-dialog'
import { DeleteGatewayDialog } from './delete-gateway-dialog'
import { TestResultPanel } from './test-result-panel'
import { CleanupResultPanel } from './cleanup-result-panel'
import { useGateway, useGatewayMutations } from '@/lib/hooks/use-gateways'
import { AURORA_DISPLAY_1 } from '@/components/aurora/tokens'
import type { Gateway, CreateGatewayInput, UpdateGatewayInput } from '@/lib/types/gateway'
import {
  applyBulkExposureToDraft,
  buildExposurePolicyFromDraft,
  createExposureDraftFromTools,
  getDraftExposureSummary,
} from '@/lib/api/tool-exposure-draft'
import { cn, getErrorMessage } from '@/lib/utils'
import { buildGatewayClientConfig } from '@/lib/api/gateway-client-config'

function SettingRow({
  title,
  description,
  checked,
  onCheckedChange,
  ariaLabel,
}: {
  title: string
  description: string
  checked: boolean
  onCheckedChange: (v: boolean) => void
  ariaLabel?: string
}) {
  return (
    <div className="flex items-start justify-between gap-4 rounded-lg border bg-aurora-control-surface/10 p-4">
      <div className="min-w-0">
        <p className="text-sm font-semibold text-aurora-text-primary">{title}</p>
        <p className="mt-1 text-sm text-aurora-text-muted">{description}</p>
      </div>
      <Switch aria-label={ariaLabel ?? title} checked={checked} onCheckedChange={onCheckedChange} />
    </div>
  )
}

interface GatewayDetailContentProps {
  gatewayId: string | null
}

function formatGatewayTimestamp(value: string | null | undefined): string {
  if (!value) {
    return 'Unknown'
  }

  const parsed = new Date(value)
  if (Number.isNaN(parsed.getTime())) {
    return value
  }

  return new Intl.DateTimeFormat('en-US', {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
    hour12: true,
    timeZone: 'UTC',
  }).format(parsed)
}

export function GatewayDetailContent({ gatewayId }: GatewayDetailContentProps) {
  const router = useRouter()
  const { data: gateway, isLoading, error } = useGateway(gatewayId)
  const {
    testGateway,
    reloadGateway,
    updateGateway,
    removeGateway,
    setExposurePolicy,
    disableVirtualServer,
    disableGateway,
    enableVirtualServer,
    enableGateway,
    cleanupGateway,
    setVirtualServerSurface,
  } = useGatewayMutations()

  const [isTesting, setIsTesting] = useState(false)
  const [isReloading, setIsReloading] = useState(false)
  const [isCleaningRuntime, setIsCleaningRuntime] = useState(false)
  const [isAggressiveCleanup, setIsAggressiveCleanup] = useState(false)
  const [configCopied, setConfigCopied] = useState(false)
  const [editOpen, setEditOpen] = useState(false)
  const [deleteOpen, setDeleteOpen] = useState(false)
  const [disableOpen, setDisableOpen] = useState(false)
  const [manageToolsMode, setManageToolsMode] = useState(false)
  const [draftSelectedToolNames, setDraftSelectedToolNames] = useState<string[]>([])
  const [selectedRowToolNames, setSelectedRowToolNames] = useState<string[]>([])
  const [isSavingExposure, setIsSavingExposure] = useState(false)
  const [exposureSaveError, setExposureSaveError] = useState<string | null>(null)
  const [inventorySearch, setInventorySearch] = useState('')
  const [inventoryFilter, setInventoryFilter] = useState<'all' | 'tools' | 'resources' | 'prompts'>('all')
  const [activeTab, setActiveTab] = useState<'catalog' | 'runtime' | 'config' | 'settings' | 'warnings'>('catalog')
  const [testResult, setTestResult] = useState<{ gateway: Gateway; result: Awaited<ReturnType<typeof testGateway>> } | null>(null)
  const [cleanupResult, setCleanupResult] = useState<{ gateway: Gateway; result: Awaited<ReturnType<typeof cleanupGateway>> } | null>(null)
  const [hasMounted, setHasMounted] = useState(false)
  const toolExposureSignature = useMemo(
    () =>
      (gateway?.discovery.tools ?? [])
        .map((tool) => `${tool.name}:${tool.exposed ? '1' : '0'}`)
        .join('|'),
    [gateway?.discovery.tools],
  )
  const allToolNames = useMemo(
    () => gateway?.discovery.tools.map((tool) => tool.name) ?? [],
    [gateway?.discovery.tools],
  )
  const currentExposedToolNames = useMemo(
    () => createExposureDraftFromTools(gateway?.discovery.tools ?? []),
    [gateway?.discovery.tools],
  )
  const isLabGateway = gateway?.source === 'in_process'
  const surfaceEntries = gateway?.surfaces
    ? ([
        ['cli', gateway.surfaces.cli],
        ['api', gateway.surfaces.api],
        ['mcp', gateway.surfaces.mcp],
      ] as const)
    : []
  const exposeAllTools =
    allToolNames.length > 0 && draftSelectedToolNames.length === allToolNames.length
  const displayedTools = useMemo(
    () => {
      const draftSet = new Set(draftSelectedToolNames)
      return (gateway?.discovery.tools ?? []).map((tool) => ({
        ...tool,
        exposed: draftSet.has(tool.name),
        matched_by: draftSet.has(tool.name) ? (exposeAllTools ? '*' : tool.name) : null,
      }))
    },
    [gateway?.discovery.tools, draftSelectedToolNames, exposeAllTools],
  )
  const clientConfigJson = useMemo(
    () => (gateway ? JSON.stringify(buildGatewayClientConfig(gateway), null, 2) : ''),
    [gateway],
  )

  useEffect(() => {
    setHasMounted(true)
  }, [])

  useEffect(() => {
    setDraftSelectedToolNames(currentExposedToolNames)
    setSelectedRowToolNames([])
    setManageToolsMode(false)
  }, [currentExposedToolNames, gateway?.id, toolExposureSignature])

  if (!gatewayId) {
    return (
      <>
        <AppHeader
          breadcrumbs={[
            { label: 'Servers', href: '/gateways' },
            { label: 'Missing Server' }
          ]}
        />
        <div className="flex-1 p-6">
          <div className="rounded-lg border bg-aurora-panel-medium p-8 text-center">
            <AlertTriangle className="size-8 mx-auto text-destructive mb-3" />
            <p className="font-medium">No server selected</p>
            <p className="text-sm text-aurora-text-muted mt-1">
              Open this page from the server list or provide a server id in the URL query string.
            </p>
            <Button variant="outline" className="mt-4" onClick={() => router.push('/gateways')}>
              Back to Servers
            </Button>
          </div>
        </div>
      </>
    )
  }

  const handleTest = async () => {
    if (!gateway || !(gateway.enabled ?? true)) return
    setIsTesting(true)
    try {
      const result = await testGateway(gateway.id)
      setTestResult({ gateway, result })
      if (result.severity === 'warning') {
        toast.warning(result.detail || result.message)
      } else if (result.success) {
        toast.success('Connection test passed')
      } else {
        toast.error(result.error || result.message)
      }
    } catch (error) {
      toast.error(getErrorMessage(error, 'Failed to test server'))
    } finally {
      setIsTesting(false)
    }
  }

  const handleReload = async () => {
    if (!gateway || gateway.source === 'in_process' || !(gateway.enabled ?? true)) return
    setIsReloading(true)
    try {
      const result = await reloadGateway(gateway.id)
      if (result.success) {
        toast.success(`Server reloaded: ${result.new_tool_count} tools discovered`)
      } else {
        toast.error(result.message)
      }
    } catch (error) {
      toast.error(getErrorMessage(error, 'Failed to reload server'))
    } finally {
      setIsReloading(false)
    }
  }

  const handleCopyConfig = async () => {
    try {
      await navigator.clipboard.writeText(clientConfigJson)
      setConfigCopied(true)
      toast.success('Configuration copied to clipboard')
      setTimeout(() => setConfigCopied(false), 2000)
    } catch {
      toast.error('Failed to copy configuration to clipboard')
    }
  }

  const handleSave = async (input: CreateGatewayInput | UpdateGatewayInput) => {
    if (!gateway) return
    await updateGateway(gateway.id, input as UpdateGatewayInput)
    toast.success('Server updated successfully')
    setEditOpen(false)
  }

  const handleDelete = async () => {
    if (!gateway) return
    try {
      await removeGateway(gateway.id)
      toast.success('Server removed successfully')
      router.push('/gateways')
    } catch (error) {
      toast.error(getErrorMessage(error, 'Failed to remove server'))
    }
  }

  const handleEnabledToggle = async (enabled: boolean) => {
    if (!gateway) return
    if (!enabled) {
      setDisableOpen(true)
      return
    }

    try {
      if (gateway.source === 'in_process') {
        await enableVirtualServer(gateway.id)
      } else {
        await enableGateway(gateway.id)
      }
      toast.success('Server enabled. Catalog change sent to clients.')
    } catch (error) {
      toast.error(getErrorMessage(error, 'Failed to update server state'))
    }
  }

  const handleDisableConfirm = async () => {
    if (!gateway) return
    try {
      if (gateway.source === 'in_process') {
        await disableVirtualServer(gateway.id)
      } else {
        await disableGateway(gateway.id)
      }
      toast.success('Server disabled. Catalog change sent and runtime cleanup requested.')
      setDisableOpen(false)
    } catch (error) {
      toast.error(getErrorMessage(error, 'Failed to update server state'))
    }
  }

  const handleSurfaceToggle = async (surface: 'cli' | 'api' | 'mcp' | 'webui', enabled: boolean) => {
    if (!gateway || gateway.source !== 'in_process') return
    try {
      await setVirtualServerSurface(gateway.id, surface, enabled)
      toast.success(`Updated ${surface.toUpperCase()} surface`)
    } catch (error) {
      toast.error(getErrorMessage(error, `Failed to update ${surface} surface`))
    }
  }

  if (!hasMounted || isLoading) {
    return (
      <>
        <AppHeader
          breadcrumbs={[
            { label: 'Servers', href: '/gateways' },
            { label: 'Loading...' }
          ]}
        />
        <div className="flex-1 p-6">
          <div className="space-y-6">
            <div className="flex items-start justify-between">
              <div className="space-y-2">
                <Skeleton className="h-8 w-48" />
                <Skeleton className="h-5 w-32" />
              </div>
              <div className="flex gap-2">
                <Skeleton className="h-9 w-20" />
                <Skeleton className="h-9 w-20" />
              </div>
            </div>
            <Skeleton className="h-[400px] w-full rounded-lg" />
          </div>
        </div>
      </>
    )
  }

  if (error || !gateway) {
    return (
      <>
        <AppHeader
          breadcrumbs={[
            { label: 'Servers', href: '/gateways' },
            { label: 'Error' }
          ]}
        />
        <div className="flex-1 p-6">
          <div className="rounded-lg border bg-aurora-panel-medium p-8 text-center">
            <AlertTriangle className="size-8 mx-auto text-destructive mb-3" />
            <p className="font-medium">Failed to load server</p>
            <p className="text-sm text-aurora-text-muted mt-1">
              {error?.message || 'Server not found'}
            </p>
            <Button variant="outline" className="mt-4" onClick={() => router.push('/gateways')}>
              Back to Servers
            </Button>
          </div>
        </div>
      </>
    )
  }

  const hasDraftChanges =
    draftSelectedToolNames.length !== currentExposedToolNames.length ||
    draftSelectedToolNames.some((toolName) => !currentExposedToolNames.includes(toolName))
  const exposureSummary = getDraftExposureSummary(allToolNames, draftSelectedToolNames)
  const resourceExposureEnabled = gateway.config.proxy_resources ?? true
  const promptExposureEnabled = gateway.config.proxy_prompts ?? true
  const toolsTabLabel = isLabGateway ? 'Actions' : 'Tools'
  const runtimeAgeLabel = gateway.status.age_seconds
    ? gateway.status.age_seconds < 60
      ? `${gateway.status.age_seconds}s old`
      : gateway.status.age_seconds < 3600
        ? `${Math.floor(gateway.status.age_seconds / 60)}m old`
        : gateway.status.age_seconds < 86400
          ? `${Math.floor(gateway.status.age_seconds / 3600)}h old`
          : `${Math.floor(gateway.status.age_seconds / 86400)}d old`
    : null

  const handleCleanupRuntime = async (aggressive: boolean, dryRun: boolean) => {
    if (!gateway || gateway.source === 'in_process') return
    const previousAggressive = isAggressiveCleanup
    setIsCleaningRuntime(true)
    setIsAggressiveCleanup(aggressive)
    try {
      const result = await cleanupGateway(gateway.id, aggressive, dryRun)
      setCleanupResult({ gateway, result })
      const totalMatched =
        (result.gateway_matched ?? result.gateway_killed) +
        (result.local_matched ?? result.local_killed) +
        (result.aggressive_matched ?? result.aggressive_killed)
      const totalKilled =
        result.gateway_killed + result.local_killed + result.aggressive_killed
      if (dryRun) {
        toast.success(
          aggressive
            ? `Aggressive runtime cleanup preview completed. ${totalMatched} processes matched.`
            : `Runtime cleanup preview completed. ${totalMatched} processes matched.`,
        )
      } else {
        toast.success(
          aggressive
            ? `Aggressive runtime cleanup completed. ${totalKilled} processes terminated.`
            : `Runtime cleanup completed. ${totalKilled} processes terminated.`,
        )
      }
    } catch (error) {
      toast.error(getErrorMessage(error, 'Failed to clean up runtime'))
    } finally {
      setIsCleaningRuntime(false)
      setIsAggressiveCleanup(previousAggressive)
    }
  }

  const handleExposeAllChange = (checked: boolean) => {
    if (!manageToolsMode) {
      return
    }
    setDraftSelectedToolNames(checked ? [...allToolNames].sort((left, right) => left.localeCompare(right)) : [])
    setSelectedRowToolNames([])
    setExposureSaveError(null)
  }

  const handleBulkEnableSelected = (toolNames: string[]) => {
    setDraftSelectedToolNames((current) => applyBulkExposureToDraft(current, toolNames, true))
    setSelectedRowToolNames([])
    setExposureSaveError(null)
  }

  const handleBulkDisableSelected = (toolNames: string[]) => {
    setDraftSelectedToolNames((current) => applyBulkExposureToDraft(current, toolNames, false))
    setSelectedRowToolNames([])
    setExposureSaveError(null)
  }

  const handleCancelExposureDraft = () => {
    setDraftSelectedToolNames(currentExposedToolNames)
    setSelectedRowToolNames([])
    setManageToolsMode(false)
    setExposureSaveError(null)
  }

  const handleSaveExposureDraft = async () => {
    setIsSavingExposure(true)
    setExposureSaveError(null)
    try {
      const policy = buildExposurePolicyFromDraft(allToolNames, draftSelectedToolNames)
      await setExposurePolicy(gateway.id, policy)
      toast.success('Tool exposure updated successfully')
      setManageToolsMode(false)
      setSelectedRowToolNames([])
    } catch (error) {
      const message = getErrorMessage(error, 'Failed to update tool exposure')
      setExposureSaveError(`Could not save these exposure changes. Your draft is still local. ${message}`)
      toast.error(message)
    } finally {
      setIsSavingExposure(false)
    }
  }

  const handleProxyResourcesToggle = async (enabled: boolean) => {
    try {
      await updateGateway(gateway.id, {
        config: {
          proxy_resources: enabled,
        },
      })
      toast.success(enabled ? 'Resource exposure enabled' : 'Resource exposure disabled')
    } catch (error) {
      toast.error(getErrorMessage(error, 'Failed to update resource exposure'))
    }
  }

  const handleProxyPromptsToggle = async (enabled: boolean) => {
    try {
      await updateGateway(gateway.id, {
        config: {
          proxy_prompts: enabled,
        },
      })
      toast.success(enabled ? 'Prompt exposure enabled' : 'Prompt exposure disabled')
    } catch (error) {
      toast.error(getErrorMessage(error, 'Failed to update prompt exposure'))
    }
  }

  // AppHeader actions: action buttons
  const headerActions = (
    <div className="flex items-center gap-2">
      {!isLabGateway && (
        <Button
          variant="outline"
          size="icon"
          onClick={handleTest}
          disabled={isTesting || !(gateway.enabled ?? true)}
          aria-label="Test server"
          title="Test server"
        >
          {isTesting ? (
            <Loader2 className="size-4 animate-spin" />
          ) : (
            <Play className="size-4" />
          )}
        </Button>
      )}
      {!isLabGateway && (
        <Button
          variant="outline"
          size="icon"
          onClick={handleReload}
          disabled={isReloading || !(gateway.enabled ?? true)}
          aria-label="Reload server"
          title="Reload server"
        >
          <RefreshCw className={`size-4 ${isReloading ? 'animate-spin' : ''}`} />
        </Button>
      )}
      <Button
        variant="outline"
        size="icon"
        onClick={() => setEditOpen(true)}
        aria-label="Edit server"
        title="Edit server"
      >
        <Pencil className="size-4" />
      </Button>
      <Button
        variant="outline"
        size="icon"
        onClick={() => setDeleteOpen(true)}
        aria-label="Remove server"
        title="Remove server"
      >
        <Trash2 className="size-4" />
      </Button>
    </div>
  )

  const updatedAtLabel = formatGatewayTimestamp(gateway.updated_at)
  const endpointDisplay =
    gateway.transport === 'http'
      ? (gateway.config.url ?? '')
      : isLabGateway
        ? gateway.config.url ?? 'Lab-managed server configuration'
        : [gateway.config.command, ...(gateway.config.args ?? [])].join(' ')

  return (
    <>
      <AppHeader
        breadcrumbs={[
          { label: 'Servers', href: '/gateways' },
          { label: gateway.name }
        ]}
        actions={headerActions}
      />

      <div className="flex-1 p-6 min-w-0 overflow-x-hidden">
        {!(gateway.enabled ?? true) ? (
          <div className="mb-4 flex items-start gap-3 rounded-lg border border-aurora-warn/30 bg-aurora-warn/10 px-4 py-3">
            <AlertTriangle className="mt-0.5 size-4 shrink-0 text-aurora-warn" />
            <div className="min-w-0">
              <p className="text-sm font-semibold text-aurora-text-primary">Server disabled</p>
              <p className="mt-1 text-sm text-aurora-text-muted">
                This server is excluded from the active catalog. Clients should no longer see its tools, resources, or prompts until you re-enable it.
              </p>
            </div>
          </div>
        ) : null}
        <Tabs value={activeTab} onValueChange={(value) => setActiveTab(value as 'catalog' | 'runtime' | 'config' | 'settings' | 'warnings')} className="space-y-4">
          {/* Header card with tabs embedded */}
          <div className="relative rounded-lg border bg-aurora-panel-medium p-5">
            <Tooltip>
              <TooltipTrigger asChild>
                <button
                  type="button"
                  className="absolute right-5 top-5 inline-flex size-8 items-center justify-center rounded-full border bg-aurora-page-bg text-aurora-text-muted transition-colors hover:bg-aurora-hover-bg hover:text-aurora-text-primary"
                  title={updatedAtLabel}
                  aria-label={`Last updated ${updatedAtLabel}`}
                >
                  <Clock className="size-3.5" />
                </button>
              </TooltipTrigger>
              <TooltipContent side="left">
                {updatedAtLabel}
              </TooltipContent>
            </Tooltip>
            <div className="flex flex-wrap items-center gap-2 pr-28">
              <TransportBadge transport={gateway.transport} iconOnly={gateway.transport === 'stdio'} />
            </div>

            {/* Name + endpoint */}
            <div className="mt-3 space-y-1">
              <div className="flex flex-wrap items-start gap-3">
                <span
                  className={`size-2.5 rounded-full ${gateway.status.healthy && gateway.status.connected ? 'bg-aurora-success' : 'bg-aurora-error'}`}
                  aria-hidden="true"
                />
                <h1 className={cn(AURORA_DISPLAY_1, 'break-words text-aurora-text-primary')}>{gateway.name}</h1>
              </div>
              <div className="max-w-3xl">
                <div className="flex items-center gap-2 overflow-hidden rounded-aurora-2 border bg-aurora-page-bg px-3 py-2">
                  <div className="min-w-0 flex-1">
                    <pre className="overflow-x-hidden rounded-aurora-2 border border-aurora-border-strong/60 bg-aurora-control-surface px-3 py-2 font-mono text-[11px] leading-5 text-aurora-text-primary whitespace-pre-wrap break-all shadow-[var(--aurora-highlight-medium)]">
                      <code className="font-mono">{endpointDisplay}</code>
                    </pre>
                  </div>
                  <div className="shrink-0">
                    <Button
                      variant="outline"
                      size="icon"
                      onClick={async () => {
                        try {
                          await navigator.clipboard.writeText(endpointDisplay)
                          toast.success('Copied to clipboard')
                        } catch {
                          toast.error('Failed to copy to clipboard')
                        }
                      }}
                      aria-label="Copy command"
                      title="Copy command"
                    >
                      <Copy className="size-4" />
                    </Button>
                  </div>
                </div>
              </div>
            </div>

            {/* Tabs directly under name/endpoint */}
            <TabsList className="-mx-1 px-1 mt-4">
              <TabsTrigger value="catalog">
                Catalog
                <Badge variant="secondary" className="ml-2 text-xs">
                  {gateway.discovery.tools.length + gateway.discovery.resources.length + gateway.discovery.prompts.length}
                </Badge>
              </TabsTrigger>
              <TabsTrigger value="runtime">
                Runtime
                <Badge variant="secondary" className="ml-2 text-xs">
                  {gateway.status.likely_stale_count ?? 0}
                </Badge>
              </TabsTrigger>
              <TabsTrigger value="config">Config</TabsTrigger>
              <TabsTrigger value="settings">
                Settings
                <Badge variant="secondary" className="ml-2 text-xs">
                  {/* gateway enabled + expose resources + expose prompts + lab surfaces */}
                  {3 + surfaceEntries.length}
                </Badge>
              </TabsTrigger>
              {gateway.warnings.length > 0 && (
                <TabsTrigger value="warnings" className="text-aurora-warn">
                  Warnings
                  <Badge variant="secondary" className="ml-2 text-xs bg-aurora-warn/10">
                    {gateway.warnings.length}
                  </Badge>
                </TabsTrigger>
              )}
            </TabsList>
          </div>

          {/* Tab content */}
          <TabsContent value="catalog">
            <div className="space-y-4">
              <div className="rounded-lg border bg-aurora-panel-medium p-4">
                <div className="space-y-3">
                  <div className="relative">
                    <Search className="absolute left-3 top-1/2 size-4 -translate-y-1/2 text-aurora-text-muted" />
                    <input
                      value={inventorySearch}
                      onChange={(event) => setInventorySearch(event.target.value)}
                      placeholder="Search tools, resources, and prompts..."
                      name="catalog-search"
                      aria-label="Search tools, resources, and prompts"
                      className="flex h-10 w-full rounded-md border bg-aurora-page-bg px-3 py-1 pl-9 text-sm shadow-xs outline-none transition-colors focus-visible:border-[var(--aurora-accent-primary)] focus-visible:ring-[3px] focus-visible:ring-[var(--aurora-accent-primary)]/34"
                    />
                  </div>
                  <div className="flex flex-wrap items-center gap-1.5">
                    {([
                      ['tools', toolsTabLabel, Wrench, gateway.discovery.tools.length],
                      ['resources', 'Resources', FileText, gateway.discovery.resources.length],
                      ['prompts', 'Prompts', MessageSquare, gateway.discovery.prompts.length],
                    ] as const).map(([value, label, Icon, count]) => (
                      <button
                        key={value}
                        type="button"
                        // Toggle: clicking an already-selected chip returns to the 'all' view.
                        onClick={() => setInventoryFilter(inventoryFilter === value ? 'all' : value)}
                        className={cn(
                          'inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1.5 text-[13px] font-medium transition-colors',
                          inventoryFilter === value
                            ? 'border-aurora-accent-primary/28 bg-[linear-gradient(180deg,rgba(16,35,48,0.96),rgba(11,25,35,0.98))] text-aurora-text-primary shadow-[var(--aurora-active-glow)]'
                            : 'border-aurora-border-strong bg-aurora-page-bg text-aurora-text-muted',
                        )}
                        aria-pressed={inventoryFilter === value}
                        aria-label={label}
                        title={label}
                      >
                        <Icon className="size-3.5" />
                        <Badge variant="secondary" className="rounded-full px-2 py-0.5 text-[11px]">{count}</Badge>
                      </button>
                    ))}
                    {inventoryFilter === 'tools' ? (
                      <Button
                        type="button"
                        variant="outline"
                        size="icon"
                        onClick={() => setManageToolsMode((current) => !current)}
                        className={cn(
                          'size-8 rounded-full',
                          manageToolsMode
                            ? 'border-aurora-accent-primary/28 bg-[linear-gradient(180deg,rgba(16,35,48,0.96),rgba(11,25,35,0.98))] text-aurora-text-primary shadow-[var(--aurora-active-glow)]'
                            : 'border-aurora-border-strong bg-aurora-page-bg text-aurora-text-muted',
                        )}
                        aria-pressed={manageToolsMode}
                        aria-label="Manage tools"
                        title="Manage tools"
                      >
                        <SlidersHorizontal className="size-3.5" />
                      </Button>
                    ) : null}
                    {gateway.warnings.length > 0 ? (
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <Button
                            type="button"
                            variant="outline"
                            size="icon"
                            onClick={() => setActiveTab('warnings')}
                            className="size-8 rounded-full border-aurora-warn/30 bg-aurora-warn/10 text-aurora-warn hover:bg-aurora-warn/14 hover:text-aurora-warn"
                            aria-label={`Open warnings (${gateway.warnings.length})`}
                            title={`Open warnings (${gateway.warnings.length})`}
                          >
                            <AlertTriangle className="size-3.5" />
                          </Button>
                        </TooltipTrigger>
                        <TooltipContent side="bottom" className="max-w-xs">
                          {gateway.warnings[0].message}
                          {gateway.warnings.length > 1 && (
                            <span className="block mt-1 text-xs opacity-70">+{gateway.warnings.length - 1} more — see Warnings tab</span>
                          )}
                        </TooltipContent>
                      </Tooltip>
                    ) : null}
                  </div>
                </div>
              </div>

              {(inventoryFilter === 'all' || inventoryFilter === 'tools') ? (
                <div className="rounded-lg border bg-aurora-panel-medium p-4">
                  <div className="mb-4 flex items-center gap-2">
                    <Wrench className="size-4 text-aurora-text-muted" />
                    <h2 className="text-lg font-semibold">{toolsTabLabel}</h2>
                  </div>
                  <ToolExposureTable
                    tools={displayedTools}
                    exposureLabel={exposureSummary.label}
                    exposeAll={exposeAllTools}
                    manageMode={manageToolsMode}
                    hasDraftChanges={hasDraftChanges}
                    isSaving={isSavingExposure}
                    selectedRowToolNames={selectedRowToolNames}
                    currentExposedToolNames={currentExposedToolNames}
                    draftSelectedToolNames={draftSelectedToolNames}
                    saveErrorMessage={exposureSaveError}
                    onExposeAllChange={handleExposeAllChange}
                    onManageModeChange={setManageToolsMode}
                    onRowSelectionChange={setSelectedRowToolNames}
                    onBulkEnableSelected={handleBulkEnableSelected}
                    onBulkDisableSelected={handleBulkDisableSelected}
                    onSaveChanges={handleSaveExposureDraft}
                    onCancelChanges={handleCancelExposureDraft}
                    searchValue={inventorySearch}
                    onSearchValueChange={setInventorySearch}
                    hideSearchAndFilterControls
                    hideManageModeToggle
                  />
                </div>
              ) : null}

              {(inventoryFilter === 'all' || inventoryFilter === 'resources') ? (
                <PrimitiveExposureTable
                  title="Discovered MCP Resources"
                  description="Search and manage which upstream resources are exposed through this server."
                  searchPlaceholder="Search resources"
                  manageLabel="Manage resources"
                  emptyLabel="No resources discovered"
                  exposureEnabled={resourceExposureEnabled}
                  icon={FileText}
                  items={gateway.discovery.resources.map((resource) => ({
                    name: resource.name,
                    description: resource.description,
                    secondary: resource.uri,
                    exposed: resource.exposed ?? false,
                  }))}
                  searchValue={inventorySearch}
                  onSearchValueChange={setInventorySearch}
                  onSaveSelection={async (selectedNames) => {
                    try {
                      await updateGateway(gateway.id, {
                        config: {
                          expose_resources: selectedNames,
                        },
                      })
                      toast.success('Resource exposure updated.')
                    } catch (error) {
                      toast.error(getErrorMessage(error, 'Failed to update resource exposure'))
                      throw error
                    }
                  }}
                />
              ) : null}

              {(inventoryFilter === 'all' || inventoryFilter === 'prompts') ? (
                <PrimitiveExposureTable
                  title="Discovered MCP Prompts"
                  description="Search and manage which upstream prompts are exposed through this server."
                  searchPlaceholder="Search prompts"
                  manageLabel="Manage prompts"
                  emptyLabel="No prompts discovered"
                  exposureEnabled={promptExposureEnabled}
                  icon={MessageSquare}
                  items={gateway.discovery.prompts.map((prompt) => ({
                    name: prompt.name,
                    description: prompt.description,
                    secondary:
                      prompt.arguments && prompt.arguments.length > 0
                        ? `${prompt.arguments.length} arg${prompt.arguments.length === 1 ? '' : 's'}`
                        : undefined,
                    exposed: prompt.exposed ?? false,
                  }))}
                  searchValue={inventorySearch}
                  onSearchValueChange={setInventorySearch}
                  onSaveSelection={async (selectedNames) => {
                    try {
                      await updateGateway(gateway.id, {
                        config: {
                          expose_prompts: selectedNames,
                        },
                      })
                      toast.success('Prompt exposure updated.')
                    } catch (error) {
                      toast.error(getErrorMessage(error, 'Failed to update prompt exposure'))
                      throw error
                    }
                  }}
                />
              ) : null}
            </div>
          </TabsContent>

          <TabsContent value="config">
            <div className="rounded-lg border bg-aurora-panel-medium p-5">
              <div className="mb-4">
                <h2 className="text-lg font-semibold">Client Configuration</h2>
                <p className="text-sm text-aurora-text-muted mt-1">
                  Add this JSON block to your MCP client configuration to connect to this server.
                </p>
              </div>
              <div className="overflow-hidden rounded-aurora-2 border bg-aurora-page-bg">
                <div className="flex items-center justify-between border-b px-4 py-3">
                  <p className="text-xs font-medium uppercase tracking-[0.16em] text-aurora-text-muted">Client JSON</p>
                  <Button
                    variant="ghost"
                    size="icon"
                    className="h-7 w-7 text-aurora-text-muted hover:text-aurora-text-primary"
                    onClick={handleCopyConfig}
                    title="Copy to clipboard"
                  >
                    {configCopied ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}
                  </Button>
                </div>
                <pre className="aurora-scrollbar overflow-x-auto whitespace-pre-wrap break-all px-4 py-4 text-sm leading-6 text-aurora-text-primary">
                  <code>{clientConfigJson}</code>
                </pre>
              </div>
            </div>
          </TabsContent>

          <TabsContent value="settings">
            <div className="rounded-lg border bg-aurora-panel-medium p-5 space-y-5">
              <div>
                <h2 className="text-lg font-semibold">Server settings</h2>
                <p className="mt-1 text-sm text-aurora-text-muted">
                  Control server availability, exposure of MCP resources and prompts, and individual lab surface toggles.
                </p>
              </div>

              <div className="grid gap-5 xl:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]">
                <div className="rounded-lg border bg-aurora-page-bg p-4">
                  <div className="flex items-center gap-2">
                    <Power className="size-4 text-aurora-text-muted" />
                    <h3 className="text-sm font-semibold text-aurora-text-primary">Server state</h3>
                  </div>
                  <div className="mt-4">
                    <SettingRow
                      title="Server enabled"
                      description="Controls whether this server participates in the active catalog and serves tools, resources, and prompts."
                      checked={gateway.enabled ?? true}
                      onCheckedChange={handleEnabledToggle}
                    />
                  </div>
                </div>

                <div className="rounded-lg border bg-aurora-page-bg p-4">
                  <div className="flex items-center gap-2">
                    <Settings className="size-4 text-aurora-text-muted" />
                    <h3 className="text-sm font-semibold text-aurora-text-primary">Exposure surfaces</h3>
                  </div>
                  <div className="mt-4 space-y-3">
                    <SettingRow
                      title="Expose resources"
                      description="Allow discovered MCP resources to be exposed through this server."
                      checked={resourceExposureEnabled}
                      onCheckedChange={handleProxyResourcesToggle}
                    />
                    <SettingRow
                      title="Expose prompts"
                      description="Allow discovered MCP prompts to be exposed through this server."
                      checked={promptExposureEnabled}
                      onCheckedChange={handleProxyPromptsToggle}
                    />
                  </div>
                </div>
              </div>

              {surfaceEntries.length > 0 ? (
                <div className="rounded-lg border bg-aurora-page-bg p-4">
                  <div className="flex items-center gap-2">
                    <Wrench className="size-4 text-aurora-text-muted" />
                    <h3 className="text-sm font-semibold text-aurora-text-primary">Lab surfaces</h3>
                  </div>
                  <div className="mt-4 grid gap-3 md:grid-cols-2 xl:grid-cols-3">
                    {surfaceEntries.map(([surface, state]) => (
                      <div key={surface} className="flex items-start justify-between gap-4 rounded-lg border bg-aurora-control-surface/10 p-4">
                        <div className="min-w-0">
                          <div className="flex items-center gap-2">
                            <span
                              className={`size-2 rounded-full ${state.connected ? 'bg-aurora-success' : 'bg-aurora-error'}`}
                              aria-hidden="true"
                            />
                            <p className="text-sm font-semibold uppercase text-aurora-text-primary">{surface}</p>
                          </div>
                          <p className="mt-1 text-sm text-aurora-text-muted">
                            {state.connected ? 'Connected and reachable.' : 'Configured but not currently connected.'}
                          </p>
                        </div>
                        <Switch
                          aria-label={`${surface.toUpperCase()} surface`}
                          checked={state.enabled}
                          onCheckedChange={(enabled) => handleSurfaceToggle(surface, enabled)}
                        />
                      </div>
                    ))}
                  </div>
                </div>
              ) : null}
            </div>
          </TabsContent>

          <TabsContent value="runtime">
            <div className="rounded-lg border bg-aurora-panel-medium p-5 space-y-5">
              <div>
                <h2 className="text-lg font-semibold">Runtime details</h2>
                <p className="text-sm text-aurora-text-muted mt-1">
                  Live process metadata comes from the active server pool. If the server restarted, orphaned upstream
                  processes are reconciled from the persisted runtime snapshot and shown here as stale runtime state.
                </p>
              </div>

              <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-4">
                <div className="rounded-lg border bg-aurora-page-bg p-4">
                  <p className="text-xs font-medium uppercase tracking-[0.14em] text-aurora-text-muted">Connection</p>
                  <p className="mt-2 text-sm font-semibold text-aurora-text-primary">
                    {gateway.status.connected ? 'Connected' : 'Not connected'}
                  </p>
                  <p className="mt-1 text-xs text-aurora-text-muted">
                    {gateway.enabled ?? false ? 'Server enabled' : 'Server disabled'}
                  </p>
                </div>
                <div className="rounded-lg border bg-aurora-page-bg p-4">
                  <p className="text-xs font-medium uppercase tracking-[0.14em] text-aurora-text-muted">Process</p>
                  <p className="mt-2 text-sm font-semibold text-aurora-text-primary font-mono">
                    {gateway.status.pid ? `pid ${gateway.status.pid}` : 'No active pid'}
                  </p>
                  <p className="mt-1 text-xs text-aurora-text-muted font-mono">
                    {gateway.status.pgid ? `pgid ${gateway.status.pgid}` : 'No process group recorded'}
                  </p>
                </div>
                <div className="rounded-lg border bg-aurora-page-bg p-4">
                  <p className="text-xs font-medium uppercase tracking-[0.14em] text-aurora-text-muted">Process age</p>
                  <p className="mt-2 text-sm font-semibold text-aurora-text-primary">
                    {runtimeAgeLabel ?? 'Unknown'}
                  </p>
                  <p className="mt-1 text-xs text-aurora-text-muted">
                    Derived from upstream process start time when available
                  </p>
                </div>
                <div className="rounded-lg border bg-aurora-page-bg p-4">
                  <p className="text-xs font-medium uppercase tracking-[0.14em] text-aurora-text-muted">Stale processes</p>
                  <p className="mt-2 text-sm font-semibold text-aurora-text-primary">
                    {gateway.status.likely_stale_count ?? 0}
                  </p>
                  <p className="mt-1 text-xs text-aurora-text-muted">
                    Persisted orphaned runtimes that still look alive after reconciliation
                  </p>
                </div>
              </div>

              {!isLabGateway ? (
                <div className="flex flex-wrap items-center gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => handleCleanupRuntime(false, false)}
                    disabled={isCleaningRuntime}
                  >
                    {isCleaningRuntime && !isAggressiveCleanup ? (
                      <Loader2 className="size-4 mr-2 animate-spin" />
                    ) : (
                      <RefreshCw className="size-4 mr-2" />
                    )}
                    Cleanup runtime
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => handleCleanupRuntime(false, true)}
                    disabled={isCleaningRuntime}
                  >
                    {isCleaningRuntime && !isAggressiveCleanup ? (
                      <Loader2 className="size-4 mr-2 animate-spin" />
                    ) : (
                      <Search className="size-4 mr-2" />
                    )}
                    Preview cleanup
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => handleCleanupRuntime(true, false)}
                    disabled={isCleaningRuntime}
                  >
                    {isCleaningRuntime && isAggressiveCleanup ? (
                      <Loader2 className="size-4 mr-2 animate-spin" />
                    ) : (
                      <AlertTriangle className="size-4 mr-2" />
                    )}
                    Aggressive cleanup
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => handleCleanupRuntime(true, true)}
                    disabled={isCleaningRuntime}
                  >
                    {isCleaningRuntime && isAggressiveCleanup ? (
                      <Loader2 className="size-4 mr-2 animate-spin" />
                    ) : (
                      <Search className="size-4 mr-2" />
                    )}
                    Preview aggressive cleanup
                  </Button>
                </div>
              ) : null}

              <div className="grid gap-5 xl:grid-cols-[minmax(0,1.4fr)_minmax(0,1fr)]">
                <div className="rounded-lg border bg-aurora-page-bg p-4">
                  <div className="flex items-center gap-2">
                    <Wrench className="size-4 text-aurora-text-muted" />
                    <h3 className="text-sm font-semibold text-aurora-text-primary">Catalog exposure</h3>
                  </div>
                  <div className="mt-4 grid gap-3 sm:grid-cols-3">
                    <div className="rounded-md border bg-aurora-control-surface/10 p-3">
                      <p className="text-xs uppercase tracking-[0.14em] text-aurora-text-muted">Tools</p>
                      <p className="mt-2 text-sm font-semibold text-aurora-text-primary">
                        {gateway.status.exposed_tool_count}/{gateway.status.discovered_tool_count}
                      </p>
                    </div>
                    <div className="rounded-md border bg-aurora-control-surface/10 p-3">
                      <p className="text-xs uppercase tracking-[0.14em] text-aurora-text-muted">Resources</p>
                      <p className="mt-2 text-sm font-semibold text-aurora-text-primary">
                        {gateway.status.exposed_resource_count}/{gateway.status.discovered_resource_count}
                      </p>
                    </div>
                    <div className="rounded-md border bg-aurora-control-surface/10 p-3">
                      <p className="text-xs uppercase tracking-[0.14em] text-aurora-text-muted">Prompts</p>
                      <p className="mt-2 text-sm font-semibold text-aurora-text-primary">
                        {gateway.status.exposed_prompt_count}/{gateway.status.discovered_prompt_count}
                      </p>
                    </div>
                  </div>
                </div>

                <div className="rounded-lg border bg-aurora-page-bg p-4">
                  <div className="flex items-center gap-2">
                    <Clock className="size-4 text-aurora-text-muted" />
                    <h3 className="text-sm font-semibold text-aurora-text-primary">Reconciliation notes</h3>
                  </div>
                  <div className="mt-4 space-y-3 text-sm text-aurora-text-muted">
                    <div className="rounded-md border bg-aurora-control-surface/10 p-3">
                      <p className="text-xs uppercase tracking-[0.14em] text-aurora-text-muted">Origin</p>
                      <p className="mt-2 text-sm font-semibold text-aurora-text-primary">
                        {gateway.status.origin ?? 'server-managed'}
                      </p>
                    </div>
                    <div className="rounded-md border bg-aurora-control-surface/10 p-3">
                      <p className="text-xs uppercase tracking-[0.14em] text-aurora-text-muted">Runtime state file</p>
                      <p className="mt-2 break-all text-xs font-mono text-aurora-text-primary">
                        {gateway.status.runtime_state_path ?? 'Unavailable'}
                      </p>
                    </div>
                    <div className="rounded-md border bg-aurora-control-surface/10 p-3">
                      <p className="text-xs uppercase tracking-[0.14em] text-aurora-text-muted">Last reconciled</p>
                      <p className="mt-2 text-sm font-semibold text-aurora-text-primary">
                        {formatGatewayTimestamp(gateway.status.reconciled_at)}
                      </p>
                    </div>
                  </div>
                  <ul className="mt-4 space-y-3 text-sm text-aurora-text-muted">
                    <li>Active runtime metadata is recorded when the server spawns stdio upstreams.</li>
                    <li>Runtime snapshots are written to disk beside the server config and survive server restarts.</li>
                    <li>Dead PIDs are pruned during runtime reconciliation; surviving non-current PIDs count as stale runtime state.</li>
                  </ul>
                </div>
              </div>
            </div>
          </TabsContent>

          {gateway.warnings.length > 0 && (
            <TabsContent value="warnings">
              <div className="rounded-lg border bg-aurora-panel-medium p-6">
                <h2 className="text-lg font-semibold mb-4">Server Warnings</h2>
                <div className="space-y-2">
                  {gateway.warnings.map((warning, index) => (
                    <div
                      key={index}
                      className="flex items-start gap-3 rounded-lg border border-aurora-warn/20 bg-aurora-warn/5 p-4"
                    >
                      <AlertTriangle className="size-4 text-aurora-warn mt-0.5 shrink-0" />
                      <div className="flex-1">
                        <p className="text-sm font-medium text-aurora-warn">
                          {warning.code}
                        </p>
                        <p className="text-sm text-aurora-text-muted mt-0.5">{warning.message}</p>
                        <p className="text-xs text-aurora-text-muted mt-2">
                          {formatGatewayTimestamp(warning.timestamp)}
                        </p>
                      </div>
                    </div>
                  ))}
                </div>
              </div>
            </TabsContent>
          )}
        </Tabs>
      </div>

      {/* Dialogs */}
      <GatewayFormDialog
        open={editOpen}
        onOpenChange={setEditOpen}
        gateway={gateway}
        onSave={handleSave}
      />

      <DeleteGatewayDialog
        gateway={deleteOpen ? gateway : null}
        onOpenChange={(open) => setDeleteOpen(open)}
        onConfirm={handleDelete}
      />

      <DisableGatewayDialog
        gateway={disableOpen ? gateway : null}
        onOpenChange={(open: boolean) => setDisableOpen(open)}
        onConfirm={handleDisableConfirm}
      />

      <TestResultPanel
        result={testResult}
        onClose={() => setTestResult(null)}
      />
      <CleanupResultPanel
        result={cleanupResult}
        onClose={() => setCleanupResult(null)}
      />
    </>
  )
}
