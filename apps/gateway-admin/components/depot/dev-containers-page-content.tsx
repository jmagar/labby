'use client'

import { useCallback, useEffect, useState } from 'react'
import { CirclePlus, Container, Play, RefreshCw, Square, Trash2 } from 'lucide-react'
import { ActionConfirmationDialog } from '@/components/action-confirmation-dialog'
import { ConsoleHero } from '@/components/console/console-hero'
import { DashboardPanel } from '@/components/dashboard/panel'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { useBrowserSession } from '@/lib/auth/session'
import * as api from '@/lib/dev-containers/client'
import type { DevContainer } from '@/lib/dev-containers/client'

export function DevContainersPageContent() {
  const session = useBrowserSession()
  const authority = session.status === 'authenticated' ? session.authority : undefined
  const capabilities = new Set(authority?.capabilities ?? [])
  const [instances, setInstances] = useState<DevContainer[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string>()
  const [instanceId, setInstanceId] = useState('')
  const [templateId, setTemplateId] = useState('')
  const [busy, setBusy] = useState<string>()
  const [destroyTarget, setDestroyTarget] = useState<DevContainer>()

  const load = useCallback(async (signal?: AbortSignal) => {
    setLoading(true); setError(undefined)
    try { setInstances(await api.listDevContainers(signal)) }
    catch (reason) { if (!(reason instanceof DOMException && reason.name === 'AbortError')) setError(reason instanceof Error ? reason.message : 'Dev Containers are unavailable.') }
    finally { if (!signal?.aborted) setLoading(false) }
  }, [])
  useEffect(() => { const controller = new AbortController(); void load(controller.signal); return () => controller.abort() }, [load, authority?.generation, authority?.activeOwner.kind, authority?.activeOwner.id])

  const operate = async (item: DevContainer, operation: 'start' | 'stop' | 'destroy' | 'reconcile') => {
    setBusy(item.instance_id); setError(undefined)
    try { await api.operateDevContainer(item.instance_id, operation); setDestroyTarget(undefined); await load() }
    catch (reason) { setError(reason instanceof Error ? reason.message : 'The operation failed.') }
    finally { setBusy(undefined) }
  }
  const create = async () => {
    if (!instanceId.trim() || !templateId.trim()) return
    setBusy('create'); setError(undefined)
    try { await api.createDevContainer(instanceId.trim(), templateId.trim()); setInstanceId(''); setTemplateId(''); await load() }
    catch (reason) { setError(reason instanceof Error ? reason.message : 'Container creation failed.') }
    finally { setBusy(undefined) }
  }

  return <>
    <ConsoleHero eyebrow={`Workspace · ${authority?.activeOwner.kind ?? 'unavailable'}`} title="Dev Containers" pulse={{ color: error ? 'var(--aurora-error)' : 'var(--aurora-success)' }} actions={<Button variant="outline" onClick={() => void load()} disabled={loading}><RefreshCw/>Refresh</Button>} stats={[{ label: 'Visible', value: loading ? '—' : instances.length, icon: <Container size={12}/> }]}/>
    {error ? <div role="alert" className="rounded-aurora-2 border border-aurora-error/35 bg-aurora-error/5 p-4 text-sm text-aurora-error">{error}</div> : null}
    <DashboardPanel title="Create from approved template">
      {capabilities.has('scope.create') ? <div className="grid gap-3 md:grid-cols-[1fr_1fr_auto]"><Input aria-label="Container ID" placeholder="Container ID" value={instanceId} onChange={event => setInstanceId(event.target.value)}/><Input aria-label="Approved template ID" placeholder="Approved template ID" value={templateId} onChange={event => setTemplateId(event.target.value)}/><Button onClick={() => void create()} disabled={busy === 'create' || !instanceId.trim() || !templateId.trim()}><CirclePlus/>Create</Button></div> : <p className="text-sm text-aurora-text-muted">You can use visible containers, but this workspace does not grant container creation.</p>}
    </DashboardPanel>
    <DashboardPanel title="Containers">
      {loading ? <p role="status" className="py-8 text-center text-sm text-aurora-text-muted">Loading authoritative container inventory…</p> : instances.length === 0 ? <p className="py-8 text-center text-sm text-aurora-text-muted">No containers are visible in this workspace.</p> : <div className="divide-y divide-aurora-border-subtle">{instances.map(item => <article key={item.instance_id} className="grid gap-3 py-4 md:grid-cols-[minmax(0,1fr)_auto_auto] md:items-center"><div><strong className="text-sm text-aurora-text-primary">{item.instance_id}</strong><p className="text-xs text-aurora-text-muted">{item.owner_kind} · {item.owner_id}</p></div><div className="flex gap-2"><Badge variant="outline">wanted: {item.desired_state}</Badge><Badge variant="outline">observed: {item.observed_state}</Badge></div><div className="flex justify-end gap-1">{capabilities.has('scope.operate') ? <><Button size="icon-sm" variant="ghost" aria-label={`Start ${item.instance_id}`} disabled={busy === item.instance_id} onClick={() => void operate(item, 'start')}><Play/></Button><Button size="icon-sm" variant="ghost" aria-label={`Stop ${item.instance_id}`} disabled={busy === item.instance_id} onClick={() => void operate(item, 'stop')}><Square/></Button><Button size="icon-sm" variant="ghost" aria-label={`Reconcile ${item.instance_id}`} disabled={busy === item.instance_id} onClick={() => void operate(item, 'reconcile')}><RefreshCw/></Button></> : null}{capabilities.has('scope.delete') ? <Button size="icon-sm" variant="ghost" aria-label={`Destroy ${item.instance_id}`} disabled={busy === item.instance_id} onClick={() => setDestroyTarget(item)}><Trash2/></Button> : null}</div></article>)}</div>}
    </DashboardPanel>
    <ActionConfirmationDialog open={Boolean(destroyTarget)} onOpenChange={open => { if (!open) setDestroyTarget(undefined) }} title="Destroy this container?" description={`This permanently destroys ${destroyTarget?.instance_id ?? 'the selected container'} and cannot be undone.`} confirmLabel="Destroy container" busy={busy === destroyTarget?.instance_id} onConfirm={() => destroyTarget ? void operate(destroyTarget, 'destroy') : undefined}/>
  </>
}
