'use client'

import { useCallback, useEffect, useRef, useState } from 'react'
import { Copy, Download, File, FileArchive, FileCode2, FileJson, FileText, Grid2X2, Inbox, List, Pencil, RefreshCw, Search, Share2, Trash2, Upload, X } from 'lucide-react'

import { ActionConfirmationDialog } from '@/components/action-confirmation-dialog'
import { ConsoleHero } from '@/components/console/console-hero'
import { DashboardPanel } from '@/components/dashboard/panel'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import * as api from '@/lib/stash/client'
import { StashError } from '@/lib/stash/client'
import type { StashFile, StashGrant, StashStats } from '@/lib/stash/types'
import { acceptGeneration, acceptGrantPage, acceptRecipientSearch, copyUri, mergeFiles, mergeGrants, selectedRecipientId } from '@/lib/stash/view-state'

type ViewMode = 'list' | 'grid'
type UploadState = { id: string; file: globalThis.File; status: 'pending' | 'uploading' | 'failed' | 'complete' | 'canceled'; abort?: AbortController; detail?: string }
const MAX_QUEUED_UPLOADS = 8
const UPLOAD_WORKERS = 2

const EMPTY_STATS: StashStats = { owned_file_count: 0, owned_shared_file_count: 0, owned_committed_bytes: 0, owned_reserved_bytes: 0 }

function bytes(value: number): string {
  if (value < 1024) return `${value} B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let size = value / 1024
  let unit = 0
  while (size >= 1024 && unit < units.length - 1) { size /= 1024; unit += 1 }
  return `${size >= 10 ? size.toFixed(0) : size.toFixed(1)} ${units[unit]}`
}

function errorCopy(error: unknown): { title: string; detail: string } {
  if (!(error instanceof StashError)) return { title: 'Stash is offline', detail: 'The service did not respond. Check the Labby connection and retry.' }
  if (error.kind === 'quota_exceeded' || error.status === 413) return { title: 'Storage quota reached', detail: 'Remove files or ask an administrator to increase your File Stash quota.' }
  if (error.kind === 'busy' || error.status === 429) return { title: 'Stash is busy', detail: 'Another file operation is in progress. Wait a moment and retry.' }
  if (error.kind === 'conflict' || error.status === 409) return { title: 'That name is already used', detail: 'Rename the existing file or choose a different filename.' }
  return { title: 'File operation failed', detail: error.message }
}

function fileIcon(name: string) {
  const ext = name.split('.').pop()?.toLowerCase()
  if (ext === 'json') return <FileJson />
  if (['md', 'txt', 'log'].includes(ext || '')) return <FileText />
  if (['zip', 'gz', 'tar', '7z'].includes(ext || '')) return <FileArchive />
  if (['rs', 'ts', 'tsx', 'js', 'py', 'prisma'].includes(ext || '')) return <FileCode2 />
  return <File />
}

export function StashPageContent() {
  const [files, setFiles] = useState<StashFile[]>([])
  const [stats, setStats] = useState(EMPTY_STATS)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<unknown>()
  const [query, setQuery] = useState('')
  const [nextCursor, setNextCursor] = useState<string | null>(null)
  const [loadingMore, setLoadingMore] = useState(false)
  const [view, setView] = useState<ViewMode>('list')
  const [uploads, setUploads] = useState<UploadState[]>([])
  const [deleteTarget, setDeleteTarget] = useState<StashFile>()
  const [busyDelete, setBusyDelete] = useState(false)
  const [deleteError, setDeleteError] = useState<unknown>()
  const [manageTarget, setManageTarget] = useState<StashFile>()
  const [announcement, setAnnouncement] = useState('')
  const input = useRef<HTMLInputElement>(null)
  const generation = useRef(0)
  const loadAbort = useRef<AbortController | null>(null)
  const uploadControllers = useRef(new Map<string, AbortController>())
  const uploadSequence = useRef(0)
  const mounted = useRef(true)
  const uploadBatchDirty = useRef(false)
  const statsLoaded = useRef(false)
  const queryRef = useRef('')
  queryRef.current = query

  const load = useCallback(async (search = '', cursor?: string, refreshStats = false) => {
    const current = ++generation.current
    if (cursor) setLoadingMore(true); else setLoading(true)
    setError(undefined)
    loadAbort.current?.abort()
    const controller = new AbortController()
    loadAbort.current = controller
    try {
      const pageRequest = api.listFiles(cursor, controller.signal, search || undefined)
      const statsRequest = refreshStats || !statsLoaded.current ? api.getStats(controller.signal) : undefined
      const [page, nextStats] = await Promise.all([pageRequest, statsRequest])
      if (acceptGeneration(generation.current, current)) {
        setFiles(previous => mergeFiles(previous, page.files, Boolean(cursor)))
        setNextCursor(page.next_cursor)
        if (nextStats) { setStats(nextStats); statsLoaded.current = true }
      }
    } catch (reason) {
      if (acceptGeneration(generation.current, current) && !(reason instanceof DOMException && reason.name === 'AbortError')) setError(reason)
    } finally { if (acceptGeneration(generation.current, current)) { setLoading(false); setLoadingMore(false); loadAbort.current = null } }
  }, [])

  useEffect(() => { const timer = window.setTimeout(() => void load(query.trim()), 250); return () => { window.clearTimeout(timer); generation.current += 1; loadAbort.current?.abort() } }, [load, query])

  useEffect(() => {
    mounted.current = true
    const controllers = uploadControllers.current
    return () => {
      mounted.current = false
      for (const controller of controllers.values()) controller.abort()
      controllers.clear()
    }
  }, [])

  const runUpload = useCallback(async (item: UploadState, abort: AbortController) => {
    try {
      await api.uploadFile(item.file, abort.signal)
      if (!mounted.current) return
      uploadBatchDirty.current = true
      setUploads(current => current.map(value => value.id === item.id ? { ...value, status: 'complete', abort: undefined } : value))
      setAnnouncement(`${item.file.name} uploaded.`)
    } catch (reason) {
      if (!mounted.current) return
      const canceled = reason instanceof DOMException && reason.name === 'AbortError'
      const detail = canceled ? 'Canceled' : errorCopy(reason).detail
      setUploads(current => current.map(value => value.id === item.id ? { ...value, status: canceled ? 'canceled' : 'failed', abort: undefined, detail } : value))
      setAnnouncement(canceled ? `${item.file.name} upload canceled.` : `${item.file.name} upload failed.`)
    } finally {
      uploadControllers.current.delete(item.id)
    }
  }, [])

  useEffect(() => {
    const availableWorkers = UPLOAD_WORKERS - uploads.filter(item => item.status === 'uploading').length
    if (availableWorkers <= 0) return
    const starting = uploads.filter(item => item.status === 'pending').slice(0, availableWorkers)
    if (!starting.length) return
    const controllers = new Map(starting.map(item => [item.id, new AbortController()]))
    for (const [id, controller] of controllers) uploadControllers.current.set(id, controller)
    setUploads(current => current.map(item => controllers.has(item.id)
      ? { ...item, status: 'uploading', abort: controllers.get(item.id), detail: undefined }
      : item))
    for (const item of starting) void runUpload(item, controllers.get(item.id)!)
  }, [runUpload, uploads])

  useEffect(() => {
    if (!uploadBatchDirty.current || uploads.some(item => item.status === 'pending' || item.status === 'uploading')) return
    uploadBatchDirty.current = false
    void load(queryRef.current.trim(), undefined, true)
  }, [load, uploads])

  const acceptFiles = useCallback((selected: FileList | File[]) => {
    const selectedFiles = Array.from(selected)
    if (!selectedFiles.length) return
    const announcedAvailable = MAX_QUEUED_UPLOADS - uploads.filter(item => item.status === 'pending' || item.status === 'uploading').length
    const announcedRejected = Math.max(0, selectedFiles.length - announcedAvailable)
    if (announcedRejected) setAnnouncement(`${announcedRejected} file${announcedRejected === 1 ? '' : 's'} not queued; the upload queue holds ${MAX_QUEUED_UPLOADS}.`)
    setUploads(current => {
      const isActive = (item: UploadState) => item.status === 'pending' || item.status === 'uploading'
      const activeCount = current.filter(isActive).length
      const accepted = selectedFiles.slice(0, Math.max(0, MAX_QUEUED_UPLOADS - activeCount))
      const batch = accepted.map((file): UploadState => {
        const invalid = !file.name.trim() || file.name.includes('/') || file.name.includes('\\')
        return {
          id: `upload-${++uploadSequence.current}`,
          file,
          status: invalid ? 'failed' : 'pending',
          detail: invalid ? 'Filename cannot be empty or contain path separators.' : undefined,
        }
      })
      const terminalSlots = MAX_QUEUED_UPLOADS - activeCount - batch.length
      const terminal = current.filter(item => !isActive(item))
      const retainedTerminal = terminalSlots > 0 ? new Set(terminal.slice(-terminalSlots)) : new Set<UploadState>()
      return [...current.filter(item => isActive(item) || retainedTerminal.has(item)), ...batch]
    })
    if (input.current) input.current.value = ''
  }, [uploads])

  const remove = async () => {
    if (!deleteTarget) return
    setBusyDelete(true); setError(undefined)
    setDeleteError(undefined)
    try {
      await api.deleteFile(deleteTarget.file_id)
      setAnnouncement(`${deleteTarget.display_name} deleted.`); setDeleteTarget(undefined); await load(query.trim(), undefined, true)
    } catch (reason) { setDeleteError(reason) } finally { setBusyDelete(false) }
  }

  const failure = error ? errorCopy(error) : undefined
  return <>
    <span className="sr-only" role="status" aria-live="polite">{announcement}</span>
    <ConsoleHero eyebrow="Workspace · Stash" title="Stash" pulse={{ color: failure ? 'var(--aurora-error)' : 'var(--aurora-success)', label: failure ? 'attention needed' : undefined }} actions={<Button onClick={() => input.current?.click()}><Upload />Upload</Button>} stats={[
      { label: 'Files', value: loading ? '—' : stats.owned_file_count, icon: <Inbox size={12}/> },
      { label: 'Size', value: loading ? '—' : bytes(stats.owned_committed_bytes), icon: <FileText size={12}/> },
      { label: 'Shared', value: loading ? '—' : stats.owned_shared_file_count, icon: <Share2 size={12}/> },
    ]}/>
    <input ref={input} type="file" multiple className="sr-only" tabIndex={-1} aria-hidden="true" onChange={event => acceptFiles(event.target.files || [])}/>
    <button type="button" onClick={() => input.current?.click()} onDragOver={event => { event.preventDefault(); event.dataTransfer.dropEffect = 'copy' }} onDrop={event => { event.preventDefault(); acceptFiles(event.dataTransfer.files) }} className="group w-full rounded-aurora-2 border border-dashed border-aurora-accent-primary/50 bg-[linear-gradient(135deg,color-mix(in_srgb,var(--aurora-accent-primary)_8%,transparent),color-mix(in_srgb,var(--aurora-success)_7%,transparent))] p-7 text-center text-sm text-aurora-text-muted transition-colors hover:border-aurora-accent-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary">
      <Upload className="mx-auto mb-2 size-6 text-aurora-accent-primary"/><strong className="block text-aurora-text-primary">Drop files here or browse</strong>Available to agents through <code className="text-aurora-success">stash://</code>
    </button>
    {uploads.length ? <div aria-label="Upload queue" className="space-y-2 rounded-aurora-2 border border-aurora-accent-primary/30 bg-aurora-panel-low p-3">{uploads.map(item => <div key={item.id} className="flex items-center justify-between gap-3 text-sm"><span className="min-w-0 truncate text-aurora-text-primary">{item.file.name} — {item.status}{item.detail ? `: ${item.detail}` : ''}</span>{item.status === 'uploading' ? <Button variant="ghost" size="sm" onClick={() => item.abort?.abort()}><X/>Cancel</Button> : item.status === 'pending' ? <Button variant="ghost" size="sm" onClick={() => setUploads(current => current.map(value => value.id === item.id ? { ...value, status: 'canceled', detail: 'Canceled' } : value))}><X/>Cancel</Button> : item.status === 'failed' || item.status === 'canceled' ? <Button variant="outline" size="sm" onClick={() => setUploads(current => current.map(value => value.id === item.id ? { ...value, status: 'pending', detail: undefined } : value))}><RefreshCw/>Retry upload</Button> : null}</div>)}</div> : null}
    {failure ? <div role="alert" className="flex flex-wrap items-center justify-between gap-3 rounded-aurora-2 border border-aurora-error/35 bg-aurora-error/5 p-4"><div><strong className="text-sm text-aurora-error">{failure.title}</strong><p className="mt-1 text-xs text-aurora-text-muted">{failure.detail}</p></div><Button variant="outline" onClick={() => void load(query.trim())}><RefreshCw/>Retry</Button></div> : null}
    <DashboardPanel title="Files" action={<div className="flex items-center gap-2"><label className="relative hidden sm:block"><span className="sr-only">Search current files</span><Search className="absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-aurora-text-muted"/><input value={query} onChange={event => setQuery(event.target.value)} placeholder="Search files…" className="h-8 w-44 rounded-aurora-1 border border-aurora-border-subtle bg-aurora-control-surface pl-8 pr-2 text-xs text-aurora-text-primary"/></label><ViewToggle value={view} onChange={setView}/></div>}>
      <label className="relative sm:hidden"><span className="sr-only">Search current files</span><Search className="absolute left-3 top-1/2 size-4 -translate-y-1/2 text-aurora-text-muted"/><input value={query} onChange={event => setQuery(event.target.value)} placeholder="Search files…" className="h-9 w-full rounded-aurora-1 border border-aurora-border-subtle bg-aurora-control-surface pl-9 pr-3 text-sm"/></label>
      {loading ? <div role="status" className="py-10 text-center text-sm text-aurora-text-muted">Loading your files…</div> : files.length === 0 ? <><div className="py-10 text-center"><Inbox className="mx-auto size-8 text-aurora-text-muted"/><strong className="mt-3 block text-aurora-text-primary">{query ? 'No matches on this page' : 'Your Stash is empty'}</strong><p className="mt-1 text-sm text-aurora-text-muted">{query ? (nextCursor ? 'More files are available to search.' : 'Try another filename.') : 'Upload a file to make it available to your agents.'}</p></div>{nextCursor ? <Button className="mx-auto mt-3" variant="outline" disabled={loadingMore} onClick={() => void load(query.trim(), nextCursor)}>{loadingMore ? 'Loading…' : 'Load more'}</Button> : null}</> : <><div className={view === 'grid' ? 'grid gap-3 sm:grid-cols-2 xl:grid-cols-3' : 'divide-y divide-aurora-border-subtle'}>{files.map(file => <FileRow key={file.file_id} file={file} grid={view === 'grid'} onDelete={() => setDeleteTarget(file)} onManage={() => setManageTarget(file)} onCopy={async () => { const result = await copyUri(value => navigator.clipboard.writeText(value), file.uri); if (result.ok) setAnnouncement(result.announcement); else setError(result.error) }}/>)}</div>{nextCursor ? <Button className="mx-auto mt-3" variant="outline" disabled={loadingMore} onClick={() => void load(query.trim(), nextCursor)}>{loadingMore ? 'Loading…' : 'Load more'}</Button> : null}</>}
    </DashboardPanel>
    <ActionConfirmationDialog open={Boolean(deleteTarget)} onOpenChange={open => { if (!open) { setDeleteTarget(undefined); setDeleteError(undefined) } }} title="Delete this file?" description={`This permanently deletes ${deleteTarget?.display_name || 'the file'} and revokes all of its grants. This cannot be undone.`} confirmLabel="Delete file" busy={busyDelete} error={deleteError ? errorCopy(deleteError) : undefined} onConfirm={() => void remove()}/>
    <ManageDialog file={manageTarget} onClose={() => setManageTarget(undefined)} onChanged={async message => { setAnnouncement(message); await load(query.trim(), undefined, true) }} onError={setError}/>
  </>
}

function ViewToggle({ value, onChange }: { value: ViewMode; onChange: (value: ViewMode) => void }) {
  return <div className="flex rounded-aurora-1 border border-aurora-border-subtle bg-aurora-control-surface p-0.5" role="group" aria-label="File layout"><Button type="button" variant="ghost" size="icon-sm" aria-label="List view" aria-pressed={value === 'list'} onClick={() => onChange('list')}><List/></Button><Button type="button" variant="ghost" size="icon-sm" aria-label="Grid view" aria-pressed={value === 'grid'} onClick={() => onChange('grid')}><Grid2X2/></Button></div>
}

function FileRow({ file, grid, onDelete, onManage, onCopy }: { file: StashFile; grid: boolean; onDelete: () => void; onManage: () => void; onCopy: () => Promise<void> }) {
  return <article className={grid ? 'rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-low p-4' : 'grid grid-cols-[34px_minmax(0,1fr)] items-center gap-3 px-2 py-3 sm:grid-cols-[34px_minmax(0,1fr)_100px_auto]'}><span className="grid size-8 place-items-center rounded-aurora-1 bg-aurora-accent-primary/10 text-aurora-accent-primary">{fileIcon(file.display_name)}</span><div className={grid ? 'mt-3 min-w-0' : 'min-w-0'}><strong className="block truncate text-sm text-aurora-text-primary">{file.display_name}</strong><button type="button" aria-label={`Copy URI for ${file.display_name}`} onClick={() => void onCopy()} title="Copy canonical URI" className="flex max-w-full items-center gap-1 text-left text-xs text-aurora-text-muted hover:text-aurora-accent-primary"><code className="truncate">{file.uri}</code><Copy className="size-3 shrink-0"/></button>{!file.owned ? <span className="mt-1 inline-block text-[10px] font-semibold text-aurora-success">Shared with you</span> : null}</div><span className={`${grid ? 'mt-3 block' : 'hidden sm:block'} text-xs text-aurora-text-muted`}>{bytes(file.size_bytes)}</span><div className={`${grid ? 'mt-3' : 'col-span-2 sm:col-span-1'} flex items-center justify-end gap-1`}><Button size="icon-sm" variant="ghost" aria-label={`Download ${file.display_name}`} asChild><a href={api.downloadUrl(file.file_id)} download><Download/></a></Button>{file.owned ? <><Button size="icon-sm" variant="ghost" aria-label={`Rename or share ${file.display_name}`} onClick={onManage}><Pencil/></Button><Button size="icon-sm" variant="ghost" aria-label={`Delete ${file.display_name}`} onClick={onDelete}><Trash2/></Button></> : null}</div></article>
}

export function ManageDialog({ file, onClose, onChanged, onError }: { file?: StashFile; onClose: () => void; onChanged: (message: string) => Promise<void>; onError: (error: unknown) => void }) {
  const [name, setName] = useState(''); const [grants, setGrants] = useState<StashGrant[]>([]); const [grantCursor, setGrantCursor] = useState<string | null>(null); const [busy, setBusy] = useState(false); const [localError, setLocalError] = useState<unknown>(); const grantGeneration = useRef(0); const [recipientQuery, setRecipientQuery] = useState(''); const [recipients, setRecipients] = useState<Array<{ principal_id: string; display_name: string }>>([]); const recipientGeneration = useRef(0)
  const loadGrants = useCallback(async (target: StashFile, cursor?: string, signal?: AbortSignal) => { const current = ++grantGeneration.current; try { const page = await api.listGrants(target.file_id, signal, cursor); if (acceptGrantPage(grantGeneration.current, current, file?.file_id, target.file_id)) { setGrants(previous => mergeGrants(previous, page.grants, Boolean(cursor))); setGrantCursor(page.next_cursor) } } catch (error) { if (!(error instanceof DOMException && error.name === 'AbortError')) onError(error) } }, [file?.file_id, onError])
  useEffect(() => { setName(file?.display_name || ''); setGrants([]); setGrantCursor(null); setRecipientQuery(''); setRecipients([]); setLocalError(undefined); recipientGeneration.current += 1; if (!file) return; const controller = new AbortController(); void loadGrants(file, undefined, controller.signal); return () => { grantGeneration.current += 1; controller.abort() } }, [file, loadGrants])
  useEffect(() => { setRecipients([]); const normalized = recipientQuery.trim(); if (normalized.length < 3 || !file) return; const responseGeneration = ++recipientGeneration.current; const responseFile = file.file_id; const controller = new AbortController(); const timer = window.setTimeout(() => { api.searchRecipients(normalized, controller.signal).then(values => { if (acceptRecipientSearch(recipientGeneration.current, responseGeneration, file.file_id, responseFile, recipientQuery, normalized)) setRecipients(values) }).catch(error => { if (!(error instanceof DOMException && error.name === 'AbortError')) onError(error) }) }, 250); return () => { window.clearTimeout(timer); recipientGeneration.current += 1; controller.abort() } }, [file, recipientQuery, onError])
  if (!file) return null
  const run = async (operation: () => Promise<unknown>, message: string) => { setBusy(true); setLocalError(undefined); try { await operation(); await onChanged(message); onClose() } catch (error) { setLocalError(error) } finally { setBusy(false) } }
  const failure = localError ? errorCopy(localError) : undefined
  return <Dialog open onOpenChange={open => { if (!open && !busy) onClose() }}><DialogContent className="border-aurora-border-strong bg-aurora-panel-medium"><DialogTitle>Manage {file.display_name}</DialogTitle><DialogDescription>Rename this file or manage read access.</DialogDescription>{failure ? <div role="alert" className="rounded-aurora-1 border border-aurora-error/35 bg-aurora-error/5 p-3"><strong className="text-sm text-aurora-error">{failure.title}</strong><p className="mt-1 text-xs text-aurora-text-muted">{failure.detail}</p></div> : null}<label className="text-xs font-semibold text-aurora-text-muted">Filename<input autoFocus value={name} onChange={event => setName(event.target.value)} className="mt-2 h-10 w-full rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface px-3 text-sm text-aurora-text-primary"/></label><Button disabled={busy || !name.trim() || name === file.display_name} onClick={() => void run(() => api.renameFile(file.file_id, name.trim()), `${file.display_name} renamed.`)}><Pencil/>Rename</Button><div className="border-t border-aurora-border-subtle pt-4"><label className="text-xs font-semibold text-aurora-text-muted">Find a recipient<input value={recipientQuery} onChange={event => setRecipientQuery(event.target.value)} placeholder="Type at least 3 characters" className="mt-2 h-10 w-full rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface px-3 text-sm text-aurora-text-primary"/></label>{recipients.length ? <ul aria-label="Recipient results" className="mt-2 divide-y divide-aurora-border-subtle">{recipients.map(recipient => <li key={recipient.principal_id} className="flex items-center justify-between py-2"><span className="text-sm text-aurora-text-primary">{recipient.display_name}</span><Button size="sm" variant="outline" disabled={busy} onClick={() => { const selected = selectedRecipientId(recipients, recipient.principal_id); if (selected) void run(() => api.createGrant(file.file_id, selected), `Access granted to ${recipient.display_name}.`) }}>Grant access</Button></li>)}</ul> : null}</div>{grants.length ? <div><h3 className="text-xs font-semibold text-aurora-text-muted">Active grants</h3><ul className="mt-2 divide-y divide-aurora-border-subtle">{grants.map(grant => <li key={grant.grant_id} className="flex items-center justify-between gap-3 py-2"><code className="truncate text-xs text-aurora-text-primary">{grant.grantee_principal_id}</code><Button size="sm" variant="ghost" disabled={busy} onClick={() => void run(() => api.revokeGrant(file.file_id, grant.grant_id), `Access revoked for ${file.display_name}.`)}>Revoke</Button></li>)}</ul>{grantCursor ? <Button variant="outline" size="sm" disabled={busy} onClick={() => void loadGrants(file, grantCursor)}>Load more grants</Button> : null}</div> : <p className="text-xs text-aurora-text-muted">No active grants.</p>}</DialogContent></Dialog>
}
