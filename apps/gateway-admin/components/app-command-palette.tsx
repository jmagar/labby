'use client'

import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from 'react'
import { usePathname, useRouter } from 'next/navigation'
import {
  Activity,
  BookOpen,
  Cable,
  LayoutDashboard,
  Loader2,
  Logs,
  MessageSquareText,
  Search,
  Settings,
  ShoppingBag,
  WandSparkles,
  type LucideIcon,
} from 'lucide-react'
import { toast } from 'sonner'

import { Badge } from '@/components/ui/badge'
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandShortcut,
} from '@/components/ui/command'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Kbd, KbdGroup } from '@/components/ui/kbd'
import { ConfirmDialog } from '@/components/marketplace/confirm-dialog'
import { cn } from '@/lib/utils'
import {
  type AppCommandIconKey,
  type AppCommandItem,
  type CatalogBrowseItem,
  buildAppCommandState,
  buildCatalogActionItems,
  buildCatalogServiceItems,
  findAppCommandItemById,
} from '@/lib/app-command-palette'
import type { CatalogAction, CatalogParam } from '@/lib/types/command-catalog'
import { useCommandCatalog } from '@/lib/hooks/use-command-catalog'
import { confirmGatewayParams } from '@/lib/api/gateway-request'
import { normalizeGatewayApiBase } from '@/lib/api/gateway-config'
import {
  isAbortError,
  performServiceAction,
  type ServiceActionError,
} from '@/lib/api/service-action-client'

// ── Constants ─────────────────────────────────────────────────────────────────

const ICONS: Record<AppCommandIconKey, LucideIcon> = {
  activity: Activity,
  chat: MessageSquareText,
  docs: BookOpen,
  gateway: Cable,
  logs: Logs,
  marketplace: ShoppingBag,
  overview: LayoutDashboard,
  settings: Settings,
  setup: WandSparkles,
}

const KIND_LABELS: Record<AppCommandItem['kind'], string> = {
  action: 'Action',
  destination: 'Destination',
}

const OPEN_COMMAND_PALETTE_EVENT = 'labby:open-command-palette'

// ── Mode state (discriminated union) ─────────────────────────────────────────

type PaletteMode =
  | { kind: 'browse' }
  | { kind: 'param_prompt'; service: string; action: CatalogAction }
  | { kind: 'confirmation'; service: string; action: CatalogAction; params: Record<string, unknown> }
  | { kind: 'result'; service: string; action: string; data: unknown }

type PaletteAction =
  | { type: 'BROWSE' }
  | { type: 'PARAM_PROMPT'; service: string; action: CatalogAction }
  | { type: 'CONFIRMATION'; service: string; action: CatalogAction; params: Record<string, unknown> }
  | { type: 'RESULT'; service: string; action: string; data: unknown }

function paletteReducer(state: PaletteMode, action: PaletteAction): PaletteMode {
  switch (action.type) {
    case 'BROWSE':
      return { kind: 'browse' }
    case 'PARAM_PROMPT':
      return { kind: 'param_prompt', service: action.service, action: action.action }
    case 'CONFIRMATION':
      return { kind: 'confirmation', service: action.service, action: action.action, params: action.params }
    case 'RESULT':
      return { kind: 'result', service: action.service, action: action.action, data: action.data }
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
  const abortRef = useRef<AbortController | null>(null)

  // Issue 4: destructure error so we can surface catalog fetch failures
  const { data: catalogServices, isLoading: catalogLoading, error: catalogError } = useCommandCatalog()

  const state = useMemo(() => buildAppCommandState(query), [query])
  const [activeItemId, setActiveItemId] = useState<string | null>(state.activeItemId)

  // Current page: top of page stack, or '' for root browse
  const currentPage = pages[pages.length - 1] ?? ''

  // Catalog items for the current page
  const catalogItems = useMemo<CatalogBrowseItem[]>(() => {
    if (currentPage === '') {
      return buildCatalogServiceItems(catalogServices)
    }
    const svc = catalogServices.find((s) => s.name === currentPage)
    if (!svc) return []
    return buildCatalogActionItems(svc.name, svc.actions)
  }, [currentPage, catalogServices])

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
      // Use performServiceAction (not raw fetch) so that:
      // 1. assertDevPreviewCanRunAction blocks write actions in /dev preview mode
      // 2. devPreviewActionUrl rewrites the URL in preview mode correctly
      // 3. CSRF token is injected via gatewayRequestInit inside performServiceAction
      const url = serviceActionUrl(service)
      const finalParams = action.destructive ? confirmGatewayParams(params) : params
      const data: unknown = await performServiceAction<unknown, ServiceActionError>({
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
      dispatch({ type: 'RESULT', service, action: action.action, data })
      setPages([])
      dispatch({ type: 'BROWSE' })
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
      if (action.destructive) {
        // Zero required params but destructive: show ConfirmDialog before dispatch
        dispatch({ type: 'CONFIRMATION', service: svc.name, action, params: {} })
      } else {
        // Zero required params and non-destructive: dispatch immediately
        void executeAction(svc.name, action, {})
      }
      return
    }

    // 1+ required params: show param prompt (destructive confirmation happens after form fill)
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

    if (mode.action.destructive) {
      dispatch({ type: 'CONFIRMATION', service: mode.service, action: mode.action, params })
    } else {
      void executeAction(mode.service, mode.action, params)
    }
  }

  // ── Confirmation dialog handlers ───────────────────────────────────────────

  function handleConfirmDialogChange(isOpen: boolean) {
    if (!isOpen && mode.kind === 'confirmation') {
      // Cancel returns to param_prompt
      dispatch({ type: 'PARAM_PROMPT', service: mode.service, action: mode.action })
    }
  }

  // ── Render ─────────────────────────────────────────────────────────────────

  const showParamForm = mode.kind === 'param_prompt'

  const confirmDialogState = mode.kind === 'confirmation'
    ? {
        title: `Confirm: ${mode.service} ${mode.action.action}`,
        description: mode.action.description
          ? `${mode.action.description} This action cannot be undone.`
          : `This will execute ${mode.action.action} on ${mode.service}. This cannot be undone.`,
        confirmLabel: 'Confirm',
        destructive: true,
        onConfirm: async () => {
          if (mode.kind !== 'confirmation') return
          await executeAction(mode.service, mode.action, mode.params)
        },
      }
    : null

  const placeholder = currentPage
    ? `Search ${currentPage} actions...`
    : 'Search pages, actions, and operational context...'

  return (
    <>
      <Dialog open={open} onOpenChange={(next) => { if (!next) closePalette() }}>
        <DialogContent
          className="top-[18%] translate-y-0 border-aurora-border-strong bg-aurora-panel-strong p-0 shadow-[var(--aurora-shadow-strong),var(--aurora-highlight-strong)] sm:max-w-2xl"
          showCloseButton={false}
        >
          <DialogHeader className="sr-only">
            <DialogTitle>Command Palette</DialogTitle>
            <DialogDescription>
              Search Labby destinations and actions.
            </DialogDescription>
          </DialogHeader>

          <Command
            shouldFilter={false}
            value={activeItemId ?? undefined}
            onValueChange={setActiveItemId}
            onKeyDown={handleCommandKeyDown}
            className="bg-transparent text-aurora-text-primary"
          >
            {/* Page breadcrumb */}
            {pages.length > 0 && (
              <div className="flex items-center gap-1.5 border-b border-aurora-border-strong px-4 py-2 text-[11px] text-aurora-text-muted">
                <span
                  className="cursor-pointer hover:text-aurora-text-primary"
                  onClick={() => { setPages([]); setQuery('') }}
                >
                  Browse
                </span>
                {pages.map((page, idx) => (
                  <span key={page} className="flex items-center gap-1.5">
                    <span>/</span>
                    <span
                      className="cursor-pointer hover:text-aurora-text-primary"
                      onClick={() => { setPages(pages.slice(0, idx + 1)); setQuery('') }}
                    >
                      {page}
                    </span>
                  </span>
                ))}
              </div>
            )}

            {/* Input — shown when not in param form mode */}
            {!showParamForm && (
              <div className="border-b border-aurora-border-strong px-4 py-3">
                <CommandInput
                  autoFocus
                  value={query}
                  onValueChange={setQuery}
                  aria-label="Search command palette"
                  name="app-command-palette-search"
                  placeholder={placeholder}
                  className="text-aurora-text-primary placeholder:text-aurora-text-muted"
                />
              </div>
            )}

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

            {/* Command list — hidden in param form mode */}
            {!showParamForm && (
              <CommandList className="aurora-scrollbar max-h-[420px] px-4 py-4">
                {/* Loading skeleton while catalog is loading */}
                {catalogLoading && currentPage === '' && (
                  <div className="flex items-center justify-center py-8 text-aurora-text-muted">
                    <Loader2 className="size-4 animate-spin" />
                    <span className="ml-2 text-sm">Loading services...</span>
                  </div>
                )}

                {/* Issue 4: surface catalog fetch error instead of silently showing empty list */}
                {catalogError && !catalogLoading && (
                  <CommandGroup>
                    <CommandItem disabled className="text-aurora-error text-xs">
                      Failed to load actions — check server connection
                    </CommandItem>
                  </CommandGroup>
                )}

                {/* Catalog service / action items for current page */}
                {!catalogLoading && !catalogError && catalogItems.length > 0 && (
                  <CommandGroup
                    heading={currentPage ? `${currentPage} actions` : 'Services'}
                    className="mb-3 overflow-visible p-0 [&_[cmdk-group-heading]]:px-0 [&_[cmdk-group-heading]]:py-0 [&_[cmdk-group-heading]]:pb-2 [&_[cmdk-group-heading]]:text-[11px] [&_[cmdk-group-heading]]:font-bold [&_[cmdk-group-heading]]:tracking-[0.18em] [&_[cmdk-group-heading]]:text-aurora-text-muted [&_[cmdk-group-heading]]:uppercase"
                  >
                    <div className="grid gap-2">
                      {catalogItems
                        .filter((item) => {
                          if (!query) return true
                          const q = query.toLowerCase()
                          return item.title.toLowerCase().includes(q) ||
                            item.description.toLowerCase().includes(q)
                        })
                        .map((item) => (
                          <CatalogItemRow
                            key={item.id}
                            item={item}
                            onSelect={() => handleCatalogItemSelect(item)}
                          />
                        ))}
                    </div>
                  </CommandGroup>
                )}

                {/* Static destination/action items — shown on root page only */}
                {currentPage === '' && (
                  state.items.length > 0 ? (
                    state.groups.map((group) => (
                      <CommandGroup
                        key={group.key}
                        heading={group.label}
                        className="mb-3 overflow-visible p-0 [&_[cmdk-group-heading]]:px-0 [&_[cmdk-group-heading]]:py-0 [&_[cmdk-group-heading]]:pb-2 [&_[cmdk-group-heading]]:text-[11px] [&_[cmdk-group-heading]]:font-bold [&_[cmdk-group-heading]]:tracking-[0.18em] [&_[cmdk-group-heading]]:text-aurora-text-muted [&_[cmdk-group-heading]]:uppercase"
                      >
                        <div className="grid gap-2">
                          {group.items.map((item) => (
                            <AppCommandPaletteRow
                              key={item.id}
                              item={item}
                              active={item.id === activeItemId}
                              onSelect={(itemId) => {
                                executeDestination(findAppCommandItemById(itemId, state.items))
                              }}
                            />
                          ))}
                        </div>
                      </CommandGroup>
                    ))
                  ) : (
                    !catalogLoading && !query && catalogItems.length === 0 ? null : (
                      <CommandEmpty className="rounded-aurora-2 border border-aurora-border-strong bg-aurora-control-surface px-5 py-6 text-left">
                        <p className="text-sm font-semibold text-aurora-text-primary">
                          No matching commands
                        </p>
                        <p className="mt-2 text-sm text-aurora-text-muted">
                          Try gateway, logs, setup, registry, marketplace, or settings.
                        </p>
                      </CommandEmpty>
                    )
                  )
                )}

                {/* Empty state on action page */}
                {currentPage !== '' && !catalogLoading && catalogItems.length === 0 && (
                  <CommandEmpty className="rounded-aurora-2 border border-aurora-border-strong bg-aurora-control-surface px-5 py-6 text-left">
                    <p className="text-sm font-semibold text-aurora-text-primary">
                      No actions found
                    </p>
                    <p className="mt-2 text-sm text-aurora-text-muted">
                      No actions available for {currentPage}.
                    </p>
                  </CommandEmpty>
                )}
              </CommandList>
            )}
          </Command>
        </DialogContent>
      </Dialog>

      {/* Confirmation dialog — rendered alongside (not inside) Command to avoid modal focus-trap nesting */}
      <ConfirmDialog
        state={confirmDialogState}
        onOpenChange={handleConfirmDialogChange}
      />
    </>
  )
}

// ── CatalogItemRow ─────────────────────────────────────────────────────────────

type CatalogItemRowProps = {
  item: CatalogBrowseItem
  onSelect: () => void
}

function CatalogItemRow({ item, onSelect }: CatalogItemRowProps) {
  return (
    <CommandItem
      value={item.id}
      onSelect={onSelect}
      className="rounded-aurora-2 border border-aurora-border-strong/80 bg-aurora-control-surface px-3 py-3 text-aurora-text-primary transition-[border-color,background-color,box-shadow] hover:bg-aurora-hover-bg"
    >
      <div className="grid min-w-0 flex-1 gap-1">
        <div className="flex min-w-0 items-center gap-2">
          <span className="truncate text-[13px] font-semibold leading-[1.2] text-aurora-text-primary">
            {item.title}
          </span>
          {item.destructive && (
            <Badge
              variant="pill"
              status="error"
              className="border-aurora-border-strong bg-aurora-panel-medium text-[11px]"
            >
              Destructive
            </Badge>
          )}
        </div>
        <span className="truncate text-[12px] leading-[1.45] text-aurora-text-muted">
          {item.description}
        </span>
      </div>
      <CommandShortcut className="text-[11px] tracking-[0.08em] text-aurora-text-muted">
        {item.kind === 'catalog-service' ? 'Browse' : 'Run'}
      </CommandShortcut>
    </CommandItem>
  )
}

// ── AppCommandPaletteRow ───────────────────────────────────────────────────────

type AppCommandPaletteRowProps = {
  item: AppCommandItem
  active: boolean
  onSelect: (itemId: string) => void
}

function AppCommandPaletteRow({
  item,
  active,
  onSelect,
}: AppCommandPaletteRowProps) {
  const Icon = ICONS[item.icon]

  return (
    <CommandItem
      value={item.id}
      keywords={item.keywords}
      onSelect={() => onSelect(item.id)}
      className={cn(
        'rounded-aurora-2 border border-aurora-border-strong/80 bg-aurora-control-surface px-3 py-3 text-aurora-text-primary transition-[border-color,background-color,box-shadow] hover:bg-aurora-hover-bg',
        active
          ? 'border-aurora-accent-primary/40 bg-aurora-panel-medium shadow-[var(--aurora-active-glow)]'
          : '',
      )}
    >
      <div className="flex size-9 items-center justify-center rounded-aurora-1 border border-aurora-border-strong bg-aurora-panel-medium text-aurora-accent-strong">
        <Icon className="size-4" />
      </div>
      <div className="grid min-w-0 flex-1 gap-1">
        <div className="flex min-w-0 items-center gap-2">
          <span className="truncate text-[13px] font-semibold leading-[1.2] text-aurora-text-primary">
            {item.title}
          </span>
          <Badge
            variant="pill"
            status={item.kind === 'action' ? 'success' : 'default'}
            className="border-aurora-border-strong bg-aurora-panel-medium text-[11px] text-aurora-text-muted"
          >
            {KIND_LABELS[item.kind]}
          </Badge>
        </div>
        <span className="truncate text-[12px] leading-[1.45] text-aurora-text-muted">
          {item.description}
        </span>
      </div>
      <CommandShortcut className="text-[11px] tracking-[0.08em] text-aurora-text-muted">
        {item.actionHint}
      </CommandShortcut>
    </CommandItem>
  )
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
    <div className="px-4 py-4">
      <div className="mb-3 text-sm font-semibold text-aurora-text-primary">
        {service} / {action.action}
      </div>
      {action.description && (
        <p className="mb-4 text-[12px] text-aurora-text-muted">{action.description}</p>
      )}

      <form onSubmit={onSubmit} className="space-y-3">
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
                className="text-[12px] text-aurora-accent-primary hover:underline"
                onClick={onToggleAdvanced}
              >
                {showAdvanced
                  ? 'Hide advanced options'
                  : `Show ${optionalParams.length} optional parameter${optionalParams.length > 1 ? 's' : ''}`}
              </button>
            )}
          </>
        )}

        <div className="flex items-center gap-2 pt-2">
          <button
            type="submit"
            disabled={isDispatching}
            className="flex items-center gap-1.5 rounded-aurora-1 bg-aurora-accent-primary px-3 py-1.5 text-[12px] font-semibold text-white transition hover:opacity-90 disabled:opacity-60"
          >
            {isDispatching && <Loader2 className="size-3 animate-spin" />}
            {action.destructive ? 'Continue...' : 'Run'}
          </button>
          <button
            type="button"
            className="rounded-aurora-1 px-3 py-1.5 text-[12px] text-aurora-text-muted transition hover:text-aurora-text-primary"
            onClick={onCancel}
          >
            Cancel
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
    <div className="grid gap-1">
      <label htmlFor={`param-${param.name}`} className="text-[11px] font-semibold text-aurora-text-primary">
        {param.name}
        {param.required && <span className="ml-1 text-red-400">*</span>}
        <span className="ml-2 font-normal text-aurora-text-muted">({param.ty})</span>
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
        className="w-full rounded-aurora-1 border border-aurora-border-strong bg-aurora-control-surface px-2.5 py-1.5 text-[13px] text-aurora-text-primary placeholder:text-aurora-text-muted focus:border-aurora-accent-primary focus:outline-none"
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
