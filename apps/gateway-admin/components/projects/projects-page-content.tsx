'use client'
import { useCallback, useEffect, useState } from 'react'
import { AppHeader } from '@/components/app-header'
import { Button } from '@/components/ui/button'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { activeTeamId, archiveProject, createProject, listProjects, type ProjectView } from '@/lib/projects/client'

export function ProjectsPageContent() {
  const [rows, setRows] = useState<ProjectView[]>([]), [error, setError] = useState<string>(), [id, setId] = useState(''), [name, setName] = useState('')
  const team = activeTeamId()
  const load = useCallback(() => void listProjects().then(setRows).catch(e => setError(e instanceof Error ? e.message : 'Projects unavailable')), [])
  useEffect(load, [load])
  const create = async () => { if (!team) return setError('Select a Team before creating a Project.'); try { await createProject(team, id.trim(), name.trim()); setId(''); setName(''); load() } catch (e) { setError(e instanceof Error ? e.message : 'Create failed') } }
  return <><AppHeader breadcrumbs={[{ label: 'Workspace' }, { label: 'Projects' }]} /><main className={`${AURORA_PAGE_SHELL} flex-1`}><div className={`${AURORA_PAGE_FRAME} space-y-4`}><header><h1 className="text-2xl font-bold text-aurora-text-primary">Projects</h1><p className="text-sm text-aurora-text-muted">Team-assigned Projects from Labby’s authority store.</p></header>{error ? <div role="alert" className="rounded border border-aurora-error/30 p-3 text-aurora-error">{error}</div> : null}<section className="grid gap-2 rounded border border-aurora-border-subtle p-4 sm:grid-cols-[1fr_1fr_auto]"><input aria-label="Project ID" value={id} onChange={e => setId(e.target.value)} placeholder="project-id" className="rounded border border-aurora-border-default bg-aurora-control-surface px-3"/><input aria-label="Project name" value={name} onChange={e => setName(e.target.value)} placeholder="Project name" className="rounded border border-aurora-border-default bg-aurora-control-surface px-3"/><Button disabled={!team || !id.trim() || !name.trim()} onClick={() => void create()}>Create Project</Button></section><section className="divide-y divide-aurora-border-subtle rounded border border-aurora-border-subtle">{rows.length === 0 ? <p className="p-5 text-sm text-aurora-text-muted">No accessible Projects.</p> : rows.map(row => <article key={`${row.team_id}:${row.project_id}`} className="flex items-center justify-between gap-4 p-4"><div><strong className="text-aurora-text-primary">{row.name}</strong><p className="text-xs text-aurora-text-muted">{row.project_id} · {row.team_id} · {row.role} · policy {row.policy_epoch}</p></div><Button variant="outline" disabled={!row.can_manage} onClick={async () => { try { await archiveProject(row.team_id, row.project_id); load() } catch (e) { setError(e instanceof Error ? e.message : 'Archive failed') } }}>Archive</Button></article>)}</section></div></main></>
}
