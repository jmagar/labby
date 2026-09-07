'use client'

import { useEffect, useState } from 'react'
import {
  Bot, Box, CheckCircle2, CirclePlus, Clock3,
  FileCode2, Grid2X2, Layers3, List,
  Pause, Play, Search, Table2, ChevronDown, ArrowUpDown,
} from 'lucide-react'

import { AppHeader } from '@/components/app-header'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { ConsoleHero } from '@/components/console/console-hero'
import { DashboardPanel } from '@/components/dashboard/panel'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { ArtifactComposer } from './artifact-composer'
import { DevContainersPageContent } from './dev-containers-page-content'
import { NewAgentSessionWizard } from './new-agent-session-wizard'
import { AlpineMark, CodexMark, DebianMark, UbuntuMark } from './brand-marks'
import { listAgents, listTasks } from '@/lib/agent-tasks/client'

const demoArtifacts = [
  ['Skill', 'repo-triage', 'Cluster open PRs and issues, then draft a triage note.', '#review · #github'],
  ['Agent', 'rust-reviewer', 'Review Rust changes and flag unsafe blocks with rationale.', '#rust · #review'],
  ['MCP', 'labby', 'Gateway control plane exposing scoped upstream MCP capabilities.', '#gateway · #mcp'],
  ['Command', '/ship', 'Run release checks and prepare a release draft.', '#release'],
  ['Loadout', 'operator-console', 'Operational tools for logs, services, and infrastructure.', '#ops · #homelab'],
  ['Snippet', 'gateway-reconcile', 'Probe disconnected servers and summarize the delta.', '#gateway'],
]

export function LibraryTabs({ active }: { active: 'artifacts' | 'loadouts' | 'snippets' }) {
  const tabs = [
    ['artifacts', '/library', 'Artifacts'],
    ['loadouts', '/loadouts', 'Loadouts'],
    ['snippets', '/snippets', 'Snippets'],
  ] as const
  return <nav aria-label="Library sections" className="flex max-w-full gap-5 overflow-x-auto border-b border-aurora-border-subtle px-1 sm:gap-6 sm:px-3">
    {tabs.map(([id, href, label]) => <a key={id} href={href} aria-current={active === id ? 'page' : undefined} className="shrink-0 border-b-2 border-transparent px-2 py-3 text-sm font-semibold text-aurora-text-muted transition-colors hover:text-aurora-text-primary aria-[current=page]:border-aurora-accent-primary aria-[current=page]:text-aurora-text-primary">{label}</a>)}
  </nav>
}

function PageFrame({ children }: { children: React.ReactNode }) {
  return <div className={`${AURORA_PAGE_SHELL} flex-1`}><div className={`${AURORA_PAGE_FRAME} space-y-4`}>{children}</div></div>
}

export function LibraryPage() {
  return <><AppHeader breadcrumbs={[{ label: 'Depot' }, { label: 'Library' }]} /><PageFrame>
    <LibraryTabs active="artifacts" />
    <ConsoleHero eyebrow="Depot · Library" title="Library" pulse={{ color: 'var(--aurora-warn)', label: 'preview layout' }} actions={<div className="flex gap-2"><Button variant="outline">Backup all</Button><Button><CirclePlus />New loadout</Button></div>} stats={[
      { label: 'Artifacts', value: '102,745', icon: <Box size={12}/> },
      { label: 'Loadouts', value: '4', icon: <Layers3 size={12}/> },
      { label: 'Snippets', value: '6', icon: <FileCode2 size={12}/> },
      { label: 'Authority', value: 'Read only', icon: <CheckCircle2 size={12}/> },
    ]}/>
    <div className="grid gap-4 lg:grid-cols-[210px_1fr]">
      <aside className="space-y-3"><DashboardPanel title="Views"><div className="space-y-1 text-sm"><button className="w-full rounded-aurora-1 bg-aurora-surface-muted px-3 py-2 text-left text-aurora-text-primary">All artifacts</button>{['Published', 'MCP servers', 'Skills', 'Agents', 'Commands'].map(x => <button key={x} className="w-full px-3 py-2 text-left text-aurora-text-muted hover:text-aurora-text-primary">{x}</button>)}</div></DashboardPanel></aside>
      <DashboardPanel title="Artifacts" action={<div className="relative"><Search className="absolute left-3 top-1/2 size-4 -translate-y-1/2 text-aurora-text-muted"/><input aria-label="Filter library" className="h-9 rounded-aurora-1 border border-aurora-border-subtle bg-aurora-panel-low pl-9 pr-3 text-sm" placeholder="Filter artifacts…"/></div>}>
        <div className="divide-y divide-aurora-border-subtle">{demoArtifacts.map(([kind, name, description, tags]) => <div key={name} className="grid gap-2 px-2 py-3 sm:grid-cols-[100px_1fr_180px] sm:items-center"><Badge variant="outline">{kind}</Badge><div><div className="font-semibold text-aurora-text-primary">{name}</div><div className="text-xs text-aurora-text-muted">{description}</div></div><div className="text-xs text-aurora-accent-primary">{tags}</div></div>)}</div>
      </DashboardPanel>
    </div>
  </PageFrame></>
}

export function CreatePage() {
  return <ArtifactComposer />
}

export function AgentsPage() {
  const [agents, setAgents] = useState<string[][]>([])
  const [loadError, setLoadError] = useState<string | null>(null)
  const [selected, setSelected] = useState<string[] | null>(null)
  const [creating, setCreating] = useState(false)
  useEffect(() => { const controller = new AbortController(); void listAgents(controller.signal).then(items => { setAgents(items.map(item => [item.state, item.agent_id, `${item.owner_kind}:${item.owner_id}`, `v${item.version}`, 'Labby', item.catalog_generation])); setLoadError(null) }).catch(error => { if (!controller.signal.aborted) setLoadError(error instanceof Error ? error.message : 'Agent service unavailable') }); return () => controller.abort() }, [])
  const active = agents.filter(row => row[0] === 'active').length
  return <><AppHeader breadcrumbs={[{ label: 'Workspace' }, { label: 'Agents' }]} /><PageFrame><ConsoleHero eyebrow="Workspace · Agents" title="Agents" pulse={{ color: 'var(--aurora-success)' }} actions={<Button onClick={() => setCreating(true)}><CirclePlus/>New session</Button>} stats={[{label:'Active',value:active,icon:<Play size={12}/>,tone:'var(--aurora-success)'},{label:'Suspended',value:agents.filter(row=>row[0]==='suspended').length,icon:<Pause size={12}/>},{label:'Definitions',value:agents.length,icon:<Bot size={12}/>},{label:'Authority',value:'Live',icon:<CheckCircle2 size={12}/>} ]}/>{loadError?<div role="alert" className="rounded-aurora-1 border border-aurora-error/30 bg-aurora-error/5 p-3 text-sm text-aurora-error">{loadError}</div>:null}<AgentsCollection rows={agents} onSelect={setSelected}/></PageFrame>
    <AgentSessionSheet session={selected} onOpenChange={(open) => !open && setSelected(null)} />
    <NewAgentSessionWizard open={creating} onOpenChange={setCreating} />
  </>
}

function AgentSessionSheet({ session, onOpenChange }: { session: string[] | null; onOpenChange: (open: boolean) => void }) {
  return <Sheet open={Boolean(session)} onOpenChange={onOpenChange}>
    <SheetContent className="!w-[min(92vw,680px)] border-aurora-border-strong bg-aurora-panel-medium p-0 sm:!max-w-[680px]">
      <SheetHeader className="border-b border-aurora-border-subtle bg-aurora-panel-strong px-6 py-5">
        <SheetTitle className="text-xl text-aurora-text-primary">{session?.[1] ?? 'Agent definition'}</SheetTitle>
        <SheetDescription className="text-aurora-text-muted">Authoritative immutable Agent definition</SheetDescription>
      </SheetHeader>
      <div className="grid grid-cols-2 border-b border-aurora-border-subtle bg-aurora-panel-low sm:grid-cols-4">
        {[['Owner',session?.[2]],['Revision',session?.[3]],['Runtime',session?.[4]],['State',session?.[0]]].map(([label,value])=><div key={label} className="border-r border-aurora-border-subtle px-5 py-4 last:border-r-0"><span className="block text-[9px] font-bold uppercase tracking-[.14em] text-aurora-text-muted">{label}</span><strong className="mt-1 block text-xs text-aurora-text-primary">{value}</strong></div>)}
      </div>
      <div className="flex-1 px-6 py-5 text-sm text-aurora-text-muted">Catalog generation: <code className="text-aurora-text-primary">{session?.[5]}</code>. Session transcripts appear only after a configured execution backend produces them.</div>
    </SheetContent>
  </Sheet>
}

export function TasksPage() {
  const [rows, setRows] = useState<string[][]>([])
  const [loadError, setLoadError] = useState<string | null>(null)
  const [selected, setSelected] = useState<string[] | null>(null)
  const [creating, setCreating] = useState(false)
  const [name,setName]=useState(''),[definition,setDefinition]=useState(''),[schedule,setSchedule]=useState('Daily · 09:00'),[loadout,setLoadout]=useState('operator-console')
  useEffect(() => { const controller = new AbortController(); void listTasks(controller.signal).then(items => { setRows(items.map(item => [item.state, item.task_id, `attempt ${item.attempt}`, `${item.owner_kind}:${item.owner_id}`, item.agent_id, `Agent revision ${item.agent_version}`, item.error_code ? 'failed' : item.output_digest ? 'passed' : 'pending'])); setLoadError(null) }).catch(error => { if (!controller.signal.aborted) setLoadError(error instanceof Error ? error.message : 'Task service unavailable') }); return () => controller.abort() }, [])
  const create=()=>{setLoadError('Task creation requires an immutable Agent revision and is not available from this form yet.');setCreating(false)}
  return <><AppHeader breadcrumbs={[{label:'Workspace'},{label:'Tasks'}]}/><PageFrame><ConsoleHero eyebrow="Team · Agent Tasks" title="Tasks" description="Durable, owner-scoped agent tasks from Labby’s authoritative task ledger." actions={<Button onClick={()=>setCreating(true)}><CirclePlus/>New Task</Button>} stats={[{label:'Tasks',value:rows.length,icon:<Clock3 size={12}/>},{label:'Queued',value:rows.filter(row=>row[0]==='queued').length,icon:<CheckCircle2 size={12}/>,tone:'var(--aurora-success)'},{label:'Running',value:rows.filter(row=>row[0]==='running').length,icon:<Play size={12}/>},{label:'Failed',value:rows.filter(row=>row[0]==='failed').length,icon:<Clock3 size={12}/>,tone:'var(--aurora-error)'}]}/>{loadError?<div role="alert" className="rounded-aurora-1 border border-aurora-error/30 bg-aurora-error/5 p-3 text-sm text-aurora-error">{loadError}</div>:null}<TasksCollection rows={rows} onSelect={setSelected}/></PageFrame>
    <TaskDialog row={selected} onOpenChange={open=>!open&&setSelected(null)}/>
    <Dialog open={creating} onOpenChange={setCreating}><DialogContent className="border-aurora-border-strong bg-aurora-panel-medium"><DialogTitle>New task</DialogTitle><DialogDescription>Schedule a reusable agent run.</DialogDescription><TaskFields name={name} setName={setName} definition={definition} setDefinition={setDefinition} schedule={schedule} setSchedule={setSchedule} loadout={loadout} setLoadout={setLoadout}/><Button onClick={create} disabled={!name.trim()||!definition.trim()}><CirclePlus/>Create task</Button></DialogContent></Dialog>
  </>
}

function SortHead({children,onClick}:{children:React.ReactNode;onClick:()=>void}){return <th className="px-3 py-2 text-left"><button type="button" onClick={onClick} className="flex items-center gap-1 text-[9px] font-bold uppercase tracking-[.14em] text-aurora-text-muted hover:text-aurora-text-primary">{children}<ArrowUpDown className="size-3 opacity-45"/></button></th>}

function AgentsCollection({rows,onSelect}:{rows:string[][];onSelect:(row:string[])=>void}){
  const [filter,setFilter]=useState('All'),[sort,setSort]=useState(1),[view,setView]=useState<ViewMode>('table')
  const shown=[...rows].filter(row=>filter==='All'||row[0]===filter).sort((a,b)=>a[sort].localeCompare(b[sort]))
  return <DashboardPanel title="Definitions" action={<div className="flex items-center gap-3"><div className="flex gap-1">{['All','active','suspended'].map(item=><button key={item} type="button" onClick={()=>setFilter(item)} aria-pressed={filter===item} className="rounded-full border border-aurora-border-subtle px-3 py-1 text-[10px] font-semibold text-aurora-text-muted aria-pressed:border-aurora-accent-primary aria-pressed:bg-aurora-accent-primary aria-pressed:text-aurora-page-bg">{item}</button>)}</div><ViewModes value={view} onChange={setView}/></div>}>
    {view==='table'?<div className="overflow-x-auto"><table className="w-full text-sm"><thead><tr className="border-b border-aurora-border-subtle">{['Status','Session','Loadout','Container','Harness','Elapsed'].map((head,index)=><SortHead key={head} onClick={()=>setSort(index)}>{head}</SortHead>)}</tr></thead><tbody>{shown.map(row=><tr key={row[1]} tabIndex={0} onClick={()=>onSelect(row)} onKeyDown={event=>event.key==='Enter'&&onSelect(row)} className="cursor-pointer border-b border-aurora-border-subtle/70 last:border-0 hover:bg-aurora-hover-bg"><td className="px-3 py-3"><StatusDot status={row[0]}/></td><td className="px-3 py-3 font-semibold text-aurora-text-primary">{row[1]}</td><td className="px-3 py-3"><Badge variant="outline" className="text-aurora-accent-primary">{row[2]}</Badge></td><td className="px-3 py-3 text-aurora-text-muted"><span className="flex items-center gap-2"><ProductMark kind={row[3]}/>{row[3]}</span></td><td className="px-3 py-3 text-aurora-text-muted"><span className="flex items-center gap-2"><ProductMark kind={row[4]}/>{row[4]}</span></td><td className="px-3 py-3 text-aurora-text-muted">{row[5]}</td></tr>)}</tbody></table></div>:<div className={view==='cards'?'grid gap-3 md:grid-cols-2 xl:grid-cols-3':'divide-y divide-aurora-border-subtle'}>{shown.map(row=><button key={row[1]} onClick={()=>onSelect(row)} className="w-full rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-low p-4 text-left"><StatusDot status={row[0]}/><strong className="mt-2 block text-aurora-text-primary">{row[1]}</strong><span className="mt-1 block text-xs text-aurora-text-muted">{row.slice(2).join(' · ')}</span></button>)}</div>}
  </DashboardPanel>
}

function StatusDot({status}:{status:string}){const color=status==='Running'||status==='Armed'?'bg-aurora-success':status==='Failed'?'bg-aurora-error':status==='Paused'?'bg-aurora-warn':'bg-aurora-text-muted';return <span role="img" aria-label={status} title={status} className={`block size-2 rounded-full ${color}`}/>}

function ProductMark({kind}:{kind:string}){
  if(kind==='platform-base')return <span className="grid size-5 place-items-center rounded bg-white/5 text-aurora-text-primary"><UbuntuMark className="size-3.5 fill-current"/></span>
  if(kind==='rust-heavy')return <span className="grid size-5 place-items-center rounded bg-white/5 text-aurora-text-primary"><DebianMark className="size-3.5 fill-current"/></span>
  if(kind==='edge-minimal')return <span className="grid size-5 place-items-center rounded bg-white/5 text-aurora-text-primary"><AlpineMark className="size-3.5 fill-current"/></span>
  if(kind==='Claude Code')return <span aria-label="Claude" className="grid size-5 place-items-center rounded bg-white/5 text-[10px] font-black text-aurora-text-primary">AI</span>
  if(kind==='Codex')return <span className="grid size-5 place-items-center rounded bg-white/5 text-aurora-text-primary"><CodexMark className="size-3.5 fill-current"/></span>
  return <span aria-label="Gemini" className="grid size-5 place-items-center rounded bg-white/5 text-sm text-aurora-text-primary">✦</span>
}

function TasksCollection({rows,onSelect}:{rows:string[][];onSelect:(row:string[])=>void}){
  const [filter,setFilter]=useState('All')
  const shown=rows.filter(row=>filter==='All'||row[0]===filter)
  return <DashboardPanel title="Task ledger" action={<div className="flex gap-1">{['All','created','queued','running','succeeded','failed','cancelled','expired'].map(item=><button key={item} type="button" onClick={()=>setFilter(item)} aria-pressed={filter===item} className="rounded-full border border-aurora-border-subtle px-3 py-1 text-[10px] font-semibold text-aurora-text-muted aria-pressed:border-aurora-accent-primary aria-pressed:bg-aurora-accent-primary aria-pressed:text-aurora-page-bg">{item}</button>)}</div>}><div className="overflow-x-auto"><table className="w-full text-sm"><thead><tr className="border-b border-aurora-border-subtle">{['State','Task','Attempt','Owner','Agent','Result'].map(head=><th key={head} className="px-3 py-2 text-left text-[9px] font-bold uppercase tracking-[.14em] text-aurora-text-muted">{head}</th>)}</tr></thead><tbody>{shown.map(row=><tr key={row[1]} className="cursor-pointer border-b border-aurora-border-subtle/70 last:border-0 hover:bg-aurora-hover-bg" onClick={()=>onSelect(row)}><td className="px-3 py-2"><StatusDot status={row[0]}/></td><td className="px-3 py-2 font-semibold text-aurora-text-primary">{row[1]}</td><td className="px-3 py-2 text-aurora-text-muted">{row[2]}</td><td className="px-3 py-2"><Badge variant="outline">{row[3]}</Badge></td><td className="px-3 py-2 text-aurora-text-muted">{row[4]}</td><td className="px-3 py-2 text-aurora-text-muted">{row[6]}</td></tr>)}</tbody></table></div></DashboardPanel>
}

function SelectField({label,value,onChange,children}:{label:string;value:string;onChange:(value:string)=>void;children:React.ReactNode}){return <label className="text-xs text-aurora-text-muted">{label}<span className="relative mt-2 block"><select value={value} onChange={event=>onChange(event.target.value)} className="h-10 w-full appearance-none rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface pl-3 pr-10 text-sm text-aurora-text-primary">{children}</select><ChevronDown className="pointer-events-none absolute right-3 top-1/2 size-4 -translate-y-1/2 text-aurora-text-muted"/></span></label>}
function TaskFields({name,setName,definition,setDefinition,schedule,setSchedule,loadout,setLoadout}:{name:string;setName:(v:string)=>void;definition:string;setDefinition:(v:string)=>void;schedule:string;setSchedule:(v:string)=>void;loadout:string;setLoadout:(v:string)=>void}){return <><label className="text-xs font-semibold text-aurora-text-muted">Task name<input autoFocus value={name} onChange={event=>setName(event.target.value)} className="mt-2 h-10 w-full rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface px-3 text-sm text-aurora-text-primary" placeholder="Weekly gateway review"/></label><label className="text-xs font-semibold text-aurora-text-muted">Define the task<textarea value={definition} onChange={event=>setDefinition(event.target.value)} rows={4} className="mt-2 w-full resize-none rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface p-3 text-sm text-aurora-text-primary" placeholder="Describe exactly what the agent should do and what a successful run produces."/></label><div className="grid grid-cols-2 gap-3"><SelectField label="Schedule" value={schedule} onChange={setSchedule}><option>Daily · 09:00</option><option>Daily · 02:00</option><option>Weekly · Monday</option><option>Weekly · Sun 03:00</option></SelectField><SelectField label="Loadout" value={loadout} onChange={setLoadout}><option>operator-console</option><option>research-workbench</option><option>project-a</option><option>project-b</option><option>platform</option><option>shared</option></SelectField></div></>}

function TaskDialog({row,onOpenChange}:{row:string[]|null;onOpenChange:(open:boolean)=>void}){return <Dialog open={Boolean(row)} onOpenChange={onOpenChange}><DialogContent className="border-aurora-border-strong bg-aurora-panel-medium"><DialogTitle>{row?.[1]??'Task'}</DialogTitle><DialogDescription>Authoritative durable Agent Task record.</DialogDescription><dl className="divide-y divide-aurora-border-subtle rounded-aurora-1 border border-aurora-border-subtle bg-aurora-panel-low px-4">{[['State',row?.[0]],['Attempt',row?.[2]],['Owner',row?.[3]],['Agent',row?.[4]],['Revision',row?.[5]],['Result',row?.[6]]].map(([label,value])=><div key={label} className="flex justify-between gap-4 py-3 text-sm"><dt className="text-aurora-text-muted">{label}</dt><dd className="font-medium text-aurora-text-primary">{value}</dd></div>)}</dl></DialogContent></Dialog>}

export function DevContainersPage() { return <><AppHeader breadcrumbs={[{label:'Workspace'},{label:'Dev Containers'}]}/><PageFrame><DevContainersPageContent /></PageFrame></> }

const logRows = [
  ['Info','gateway','catalog reconciled · 2 healthy upstreams'],
  ['Info','context7','tools/call completed · 200 in 1.7s'],
  ['Warn','claude-macpoo','SSH session reconnected'],
  ['Debug','depot','catalog search returned 50 of 102745'],
  ['Info','labby','skills catalog refreshed'],
]
export function LogsPage() { return <><AppHeader breadcrumbs={[{label:'Logs'}]}/><PageFrame><ConsoleHero eyebrow="Observability" title="Logs" pulse={{color:'var(--aurora-warn)',label:'preview data'}}/><DashboardPanel title="Event stream" action={<div className="flex gap-2"><Button variant="outline">Follow</Button><Button variant="outline">Download</Button></div>}><div className="mb-3 flex gap-2"><div className="relative flex-1"><Search className="absolute left-3 top-1/2 size-4 -translate-y-1/2 text-aurora-text-muted"/><input className="h-9 w-full rounded-aurora-1 border border-aurora-border-subtle bg-aurora-panel-low pl-9 text-sm" placeholder="Filter lines…"/></div><Badge variant="outline">All sources</Badge></div><DataTable headings={['Level','Source','Message']} rows={logRows}/></DashboardPanel></PageFrame></> }

function DataTable({ headings, rows }: { headings: string[]; rows: string[][] }) {
  return <div className="overflow-x-auto"><table className="w-full text-left text-sm"><thead><tr className="border-b border-aurora-border-subtle">{headings.map(h=><th key={h} className="px-3 py-2 text-[11px] uppercase tracking-[.14em] text-aurora-text-muted">{h}</th>)}</tr></thead><tbody>{rows.map((row,index)=><tr key={index} className="border-b border-aurora-border-subtle/70 last:border-0">{row.map((cell,i)=><td key={i} className={`px-3 py-3 ${i === 1 ? 'font-semibold text-aurora-text-primary' : 'text-aurora-text-muted'}`}>{i === 0 ? <Badge variant="outline">{cell}</Badge> : cell}</td>)}</tr>)}</tbody></table></div>
}

type ViewMode = 'table'|'list'|'cards'
function ViewModes({value,onChange}:{value:ViewMode;onChange:(value:ViewMode)=>void}) { return <div className="flex rounded-aurora-1 border border-aurora-border-subtle bg-aurora-control-surface p-0.5">{([[Table2,'Table','table'],[List,'List','list'],[Grid2X2,'Cards','cards']] as const).map(([Icon,label,mode])=><button key={mode} type="button" onClick={()=>onChange(mode)} aria-pressed={value===mode} aria-label={`${label} view`} title={`${label} view`} className={`rounded p-1.5 ${value===mode?'bg-aurora-selected-bg text-aurora-accent-primary':'text-aurora-text-muted hover:text-aurora-text-primary'}`}><Icon className="size-3.5"/></button>)}</div> }
