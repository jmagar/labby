'use client'

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { usePathname, useRouter, useSearchParams } from 'next/navigation'
import { Archive, Box, Check, ChevronDown, ChevronRight, Copy, Download, ExternalLink, FileText, Filter, Grid2X2, Link2, List, Loader2, RefreshCw, Search, ShieldCheck, Table2, X } from 'lucide-react'
import { toast } from 'sonner'

import { AppHeader } from '@/components/app-header'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { ConsoleHero } from '@/components/console/console-hero'
import { DashboardPanel } from '@/components/dashboard/panel'
import { LibraryTabs } from '@/components/depot/depot-workspace-pages'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { type DepotArtifact, type DepotStatus } from '@/lib/api/depot-client'
import { controlPlaneAction } from '@/lib/api/artifact-control-client'
import { artifactDescription, artifactExportFilename, artifactId, artifactKind, artifactLabel, collectArtifactKinds, filterArtifacts, serializeArtifact } from './library-model'
import { ARTIFACT_TYPES, ArtifactTypeMark, artifactTypeDefinition } from './artifact-type'

type LibraryState = {
  artifacts: DepotArtifact[]
  cursor?: string
  error?: string
  loading: boolean
  status?: DepotStatus
  total?: number
}

const PAGE_SIZE = 50
type ViewMode = 'table' | 'list' | 'cards'

export function LibraryPageContent() {
  const router = useRouter()
  const pathname = usePathname()
  const searchParams = useSearchParams()
  const selectedId = searchParams.get('artifact')?.trim() ?? ''
  const initialQuery = searchParams.get('q')?.trim() ?? ''
  const [query, setQuery] = useState(initialQuery)
  const [activeQuery, setActiveQuery] = useState(initialQuery)
  const [kind, setKind] = useState(searchParams.get('kind')?.trim().toLocaleLowerCase() || 'all')
  const [state, setState] = useState<LibraryState>({ artifacts: [], loading: true })
  const [detail, setDetail] = useState<DepotArtifact | null>(null)
  const [detailLoading, setDetailLoading] = useState(false)
  const [copied, setCopied] = useState<string>()
  const [view, setViewState] = useState<ViewMode>('table')
  const viewSelectedByUser = useRef(false)
  const setView = useCallback((next: ViewMode) => {
    viewSelectedByUser.current = true
    setViewState(next)
  }, [])

  useEffect(() => {
    const media = window.matchMedia('(max-width: 640px)')
    const applyResponsiveDefault = () => {
      if (!viewSelectedByUser.current) setViewState(media.matches ? 'cards' : 'table')
    }
    applyResponsiveDefault()
    media.addEventListener('change', applyResponsiveDefault)
    return () => media.removeEventListener('change', applyResponsiveDefault)
  }, [])

  const updateUrl = useCallback((values: { artifact?: string | null; kind?: string; q?: string }) => {
    const params = new URLSearchParams(window.location.search)
    for (const [key, value] of Object.entries(values)) {
      if (value && value !== 'all') params.set(key, value)
      else params.delete(key)
    }
    router.replace(`${pathname}${params.size ? `?${params}` : ''}`, { scroll: false })
  }, [pathname, router])

  const load = useCallback(async (search: string, cursor?: string, signal?: AbortSignal) => {
    setState((current) => ({ ...current, loading: true, error: undefined, artifacts: cursor ? current.artifacts : [] }))
    try {
      const response = await controlPlaneAction<{ artifacts?: DepotArtifact[]; nextCursor?: string; total?: number }>(
        'artifacts',
        'artifacts.list_remote',
        { limit: PAGE_SIZE, ...(search ? { query: search } : {}), ...(cursor ? { cursor } : {}) },
        signal,
      )
      const status: DepotStatus = { configured: true, enabled: true, mutationAuthority: false, maxResponseBytes: 1_048_576 }
      setState((current) => ({
        artifacts: cursor ? [...current.artifacts, ...(response.artifacts ?? [])] : (response.artifacts ?? []),
        cursor: response.nextCursor,
        loading: false,
        status,
        total: response.total,
      }))
    } catch (error) {
      if (!signal?.aborted) setState((current) => ({ ...current, error: error instanceof Error ? error.message : String(error), loading: false }))
    }
  }, [])

  useEffect(() => {
    const controller = new AbortController()
    const timer = window.setTimeout(() => {
      const next = query.trim()
      setActiveQuery(next)
      updateUrl({ artifact: null, q: next })
      void load(next, undefined, controller.signal)
    }, query ? 300 : 0)
    return () => { window.clearTimeout(timer); controller.abort() }
  }, [load, query, updateUrl])

  useEffect(() => {
    if (!selectedId) { setDetail(null); return }
    const controller = new AbortController()
    setDetailLoading(true)
    void controlPlaneAction<{ artifact?: DepotArtifact }>('artifacts', 'artifacts.get_remote', { id: selectedId }, controller.signal)
      .then((response) => setDetail(response.artifact ?? null))
      .catch((error) => { if (!controller.signal.aborted) toast.error(error instanceof Error ? error.message : String(error)) })
      .finally(() => { if (!controller.signal.aborted) setDetailLoading(false) })
    return () => controller.abort()
  }, [selectedId])

  const kinds = useMemo(() => collectArtifactKinds(state.artifacts), [state.artifacts])
  const visible = useMemo(() => filterArtifacts(state.artifacts, kind), [kind, state.artifacts])
  const copy = useCallback(async (label: string, value: string) => {
    await navigator.clipboard.writeText(value)
    setCopied(label)
    toast.success(`${label} copied`)
    window.setTimeout(() => setCopied((current) => current === label ? undefined : current), 1_500)
  }, [])
  const exportArtifact = useCallback((artifact: DepotArtifact) => {
    const url = URL.createObjectURL(new Blob([serializeArtifact(artifact)], { type: 'application/json' }))
    const anchor = document.createElement('a')
    anchor.href = url
    anchor.download = artifactExportFilename(artifact)
    anchor.click()
    URL.revokeObjectURL(url)
    toast.success('Artifact metadata exported')
  }, [])
  const shareArtifact = useCallback(async () => {
    await copy('Share link', window.location.href)
  }, [copy])

  return <>
    <AppHeader breadcrumbs={[{ label: 'Depot' }, { label: 'Library' }]} />
    <div className={`${AURORA_PAGE_SHELL} flex-1`}><div className={`${AURORA_PAGE_FRAME} space-y-4`}>
      <LibraryTabs active="artifacts" />
      <ConsoleHero eyebrow="Depot · Library" title="Library" pulse={{ color: state.status?.enabled ? 'var(--aurora-success)' : 'var(--aurora-warn)', label: state.status?.enabled ? 'live catalog' : 'Depot unavailable' }} actions={<div className="flex flex-wrap gap-2"><Button variant="outline" size="sm" asChild><a href="/depot"><Search className="size-4"/>Discover</a></Button><Button variant="outline" size="sm" disabled={state.loading} onClick={() => void load(activeQuery)}>{state.loading ? <Loader2 className="size-4 animate-spin"/> : <RefreshCw className="size-4"/>}Refresh</Button></div>} stats={[
        { label: activeQuery ? 'Matches' : 'Published artifacts', value: state.total ?? '—', icon: <Archive size={12}/> },
        { label: 'Loaded', value: state.artifacts.length, icon: <Box size={12}/> },
        { label: 'Kinds loaded', value: kinds.length, icon: <FileText size={12}/> },
        { label: 'Authority', value: state.status?.authority === 'write' ? 'Read + write' : state.status?.authority === 'read' ? 'Read only' : 'Unknown', icon: <ShieldCheck size={12}/> },
      ]}/>
      {state.error ? <DashboardPanel title="Depot unavailable"><p role="alert" className="text-sm text-aurora-error">{state.error}. Refresh after Depot is connected.</p></DashboardPanel> : null}
      <DashboardPanel title="Artifacts" icon={<Box className="size-4"/>} action={<div className="flex flex-wrap items-center justify-end gap-2"><div className="relative"><Search className="absolute left-3 top-1/2 size-4 -translate-y-1/2 text-aurora-text-muted"/><Input aria-label="Search library" className="h-9 w-[min(22rem,52vw)] pl-9 pr-9" placeholder="Search the full Depot catalog…" value={query} onChange={(event) => setQuery(event.target.value)}/>{query ? <button type="button" aria-label="Clear library search" className="absolute right-2 top-1/2 -translate-y-1/2 p-1 text-aurora-text-muted" onClick={() => setQuery('')}><X className="size-4"/></button> : null}</div><Popover><PopoverTrigger asChild><Button variant="outline" size="sm" aria-label="Filter library by artifact type" className={kind !== 'all' ? 'border-aurora-accent-primary text-aurora-text-primary' : ''}><Filter className="size-3.5"/>{kind === 'all' ? 'Filters' : artifactTypeDefinition(kind).label}<ChevronDown className="size-3.5"/></Button></PopoverTrigger><PopoverContent align="end" className="w-64 p-2"><div className="px-2 pb-2 pt-1"><p className="text-xs font-semibold text-aurora-text-primary">Artifact type</p><p className="mt-0.5 text-[11px] text-aurora-text-muted">Show one catalog family at a time.</p></div><button type="button" onClick={() => { setKind('all'); updateUrl({ kind: 'all' }) }} aria-pressed={kind === 'all'} className="flex w-full items-center gap-2 rounded-aurora-1 px-2 py-2 text-left text-xs text-aurora-text-muted hover:bg-aurora-hover-bg aria-pressed:bg-aurora-selected-bg aria-pressed:text-aurora-text-primary"><span className="grid size-7 place-items-center rounded-aurora-1 border border-aurora-border-subtle"><Box className="size-3.5"/></span><span className="flex-1 font-semibold">All artifacts</span>{kind === 'all' ? <Check className="size-4 text-aurora-accent-primary"/> : null}</button>{ARTIFACT_TYPES.map((item) => { const definition = artifactTypeDefinition(item); const Icon = definition.icon; return <button key={item} type="button" onClick={() => { setKind(item); updateUrl({ kind: item }) }} aria-pressed={kind === item} className="flex w-full items-center gap-2 rounded-aurora-1 px-2 py-2 text-left text-xs text-aurora-text-muted hover:bg-aurora-hover-bg aria-pressed:bg-aurora-selected-bg aria-pressed:text-aurora-text-primary"><span className="grid size-7 place-items-center rounded-aurora-1 border" style={{ color: definition.color, borderColor: `color-mix(in srgb, ${definition.color} 38%, transparent)` }}><Icon className="size-3.5"/></span><span className="flex-1 font-semibold">{definition.label}</span>{kind === item ? <Check className="size-4 text-aurora-accent-primary"/> : null}</button> })}</PopoverContent></Popover><div className="hidden rounded-aurora-1 border border-aurora-border-subtle bg-aurora-control-surface p-0.5 sm:flex">{([[Table2,'Table','table'],[List,'List','list'],[Grid2X2,'Cards','cards']] as const).map(([Icon,label,mode]) => <button key={mode} type="button" aria-label={`${label} view`} title={`${label} view`} aria-pressed={view === mode} onClick={() => setView(mode)} className="rounded p-1.5 text-aurora-text-muted transition-colors hover:text-aurora-text-primary aria-pressed:bg-aurora-selected-bg aria-pressed:text-aurora-accent-primary"><Icon className="size-3.5"/></button>)}</div></div>}>
        <div className="mb-3 flex flex-wrap items-center gap-2 border-b border-aurora-border-subtle pb-3">
          <span className="text-xs font-semibold text-aurora-text-primary">{kind === 'all' ? 'All artifact types' : artifactTypeDefinition(kind).label}</span>
          {kind !== 'all' ? <button type="button" onClick={() => { setKind('all'); updateUrl({ kind: 'all' }) }} className="inline-flex items-center gap-1 rounded-full border border-aurora-border-subtle px-2 py-1 text-[11px] text-aurora-text-muted hover:text-aurora-text-primary">Clear filter<X className="size-3"/></button> : null}
          <span className="ml-auto text-xs text-aurora-text-muted">{visible.length} shown · {state.artifacts.length} loaded · {state.total ?? 0} catalog total</span>
        </div>
        {view !== 'table' ? <div className={view === 'cards' ? 'grid gap-3 sm:grid-cols-2 xl:grid-cols-3' : 'divide-y divide-aurora-border-subtle'}>{visible.map((artifact) => { const id = artifactId(artifact); return <button key={id} type="button" onClick={() => updateUrl({ artifact: id })} className={view === 'cards' ? 'group rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-low p-4 text-left transition-[transform,border-color] hover:-translate-y-0.5 hover:border-aurora-border-strong' : 'group flex w-full items-start gap-3 px-3 py-3 text-left transition-colors hover:bg-aurora-surface-muted'}><ArtifactTypeMark artifact={artifact} compact/><span className="min-w-0 flex-1"><span className="block truncate font-semibold text-aurora-text-primary">{artifactLabel(artifact)}</span><span className="mt-1 line-clamp-2 block text-xs leading-5 text-aurora-text-muted">{artifactDescription(artifact)}</span><span className="mt-2 block truncate text-[11px] text-aurora-text-muted">{artifact.namespace ?? artifact.descriptor?.namespace ?? 'Unknown namespace'}</span></span><ChevronRight className="mt-1 size-4 shrink-0 text-aurora-text-muted group-hover:text-aurora-accent-primary"/></button> })}</div> : <div className="overflow-x-auto"><table className="w-full min-w-[700px] text-left text-sm"><thead><tr className="border-b border-aurora-border-subtle text-[11px] uppercase tracking-[.14em] text-aurora-text-muted"><th className="px-3 py-2">Kind</th><th className="px-3 py-2">Artifact</th><th className="px-3 py-2">Namespace</th><th className="px-3 py-2">Visibility</th><th className="px-3 py-2"><span className="sr-only">Open</span></th></tr></thead><tbody>
          {visible.map((artifact) => { const id = artifactId(artifact); return <tr key={id} tabIndex={0} role="link" aria-label={`Inspect ${artifactLabel(artifact)}`} onClick={() => updateUrl({ artifact: id })} onKeyDown={(event) => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); updateUrl({ artifact: id }) } }} className="cursor-pointer border-b border-aurora-border-subtle/70 transition-colors hover:bg-aurora-surface-muted focus-visible:bg-aurora-surface-muted focus-visible:outline-none"><td className="px-3 py-3"><ArtifactTypeMark artifact={artifact} compact/></td><td className="max-w-xl px-3 py-3"><div className="font-semibold text-aurora-text-primary">{artifactLabel(artifact)}</div><div className="line-clamp-1 text-xs text-aurora-text-muted">{artifactDescription(artifact)}</div></td><td className="px-3 py-3 text-aurora-text-muted">{artifact.namespace ?? artifact.descriptor?.namespace ?? '—'}</td><td className="px-3 py-3"><Badge variant="outline">{artifact.publication?.visibility ?? 'unknown'}</Badge></td><td className="px-3 py-3"><ChevronRight className="size-4 text-aurora-text-muted"/></td></tr> })}
        </tbody></table></div>}
        {state.loading && state.artifacts.length === 0 ? <p className="flex items-center justify-center gap-2 py-10 text-sm text-aurora-text-muted"><Loader2 className="size-4 animate-spin"/>Loading the Depot catalog…</p> : null}
        {!state.loading && visible.length === 0 ? <p className="py-10 text-center text-sm text-aurora-text-muted">No loaded artifacts match this view.</p> : null}
        {state.cursor ? <div className="pt-4 text-center"><Button variant="outline" disabled={state.loading} onClick={() => void load(activeQuery, state.cursor)}>{state.loading ? <Loader2 className="size-4 animate-spin"/> : null}Load 50 more</Button></div> : null}
      </DashboardPanel>
    </div></div>
    <Sheet open={Boolean(selectedId)} onOpenChange={(open) => { if (!open) updateUrl({ artifact: null }) }}><SheetContent className="!w-[min(92vw,620px)] min-w-0 overflow-x-hidden overflow-y-auto border-aurora-border-subtle bg-aurora-panel-medium sm:!max-w-[620px]"><SheetHeader className="min-w-0 border-b border-aurora-border-subtle pr-10"><SheetTitle className="break-words">{detail ? artifactLabel(detail) : 'Artifact details'}</SheetTitle><SheetDescription className="break-all">{detail ? `${detail.namespace ?? detail.descriptor?.namespace ?? 'unknown'} · ${artifactKind(detail)}` : selectedId}</SheetDescription></SheetHeader>
      {detailLoading ? <p className="flex items-center gap-2 p-5 text-sm text-aurora-text-muted"><Loader2 className="size-4 animate-spin"/>Loading artifact…</p> : detail ? <div className="min-w-0 space-y-5 p-5"><div className="flex flex-wrap gap-2"><Button size="sm" onClick={() => void shareArtifact()}><Link2 className="size-4"/>{copied === 'Share link' ? 'Link copied' : 'Copy link'}</Button><Button size="sm" variant="outline" onClick={() => exportArtifact(detail)}><Download className="size-4"/>Export JSON</Button><Button size="sm" variant="outline" asChild><a href={`/depot?artifact=${encodeURIComponent(artifactId(detail))}`}><ExternalLink className="size-4"/>Open in Discover</a></Button></div><p className="break-words text-sm leading-6 text-aurora-text-muted">{artifactDescription(detail)}</p><div className="flex flex-wrap gap-2"><Badge variant="outline" className="capitalize">{artifactKind(detail)}</Badge><Badge variant="outline">{detail.publication?.state ?? 'unknown state'}</Badge><Badge variant="outline">{detail.publication?.visibility ?? 'unknown visibility'}</Badge></div><dl className="grid grid-cols-2 gap-px overflow-hidden rounded-aurora-1 border border-aurora-border-subtle bg-aurora-border-subtle">{([
        ['Distribution', detail.publication?.distribution], ['License review', detail.license?.reviewState], ['Redistribution', detail.license?.redistribution], ['Revisions', detail.revisionCount?.toString()],
      ] satisfies Array<[string, string | undefined]>).map(([label, value]) => <div key={label} className="bg-aurora-panel-low p-3"><dt className="text-[10px] font-bold uppercase tracking-wider text-aurora-text-muted">{label}</dt><dd className="mt-1 text-sm text-aurora-text-primary">{value || 'Not supplied'}</dd></div>)}</dl>{([
        ['Artifact ID', artifactId(detail)], ['Revision ID', detail.currentRevisionId ?? detail.currentRevision?.id], ['Content digest', detail.contentDigest ?? detail.currentRevision?.contentDigest],
      ] satisfies Array<[string, string | undefined]>).map(([label, value]) => value ? <div key={label} className="flex items-center gap-2 rounded-aurora-1 border border-aurora-border-subtle bg-aurora-panel-low p-3"><div className="min-w-0 flex-1"><div className="text-[10px] font-bold uppercase tracking-wider text-aurora-text-muted">{label}</div><code className="block truncate pt-1 text-xs text-aurora-text-primary" title={value}>{value}</code></div><Button variant="ghost" size="icon-sm" aria-label={`Copy ${label}`} onClick={() => void copy(label, value)}>{copied === label ? <Check className="size-4 text-aurora-success"/> : <Copy className="size-4"/>}</Button></div> : null)}{detail.currentRevision?.components?.length ? <div><h3 className="mb-2 text-sm font-semibold text-aurora-text-primary">Components</h3><div className="divide-y divide-aurora-border-subtle rounded-aurora-1 border border-aurora-border-subtle">{detail.currentRevision.components.map((component, index) => <div key={component.id ?? index} className="flex justify-between gap-3 p-3 text-xs"><code className="truncate">{component.path ?? component.id}</code><span className="shrink-0 text-aurora-text-muted">{component.mediaType ?? component.kind ?? 'file'}</span></div>)}</div></div> : null}</div> : <p className="p-5 text-sm text-aurora-text-muted">Artifact details are unavailable.</p>}
    </SheetContent></Sheet>
  </>
}
