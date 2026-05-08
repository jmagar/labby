'use client'

import Image from 'next/image'
import { useDeferredValue, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import {
  Bot,
  Boxes,
  Code2,
  FileCode2,
  Grid2X2,
  Hammer,
  LayoutList,
  MonitorSmartphone,
  Plus,
  RefreshCw,
  Search,
  Server,
  ShoppingBag,
  SlidersHorizontal,
  Sparkles,
  TerminalSquare,
  X,
} from 'lucide-react'
import { toast } from 'sonner'
import { AppHeader } from '@/components/app-header'
import { useMediaQuery } from '@/lib/hooks/use-media-query'
import {
  AURORA_BADGE_LABEL,
  AURORA_CARD_TITLE,
  AURORA_COMPACT_TITLE,
  AURORA_DENSE_META,
  AURORA_DISPLAY_1,
  AURORA_MEDIUM_PANEL,
  AURORA_MUTED_LABEL,
  AURORA_PAGE_FRAME,
  AURORA_PAGE_SHELL,
  AURORA_STAT_PANEL,
  AURORA_STRONG_PANEL,
} from '@/components/aurora/tokens'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { useAcpAgents, useMarketplaceMutations, useMarketplaces, useMcpServers, usePlugins } from '@/lib/hooks/use-marketplace'
import { cn, getErrorMessage } from '@/lib/utils'
import {
  buildMarketplaceCatalogItems,
  catalogItemMcpServer,
  filterMarketplaceCatalogItems,
  isAcpAgentCatalogItem,
  isMcpServerCatalogItem,
  isPluginCatalogItem,
  isPluginComponentCatalogItem,
  marketplaceCatalogSummary,
  sortMarketplaceCatalogItems,
  type MarketplaceCatalogFilterState,
  type MarketplaceCatalogItem,
  type MarketplaceCatalogKind,
  type MarketplaceInstallFacet,
  type MarketplaceSort,
} from './marketplace-state'
import { gatewayActionTone } from '@/components/gateway/gateway-theme'
import { AddMarketplaceModal } from './add-marketplace-modal'
import { AcpAgentInstallModal } from './acp-agent-install-modal'
import { CherryPickDialog } from './cherry-pick-dialog'
import { McpInstallModal } from './mcp-install-modal'
import {
  MARKETPLACE_VIEW_MODE_STORAGE_KEY,
  isMarketplaceViewPreference,
  resolveMarketplaceViewMode,
  type MarketplaceViewMode,
  type MarketplaceViewPreference,
} from './marketplace-view-preference'

const DEFAULT_FILTERS: MarketplaceCatalogFilterState = {
  lens: 'all',
  search: '',
  types: [],
  installStates: [],
  ecosystems: [],
  sourceIds: [],
  distributions: [],
  sort: 'updated',
}

const TYPE_OPTIONS: Array<{ value: MarketplaceCatalogKind; label: string }> = [
  { value: 'plugin', label: 'Plugins' },
  { value: 'agent', label: 'Agents' },
  { value: 'skill', label: 'Skills' },
  { value: 'command', label: 'Commands' },
  { value: 'mcp_server', label: 'MCP servers' },
  { value: 'lsp_server', label: 'LSP servers' },
  { value: 'acp_agent', label: 'ACP agents' },
  { value: 'app', label: 'Apps' },
  { value: 'hook', label: 'Hooks' },
  { value: 'channel', label: 'Channels' },
  { value: 'executable', label: 'Executables' },
  { value: 'theme', label: 'Themes' },
  { value: 'asset', label: 'Assets' },
  { value: 'file', label: 'Files' },
  { value: 'config', label: 'Config' },
  { value: 'settings', label: 'Settings' },
  { value: 'monitor', label: 'Monitors' },
  { value: 'output_style', label: 'Output styles' },
  { value: 'source', label: 'Sources' },
]

const INSTALL_OPTIONS: Array<{ value: MarketplaceInstallFacet; label: string }> = [
  { value: 'installed', label: 'Installed' },
  { value: 'not_installed', label: 'Not installed' },
  { value: 'update_available', label: 'Update available' },
  { value: 'builtin', label: 'Built-in' },
]

const SORT_OPTIONS: Array<{ value: MarketplaceSort; label: string }> = [
  { value: 'name', label: 'A-Z' },
  { value: 'source', label: 'Source/package' },
  { value: 'installed', label: 'Installed first' },
  { value: 'updated', label: 'Recently updated' },
]

const VIEW_OPTIONS: Array<{ value: MarketplaceViewPreference; label: string; icon: ReactNode }> = [
  { value: 'auto', label: 'Auto view', icon: <MonitorSmartphone className="size-4" /> },
  { value: 'cards', label: 'Card view', icon: <Grid2X2 className="size-4" /> },
  { value: 'table', label: 'Table view', icon: <LayoutList className="size-4" /> },
]

const CATALOG_RENDER_BATCH = 200
function toggleValue<T extends string>(values: T[], value: T): T[] {
  return values.includes(value) ? values.filter((item) => item !== value) : [...values, value]
}

function marketplacePillTone(active: boolean): string {
  return active
    ? 'border-aurora-accent-primary/45 bg-aurora-accent-primary/14 text-aurora-text-primary shadow-[var(--aurora-active-glow)]'
    : 'border-aurora-border-strong bg-aurora-control-surface text-aurora-text-primary hover:border-aurora-accent-primary/30 hover:bg-aurora-hover-bg'
}

const KIND_META: Record<MarketplaceCatalogKind, { label: string; icon: ReactNode }> = {
  plugin:       { label: 'Plugin',       icon: <Boxes className="size-4" /> },
  mcp_server:   { label: 'MCP server',   icon: <Server className="size-4" /> },
  lsp_server:   { label: 'LSP server',   icon: <Code2 className="size-4" /> },
  acp_agent:    { label: 'ACP agent',    icon: <Bot className="size-4" /> },
  agent:        { label: 'Agent',        icon: <Bot className="size-4" /> },
  skill:        { label: 'Skill',        icon: <Sparkles className="size-4" /> },
  command:      { label: 'Command',      icon: <TerminalSquare className="size-4" /> },
  app:          { label: 'App',          icon: <Code2 className="size-4" /> },
  hook:         { label: 'Hook',         icon: <Hammer className="size-4" /> },
  channel:      { label: 'Channel',      icon: <TerminalSquare className="size-4" /> },
  executable:   { label: 'Executable',   icon: <TerminalSquare className="size-4" /> },
  theme:        { label: 'Theme',        icon: <FileCode2 className="size-4" /> },
  asset:        { label: 'Asset',        icon: <FileCode2 className="size-4" /> },
  file:         { label: 'File',         icon: <FileCode2 className="size-4" /> },
  config:       { label: 'Config',       icon: <FileCode2 className="size-4" /> },
  settings:     { label: 'Settings',     icon: <FileCode2 className="size-4" /> },
  monitor:      { label: 'Monitor',      icon: <FileCode2 className="size-4" /> },
  output_style: { label: 'Output style', icon: <FileCode2 className="size-4" /> },
  source:       { label: 'Source',       icon: <ShoppingBag className="size-4" /> },
}

function kindLabel(kind: MarketplaceCatalogKind): string {
  return KIND_META[kind].label
}

function kindIcon(kind: MarketplaceCatalogKind): ReactNode {
  return KIND_META[kind].icon
}

function itemInitials(name: string): string {
  return name
    .replace(/[-_/]+/g, ' ')
    .split(/\s+/)
    .filter(Boolean)
    .map((word) => word[0])
    .join('')
    .toUpperCase()
    .slice(0, 2)
}

function CatalogIdentityMark({
  item,
  size = 40,
}: {
  item: MarketplaceCatalogItem
  size?: number
}) {
  const [imageFailed, setImageFailed] = useState(false)
  const owner = item.avatar?.kind === 'github' ? item.avatar.owner : undefined

  if (owner && !imageFailed) {
    return (
      <div
        className="flex shrink-0 overflow-hidden rounded-aurora-2 border border-aurora-border-default bg-aurora-panel-medium shadow-[var(--aurora-shadow-small)]"
        style={{ width: size, height: size }}
      >
        <Image
          key={`${owner}-${item.name}`}
          src={`https://github.com/${owner}.png?size=${size * 2}`}
          alt={`${owner} GitHub avatar`}
          width={size}
          height={size}
          unoptimized
          className="h-full w-full object-cover"
          onError={() => setImageFailed(true)}
        />
      </div>
    )
  }

  return (
    <div
      className="flex shrink-0 items-center justify-center rounded-aurora-2 border border-aurora-border-default bg-aurora-control-surface font-display text-sm font-black text-aurora-accent-strong shadow-[var(--aurora-shadow-small)]"
      style={{ width: size, height: size }}
      aria-hidden="true"
    >
      {item.kind === 'plugin' || item.kind === 'source' ? itemInitials(item.name) : kindIcon(item.kind)}
    </div>
  )
}

function primaryActionLabel(item: MarketplaceCatalogItem, readOnlyPreview = false): string {
  if (item.kind === 'source') return 'Filter source'
  if (item.installed) return item.hasUpdate ? 'Update' : 'Remove'
  if (item.kind === 'acp_agent') return readOnlyPreview ? 'Preview wiring' : 'Wire agent'
  if (item.kind === 'mcp_server') return readOnlyPreview ? 'Preview install' : 'Install'
  if (item.kind !== 'plugin') return readOnlyPreview ? 'Preview component' : 'Install component'
  if (readOnlyPreview) return 'Preview install'
  return 'Install'
}

function canRunProductionMutation(item: MarketplaceCatalogItem | null): item is MarketplaceCatalogItem {
  return isPluginCatalogItem(item)
}

function activeFilterLabels(filters: MarketplaceCatalogFilterState, sourceLabels: Map<string, string>): string[] {
  return [
    ...filters.types.map((value) => TYPE_OPTIONS.find((option) => option.value === value)?.label ?? value),
    ...filters.installStates.map((value) => INSTALL_OPTIONS.find((option) => option.value === value)?.label ?? value),
    ...filters.ecosystems,
    ...filters.sourceIds.map((value) => sourceLabels.get(value) ?? value),
    ...filters.distributions,
    ...(filters.sort === DEFAULT_FILTERS.sort ? [] : [SORT_OPTIONS.find((option) => option.value === filters.sort)?.label ?? filters.sort]),
  ]
}

function FilterCheckbox({ checked, label, onChange }: { checked: boolean; label: string; onChange: () => void }) {
  return (
    <label className="flex items-center gap-2 text-[13px] font-medium leading-[1.2] text-aurora-text-primary">
      <Checkbox checked={checked} onCheckedChange={onChange} aria-label={label} className="border-aurora-border-strong bg-aurora-control-surface" />
      <span>{label}</span>
    </label>
  )
}

function FilterGroup({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="space-y-2.5">
      <p className={AURORA_MUTED_LABEL}>{label}</p>
      <div className="space-y-2">{children}</div>
    </div>
  )
}

function CatalogCard({
  item,
  onAction,
}: {
  item: MarketplaceCatalogItem
  onAction: (item: MarketplaceCatalogItem) => void
}) {
  return (
    <button
      type="button"
      onClick={() => onAction(item)}
      className={cn(
        AURORA_STRONG_PANEL,
        'flex min-h-[214px] w-full min-w-0 overflow-hidden flex-col gap-3 p-4 text-left transition-[background-color,border-color,box-shadow,transform] hover:-translate-y-px hover:bg-aurora-hover-bg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary/34',
      )}
      aria-label={`${primaryActionLabel(item)} ${item.name}`}
    >
      <div className="grid min-w-0 grid-cols-[auto_minmax(0,1fr)] items-start gap-3 sm:grid-cols-[auto_minmax(0,1fr)_auto]">
        <CatalogIdentityMark item={item} />
        <div className="min-w-0">
          <h3 className={cn(AURORA_CARD_TITLE, 'truncate text-aurora-text-primary')}>{item.name}</h3>
          <p className={cn(AURORA_DENSE_META, 'mt-1 truncate text-aurora-text-muted')}>{item.subtitle}</p>
        </div>
        <span className={cn(AURORA_BADGE_LABEL, 'col-span-2 w-fit rounded-full border px-2 py-1 sm:col-span-1', marketplacePillTone(item.installed || item.hasUpdate))}>
          {item.hasUpdate ? 'Update' : item.installed ? 'Installed' : kindLabel(item.kind)}
        </span>
      </div>
      <p className="line-clamp-3 min-h-[60px] min-w-0 text-[13px] leading-[1.55] text-aurora-text-muted">{item.description || 'No description provided.'}</p>
      <div className="flex min-w-0 flex-wrap gap-1">
        {[item.ecosystem, item.distribution, item.sourceName, ...item.tags].filter(Boolean).slice(0, 4).map((tag) => (
          <span key={tag} className={cn(AURORA_BADGE_LABEL, 'rounded-full border border-aurora-border-default bg-aurora-control-surface px-2 py-1 text-aurora-text-muted')}>
            {tag}
          </span>
        ))}
      </div>
      <div className="mt-auto flex min-w-0 items-center justify-between gap-2 border-t border-aurora-border-default pt-2">
        <span className={cn(AURORA_DENSE_META, 'rounded-full border border-aurora-border-default bg-aurora-control-surface px-2.5 py-1 font-semibold text-aurora-text-muted')}>
          {item.version ? `v${item.version}` : item.kind === 'source' ? 'source' : 'latest'}
        </span>
        <span className={cn(gatewayActionTone(), 'inline-flex h-8 shrink-0 items-center rounded-aurora-1 border px-3 text-[13px] font-semibold text-aurora-text-primary')}>
          {primaryActionLabel(item)}
        </span>
      </div>
    </button>
  )
}

function CatalogTable({ items, onAction }: { items: MarketplaceCatalogItem[]; onAction: (item: MarketplaceCatalogItem) => void }) {
  return (
    <div className={cn(AURORA_STRONG_PANEL, 'overflow-hidden')}>
      <div className="aurora-scrollbar overflow-x-auto">
        <Table className="min-w-[520px] sm:min-w-[760px]">
          <TableHeader>
            <TableRow>
              <TableHead>Item</TableHead>
              <TableHead className="hidden sm:table-cell">Type</TableHead>
              <TableHead className="hidden md:table-cell">Source/package</TableHead>
              <TableHead className="hidden sm:table-cell">Version</TableHead>
              <TableHead>State</TableHead>
              <TableHead className="text-right">Action</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {items.map((item) => (
              <TableRow
                key={item.id}
                tabIndex={0}
                role="button"
                aria-label={`${primaryActionLabel(item)} ${item.name}`}
                onClick={() => onAction(item)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter' || event.key === ' ') {
                    event.preventDefault()
                    onAction(item)
                  }
                }}
                className="cursor-pointer focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary/34"
              >
                <TableCell>
                  <div className="flex max-w-[280px] items-center gap-2 sm:max-w-[320px] sm:gap-3">
                    <CatalogIdentityMark item={item} size={32} />
                    <div className="min-w-0">
                      <p className="truncate font-semibold text-aurora-text-primary">{item.name}</p>
                      <p className={cn(AURORA_DENSE_META, 'truncate text-aurora-text-muted')}>{item.description || item.subtitle}</p>
                    </div>
                  </div>
                </TableCell>
                <TableCell className="hidden sm:table-cell">{kindLabel(item.kind)}</TableCell>
                <TableCell className="hidden md:table-cell">{item.sourceName ?? item.subtitle}</TableCell>
                <TableCell className="hidden sm:table-cell">{item.version ? `v${item.version}` : '-'}</TableCell>
                <TableCell>{item.hasUpdate ? 'Update available' : item.installed ? 'Installed' : item.builtin ? 'Built-in' : 'Available'}</TableCell>
                <TableCell className="text-right">
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    onClick={(event) => {
                      event.stopPropagation()
                      onAction(item)
                    }}
                    className={cn(gatewayActionTone(), 'h-8 px-3 text-aurora-text-primary hover:bg-aurora-hover-bg')}
                  >
                    {primaryActionLabel(item)}
                  </Button>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>
    </div>
  )
}

function LensCard({
  label,
  value,
  active,
  onClick,
}: {
  label: string
  value: number
  active: boolean
  onClick: () => void
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={active}
      className={cn(
        AURORA_STAT_PANEL,
        'cursor-pointer text-left transition-[background-color,border-color,box-shadow,transform] duration-150 ease-out focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary/34',
        !active && 'bg-aurora-panel/72 hover:border-aurora-accent-primary/28 hover:bg-aurora-hover-bg hover:shadow-[0_0_0_1px_rgba(87,190,255,0.08)]',
        active && 'border-aurora-accent-primary/40 bg-aurora-accent-primary/8 shadow-[inset_0_0_0_1px_rgba(87,190,255,0.12)]',
      )}
    >
      <p className={cn(AURORA_MUTED_LABEL, 'truncate')}>{label}</p>
      <p className="mt-2 font-display text-[1.5rem] font-extrabold tracking-[-0.03em] tabular-nums text-aurora-text-primary leading-none">
        {value}
      </p>
    </button>
  )
}

function LensChip({
  label,
  value,
  active,
  onClick,
}: {
  label: string
  value: number
  active: boolean
  onClick: () => void
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={active}
      className={cn(
        'flex h-9 flex-col items-center justify-center rounded-aurora-1 border px-2 text-[12px] font-semibold transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary/34',
        active
          ? 'border-aurora-accent-primary/40 bg-aurora-accent-primary/8 text-aurora-accent-strong'
          : 'border-aurora-border-strong bg-aurora-panel-medium text-aurora-text-muted hover:bg-aurora-hover-bg',
      )}
    >
      <span className="tabular-nums text-aurora-text-primary font-bold">{value}</span>
      <span className="text-[10px] leading-none mt-0.5 truncate w-full text-center">{label}</span>
    </button>
  )
}

export function MarketplaceListContent({ readOnlyPreview = false }: { readOnlyPreview?: boolean }) {
  const { data: sources = [], error: sourcesError, mutate: refreshSources } = useMarketplaces()
  const { data: plugins = [], error: pluginsError, mutate: refreshPlugins } = usePlugins()
  const { data: mcpServers = [], error: mcpError, mutate: refreshMcpServers } = useMcpServers()
  const { data: acpAgents = [], error: acpError, mutate: refreshAcpAgents } = useAcpAgents()
  const { install, uninstall, addSource } = useMarketplaceMutations()

  const [filters, setFilters] = useState<MarketplaceCatalogFilterState>(DEFAULT_FILTERS)
  const [viewPreference, setViewPreference] = useState<MarketplaceViewPreference>('auto')
  const prefersDesktopLayout = useMediaQuery('(min-width: 1024px)')
  const effectiveViewMode: MarketplaceViewMode = resolveMarketplaceViewMode(viewPreference, prefersDesktopLayout)
  const [mobileSheetOpen, setMobileSheetOpen] = useState(false)
  const [isRefreshing, setIsRefreshing] = useState(false)
  const isMutatingRef = useRef(false)
  const [isMutating, setIsMutating] = useState(false)
  const [addSourceOpen, setAddSourceOpen] = useState(false)
  const [previewItem, setPreviewItem] = useState<MarketplaceCatalogItem | null>(null)
  const [mcpInstallItem, setMcpInstallItem] = useState<MarketplaceCatalogItem | null>(null)
  const [acpInstallItem, setAcpInstallItem] = useState<MarketplaceCatalogItem | null>(null)
  const [componentInstallItem, setComponentInstallItem] = useState<MarketplaceCatalogItem | null>(null)
  const deferredFilters = useDeferredValue(filters)
  const [renderLimit, setRenderLimit] = useState(CATALOG_RENDER_BATCH)

  useEffect(() => {
    const storedViewPreference = window.localStorage.getItem(MARKETPLACE_VIEW_MODE_STORAGE_KEY)
    if (isMarketplaceViewPreference(storedViewPreference)) {
      setViewPreference(storedViewPreference)
    }
  }, [])

  const updateViewPreference = (nextViewPreference: MarketplaceViewPreference) => {
    setViewPreference(nextViewPreference)
    window.localStorage.setItem(MARKETPLACE_VIEW_MODE_STORAGE_KEY, nextViewPreference)
  }

  const items = useMemo(
    () => buildMarketplaceCatalogItems({ plugins, sources, mcpServers, acpAgents }),
    [acpAgents, mcpServers, plugins, sources],
  )
  const { sourceLabels, sourceOptions, ecosystemOptions, distributionOptions } = useMemo(() => {
    const labels = new Map<string, string>()
    const ecosystems = new Set<string>()
    const distributions = new Set<string>()
    for (const item of items) {
      if (item.sourceId) labels.set(item.sourceId, item.sourceName ?? item.sourceId)
      ecosystems.add(item.ecosystem)
      if (item.distribution) distributions.add(item.distribution)
    }
    const options = [...labels.entries()]
      .map(([id, label]) => ({ id, label }))
      .sort((left, right) => left.label.localeCompare(right.label, undefined, { sensitivity: 'base' }))
    return {
      sourceLabels: labels,
      sourceOptions: options,
      ecosystemOptions: [...ecosystems].sort(),
      distributionOptions: [...distributions].sort(),
    }
  }, [items])
  const activeLabels = useMemo(
    () => activeFilterLabels(filters, sourceLabels),
    [filters, sourceLabels],
  )
  const filteredItems = useMemo(
    () => sortMarketplaceCatalogItems(
      filterMarketplaceCatalogItems(items, deferredFilters),
      deferredFilters.sort,
      deferredFilters.search,
    ),
    [deferredFilters, items],
  )
  const summaryItems = useMemo(
    () => filterMarketplaceCatalogItems(items, { ...deferredFilters, lens: 'all' }),
    [deferredFilters, items],
  )
  const summary = useMemo(() => marketplaceCatalogSummary(summaryItems), [summaryItems])
  const renderedItems = useMemo(() => filteredItems.slice(0, renderLimit), [filteredItems, renderLimit])
  const loadErrors = [sourcesError, pluginsError, mcpError, acpError].filter(Boolean)

  useEffect(() => {
    setRenderLimit(CATALOG_RENDER_BATCH)
  }, [deferredFilters, effectiveViewMode])

  const updateFilters = (patch: Partial<MarketplaceCatalogFilterState>) => {
    setFilters((current) => ({ ...current, ...patch }))
  }

  const clearFilters = () => {
    setFilters({ ...DEFAULT_FILTERS, lens: filters.lens })
  }

  const isRefreshingRef = useRef(false)

  const handleRefresh = async () => {
    if (isRefreshingRef.current) return
    isRefreshingRef.current = true
    setIsRefreshing(true)
    try {
      const results = await Promise.allSettled([refreshSources(), refreshPlugins(), refreshMcpServers(), refreshAcpAgents()])
      const failures = results.filter((r): r is PromiseRejectedResult => r.status === 'rejected')
      if (failures.length > 0) {
        toast.error(`${failures.length} catalog source(s) failed to refresh.`)
      }
    } finally {
      isRefreshingRef.current = false
      setIsRefreshing(false)
    }
  }

  const handleItemAction = (item: MarketplaceCatalogItem) => {
    if (item.kind === 'source') {
      updateFilters({ lens: 'all', sourceIds: item.sourceId ? [item.sourceId] : [] })
      toast.info(`Filtered catalog to ${item.name}`)
      return
    }
    if (readOnlyPreview) {
      setPreviewItem(item)
      toast.info('Dev preview is read-only. No install or removal action was sent.')
      return
    }
    if (isMcpServerCatalogItem(item)) {
      setMcpInstallItem(item)
      return
    }
    if (isAcpAgentCatalogItem(item)) {
      setAcpInstallItem(item)
      return
    }
    if (isPluginComponentCatalogItem(item)) {
      setComponentInstallItem(item)
      return
    }
    setPreviewItem(item)
  }

  const handlePrimaryMutation = async () => {
    if (!previewItem) return
    if (readOnlyPreview) {
      toast.info('Dev preview is read-only. No install or removal action was sent.')
      return
    }
    if (!canRunProductionMutation(previewItem)) {
      toast.error('This catalog item does not have a direct package action.')
      return
    }
    if (isMutatingRef.current) return
    isMutatingRef.current = true
    setIsMutating(true)
    try {
      let succeeded = false
      if (previewItem.installed) {
        succeeded = await uninstall(previewItem.id, previewItem.name)
      } else {
        succeeded = await install(previewItem.id, previewItem.name)
      }
      if (succeeded) setPreviewItem(null)
    } finally {
      isMutatingRef.current = false
      setIsMutating(false)
    }
  }

  const filterGroups = (
    <div className="space-y-4">
      <FilterGroup label="Sort">
        <div className="flex flex-wrap gap-2">
          {SORT_OPTIONS.map((option) => (
            <button key={option.value} type="button" onClick={() => updateFilters({ sort: option.value })} className={cn('rounded-full border px-3 py-2 text-[13px] font-medium', marketplacePillTone(filters.sort === option.value))} aria-label={`Sort by ${option.label}`} aria-pressed={filters.sort === option.value}>
              {option.label}
            </button>
          ))}
        </div>
      </FilterGroup>
      <FilterGroup label="Type">
        {TYPE_OPTIONS.map((option) => <FilterCheckbox key={option.value} checked={filters.types.includes(option.value)} label={option.label} onChange={() => updateFilters({ types: toggleValue(filters.types, option.value) })} />)}
      </FilterGroup>
      <FilterGroup label="Install state">
        {INSTALL_OPTIONS.map((option) => <FilterCheckbox key={option.value} checked={filters.installStates.includes(option.value)} label={option.label} onChange={() => updateFilters({ installStates: toggleValue(filters.installStates, option.value) })} />)}
      </FilterGroup>
      <FilterGroup label="Ecosystem">
        {ecosystemOptions.map((option) => <FilterCheckbox key={option} checked={filters.ecosystems.includes(option)} label={option} onChange={() => updateFilters({ ecosystems: toggleValue(filters.ecosystems, option) })} />)}
      </FilterGroup>
      <FilterGroup label="Source">
        {sourceOptions.map((source) => <FilterCheckbox key={source.id} checked={filters.sourceIds.includes(source.id)} label={source.label} onChange={() => updateFilters({ sourceIds: toggleValue(filters.sourceIds, source.id) })} />)}
      </FilterGroup>
      <FilterGroup label="Distribution">
        {distributionOptions.map((option) => <FilterCheckbox key={option} checked={filters.distributions.includes(option)} label={option} onChange={() => updateFilters({ distributions: toggleValue(filters.distributions, option) })} />)}
      </FilterGroup>
    </div>
  )

  return (
    <>
      <AppHeader
        breadcrumbs={[{ label: 'Labby', href: '/' }, { label: 'Marketplace' }]}
        actions={<div className="flex items-center gap-2">
          {VIEW_OPTIONS.map((option) => (
            <Button
              key={option.value}
              variant="outline"
              size="icon"
              onClick={() => updateViewPreference(option.value)}
              className={cn(gatewayActionTone(), 'size-9', viewPreference === option.value && 'border-aurora-accent-primary/45 text-aurora-accent-strong')}
              aria-label={option.label}
              aria-pressed={viewPreference === option.value}
            >
              {option.icon}
            </Button>
          ))}
          <Button variant="outline" size="icon" onClick={() => readOnlyPreview ? toast.info('Dev preview is read-only. Source add flow is visible but writes are blocked.') : setAddSourceOpen(true)} className={cn(gatewayActionTone(), 'size-9')} aria-label="Add marketplace source"><Plus className="size-4" /></Button>
          <Button size="icon" onClick={() => { void handleRefresh() }} disabled={isRefreshing} className={cn(gatewayActionTone('accent'), 'size-9 border')} aria-label="Refresh marketplace catalog"><RefreshCw className={cn('size-4', isRefreshing && 'animate-spin')} /></Button>
        </div>}
      />

      <main className={cn('min-h-[calc(100vh-3.5rem)] bg-aurora-page-bg text-aurora-text-primary', AURORA_PAGE_SHELL)}>
        <div className={cn(AURORA_PAGE_FRAME, 'gap-6')}>
          <section className={cn(AURORA_MEDIUM_PANEL, 'p-5')}>
            <p className={AURORA_MUTED_LABEL}>Operator catalog</p>
            <h1 className={cn(AURORA_DISPLAY_1, 'mt-2 text-aurora-text-primary')}>Marketplace</h1>
            <p className="mt-3 max-w-3xl text-[14px] leading-[1.55] text-aurora-text-muted">
              Browse plugins, MCP servers, and ACP agents from one live catalog. Preview install flows safely from the read-only dev route.
            </p>
            <p className={cn('mt-3 text-[12px] font-semibold text-aurora-accent-strong', !readOnlyPreview && 'hidden')}>
              Dev preview: live backend reads are enabled; install, remove, source, and wiring mutations are blocked.
            </p>
          </section>

          <section className={cn(AURORA_MEDIUM_PANEL, 'p-1.5 lg:hidden')}>
            <div className="grid grid-cols-3 gap-1">
              <LensChip label="All" value={summary.all} active={filters.lens === 'all'} onClick={() => updateFilters({ lens: 'all' })} />
              <LensChip label="Installed" value={summary.installed} active={filters.lens === 'installed'} onClick={() => updateFilters({ lens: 'installed' })} />
              <LensChip label="Plugins" value={summary.plugins} active={filters.lens === 'plugins'} onClick={() => updateFilters({ lens: 'plugins' })} />
              <LensChip label="MCP servers" value={summary.mcpServers} active={filters.lens === 'mcp_servers'} onClick={() => updateFilters({ lens: 'mcp_servers' })} />
              <LensChip label="ACP agents" value={summary.acpAgents} active={filters.lens === 'acp_agents'} onClick={() => updateFilters({ lens: 'acp_agents' })} />
              <LensChip label="Sources" value={summary.sources} active={filters.lens === 'sources'} onClick={() => updateFilters({ lens: 'sources' })} />
            </div>
          </section>

          <section className={cn(AURORA_MEDIUM_PANEL, 'hidden p-1.5 lg:block')}>
            <div className="grid grid-cols-6 gap-1">
              <LensCard label="All" value={summary.all} active={filters.lens === 'all'} onClick={() => updateFilters({ lens: 'all' })} />
              <LensCard label="Installed" value={summary.installed} active={filters.lens === 'installed'} onClick={() => updateFilters({ lens: 'installed' })} />
              <LensCard label="Plugins" value={summary.plugins} active={filters.lens === 'plugins'} onClick={() => updateFilters({ lens: 'plugins' })} />
              <LensCard label="MCP servers" value={summary.mcpServers} active={filters.lens === 'mcp_servers'} onClick={() => updateFilters({ lens: 'mcp_servers' })} />
              <LensCard label="ACP agents" value={summary.acpAgents} active={filters.lens === 'acp_agents'} onClick={() => updateFilters({ lens: 'acp_agents' })} />
              <LensCard label="Sources" value={summary.sources} active={filters.lens === 'sources'} onClick={() => updateFilters({ lens: 'sources' })} />
            </div>
          </section>

          <div className="grid gap-6 lg:grid-cols-[280px_minmax(0,1fr)] lg:items-start">
            <aside>
              <div className="space-y-3 lg:hidden">
                <div className="relative">
                  <Search className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-aurora-text-muted" />
                  <Input value={filters.search} onChange={(event) => updateFilters({ search: event.target.value })} name="marketplace-search-mobile" aria-label="Search marketplace catalog" placeholder="Search marketplace" className="h-10 border border-aurora-border-strong bg-aurora-control-surface pl-9 pr-[4.75rem] text-aurora-text-primary placeholder:text-aurora-text-muted" />
                  <div className="absolute inset-y-0 right-1 flex items-center gap-1">
                    {filters.search ? <Button type="button" variant="outline" size="icon" onClick={() => updateFilters({ search: '' })} className={cn(gatewayActionTone(), 'size-7 rounded-full')} aria-label="Clear search"><X className="size-3.5" /></Button> : null}
                    <Button type="button" variant="outline" size="icon" onClick={() => setMobileSheetOpen(!mobileSheetOpen)} className={cn(gatewayActionTone(), 'relative size-7 rounded-full')} aria-label="Open filters"><SlidersHorizontal className="size-3.5" />{activeLabels.length ? <span className={cn(AURORA_BADGE_LABEL, 'absolute -top-1 -right-1 rounded-full border border-aurora-accent-primary/35 bg-aurora-accent-primary/14 px-2 leading-4 text-aurora-accent-strong')}>{activeLabels.length}</span> : null}</Button>
                  </div>
                </div>
                {activeLabels.length ? <div className="flex flex-wrap gap-2">{activeLabels.map((label) => <span key={label} className={cn(AURORA_BADGE_LABEL, 'rounded-full border px-2.5 py-1', marketplacePillTone(true))}>{label}</span>)}</div> : null}
                <Sheet open={mobileSheetOpen} onOpenChange={setMobileSheetOpen}>
                  <SheetContent side="bottom" className="max-h-[82svh] overflow-hidden rounded-t-aurora-3 border-aurora-border-strong bg-aurora-panel-strong p-0 text-aurora-text-primary">
                    <SheetHeader className="border-b border-aurora-border-strong px-4 py-4 text-left">
                      <SheetTitle>Filter catalog</SheetTitle>
                      <SheetDescription>Refine marketplace items without leaving the results list.</SheetDescription>
                    </SheetHeader>
                    <div className="aurora-scrollbar overflow-y-auto px-4 py-4">
                      <div className="mb-4 flex justify-end">
                        <Button variant="outline" size="sm" onClick={clearFilters} className={cn(gatewayActionTone(), 'h-8 px-3 text-aurora-text-primary')}>Clear</Button>
                      </div>
                      {filterGroups}
                    </div>
                  </SheetContent>
                </Sheet>
              </div>

              <div className={cn(AURORA_MEDIUM_PANEL, 'hidden space-y-4 p-4 lg:block')}>
                <div className="space-y-2">
                  <p className={AURORA_MUTED_LABEL}>Search</p>
                  <div className="relative">
                    <Search className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-aurora-text-muted" />
                    <Input value={filters.search} onChange={(event) => updateFilters({ search: event.target.value })} name="marketplace-search" aria-label="Search marketplace catalog" placeholder="Search plugins, MCP servers, agents" className="h-11 border border-aurora-border-strong bg-aurora-control-surface pl-9 text-aurora-text-primary placeholder:text-aurora-text-muted" />
                  </div>
                </div>
                <div className="flex items-center justify-between gap-3">
                  <p className={AURORA_MUTED_LABEL}>Filter catalog</p>
                  {activeLabels.length || filters.search ? <Button variant="outline" size="sm" onClick={clearFilters} className={cn(gatewayActionTone(), 'h-8 px-3')}>Clear</Button> : null}
                </div>
                {filterGroups}
              </div>
            </aside>

            <section className="aurora-scrollbar min-w-0">
              {loadErrors.length ? <div className={cn(AURORA_MEDIUM_PANEL, 'mb-4 p-4 text-[13px] text-aurora-warn')}>Partial catalog load issue: {loadErrors.map((error) => getErrorMessage(error, 'load failed')).join(' / ')}</div> : null}
              <div className="mb-3 flex items-center justify-between gap-3">
                <p className={AURORA_MUTED_LABEL}>{filteredItems.length} results</p>
                {summary.updates ? <span className={cn(AURORA_DENSE_META, 'rounded-full border border-aurora-warn/35 bg-aurora-control-surface px-2.5 py-1 font-semibold text-aurora-warn')}>{summary.updates} updates</span> : null}
              </div>
              {filteredItems.length === 0 ? (
                <div className={cn(AURORA_STRONG_PANEL, 'p-8 text-center')}><p className={cn(AURORA_COMPACT_TITLE, 'text-aurora-text-primary')}>No matching marketplace items</p><p className="mt-2 text-sm text-aurora-text-muted">Adjust search or filters to return plugins, MCP servers, ACP agents, or sources.</p></div>
              ) : (
                <>
                  {filteredItems.length > renderedItems.length ? (
                    <div className={cn(AURORA_MEDIUM_PANEL, 'mb-3 flex flex-wrap items-center justify-between gap-3 p-3 text-[13px] text-aurora-text-muted')}>
                      <span>Showing {renderedItems.length} of {filteredItems.length} results.</span>
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        onClick={() => setRenderLimit((current) => current + CATALOG_RENDER_BATCH)}
                        className={cn(gatewayActionTone(), 'h-8 px-3 text-aurora-text-primary')}
                      >
                        Show more
                      </Button>
                    </div>
                  ) : null}
                  {effectiveViewMode === 'table' ? (
                    <CatalogTable items={renderedItems} onAction={handleItemAction} />
                  ) : (
                    <div className="grid min-w-0 gap-3 xl:grid-cols-2 2xl:grid-cols-3">{renderedItems.map((item) => <CatalogCard key={item.id} item={item} onAction={handleItemAction} />)}</div>
                  )}
                </>
              )}
            </section>
          </div>
        </div>
      </main>

      <Dialog open={previewItem !== null} onOpenChange={(open) => !open && setPreviewItem(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{previewItem ? primaryActionLabel(previewItem) : 'Preview action'}</DialogTitle>
            <DialogDescription>
              {previewItem?.kind === 'acp_agent'
                ? 'This would wire an Agent Client Protocol implementation for compatible ACP clients. It does not automatically make the agent available in /chat unless that backend flow is implemented.'
                : readOnlyPreview
                  ? 'This is a live read-only dev preview. The final mutation is blocked before it reaches the backend.'
                  : 'Review the action before continuing.'}
            </DialogDescription>
          </DialogHeader>
          {previewItem ? <div className="rounded-aurora-2 border border-aurora-border-strong bg-aurora-control-surface p-4"><p className="font-semibold text-aurora-text-primary">{previewItem.name}</p><p className="mt-1 text-sm text-aurora-text-muted">{previewItem.description || previewItem.subtitle}</p></div> : null}
          <Button
            type="button"
            disabled={isMutating || readOnlyPreview || !canRunProductionMutation(previewItem)}
            onClick={() => { void handlePrimaryMutation() }}
            className="mt-2"
          >
            {readOnlyPreview
              ? 'Read-only preview: mutation disabled'
              : canRunProductionMutation(previewItem)
                ? isMutating
                  ? 'Working...'
                  : primaryActionLabel(previewItem)
                : 'No direct package action'}
          </Button>
        </DialogContent>
      </Dialog>

      <AddMarketplaceModal
        open={addSourceOpen}
        onClose={() => setAddSourceOpen(false)}
        onAdd={async (input) => {
          const source = await addSource(input)
          if (source) setAddSourceOpen(false)
          return source
        }}
      />

      <McpInstallModal
        server={mcpInstallItem ? catalogItemMcpServer(mcpInstallItem) : null}
        onClose={() => setMcpInstallItem(null)}
        onSuccess={() => { void refreshMcpServers() }}
      />

      {acpInstallItem && isAcpAgentCatalogItem(acpInstallItem) ? (
        <AcpAgentInstallModal
          agent={acpInstallItem.raw}
          open
          onClose={() => {
            setAcpInstallItem(null)
            void refreshAcpAgents()
          }}
        />
      ) : null}

      {componentInstallItem && isPluginComponentCatalogItem(componentInstallItem) ? (
        <CherryPickDialog
          pluginId={componentInstallItem.raw.plugin.id}
          pluginName={componentInstallItem.raw.plugin.name}
          open
          onClose={() => {
            setComponentInstallItem(null)
            void refreshPlugins()
          }}
          components={[
            {
              type: componentInstallItem.raw.component.kind,
              name: componentInstallItem.raw.component.name,
              path: componentInstallItem.raw.component.path,
            },
          ]}
        />
      ) : null}
    </>
  )
}
