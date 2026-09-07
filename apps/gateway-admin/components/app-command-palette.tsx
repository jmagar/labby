'use client'

import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from 'react'
import { usePathname, useRouter } from 'next/navigation'
import * as DialogPrimitive from '@radix-ui/react-dialog'
import {
  Activity,
  ArrowLeft,
  BookOpen,
  Cable,
  Check,
  Copy,
  ExternalLink,
  FileCode2,
  LayoutDashboard,
  Loader2,
  Play,
  Plus,
  Power,
  RefreshCw,
  Search,
  Settings,
  SlidersHorizontal,
  X,
  type LucideIcon,
} from 'lucide-react'
import { toast } from 'sonner'

import {
  Command,
  CommandEmpty,
  CommandItem,
  CommandList,
} from '@/components/ui/command'
import {
  Dialog,
  DialogDescription,
  DialogOverlay,
  DialogPortal,
  DialogTitle,
} from '@/components/ui/dialog'
import { Kbd, KbdGroup } from '@/components/ui/kbd'
import { PaletteAddServer } from '@/components/palette/palette-add-server'
import {
  PaletteCountsStrip,
  PaletteDot,
  PaletteFooter,
  PaletteSectionHeader,
  PaletteSplit,
  paletteToneVar,
} from '@/components/palette/palette-parts'
import {
  PaletteAlertRow,
  PaletteCommandRow,
  PaletteServerRow,
} from '@/components/palette/palette-rows'
import { PaletteStyles } from '@/components/palette/palette-styles'
import {
  EMPTY_PALETTE_FILTERS,
  PALETTE_SCOPE_HINT,
  PALETTE_SCOPE_LABELS,
  PALETTE_STATUS_FILTERS,
  PALETTE_TRANSPORT_FILTERS,
  type AppCommandIconKey,
  type AppCommandItem,
  type CatalogBrowseItem,
  type PaletteServerFilters,
  appCommandItems,
  buildAppCommandState,
  buildCatalogActionItems,
  buildCatalogServiceItems,
  buildGatewayAlerts,
  buildPaletteCounts,
  buildPaletteFooterLabel,
  countPaletteFilterMatches,
  describeGatewayConnection,
  findAppCommandItemById,
  gatewayMatchesPaletteFilters,
  paletteFiltersActive,
  paletteScopeShows,
  parsePaletteScope,
  togglePaletteFilter,
} from '@/lib/app-command-palette'
import type { CatalogAction, CatalogParam } from '@/lib/types/command-catalog'
import { useCommandCatalog } from '@/lib/hooks/use-command-catalog'
import { useCopyTimeout } from '@/lib/hooks/use-copy-timeout'
import { useGateways, useGatewayMutations } from '@/lib/hooks/use-gateways'
import { confirmGatewayParams } from '@/lib/api/gateway-request'
import { gatewayDetailHref, normalizeGatewayApiBase } from '@/lib/api/gateway-config'
import { buildGatewayClientConfig } from '@/lib/api/gateway-client-config'
import { buildGatewayEndpointPreview } from '@/lib/api/gateway-mobile'
import {
  isAbortError,
  performServiceAction,
  type ServiceActionError,
} from '@/lib/api/service-action-client'
import type { CreateGatewayInput, Gateway } from '@/lib/types/gateway'
import { OPEN_COMMAND_PALETTE_EVENT } from '@/lib/command-palette-events'
import { capabilityForPath } from '@/components/console/nav-model'
import { AUTHORITY_WORKSPACE_CHANGED_EVENT, useBrowserSession } from '@/lib/auth/session'

// ── Constants ─────────────────────────────────────────────────────────────────

const ICONS: Record<AppCommandIconKey, LucideIcon> = {
  docs: BookOpen,
  gateway: Cable,
  overview: LayoutDashboard,
  settings: Settings,
  snippets: FileCode2,
  usage: Activity,
}

const KIND_LABELS: Record<AppCommandItem['kind'], string> = {
  action: 'Action',
  destination: 'Page',
}

/** Mock parity: the server list is capped at seven rows. */
const SERVER_ROW_LIMIT = 7

/**
 * Filter groups shown in the palette's filter panel. The mock also carries a
 * `Source` group (Gateway / Registry); this console has no equivalent
 * taxonomy on `Gateway.source`, so it is omitted rather than faked.
 */
const FILTER_GROUPS = [
  { group: 'status' as const, label: 'Status', pills: PALETTE_STATUS_FILTERS },
  { group: 'transport' as const, label: 'Transport', pills: PALETTE_TRANSPORT_FILTERS },
]

export { OPEN_COMMAND_PALETTE_EVENT }

// ── Mode state (discriminated union) ─────────────────────────────────────────

type PaletteMode =
  | { kind: 'browse' }
  | { kind: 'add_server' }
  | { kind: 'service_pane'; gatewayId: string }
  | { kind: 'param_prompt'; service: string; action: CatalogAction }

type PaletteAction =
  | { type: 'BROWSE' }
  | { type: 'ADD_SERVER' }
  | { type: 'SERVICE_PANE'; gatewayId: string }
  | { type: 'PARAM_PROMPT'; service: string; action: CatalogAction }

function paletteReducer(state: PaletteMode, action: PaletteAction): PaletteMode {
  switch (action.type) {
    case 'BROWSE':
      return { kind: 'browse' }
    case 'ADD_SERVER':
      return { kind: 'add_server' }
    case 'SERVICE_PANE':
      return { kind: 'service_pane', gatewayId: action.gatewayId }
    case 'PARAM_PROMPT':
      return { kind: 'param_prompt', service: action.service, action: action.action }
  }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

function isCommandK(event: KeyboardEvent): boolean {
  return (event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k'
}

function isMacOS(): boolean {
  return typeof navigator !== 'undefined' && /Mac|iPhone|iPad|iPod/.test(navigator.platform)
}

function serviceActionUrl(service: string): string {
  // normalizeGatewayApiBase strips trailing slash; service path is /v1/{service}
  return `${normalizeGatewayApiBase()}/${service}`
}

/** Simple error factory for palette dispatched actions (no typed error class needed). */
function makePaletteError(message: string, status: number, code?: string): ServiceActionError {
  return Object.assign(new Error(message), { status, code }) as ServiceActionError
}

/**
 * Coerce a raw form string value to the JSON type declared in `CatalogParam.ty`.
 * Without coercion, integer/boolean params would arrive as strings and fail server-side
 * `invalid_param` validation even when the user entered a correct value.
 */
function coerceParamValue(rawValue: string, ty: string): unknown {
  const normalized = ty.toLowerCase()
  if (normalized === 'integer' || normalized === 'number') {
    // Issue 1: empty/whitespace string must NOT coerce to 0 — let server reject it
    if (rawValue.trim() === '') return rawValue
    const n = Number(rawValue)
    return Number.isFinite(n) ? n : rawValue
  }
  if (normalized === 'boolean') {
    if (rawValue === 'true' || rawValue === '1') return true
    if (rawValue === 'false' || rawValue === '0') return false
    return rawValue
  }
  // Issue 2: object/array params must be parsed so server receives a JSON value, not a string
  if (normalized === 'object' || normalized === 'array') {
    try {
      return JSON.parse(rawValue)
    } catch {
      return rawValue // let server reject malformed JSON
    }
  }
  // string and union types: pass through as-is
  return rawValue
}

/**
 * Parse instance labels from a param description.
 * Handles patterns like "Valid labels: default, node2" or "one of: default, node2".
 * Returns [] when no recognisable pattern is found.
 */
function parseInstanceLabels(description: string): string[] {
  // Look for text after a colon, then split on commas
  const match = description.match(/:\s*([a-zA-Z0-9_,\s-]+)$/)
  if (!match) return []
  return match[1]
    .split(',')
    .map((s) => s.trim())
    .filter((s) => s.length > 0 && s.length <= 64)
}

function matchesQuery(query: string, ...fields: string[]): boolean {
  if (!query) return true
  const q = query.toLowerCase()
  return fields.some((field) => field.toLowerCase().includes(q))
}

// ── Public components ─────────────────────────────────────────────────────────

export function AppCommandPaletteTrigger() {
  const modKey = isMacOS() ? '⌘' : 'Ctrl'
  return (
    <button
      type="button"
      onClick={() => window.dispatchEvent(new Event(OPEN_COMMAND_PALETTE_EVENT))}
      className="hidden min-w-[220px] items-center justify-between gap-3 rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface px-3 py-1.5 text-left text-xs text-aurora-text-muted transition hover:border-aurora-border-strong hover:bg-aurora-hover-bg hover:text-aurora-text-primary md:flex"
      aria-label="Open command palette"
    >
      <span className="inline-flex items-center gap-2">
        <Search className="size-3.5" />
        Search or jump...
      </span>
      <KbdGroup>
        <Kbd className="border border-aurora-border-default bg-aurora-panel-medium text-[10px] text-aurora-text-muted">
          {modKey}
        </Kbd>
        <Kbd className="border border-aurora-border-default bg-aurora-panel-medium text-[10px] text-aurora-text-muted">
          K
        </Kbd>
      </KbdGroup>
    </button>
  )
}

export function AppCommandPalette() {
  const router = useRouter()
  const pathname = usePathname()
  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const [pages, setPages] = useState<string[]>([])
  const [mode, dispatch] = useReducer(paletteReducer, { kind: 'browse' })
  const [showAdvanced, setShowAdvanced] = useState(false)
  const [isDispatching, setIsDispatching] = useState(false)
  const [pendingGatewayId, setPendingGatewayId] = useState<string | null>(null)
  const [filters, setFilters] = useState<PaletteServerFilters>(EMPTY_PALETTE_FILTERS)
  // null = auto: the panel opens by itself once a filter is active (mock parity).
  const [filtersOpenOverride, setFiltersOpenOverride] = useState<boolean | null>(null)
  const [copiedId, setCopiedId] = useState<string | null>(null)
  const [isCopied, markCopied] = useCopyTimeout(1500)
  const abortRef = useRef<AbortController | null>(null)
  const session = useBrowserSession()
  const capabilities = useMemo(
    () => session.status === 'authenticated' ? session.authority?.capabilities ?? [] : [],
    [session],
  )
  const allowedCommands = useMemo(() => appCommandItems.filter((item) => {
    if (!item.href) return capabilities.includes('scope.operate')
    const required = capabilityForPath(item.href)
    return required === null || (required !== undefined && capabilities.includes(required))
  }), [capabilities])

  useEffect(() => {
    const clearSensitiveState = () => {
      abortRef.current?.abort()
      abortRef.current = null
      setOpen(false)
      setQuery('')
      setPages([])
      dispatch({ type: 'BROWSE' })
      setPendingGatewayId(null)
    }
    window.addEventListener(AUTHORITY_WORKSPACE_CHANGED_EVENT, clearSensitiveState)
    return () => window.removeEventListener(AUTHORITY_WORKSPACE_CHANGED_EVENT, clearSensitiveState)
  }, [])

  // Issue 4: destructure error so we can surface catalog fetch failures
  const { data: catalogServices, isLoading: catalogLoading, error: catalogError } = useCommandCatalog()
  // Gateway rows only exist inside the open palette, and `gateway.list` warms
  // the whole upstream pool — cold-spawning every stdio server. The palette is
  // mounted by the admin layout on every page, so fetching on mount undid the
  // deliberate gating the Loadouts page added and made each navigation spawn
  // the fleet. Fetch when the operator actually opens the palette.
  const canManageGateways = capabilities.includes('scope.manage') || capabilities.includes('platform.manage')
  const canOperate = capabilities.includes('scope.operate')
  const { data: gateways = [], isLoading: gatewaysLoading } = useGateways(open && canManageGateways)
  const { createGateway, testGateway, reloadGateway, enableGateway, disableGateway } =
    useGatewayMutations()

  // Scoped prefixes — `>` actions · `#` servers · `/` pages (mock parity).
  const { scope, query: scopedQuery } = useMemo(() => parsePaletteScope(query), [query])

  const scopedCommandItems = useMemo(() => {
    if (scope === null) return undefined
    if (scope === 'actions') return allowedCommands.filter((item) => item.kind === 'action')
    if (scope === 'pages') return allowedCommands.filter((item) => item.kind === 'destination')
    return []
  }, [allowedCommands, scope])

  const state = useMemo(
    () => buildAppCommandState(scopedQuery, scopedCommandItems ?? allowedCommands),
    [allowedCommands, scopedQuery, scopedCommandItems],
  )
  const [activeItemId, setActiveItemId] = useState<string | null>(state.activeItemId)

  // Current page: top of page stack, or '' for root browse
  const currentPage = pages[pages.length - 1] ?? ''

  // Catalog items for the current page
  const catalogItems = useMemo<CatalogBrowseItem[]>(() => {
    if (!canOperate) return []
    if (currentPage === '') {
      if (!paletteScopeShows(scope, 'actions')) return []
      return buildCatalogServiceItems(catalogServices)
    }
    const svc = catalogServices.find((s) => s.name === currentPage)
    if (!svc) return []
    return buildCatalogActionItems(svc.name, svc.actions)
  }, [canOperate, currentPage, catalogServices, scope])

  const visibleCatalogItems = useMemo(
    () =>
      catalogItems.filter((item) => matchesQuery(scopedQuery, item.title, item.description)),
    [catalogItems, scopedQuery],
  )

  const hasFilters = paletteFiltersActive(filters)
  const filtersOpen = filtersOpenOverride ?? hasFilters
  const activeFilterChips = useMemo(
    () =>
      FILTER_GROUPS.flatMap((group) =>
        group.pills
          .filter((pill) => (filters[group.group] as string[]).includes(pill.value))
          .map((pill) => ({ group: group.group, value: pill.value, label: pill.label })),
      ),
    [filters],
  )

  const gatewayItems = useMemo(() => {
    if (currentPage !== '' || !paletteScopeShows(scope, 'servers')) return []
    return gateways
      .filter((gateway) => gatewayMatchesPaletteFilters(gateway, filters))
      .filter((gateway) => {
        const endpoint = buildGatewayEndpointPreview(gateway)
        return matchesQuery(
          scopedQuery,
          gateway.name,
          endpoint,
          gateway.transport,
          gateway.source ?? '',
          ...gateway.discovery.tools.map((tool) => tool.name),
        )
      })
      .slice(0, SERVER_ROW_LIMIT)
  }, [currentPage, gateways, scopedQuery, scope, filters])

  // "Needs Attention" rows — derived from live gateway health, empty query only.
  const alerts = useMemo(() => {
    if (currentPage !== '' || query.trim() || mode.kind !== 'browse') return []
    return buildGatewayAlerts(gateways)
  }, [currentPage, gateways, query, mode.kind])

  const showAddServerRow =
    currentPage === '' &&
    paletteScopeShows(scope, 'actions') &&
    matchesQuery(scopedQuery, 'Add Server')

  const paneGateway = useMemo(
    () =>
      mode.kind === 'service_pane'
        ? (gateways.find((gateway) => gateway.id === mode.gatewayId) ?? null)
        : null,
    [mode, gateways],
  )

  // ── Abort management ───────────────────────────────────────────────────────

  const abortPending = useCallback(() => {
    if (abortRef.current) {
      abortRef.current.abort()
      abortRef.current = null
    }
  }, [])

  // ── Open/close ─────────────────────────────────────────────────────────────

  const openPalette = useCallback(() => {
    abortPending()
    setPages([])
    setQuery('')
    dispatch({ type: 'BROWSE' })
    setShowAdvanced(false)
    setIsDispatching(false)
    setPendingGatewayId(null)
    setFilters(EMPTY_PALETTE_FILTERS)
    setFiltersOpenOverride(null)
    setOpen(true)
  }, [abortPending])

  const closePalette = useCallback(() => {
    abortPending()
    setOpen(false)
    setQuery('')
    setPages([])
    dispatch({ type: 'BROWSE' })
    setShowAdvanced(false)
    setIsDispatching(false)
    setPendingGatewayId(null)
    setFilters(EMPTY_PALETTE_FILTERS)
    setFiltersOpenOverride(null)
  }, [abortPending])

  // ── Event listeners ────────────────────────────────────────────────────────

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (!isCommandK(event)) return
      event.preventDefault()
      if (open) {
        closePalette()
      } else {
        openPalette()
      }
    }
    function onOpenPalette() {
      openPalette()
    }

    window.addEventListener('keydown', onKeyDown)
    window.addEventListener(OPEN_COMMAND_PALETTE_EVENT, onOpenPalette)
    return () => {
      window.removeEventListener('keydown', onKeyDown)
      window.removeEventListener(OPEN_COMMAND_PALETTE_EVENT, onOpenPalette)
    }
  }, [open, openPalette, closePalette])

  // Close on pathname change. Use a ref so `open` is not a dep (avoids a
  // re-run every time closePalette flips open→false, which would re-trigger
  // this effect and create an infinite loop).
  const openRef = useRef(open)
  openRef.current = open
  useEffect(() => {
    if (openRef.current) closePalette()
  }, [pathname, closePalette])

  // Sync active item when state changes
  useEffect(() => {
    if (!open) return
    setActiveItemId((current) => {
      if (current && state.items.some((item) => item.id === current)) return current
      return state.activeItemId
    })
  }, [open, state.activeItemId, state.items])

  // ── Page-stack keyboard: Backspace-when-empty pops page ────────────────────

  function handleCommandKeyDown(event: React.KeyboardEvent) {
    if (event.key === 'Backspace' && !query && pages.length > 0) {
      event.preventDefault()
      setPages((prev) => prev.slice(0, -1))
      setQuery('')
    }
  }

  // ── Dispatch (catalog action execution) ────────────────────────────────────

  async function executeAction(service: string, action: CatalogAction, params: Record<string, unknown>) {
    const controller = new AbortController()
    abortRef.current = controller

    setIsDispatching(true)
    try {
      // Use performServiceAction so CSRF and error shaping stay consistent.
      const url = serviceActionUrl(service)
      const finalParams = action.destructive ? confirmGatewayParams(params) : params
      // The response body is intentionally discarded: this path reports via
      // toast and closes. The reducer's `RESULT` mode is reached from the
      // catalog drill-down, not from here.
      await performServiceAction<unknown, ServiceActionError>({
        action: action.action,
        params: finalParams,
        signal: controller.signal,
        serviceLabel: service,
        url,
        createError: makePaletteError,
        source: 'palette',
      })
      toast.success(`${service} ${action.action}`, {
        description: 'Action completed successfully.',
      })
      closePalette()
    } catch (err) {
      if (isAbortError(err)) return
      const message = err instanceof Error ? err.message : 'Unknown error'
      toast.error(`${service} ${action.action} failed`, { description: message })
      dispatch({ type: 'PARAM_PROMPT', service, action })
    } finally {
      setIsDispatching(false)
      abortRef.current = null
    }
  }

  // ── Catalog item selection ─────────────────────────────────────────────────

  function handleCatalogItemSelect(item: CatalogBrowseItem) {
    if (item.kind === 'catalog-service') {
      setPages((prev) => [...prev, item.service])
      setQuery('')
      return
    }

    // catalog-action
    const svc = catalogServices.find((s) => s.name === item.service)
    const action = svc?.actions.find((a) => a.action === item.actionName)
    if (!svc || !action) return

    const requiredParams = action.params.filter((p) => p.required)

    if (requiredParams.length === 0) {
      void executeAction(svc.name, action, {})
      return
    }

    dispatch({ type: 'PARAM_PROMPT', service: svc.name, action })
  }

  // ── Destination item selection ─────────────────────────────────────────────

  function executeDestination(item: AppCommandItem | null) {
    if (!item) return
    closePalette()
    router.push(item.href)
    if (item.kind === 'action') {
      toast.message(item.title, { description: item.description })
    }
  }

  function executeGatewayDestination(gatewayId: string) {
    closePalette()
    router.push(gatewayDetailHref(gatewayId))
  }

  // ── Gateway row / service-pane operations (all backed by real mutations) ───

  async function runGatewayOp(
    gateway: Gateway,
    label: string,
    op: () => Promise<unknown>,
  ) {
    setPendingGatewayId(gateway.id)
    try {
      await op()
      toast.success(`${gateway.name} ${label}`)
    } catch (err) {
      if (isAbortError(err)) return
      const message = err instanceof Error ? err.message : 'Unknown error'
      toast.error(`${gateway.name} ${label} failed`, { description: message })
    } finally {
      setPendingGatewayId(null)
    }
  }

  async function copyGatewayConfig(gateway: Gateway) {
    const json = JSON.stringify(buildGatewayClientConfig(gateway), null, 2)
    try {
      await navigator.clipboard.writeText(json)
      setCopiedId(gateway.id)
      markCopied()
    } catch {
      toast.error(`Could not copy ${gateway.name} connection JSON`)
    }
  }

  function togglePower(gateway: Gateway) {
    const enabled = gateway.enabled ?? true
    void runGatewayOp(gateway, enabled ? 'disabled' : 'enabled', () =>
      enabled ? disableGateway(gateway.id) : enableGateway(gateway.id),
    )
  }

  function testConnection(gateway: Gateway) {
    void runGatewayOp(gateway, 'tested', async () => {
      const result = await testGateway(gateway.id)
      if (!result.success) throw new Error(result.message)
    })
  }

  function reload(gateway: Gateway) {
    void runGatewayOp(gateway, 'reloaded', () => reloadGateway(gateway.id))
  }

  function submitAddServer(input: CreateGatewayInput) {
    setIsDispatching(true)
    void (async () => {
      try {
        const gateway = await createGateway(input)
        toast.success(`${gateway.name} added`, { description: 'Probing the upstream…' })
        closePalette()
        router.push(gatewayDetailHref(gateway.id))
      } catch (err) {
        const message = err instanceof Error ? err.message : 'Unknown error'
        toast.error('Could not add server', { description: message })
      } finally {
        setIsDispatching(false)
      }
    })()
  }

  // ── Param prompt form submit ───────────────────────────────────────────────

  function handleParamSubmit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (mode.kind !== 'param_prompt') return

    const formData = new FormData(event.currentTarget)
    const params: Record<string, unknown> = {}
    for (const [key, value] of formData.entries()) {
      if (typeof value !== 'string') continue
      // Coerce to declared type (integer, boolean, etc.) so Rust param validators don't reject
      // valid user input that arrives as a string from FormData.
      const paramSpec = mode.action.params.find((p) => p.name === key)
      params[key] = paramSpec ? coerceParamValue(value, paramSpec.ty) : value
    }

    // Issue 7: client-side required-field validation before dispatch
    const requiredParams = mode.action.params.filter((p) => p.required)
    const emptyRequired = requiredParams.filter((p) => {
      const val = params[p.name]
      if (val === undefined || val === null) return true
      if (typeof val === 'string') return val.trim() === ''
      if (Array.isArray(val)) return false  // empty array is a valid value
      return false  // numbers, booleans, objects — presence is sufficient
    })
    if (emptyRequired.length > 0) {
      toast.error(`Required: ${emptyRequired.map((p) => p.name).join(', ')}`)
      return
    }

    void executeAction(mode.service, mode.action, params)
  }

  // ── Render ─────────────────────────────────────────────────────────────────

  const showParamForm = mode.kind === 'param_prompt'
  const showAddForm = mode.kind === 'add_server'
  const showList = !showParamForm && !showAddForm

  const placeholder = showAddForm
    ? 'Adding a server…'
    : currentPage
      ? `Search ${currentPage} actions...`
      : 'Search pages, actions, and operational context...'

  const actionCount =
    (showList && currentPage === '' ? state.items.length + visibleCatalogItems.length : 0) +
    (showAddServerRow ? 1 : 0)
  const pageCount = 0
  const counts = buildPaletteCounts({
    servers: gatewayItems.length,
    actions: actionCount,
    pages: pageCount,
    alerts: alerts.length,
  })
  const footerLabel = buildPaletteFooterLabel({
    servers: gatewayItems.length,
    actions: actionCount,
    pages: pageCount,
    alerts: alerts.length,
  })
  const showCounts = mode.kind === 'browse' && (query.trim().length > 0 || alerts.length > 0)

  const backTarget =
    showAddForm || mode.kind === 'service_pane' ? 'mode' : pages.length > 0 ? 'page' : null

  function goBack() {
    if (backTarget === 'mode') {
      dispatch({ type: 'BROWSE' })
      return
    }
    if (backTarget === 'page') {
      setPages((prev) => prev.slice(0, -1))
      setQuery('')
    }
  }

  // Row index feeds the mock's zebra striping across the whole flat list.
  let zebra = 0
  const nextZebra = () => zebra++

  return (
    <Dialog open={open} onOpenChange={(next) => { if (!next) closePalette() }}>
      <DialogPortal>
        <PaletteStyles />
        <DialogOverlay className="z-50 bg-[rgba(3,9,14,0.62)]" />
        <DialogPrimitive.Content
          data-slot="dialog-content"
          data-palette="1"
          aria-label="Search and filter servers"
          className="data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 text-aurora-text-primary duration-200"
        >
          <DialogTitle className="sr-only">Command Palette</DialogTitle>
          <DialogDescription className="sr-only">
            Search Labby destinations, actions, and gateway servers.
          </DialogDescription>

          <Command
            shouldFilter={false}
            value={activeItemId ?? undefined}
            onValueChange={setActiveItemId}
            onKeyDown={handleCommandKeyDown}
            className="block h-auto w-full overflow-visible rounded-none bg-transparent text-aurora-text-primary"
          >
            {/* Header — search field, clear, Esc hint */}
            <div className="pal-head">
              {backTarget ? (
                <button
                  type="button"
                  className="pal-back"
                  aria-label="Back"
                  title="Back to palette — Esc"
                  onClick={goBack}
                >
                  <ArrowLeft size={14} />
                </button>
              ) : null}
              <div className="pal-field">
                <span className="pal-field-icon">
                  <Search size={14} />
                </span>
                <PaletteInput value={query} onValueChange={setQuery} placeholder={placeholder} />
                <kbd className="pal-kbd">↑↓ · ↵</kbd>
              </div>
              {showList ? (
                <button
                  type="button"
                  className="pal-filterbtn"
                  aria-pressed={filtersOpen}
                  aria-label="Toggle filters"
                  title="Filters"
                  onClick={() => setFiltersOpenOverride(!filtersOpen)}
                >
                  <SlidersHorizontal size={13} />
                </button>
              ) : null}
              {query ? (
                <button
                  type="button"
                  className="pal-iconbtn"
                  aria-label="Clear search"
                  onClick={() => setQuery('')}
                >
                  <X size={13} />
                </button>
              ) : null}
              <kbd className="pal-kbd-esc">Esc</kbd>
            </div>

            {/* Counts strip */}
            {showCounts ? (
              <PaletteCountsStrip
                counts={counts}
                scopeLabel={scope ? PALETTE_SCOPE_LABELS[scope] : null}
                onClearScope={() => setQuery('')}
                hint={PALETTE_SCOPE_HINT}
              />
            ) : null}

            {/* Active filter chips — shown when filters are set but the panel is closed */}
            {showList && hasFilters && !filtersOpen ? (
              <div className="pal-chips">
                {activeFilterChips.map((chip) => (
                  <button
                    key={`${chip.group}:${chip.value}`}
                    type="button"
                    className="pal-chipbtn"
                    title="Remove filter"
                    onClick={() =>
                      setFilters((current) => togglePaletteFilter(current, chip.group, chip.value))
                    }
                  >
                    {chip.label}
                    <X size={9} strokeWidth={2.4} />
                  </button>
                ))}
                <span className="pal-grow" />
                <button
                  type="button"
                  className="pal-clear"
                  onClick={() => setFilters(EMPTY_PALETTE_FILTERS)}
                >
                  Clear All
                </button>
              </div>
            ) : null}

            {/* Filter panel — scopes the palette's own Servers list */}
            {showList && filtersOpen ? (
              <div className="pal-filters">
                {FILTER_GROUPS.map((group) => (
                  <div key={group.group} className="pal-filterrow">
                    <span className="pal-filterlabel">{group.label}</span>
                    <div className="pal-filterpills">
                      {group.pills.map((pill) => {
                        const active = (filters[group.group] as string[]).includes(pill.value)
                        return (
                          <button
                            key={pill.value}
                            type="button"
                            className="pal-pill"
                            aria-pressed={active}
                            onClick={() =>
                              setFilters((current) =>
                                togglePaletteFilter(current, group.group, pill.value),
                              )
                            }
                          >
                            {active ? <Check size={11} strokeWidth={2.2} /> : null}
                            {pill.label} {countPaletteFilterMatches(gateways, group.group, pill.value)}
                          </button>
                        )
                      })}
                    </div>
                  </div>
                ))}
              </div>
            ) : null}

            {/* Inline add-server flow */}
            {showAddForm ? (
              <PaletteAddServer isSubmitting={isDispatching} onSubmit={submitAddServer} />
            ) : null}

            {/* Param form — rendered OUTSIDE CommandList to avoid cmdk arrow-key interception.
                The `mode.kind === 'param_prompt'` check is needed for TypeScript narrowing even
                though showParamForm already captures this condition. */}
            {showParamForm && mode.kind === 'param_prompt' && (
              <ParamPromptForm
                service={mode.service}
                action={mode.action}
                isDispatching={isDispatching}
                showAdvanced={showAdvanced}
                onToggleAdvanced={() => setShowAdvanced((v) => !v)}
                onSubmit={handleParamSubmit}
                onCancel={() => {
                  setPages((prev) => prev.slice(0, -1))
                  dispatch({ type: 'BROWSE' })
                }}
              />
            )}

            {showList && (
              <div className="pal-listwrap">
                <CommandList data-pallist="1" className="aurora-scrollbar">
                  {/* Per-service pane header (mock: palSvcOpen) */}
                  {paneGateway ? (
                    <ServicePaneHeader
                      gateway={paneGateway}
                      onBack={() => dispatch({ type: 'BROWSE' })}
                    />
                  ) : null}

                  {/* Catalog drill-down header */}
                  {!paneGateway && currentPage !== '' ? (
                    <div className="pal-svchead">
                      <button
                        type="button"
                        className="pal-rowbtn"
                        aria-label="Back"
                        title="Back to palette"
                        onClick={() => { setPages((prev) => prev.slice(0, -1)); setQuery('') }}
                      >
                        <ArrowLeft size={13} />
                      </button>
                      <span className="pal-svcname">{currentPage}</span>
                      <span className="pal-grow" />
                      <kbd className="pal-kbd">Esc</kbd>
                    </div>
                  ) : null}

                  {/* Per-service actions */}
                  {paneGateway
                    ? renderServicePaneRows({
                        gateway: paneGateway,
                        pending: pendingGatewayId === paneGateway.id,
                        onTest: () => testConnection(paneGateway),
                        onReload: () => reload(paneGateway),
                        onTogglePower: () => togglePower(paneGateway),
                        onCopy: () => void copyGatewayConfig(paneGateway),
                        onOpen: () => executeGatewayDestination(paneGateway.id),
                        nextZebra,
                      })
                    : null}

                  {!paneGateway && (
                    <>
                      {/* Loading skeleton while catalog is loading */}
                      {catalogLoading && currentPage === '' && paletteScopeShows(scope, 'actions') && (
                        <div className="pal-empty">
                          <Loader2 className="mr-2 inline size-4 animate-spin" />
                          Loading services...
                        </div>
                      )}

                      {/* Issue 4: surface catalog fetch error instead of silently showing empty list */}
                      {catalogError && !catalogLoading && paletteScopeShows(scope, 'actions') && (
                        <div className="pal-empty" style={{ color: 'var(--aurora-error)' }}>
                          Failed to load actions — check server connection
                        </div>
                      )}

                      {/* Needs Attention */}
                      {alerts.length > 0 && (
                        <>
                          <PaletteSectionHeader label="Needs Attention" />
                          {alerts.map((alert) => (
                            <PaletteAlertRow
                              key={alert.id}
                              value={alert.id}
                              label={alert.label}
                              tone={alert.tone}
                              onSelect={() =>
                                dispatch({ type: 'SERVICE_PANE', gatewayId: alert.gatewayId })
                              }
                            />
                          ))}
                          <PaletteSplit />
                        </>
                      )}

                      {/* Add Server — backed by the real createGateway mutation */}
                      {showAddServerRow && (
                        <PaletteCommandRow
                          value="palette-add-server"
                          keywords={['add', 'server', 'upstream', 'mcp']}
                          zebra={nextZebra()}
                          icon={<Plus size={12} />}
                          label="Add Server"
                          trailing="Action"
                          onSelect={() => dispatch({ type: 'ADD_SERVER' })}
                        />
                      )}

                      {/* Static destinations / actions, grouped by rank */}
                      {currentPage === '' &&
                        state.groups.map((group) => (
                          <div key={group.key}>
                            <PaletteSectionHeader label={group.label} />
                            {group.items.map((item) => {
                              const Icon = ICONS[item.icon]
                              return (
                                <PaletteCommandRow
                                  key={item.id}
                                  value={item.id}
                                  keywords={item.keywords}
                                  zebra={nextZebra()}
                                  icon={<Icon size={12} />}
                                  iconTone={item.kind === 'action' ? 'accent' : 'muted'}
                                  label={item.title}
                                  trailing={KIND_LABELS[item.kind]}
                                  onSelect={(() => {
                                    const itemId = item.id
                                    return () =>
                                      executeDestination(
                                        findAppCommandItemById(itemId, state.items),
                                      )
                                  })()}
                                />
                              )
                            })}
                          </div>
                        ))}

                      {/* Catalog services (root) or a service's actions (drill-down) */}
                      {!catalogLoading && !catalogError && visibleCatalogItems.length > 0 && (
                        <>
                          {currentPage === '' ? <PaletteSectionHeader label="Services" /> : null}
                          {visibleCatalogItems.map((item) => (
                            <PaletteCommandRow
                              key={item.id}
                              value={item.id}
                              zebra={nextZebra()}
                              icon={<Cable size={12} />}
                              label={item.title}
                              trailing={
                                item.kind === 'catalog-service'
                                  ? 'Browse'
                                  : item.destructive
                                    ? 'Destructive'
                                    : 'Run'
                              }
                              onSelect={() => handleCatalogItemSelect(item)}
                            />
                          ))}
                        </>
                      )}

                      {/* Servers are fetched on first open, so say so rather
                          than rendering as if the fleet were empty. */}
                      {gatewaysLoading && currentPage === '' && paletteScopeShows(scope, 'servers') && (
                        <div className="pal-empty">
                          <Loader2 className="mr-2 inline size-4 animate-spin" />
                          Loading servers...
                        </div>
                      )}

                      {/* Servers */}
                      {gatewayItems.length > 0 && (
                        <>
                          <PaletteSectionHeader label="Servers" />
                          {gatewayItems.map((gateway) => (
                            <PaletteServerRow
                              key={gateway.id}
                              gateway={gateway}
                              endpoint={buildGatewayEndpointPreview(gateway)}
                              connection={describeGatewayConnection(gateway)}
                              zebra={nextZebra()}
                              copied={isCopied && copiedId === gateway.id}
                              pending={pendingGatewayId === gateway.id}
                              onSelect={() => executeGatewayDestination(gateway.id)}
                              onCopy={() => void copyGatewayConfig(gateway)}
                              onTogglePower={() => togglePower(gateway)}
                              onTest={() => testConnection(gateway)}
                              onReload={() => reload(gateway)}
                            />
                          ))}
                        </>
                      )}

                      {/* Empty state */}
                      {!catalogLoading &&
                        !gatewaysLoading &&
                        alerts.length === 0 &&
                        !showAddServerRow &&
                        state.items.length === 0 &&
                        visibleCatalogItems.length === 0 &&
                        gatewayItems.length === 0 && (
                          <CommandEmpty className="pal-empty">
                            {currentPage
                              ? `No actions available for ${currentPage}.`
                              : 'No matching commands. Try gateway, snippets, usage, or settings.'}
                          </CommandEmpty>
                        )}
                    </>
                  )}
                </CommandList>
              </div>
            )}

            {showList ? (
              <PaletteFooter label={footerLabel}>
                {hasFilters ? (
                  <button
                    type="button"
                    className="pal-footclear"
                    onClick={() => setFilters(EMPTY_PALETTE_FILTERS)}
                  >
                    Clear All
                  </button>
                ) : null}
              </PaletteFooter>
            ) : null}
          </Command>
        </DialogPrimitive.Content>
      </DialogPortal>
    </Dialog>
  )
}

/** cmdk input, styled as the mock's search line. */
function PaletteInput({
  value,
  onValueChange,
  placeholder,
}: {
  value: string
  onValueChange: (next: string) => void
  placeholder: string
}) {
  return (
    <input
      autoFocus
      className="pal-input"
      value={value}
      onChange={(event) => onValueChange(event.target.value)}
      aria-label="Search command palette"
      name="app-command-palette-search"
      placeholder={placeholder}
      autoComplete="off"
      spellCheck={false}
    />
  )
}

// ── Service pane ──────────────────────────────────────────────────────────────

function ServicePaneHeader({ gateway, onBack }: { gateway: Gateway; onBack: () => void }) {
  const connection = describeGatewayConnection(gateway)
  return (
    <div className="pal-svchead">
      <button
        type="button"
        className="pal-rowbtn"
        aria-label="Back"
        title="Back to palette"
        onClick={onBack}
      >
        <ArrowLeft size={13} />
      </button>
      <PaletteDot tone={connection.tone} />
      <span className="pal-svcname">{gateway.name}</span>
      <span className="pal-svcstatus" style={{ color: paletteToneVar(connection.tone) }}>
        {connection.label}
      </span>
      <span className="pal-grow" />
      <kbd className="pal-kbd">Esc</kbd>
    </div>
  )
}

function renderServicePaneRows({
  gateway,
  pending,
  onTest,
  onReload,
  onTogglePower,
  onCopy,
  onOpen,
  nextZebra,
}: {
  gateway: Gateway
  pending: boolean
  onTest: () => void
  onReload: () => void
  onTogglePower: () => void
  onCopy: () => void
  onOpen: () => void
  nextZebra: () => number
}) {
  const enabled = gateway.enabled ?? true
  const rows: Array<{
    key: string
    icon: React.ReactNode
    label: string
    sub: string
    onSelect: () => void
  }> = [
    {
      key: 'test',
      icon: <Play size={12} />,
      label: 'Test Connection',
      sub: gateway.transport === 'stdio' ? 'spawn + initialize' : 'initialize + tools/list',
      onSelect: onTest,
    },
    {
      key: 'reload',
      icon: <RefreshCw size={12} />,
      label: 'Reload Server',
      sub: 'Re-probe catalog',
      onSelect: onReload,
    },
    {
      key: 'power',
      icon: <Power size={12} />,
      label: enabled ? 'Disable Server' : 'Enable Server',
      sub: enabled ? 'Stop exposing this upstream' : 'Expose this upstream again',
      onSelect: onTogglePower,
    },
    {
      key: 'copy',
      icon: <Copy size={12} />,
      label: 'Copy Connection JSON',
      sub: '.mcp.json entry',
      onSelect: onCopy,
    },
    {
      key: 'open',
      icon: <ExternalLink size={12} />,
      label: 'Open Server Page',
      sub: 'Full detail hub',
      onSelect: onOpen,
    },
  ]

  return rows.map((row) => (
    <CommandItem
      key={row.key}
      data-palrow="1"
      data-palzebra={nextZebra() % 2 === 1 ? '1' : '0'}
      value={`svcaction:${gateway.id}:${row.key}`}
      disabled={pending}
      onSelect={row.onSelect}
    >
      <span className="pal-chip">{row.icon}</span>
      <span className="pal-label">{row.label}</span>
      <span className="pal-grow" />
      <span className="pal-sub" data-plain="1">
        {row.sub}
      </span>
    </CommandItem>
  ))
}

// ── ParamPromptForm ───────────────────────────────────────────────────────────

type ParamPromptFormProps = {
  service: string
  action: CatalogAction
  isDispatching: boolean
  showAdvanced: boolean
  onToggleAdvanced: () => void
  onSubmit: (event: React.FormEvent<HTMLFormElement>) => void
  onCancel: () => void
}

function ParamPromptForm({
  service,
  action,
  isDispatching,
  showAdvanced,
  onToggleAdvanced,
  onSubmit,
  onCancel,
}: ParamPromptFormProps) {
  const requiredParams = action.params.filter((p) => p.required)
  const optionalParams = action.params.filter((p) => !p.required)
  const totalParams = action.params.length

  // Issue 8: when there are 5+ total params, hide optional params by default behind the
  // Advanced toggle. Also hide when 3+ required params (existing rule). Show when
  // toggle is open (showAdvanced).
  const showAllOptional = (requiredParams.length < 3 && totalParams < 5) || showAdvanced
  // Show the Advanced toggle whenever there are optional params AND they are hidden by default
  const showAdvancedToggle = optionalParams.length > 0 && (requiredParams.length >= 3 || totalParams >= 5)

  return (
    <div className="pal-form">
      <div>
        <div className="pal-svcname">
          {service} / {action.action}
        </div>
        {action.description && (
          <p className="mt-1 text-[12px] text-aurora-text-muted">{action.description}</p>
        )}
      </div>

      <form onSubmit={onSubmit} className="flex flex-col gap-3">
        {/* Required params */}
        {requiredParams.map((param) => (
          <ParamField key={param.name} param={param} actionName={action.action} />
        ))}

        {/* Optional params */}
        {optionalParams.length > 0 && (
          <>
            {showAllOptional && optionalParams.map((param) => (
              <ParamField key={param.name} param={param} actionName={action.action} />
            ))}

            {/* Advanced toggle: shown when optional params are hidden by default */}
            {showAdvancedToggle && (
              <button
                type="button"
                className="text-left text-[12px] text-aurora-accent-primary hover:underline"
                onClick={onToggleAdvanced}
              >
                {showAdvanced
                  ? 'Hide advanced options'
                  : `Show ${optionalParams.length} optional parameter${optionalParams.length > 1 ? 's' : ''}`}
              </button>
            )}
          </>
        )}

        <div className="pal-add-foot">
          <span className="pal-grow" />
          <button type="button" className="pal-btn" onClick={onCancel}>
            Cancel
          </button>
          <button type="submit" className="pal-btn" data-primary="1" disabled={isDispatching}>
            {isDispatching && <Loader2 size={11} className="mr-1 inline animate-spin" />}
            {action.destructive ? 'Continue...' : 'Run'}
          </button>
        </div>
      </form>
    </div>
  )
}

// ── ParamField ─────────────────────────────────────────────────────────────────

function ParamField({ param, actionName }: { param: CatalogParam; actionName: string }) {
  const isPassword = param.secret === true
  const normalized = param.ty.toLowerCase()

  // Issue 6: for 'instance' params, parse labels from description and render a datalist
  const isInstanceParam = param.name === 'instance'
  const instanceLabels = isInstanceParam && param.description
    ? parseInstanceLabels(param.description)
    : []
  const datalistId = instanceLabels.length > 0 ? `instance-${actionName}` : undefined

  // Issue 3: cap input length by type to prevent bloated payloads
  const maxLength = (normalized === 'integer' || normalized === 'number')
    ? 20
    : normalized === 'boolean'
      ? 5
      : 2000

  return (
    <div className="pal-add-col">
      <label htmlFor={`param-${param.name}`} className="pal-add-label">
        {param.name}
        {param.required && <span className="ml-1 text-aurora-error">*</span>}
        <span className="ml-2 font-normal normal-case tracking-normal">({param.ty})</span>
      </label>
      {param.description && (
        <p className="text-[11px] text-aurora-text-muted">{param.description}</p>
      )}
      <input
        id={`param-${param.name}`}
        name={param.name}
        type={isPassword ? 'password' : 'text'}
        required={param.required}
        autoComplete={isPassword ? 'current-password' : 'off'}
        maxLength={maxLength}
        list={datalistId}
        className="pal-add-input"
        placeholder={isPassword ? '••••••••' : param.description || param.name}
      />
      {datalistId && (
        <datalist id={datalistId}>
          {instanceLabels.map((label) => (
            <option key={label} value={label} />
          ))}
        </datalist>
      )}
    </div>
  )
}
