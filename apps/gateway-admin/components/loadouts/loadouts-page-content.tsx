'use client'

import { useMemo, useState } from 'react'
import Link from 'next/link'
import { BookOpen, Boxes, Cable, ChevronDown, Clipboard, Download, Grid2X2, List, Loader2, PackageOpen, Pencil, Plus, RefreshCw, Search, ShieldCheck, Table2, Trash2 } from 'lucide-react'
import { toast } from 'sonner'
import { ActionConfirmationDialog } from '@/components/action-confirmation-dialog'
import { AppHeader } from '@/components/app-header'
import { LibraryTabs } from '@/components/depot/depot-workspace-pages'
import { ConsoleHero, type ConsoleHeroStat } from '@/components/console/console-hero'
import { DashboardPanel } from '@/components/dashboard/panel'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import { AURORA_CARD_TITLE, AURORA_DENSE_META, AURORA_MUTED_LABEL, AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { useGatewaySnapshots, useGatewayMutations, useLoadouts, useProtectedMcpRoutes, useSupportedServices } from '@/lib/hooks/use-gateways'
import type { GatewayLoadout } from '@/lib/types/gateway'
import { cn, getErrorMessage } from '@/lib/utils'
import { LOADOUT_CAPABILITIES, LoadoutFormDialog } from './loadout-form-dialog'
import { portableLoadoutFilename, portableLoadoutSource, type LoadoutExportTarget } from './loadout-portability'
import { useBrowserSession } from '@/lib/auth/session'

export function filterLoadouts(loadouts: GatewayLoadout[], query: string): GatewayLoadout[] {
  const needle = query.trim().toLocaleLowerCase()
  if (!needle) return loadouts
  return loadouts.filter((loadout) => [
    loadout.name,
    loadout.description ?? '',
    ...loadout.upstreams,
    ...loadout.services,
  ].some((value) => value.toLocaleLowerCase().includes(needle)))
}

const EXPORT_TARGETS: Array<[LoadoutExportTarget, string]> = [
  ['apm', 'APM portable bundle'],
  ['claude-code', 'Claude Code target'],
  ['codex', 'Codex target'],
  ['gemini-cli', 'Gemini CLI target'],
]

function downloadLoadout(loadout: GatewayLoadout, target: LoadoutExportTarget) {
  const url = URL.createObjectURL(new Blob([portableLoadoutSource(loadout, target)], { type: 'application/json' }))
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = portableLoadoutFilename(loadout, target)
  anchor.click()
  URL.revokeObjectURL(url)
}

export function LoadoutsPageContent() {
  const session = useBrowserSession()
  const canManage = session.status === 'authenticated'
    && Boolean(session.authority?.capabilities.includes('scope.manage'))
  const [formOpen, setFormOpen] = useState(false)
  const [editing, setEditing] = useState<GatewayLoadout | null>(null)
  const [deleting, setDeleting] = useState<GatewayLoadout | null>(null)
  const [deleteBusy, setDeleteBusy] = useState(false)
  const [query, setQuery] = useState('')
  const [view, setView] = useState<'table' | 'list' | 'cards'>('cards')
  const { data: loadouts = [], isLoading, error, mutate: refreshLoadouts, isValidating } = useLoadouts()
  // Gateway configuration is only needed to populate the add/edit dialog. A full
  // gateway list can cold-connect many stdio upstreams, so do not hydrate the
  // fleet merely to render the Loadouts overview.
  const {
    data: gateways = [],
    isLoading: gatewaysLoading,
    error: gatewaysError,
  } = useGatewaySnapshots(formOpen)
  const { data: services = [] } = useSupportedServices()
  const {
    data: protectedRoutes = [],
    isLoading: protectedRoutesLoading,
    error: protectedRoutesError,
  } = useProtectedMcpRoutes()
  const { addLoadout, patchLoadout, removeLoadout, stageLoadoutUpdate, stageLoadoutRemove } = useGatewayMutations()

  const gatewayOptions = useMemo(() => gateways.filter(g => g.source !== 'in_process' && g.transport !== 'in_process').map(g => ({ value: g.name, label: g.name, meta: g.config.url ?? g.config.command ?? g.transport })), [gateways])
  const serviceOptions = useMemo(() => services.map(s => ({ value: s.key, label: s.display_name, meta: s.description })), [services])
  const mountedBy = useMemo(() => {
    const map = new Map<string, string[]>()
    for (const route of protectedRoutes) {
      const name = route.target?.kind === 'gateway_subset' ? route.target.loadout : undefined
      if (name) map.set(name, [...(map.get(name) ?? []), route.name])
    }
    return map
  }, [protectedRoutes])
  const pendingRestartCount = loadouts.filter((loadout) => loadout.restart_required).length
  const visibleLoadouts = useMemo(() => filterLoadouts(loadouts, query), [loadouts, query])
  const stats: ConsoleHeroStat[] = [
    { label: 'Loadouts', value: isLoading ? '—' : loadouts.length, icon: <Boxes size={12} /> },
    { label: 'MCP server refs', value: isLoading ? '—' : loadouts.reduce((n, x) => n + x.upstreams.length, 0), icon: <Cable size={12} /> },
    { label: 'Plugin refs', value: isLoading ? '—' : loadouts.reduce((n, x) => n + x.services.length, 0), icon: <PackageOpen size={12} /> },
    { label: 'Skills enabled', value: isLoading ? '—' : loadouts.filter(x => x.expose_skills).length, icon: <BookOpen size={12} /> },
  ]

  return <>
    <AppHeader breadcrumbs={[{ label: 'Depot' }, { label: 'Library' }, { label: 'Loadouts' }]} />
    <div className={cn(AURORA_PAGE_SHELL, 'flex-1')}><div className={AURORA_PAGE_FRAME}>
      <LibraryTabs active="loadouts" />
      <ConsoleHero eyebrow="Depot · Portable bundles" pulse={loadouts.length ? { color: 'var(--aurora-success)', label: loadouts.length + ' configured' } : undefined} title="Loadouts" stats={stats} actions={<div className="flex gap-2"><Button variant="outline" size="sm" disabled={isValidating} onClick={() => void refreshLoadouts()}><RefreshCw className={cn('size-4', isValidating && 'animate-spin')} />Refresh</Button>{canManage ? <Button size="sm" onClick={() => { setEditing(null); setFormOpen(true) }}><Plus className="size-4" />New Loadout</Button> : null}</div>} />
      <DashboardPanel title="Curate once, run anywhere" icon={<PackageOpen className="size-4" />} action={<Button variant="outline" size="sm" asChild><Link href="/create">Publish in Depot</Link></Button>}><p className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>Bundle MCP servers and tools, plugins, prompts, resources, Skills, and Code Mode into one portable unit. Export the APM source directly or compile a target request for Claude Code, Codex, or Gemini CLI. Publishing is handled by Depot; a hosted MCP route is optional.</p></DashboardPanel>
      <div className="flex items-center justify-between gap-3"><div className="relative w-full max-w-xl"><Search aria-hidden="true" className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-aurora-text-muted" /><Input aria-label="Search Loadouts" value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search Loadouts, upstreams, and services…" className="pl-9" /></div><div className="flex shrink-0 rounded-aurora-1 border border-aurora-border-subtle bg-aurora-control-surface p-0.5">{([[Table2,'Table','table'],[List,'List','list'],[Grid2X2,'Cards','cards']] as const).map(([Icon,label,mode])=><button key={mode} type="button" aria-label={`${label} view`} title={`${label} view`} aria-pressed={view===mode} onClick={()=>setView(mode)} className="rounded p-1.5 text-aurora-text-muted hover:text-aurora-text-primary aria-pressed:bg-aurora-selected-bg aria-pressed:text-aurora-accent-primary"><Icon className="size-3.5"/></button>)}</div></div>
      {protectedRoutesError && <div role="alert" className="rounded-lg border border-destructive/35 bg-destructive/10 px-3 py-2 text-sm text-aurora-text-primary">Could not verify protected route mounts. Editing and removal are disabled to prevent applying the wrong update mode. {getErrorMessage(protectedRoutesError, 'Protected routes failed to load')}</div>}
      {pendingRestartCount > 0 && <div className="rounded-lg border border-aurora-warning/35 bg-aurora-warning/10 px-3 py-2 text-sm text-aurora-text-primary">{pendingRestartCount} Loadout change{pendingRestartCount === 1 ? ' is' : 's are'} saved for restart. Running protected routes still use their startup projections.</div>}
      {isLoading ? <DashboardPanel title="Loadouts" icon={<Loader2 className="size-4 animate-spin" />}><span className={AURORA_MUTED_LABEL}>Loading Loadouts…</span></DashboardPanel>
      : error ? <DashboardPanel title="Loadouts" icon={<ShieldCheck className="size-4 text-destructive" />}><span className={AURORA_CARD_TITLE}>Could not load Loadouts</span><p className={AURORA_DENSE_META}>{getErrorMessage(error, 'Gateway Loadout request failed')}</p></DashboardPanel>
      : loadouts.length === 0 ? <DashboardPanel title="Loadouts" icon={<Boxes className="size-4" />}><span className={AURORA_CARD_TITLE}>No Loadouts configured</span><p className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>Add a Loadout to create a reusable gateway capability projection for protected MCP routes.</p></DashboardPanel>
      : visibleLoadouts.length === 0 ? <DashboardPanel title="No matching Loadouts" icon={<Search className="size-4" />}><p className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>No names, descriptions, upstreams, or services match “{query.trim()}”.</p></DashboardPanel>
      : <div className={cn('grid gap-3', view === 'cards' && 'xl:grid-cols-2', view === 'table' && 'gap-1')}>{visibleLoadouts.map(loadout => {
        const mounts = mountedBy.get(loadout.name) ?? []
        const caps = LOADOUT_CAPABILITIES.filter(([key]) => loadout[key])
        const routeStateUnavailable = !canManage || protectedRoutesLoading || Boolean(protectedRoutesError)
        return <DashboardPanel key={loadout.name} title={loadout.name} icon={<Boxes className="size-4" />} meta={loadout.restart_required ? 'restart required' : `${loadout.upstreams.length + loadout.services.length} portable references`} action={<div className="flex gap-1"><Button variant="ghost" size="icon-sm" aria-label={'Copy ' + loadout.name + ' APM source'} onClick={async () => { try { await navigator.clipboard.writeText(portableLoadoutSource(loadout)); toast.success('Copied ' + loadout.name + ' APM source.') } catch (e) { toast.error(getErrorMessage(e, 'Could not copy Loadout source')) } }}><Clipboard className="size-3.5" /></Button><DropdownMenu><DropdownMenuTrigger asChild><Button variant="ghost" size="sm" aria-label={'Export ' + loadout.name}><Download className="size-3.5" />Export<ChevronDown className="size-3" /></Button></DropdownMenuTrigger><DropdownMenuContent align="end">{EXPORT_TARGETS.map(([target, label]) => <DropdownMenuItem key={target} onSelect={() => downloadLoadout(loadout, target)}><Download />{label}</DropdownMenuItem>)}</DropdownMenuContent></DropdownMenu><Button variant="ghost" size="icon-sm" aria-label={'Edit ' + loadout.name} disabled={routeStateUnavailable || loadout.pending_operation === 'remove'} onClick={() => { setEditing(loadout); setFormOpen(true) }}><Pencil className="size-3.5" /></Button><Button variant="ghost" size="icon-sm" aria-label={'Remove ' + loadout.name} disabled={routeStateUnavailable || loadout.pending_operation === 'remove'} onClick={() => setDeleting(loadout)}><Trash2 className="size-3.5" /></Button></div>}>
          {loadout.description && <p className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>{loadout.description}</p>}
          <div className="flex flex-wrap gap-2">{loadout.restart_required && <Badge variant="outline" className="border-aurora-warning/50 text-aurora-warning">Restart · {loadout.pending_operation ?? 'update'}</Badge>}{caps.map(([key, label, , Icon]) => <Badge key={key} variant="secondary" className="gap-1"><Icon className="size-3" />{label}</Badge>)}</div>
          <div className="grid gap-3 sm:grid-cols-2"><div><p className={AURORA_MUTED_LABEL}>MCP servers and their tools</p><div className="mt-1 flex flex-wrap gap-1.5">{loadout.upstreams.length ? loadout.upstreams.map(name => <Badge key={name} variant="outline">{name}</Badge>) : <span className={AURORA_DENSE_META}>None</span>}</div></div><div><p className={AURORA_MUTED_LABEL}>Lab plugins</p><div className="mt-1 flex flex-wrap gap-1.5">{loadout.services.length ? loadout.services.map(name => <Badge key={name} variant="outline">{name}</Badge>) : <span className={AURORA_DENSE_META}>None</span>}</div></div></div>
          <div className="flex items-center justify-between gap-3 border-t border-aurora-border-subtle pt-2"><span className={AURORA_DENSE_META}>APM portable · Claude Code · Codex · Gemini CLI</span>{mounts.length > 0 ? <span className={AURORA_DENSE_META}>Also mounted on {mounts.length} route{mounts.length === 1 ? '' : 's'}</span> : null}</div>
        </DashboardPanel>
      })}</div>}
      <DashboardPanel title="Optional hosted compatibility" icon={<ShieldCheck className="size-4" />} action={<Button variant="outline" size="sm" asChild><Link href="/gateways">Manage routes</Link></Button>}><p className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>Need a stable OAuth-protected MCP URL for an existing client? A Loadout can also back a protected Labby route. This is an optional distribution target, not a requirement for composing, sharing, or exporting a Loadout.</p></DashboardPanel>
    </div></div>
    <LoadoutFormDialog open={formOpen} loadout={editing} gatewayOptions={gatewayOptions} gatewayOptionsLoading={gatewaysLoading} gatewayOptionsError={gatewaysError ? getErrorMessage(gatewaysError, 'Gateway options failed to load') : null} serviceOptions={serviceOptions} onOpenChange={setFormOpen} onSave={async (original, draft) => { if (original) { const current = loadouts.find((loadout) => loadout.name === original); const mounted = (mountedBy.get(original)?.length ?? 0) > 0; if (mounted || current?.restart_required) { await stageLoadoutUpdate(original, draft); toast.success('Loadout ' + draft.name + ' saved. Restart Labby to apply it to mounted routes.') } else { await patchLoadout(original, draft); toast.success('Loadout ' + draft.name + ' updated.') } } else { await addLoadout(draft); toast.success('Loadout ' + draft.name + ' added.') } }} />
    <ActionConfirmationDialog open={deleting !== null} title="Remove Loadout?" description={deleting ? 'Remove ' + deleting.name + '? If a running protected route still uses it, first stage that route away from this Loadout; the Loadout removal can then be staged for the same restart.' : ''} confirmLabel="Remove Loadout" busy={deleteBusy} onOpenChange={open => !open && setDeleting(null)} onConfirm={async () => { if (!deleting) return; setDeleteBusy(true); try { const mounted = (mountedBy.get(deleting.name)?.length ?? 0) > 0; if (mounted || deleting.restart_required) { await stageLoadoutRemove(deleting.name); toast.success('Loadout removal saved. Restart Labby to apply it.') } else { await removeLoadout(deleting.name); toast.success('Loadout ' + deleting.name + ' removed.') } setDeleting(null) } catch (e) { toast.error(getErrorMessage(e, 'Failed to remove Loadout')) } finally { setDeleteBusy(false) } }} />
  </>
}
