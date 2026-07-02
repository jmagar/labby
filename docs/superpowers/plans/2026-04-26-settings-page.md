# Settings Page Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the existing read-only `/settings` stub with the full settings rail UI designed in `~/.superpowers/brainstorm/content/settings.html`.

**Architecture:** Settings stays in `app/(admin)/settings/` (uses the existing AppSidebar + AuthBootstrap layout). The page renders a two-column layout: a 160px settings rail on the left and a content panel on the right. All five panels (Core, Services, Doctor, Extract, v2 stubs) are client-side. Data comes from `/dev/api/nodeinfo` for pre-population and `/v1/nodes` for live node connectivity. Writes are deferred until lab-bg3e.3 ships — Save buttons log to console for now.

**Tech Stack:** Next.js 16 (output:'export'), React 19, TypeScript, react-hook-form 7, Zod 3, SWR 2, shadcn/ui, Aurora tokens

**Depends on:** Plan 1 (Setup Wizard) — reuses `ServiceForm`, `useNodeInfo`, `useNodes`, `services-catalog.ts`, `schema.ts`

**Reference:** `~/.superpowers/brainstorm/content/settings.html` (approved mockup), `docs/superpowers/specs/2026-04-26-setup-settings-design.md`

---

## File Map

```
apps/gateway-admin/
  app/
    (admin)/
      settings/
        page.tsx                          # REPLACE — new settings page wrapper
  components/
    settings/
      settings-layout.tsx                # NEW — rail + content shell
      settings-rail.tsx                  # NEW — left nav (Core, Services, Doctor, Extract, stubs)
      panels/
        settings-core.tsx                # NEW — bind, port, log, auth mode (read + save)
        settings-services.tsx            # NEW — 21 services table + inline expand
        settings-doctor.tsx              # NEW — health cards + per-service audit list
        settings-extract.tsx             # NEW — scan + results table + apply
        settings-stub.tsx                # NEW — v2 placeholder (Surfaces, Features, Advanced)
```

All five panels are independent client components. `ServiceForm` is imported from `@/components/setup/service-form` (built in Plan 1). `useNodeInfo` and `useNodes` are imported from `@/lib/setup/`.

---

## Task 1: Settings rail component

**Files:**
- Create: `apps/gateway-admin/components/settings/settings-rail.tsx`

- [ ] **Create:**

```tsx
// apps/gateway-admin/components/settings/settings-rail.tsx
'use client'
import { Settings, Activity, Search, Cpu, Layers, Zap, LayoutGrid } from 'lucide-react'
import { cn } from '@/lib/utils'
import { AURORA_MUTED_LABEL } from '@/components/aurora/tokens'

export type SettingsPanel = 'core' | 'services' | 'doctor' | 'extract' | 'surfaces' | 'features' | 'advanced'

interface NavItem {
  id: SettingsPanel
  label: string
  icon: React.ReactNode
  stub?: boolean
}

const CONFIG_ITEMS: NavItem[] = [
  { id: 'core',     label: 'Core',     icon: <Settings className="size-3.5" /> },
  { id: 'services', label: 'Services', icon: <Layers className="size-3.5" /> },
]

const SYSTEM_ITEMS: NavItem[] = [
  { id: 'doctor',  label: 'Doctor',  icon: <Activity className="size-3.5" /> },
  { id: 'extract', label: 'Extract', icon: <Search className="size-3.5" /> },
]

const STUB_ITEMS: NavItem[] = [
  { id: 'surfaces',  label: 'Surfaces',  icon: <Cpu className="size-3.5" />,        stub: true },
  { id: 'features',  label: 'Features',  icon: <Zap className="size-3.5" />,         stub: true },
  { id: 'advanced',  label: 'Advanced',  icon: <LayoutGrid className="size-3.5" />,  stub: true },
]

interface Props {
  active: SettingsPanel
  onSelect: (panel: SettingsPanel) => void
}

function Section({ label, items, active, onSelect }: { label: string; items: NavItem[]; active: SettingsPanel; onSelect: (p: SettingsPanel) => void }) {
  return (
    <div>
      <p className={cn(AURORA_MUTED_LABEL, 'px-4 py-2')}>{label}</p>
      {items.map(item => (
        <button
          key={item.id}
          onClick={() => onSelect(item.id)}
          className={cn(
            'w-full flex items-center gap-2 px-3 py-1.5 mx-2 rounded-[6px] text-left transition-colors text-[12px] font-medium',
            'hover:bg-aurora-hover-bg',
            active === item.id
              ? 'bg-[color-mix(in_srgb,var(--aurora-accent-primary)_10%,transparent)] text-aurora-accent-strong'
              : 'text-aurora-text-muted',
          )}
          style={{ width: 'calc(100% - 16px)' }}
        >
          {item.icon}
          <span className="flex-1">{item.label}</span>
          {item.stub && (
            <span className="text-[9px] font-bold uppercase tracking-wide text-aurora-text-muted bg-aurora-control-surface border border-aurora-border-strong px-1.5 py-0.5 rounded">
              v2
            </span>
          )}
        </button>
      ))}
    </div>
  )
}

export function SettingsRail({ active, onSelect }: Props) {
  return (
    <aside className="w-[160px] min-w-[160px] flex-shrink-0 bg-aurora-nav-bg border-r border-aurora-border-default overflow-y-auto py-3 space-y-1">
      <Section label="Config"  items={CONFIG_ITEMS} active={active} onSelect={onSelect} />
      <Section label="System"  items={SYSTEM_ITEMS} active={active} onSelect={onSelect} />
      <Section label="v2 Stubs" items={STUB_ITEMS}  active={active} onSelect={onSelect} />
    </aside>
  )
}
```

- [ ] **Commit:**

```bash
git add apps/gateway-admin/components/settings/settings-rail.tsx
git commit -m "feat(settings): SettingsRail navigation component"
```

---

## Task 2: Settings layout shell

**Files:**
- Create: `apps/gateway-admin/components/settings/settings-layout.tsx`

- [ ] **Create:**

```tsx
// apps/gateway-admin/components/settings/settings-layout.tsx
'use client'
import { useState } from 'react'
import { AppHeader } from '@/components/app-header'
import { SettingsRail, type SettingsPanel } from './settings-rail'
import { SettingsCore } from './panels/settings-core'
import { SettingsServices } from './panels/settings-services'
import { SettingsDoctor } from './panels/settings-doctor'
import { SettingsExtract } from './panels/settings-extract'
import { SettingsStub } from './panels/settings-stub'
import { useNodeInfo } from '@/lib/setup/use-nodeinfo'
import { cn } from '@/lib/utils'
import { AURORA_PAGE_SHELL } from '@/components/aurora/tokens'

export function SettingsLayout() {
  const [panel, setPanel] = useState<SettingsPanel>('services')
  const { nodeInfo } = useNodeInfo()

  return (
    <div className={cn(AURORA_PAGE_SHELL, 'flex flex-col flex-1')}>
      <AppHeader breadcrumbs={[{ label: 'Settings' }]} />
      <div className="flex flex-1 overflow-hidden">
        <SettingsRail active={panel} onSelect={setPanel} />
        <main className="flex-1 overflow-y-auto p-8">
          {panel === 'core'     && <SettingsCore     nodeInfo={nodeInfo} />}
          {panel === 'services' && <SettingsServices  nodeInfo={nodeInfo} />}
          {panel === 'doctor'   && <SettingsDoctor />}
          {panel === 'extract'  && <SettingsExtract />}
          {(panel === 'surfaces' || panel === 'features' || panel === 'advanced') && (
            <SettingsStub panel={panel} />
          )}
        </main>
      </div>
    </div>
  )
}
```

- [ ] **Commit:**

```bash
git add apps/gateway-admin/components/settings/settings-layout.tsx
git commit -m "feat(settings): SettingsLayout shell"
```

---

## Task 3: Core panel

**Files:**
- Create: `apps/gateway-admin/components/settings/panels/settings-core.tsx`

- [ ] **Create:**

```tsx
// apps/gateway-admin/components/settings/panels/settings-core.tsx
'use client'
import { useState } from 'react'
import { useForm } from 'react-hook-form'
import { zodResolver } from '@hookform/resolvers/zod'
import { toast } from 'sonner'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import {
  Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
} from '@/components/ui/select'
import { AURORA_DISPLAY_1, AURORA_MUTED_LABEL, AURORA_MEDIUM_PANEL } from '@/components/aurora/tokens'
import { cn } from '@/lib/utils'
import { CoreConfigSchema, type CoreConfig } from '@/lib/setup/schema'
import type { NodeInfo } from '@/lib/api/nodeinfo-client'

interface Props { nodeInfo: NodeInfo | undefined }

export function SettingsCore({ nodeInfo }: Props) {
  const env = nodeInfo?.env ?? {}
  const [saving, setSaving] = useState(false)

  const { register, watch, setValue, handleSubmit } = useForm<CoreConfig>({
    resolver: zodResolver(CoreConfigSchema.partial()),
    defaultValues: {
      bindHost:  env.LAB_MCP_HTTP_HOST ?? '127.0.0.1',
      port:      Number(env.LAB_MCP_HTTP_PORT ?? 8765),
      logLevel:  env.LAB_LOG            ?? 'labby=info,lab_apis=warn',
      logFormat: (env.LAB_LOG_FORMAT as 'text' | 'json') ?? 'text',
    },
  })

  const authMode = env.LAB_AUTH_MODE === 'oauth' || env.LAB_GOOGLE_CLIENT_ID ? 'OAuth (Google)' : 'Bearer Token'

  async function onSubmit(data: Partial<CoreConfig>) {
    setSaving(true)
    await new Promise(r => setTimeout(r, 600))
    // TODO: POST to setup.draft.set + setup.draft.commit when lab-bg3e.3 ships
    console.log('[settings/core] save:', data)
    toast.success('Core settings saved')
    setSaving(false)
  }

  return (
    <div className="space-y-6 max-w-[640px]">
      <div>
        <h1 className={cn(AURORA_DISPLAY_1, 'text-aurora-text-primary')}>Core</h1>
        <p className="mt-1 text-[13px] text-aurora-text-muted">
          Server bind address, ports, authentication, and logging configuration.
        </p>
      </div>

      <form onSubmit={handleSubmit(onSubmit)} className="space-y-5">
        {/* Server */}
        <section className={cn(AURORA_MEDIUM_PANEL, 'p-5 space-y-4')}>
          <p className={AURORA_MUTED_LABEL}>Server</p>

          <div className="grid grid-cols-2 gap-4">
            <div className="space-y-1.5">
              <Label className="text-[12px] font-semibold">
                Bind Address <code className="ml-1 text-[10px] text-aurora-text-muted">LAB_MCP_HTTP_HOST</code>
              </Label>
              <Input {...register('bindHost')} className="bg-aurora-control-surface border-aurora-border-strong" />
            </div>
            <div className="space-y-1.5">
              <Label className="text-[12px] font-semibold">
                Port <code className="ml-1 text-[10px] text-aurora-text-muted">LAB_MCP_HTTP_PORT</code>
              </Label>
              <Input
                {...register('port', { valueAsNumber: true })}
                type="number"
                className="bg-aurora-control-surface border-aurora-border-strong [appearance:textfield] [&::-webkit-outer-spin-button]:appearance-none [&::-webkit-inner-spin-button]:appearance-none"
              />
              <p className="text-[11px] text-aurora-text-muted">Shared port for Web UI, HTTP API, and MCP server.</p>
            </div>
          </div>

          <div className="grid grid-cols-2 gap-4">
            <div className="space-y-1.5">
              <Label className="text-[12px] font-semibold">
                Log Level <code className="ml-1 text-[10px] text-aurora-text-muted">LAB_LOG</code>
              </Label>
              <Select value={watch('logLevel')} onValueChange={v => setValue('logLevel', v)}>
                <SelectTrigger className="bg-aurora-control-surface border-aurora-border-strong">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="labby=error,lab_apis=error">error</SelectItem>
                  <SelectItem value="labby=warn,lab_apis=warn">warn</SelectItem>
                  <SelectItem value="labby=info,lab_apis=warn">info (default)</SelectItem>
                  <SelectItem value="labby=debug,lab_apis=debug">debug</SelectItem>
                  <SelectItem value="labby=trace,lab_apis=trace">trace</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div className="space-y-1.5">
              <Label className="text-[12px] font-semibold">
                Log Format <code className="ml-1 text-[10px] text-aurora-text-muted">LAB_LOG_FORMAT</code>
              </Label>
              <Select value={watch('logFormat')} onValueChange={v => setValue('logFormat', v as 'text' | 'json')}>
                <SelectTrigger className="bg-aurora-control-surface border-aurora-border-strong">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="text">text (human-readable)</SelectItem>
                  <SelectItem value="json">json (structured)</SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>
        </section>

        {/* Authentication — read-only display */}
        <section className={cn(AURORA_MEDIUM_PANEL, 'p-5 space-y-3')}>
          <p className={AURORA_MUTED_LABEL}>Authentication</p>
          <div className="flex items-center gap-3">
            <span className="text-[13px] text-aurora-text-muted">Auth Mode</span>
            <Badge variant="outline" className="text-aurora-accent-strong border-aurora-accent-primary/30 bg-aurora-accent-primary/10">
              {authMode}
            </Badge>
          </div>
          {env.LAB_PUBLIC_URL && (
            <div className="text-[12px] text-aurora-text-muted">
              Public URL: <code className="text-aurora-text-primary">{env.LAB_PUBLIC_URL}</code>
            </div>
          )}
          <p className="text-[11px] text-aurora-text-muted">
            To change auth mode, re-run Setup or edit <code>~/.labby/config.toml</code>.
          </p>
        </section>

        <Button type="submit" disabled={saving} className="bg-aurora-accent-primary text-aurora-page-bg hover:bg-aurora-accent-strong">
          {saving ? 'Saving…' : 'Save Changes'}
        </Button>
      </form>
    </div>
  )
}
```

- [ ] **Commit:**

```bash
git add apps/gateway-admin/components/settings/panels/settings-core.tsx
git commit -m "feat(settings): Core panel with live env pre-population"
```

---

## Task 4: Services panel

**Files:**
- Create: `apps/gateway-admin/components/settings/panels/settings-services.tsx`

- [ ] **Create** — service table with inline expand:

```tsx
// apps/gateway-admin/components/settings/panels/settings-services.tsx
'use client'
import { useState } from 'react'
import Image from 'next/image'
import { ChevronDown, ChevronRight } from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { ServiceForm } from '@/components/setup/service-form'
import { SERVICES, SERVICES_BY_ID } from '@/lib/setup/services-catalog'
import { SERVICE_BRANDS, SERVICE_LOGOS, type ServiceKey } from '@/lib/branding/service-brands'
import { AURORA_DISPLAY_1, AURORA_MUTED_LABEL } from '@/components/aurora/tokens'
import { cn } from '@/lib/utils'
import type { NodeInfo } from '@/lib/api/nodeinfo-client'

interface Props { nodeInfo: NodeInfo | undefined }

function ServiceIcon({ id, size = 34 }: { id: string; size?: number }) {
  const svc = SERVICES_BY_ID[id]
  const color = SERVICE_BRANDS[id as ServiceKey] ?? '#29b6f6'
  const logo  = svc?.slug ? SERVICE_LOGOS[id as ServiceKey] : null
  return (
    <div
      className="rounded-[8px] flex items-center justify-center overflow-hidden flex-shrink-0"
      style={{ width: size, height: size, background: `${color}22`, border: `1px solid ${color}44` }}
    >
      {logo ? (
        <Image
          src={logo.replace('/png/', '/svg/').replace('.png', '.svg')}
          alt={svc?.name ?? id}
          width={Math.round(size * 0.65)}
          height={Math.round(size * 0.65)}
          unoptimized
          onError={e => {
            const img = e.currentTarget as HTMLImageElement
            if (img.src.includes('/svg/')) img.src = logo
          }}
        />
      ) : (
        <span style={{ fontSize: Math.round(size * 0.4), fontWeight: 800, color }}>
          {svc?.name[0] ?? '?'}
        </span>
      )}
    </div>
  )
}

export function SettingsServices({ nodeInfo }: Props) {
  const env = nodeInfo?.env ?? {}
  const [expanded, setExpanded] = useState<string | null>(null)
  const [enabled, setEnabled] = useState<Record<string, boolean>>(
    Object.fromEntries(SERVICES.map(s => [s.id, !!env[s.fields[0]?.envKey ?? '']]))
  )
  const [drafts, setDrafts] = useState<Record<string, Record<string, string>>>(
    Object.fromEntries(SERVICES.map(s => [s.id, Object.fromEntries(s.fields.map(f => [f.envKey, env[f.envKey] ?? '']))]))
  )
  const [testStates, setTestStates] = useState<Record<string, 'idle' | 'testing' | 'ok' | 'fail'>>({})
  const [savingId, setSavingId] = useState<string | null>(null)

  async function handleTest(id: string) {
    setTestStates(p => ({ ...p, [id]: 'testing' }))
    await new Promise(r => setTimeout(r, 800 + Math.random() * 500))
    setTestStates(p => ({ ...p, [id]: Math.random() > 0.15 ? 'ok' : 'fail' }))
  }

  async function handleSave(id: string) {
    setSavingId(id)
    await new Promise(r => setTimeout(r, 500))
    // TODO: POST to setup.draft.set + setup.draft.commit when lab-bg3e.3 ships
    console.log('[settings/services] save:', id, drafts[id])
    toast.success(`${SERVICES_BY_ID[id]?.name ?? id} saved`)
    setSavingId(null)
  }

  // Group services by category
  const categories = Array.from(new Set(SERVICES.map(s => s.category)))

  return (
    <div className="space-y-6 max-w-[760px]">
      <div>
        <h1 className={cn(AURORA_DISPLAY_1, 'text-aurora-text-primary')}>Services</h1>
        <p className="mt-1 text-[13px] text-aurora-text-muted">
          Manage your 21 integrated homelab services. Enable, configure, and test connections.
        </p>
      </div>

      {/* Table header */}
      <div className="rounded-aurora-3 border border-aurora-border-strong overflow-hidden">
        <div className="grid grid-cols-[1fr_140px_80px_120px] gap-2 px-4 py-2 bg-aurora-panel-strong border-b border-aurora-border-default">
          <p className={AURORA_MUTED_LABEL}>Service</p>
          <p className={AURORA_MUTED_LABEL}>Status</p>
          <p className={AURORA_MUTED_LABEL}>Enabled</p>
          <p className={AURORA_MUTED_LABEL}>Actions</p>
        </div>

        {categories.map(cat => {
          const catServices = SERVICES.filter(s => s.category === cat)
          return (
            <div key={cat}>
              {catServices.map((svc, idx) => {
                const isExpanded = expanded === svc.id
                const isConfigured = Object.values(drafts[svc.id] ?? {}).some(v => v && v !== '***')
                const isFirst = idx === 0

                return (
                  <div key={svc.id} className={cn('border-b border-aurora-border-default last:border-b-0', isExpanded && 'bg-[color-mix(in_srgb,var(--aurora-accent-primary)_4%,transparent)]')}>
                    {/* Row */}
                    <div
                      className="grid grid-cols-[1fr_140px_80px_120px] gap-2 px-4 py-3 items-center cursor-pointer hover:bg-aurora-hover-bg transition-colors"
                      onClick={() => setExpanded(isExpanded ? null : svc.id)}
                    >
                      <div className="flex items-center gap-3">
                        {isFirst && <p className={cn(AURORA_MUTED_LABEL, 'absolute -mt-8 text-[9px]')}>{/* category shown in group header */}</p>}
                        <ServiceIcon id={svc.id} size={32} />
                        <div>
                          <p className="text-[13px] font-semibold text-aurora-text-primary">{svc.name}</p>
                          <p className="text-[10px] font-bold uppercase tracking-wide text-aurora-text-muted">{svc.category}</p>
                        </div>
                      </div>

                      <div className="flex items-center gap-1.5">
                        <div className={cn('size-[6px] rounded-full', isConfigured ? 'bg-aurora-success' : 'bg-aurora-border-strong')} />
                        <span className={cn('text-[12px]', isConfigured ? 'text-aurora-success' : 'text-aurora-text-muted')}>
                          {isConfigured ? 'Configured' : 'Not configured'}
                        </span>
                      </div>

                      <div onClick={e => e.stopPropagation()}>
                        <Switch
                          checked={enabled[svc.id] ?? false}
                          onCheckedChange={v => setEnabled(p => ({ ...p, [svc.id]: v }))}
                          className="data-[state=checked]:bg-aurora-accent-primary"
                        />
                      </div>

                      <div className="flex items-center gap-2" onClick={e => e.stopPropagation()}>
                        <Button
                          variant="outline"
                          size="sm"
                          onClick={() => setExpanded(isExpanded ? null : svc.id)}
                          className="text-[12px] h-7 px-2.5"
                        >
                          {isExpanded ? <ChevronDown className="size-3.5 mr-1" /> : <ChevronRight className="size-3.5 mr-1" />}
                          Configure
                        </Button>
                      </div>
                    </div>

                    {/* Expanded form */}
                    {isExpanded && (
                      <div className="px-5 pb-4 pt-1 border-t border-aurora-border-default bg-aurora-panel-medium">
                        <ServiceForm
                          service={svc}
                          values={drafts[svc.id] ?? {}}
                          onChange={(envKey, value) => setDrafts(p => ({ ...p, [svc.id]: { ...(p[svc.id] ?? {}), [envKey]: value } }))}
                          onTest={() => handleTest(svc.id)}
                          testState={testStates[svc.id]}
                        />
                        <div className="flex items-center justify-between mt-4 pt-3 border-t border-aurora-border-default">
                          <p className="text-[11px] text-aurora-text-muted">
                            Saves directly to <code>~/.labby/.env</code> — effective on next restart.
                          </p>
                          <Button
                            size="sm"
                            onClick={() => handleSave(svc.id)}
                            disabled={savingId === svc.id}
                            className="bg-aurora-accent-primary text-aurora-page-bg hover:bg-aurora-accent-strong"
                          >
                            {savingId === svc.id ? 'Saving…' : 'Save'}
                          </Button>
                        </div>
                      </div>
                    )}
                  </div>
                )
              })}
            </div>
          )
        })}
      </div>
    </div>
  )
}
```

- [ ] **Commit:**

```bash
git add apps/gateway-admin/components/settings/panels/settings-services.tsx
git commit -m "feat(settings): Services panel with inline expand + ServiceForm"
```

---

## Task 5: Doctor panel

**Files:**
- Create: `apps/gateway-admin/components/settings/panels/settings-doctor.tsx`

- [ ] **Create:**

```tsx
// apps/gateway-admin/components/settings/panels/settings-doctor.tsx
'use client'
import Image from 'next/image'
import { CheckCircle, AlertTriangle, XCircle } from 'lucide-react'
import { AURORA_DISPLAY_1, AURORA_DISPLAY_NUMBER, AURORA_MEDIUM_PANEL, AURORA_MUTED_LABEL } from '@/components/aurora/tokens'
import { cn } from '@/lib/utils'
import { SERVICES } from '@/lib/setup/services-catalog'
import { SERVICE_BRANDS, SERVICE_LOGOS, type ServiceKey } from '@/lib/branding/service-brands'

// Mock audit data — replaced by real doctor.audit.full SSE stream when lab-bg3e.2 wires it
const MOCK_AUDIT = SERVICES.map((svc, i) => ({
  id:      svc.id,
  name:    svc.name,
  slug:    svc.slug,
  status:  i < 14 ? 'pass' as const : i < 16 ? 'warn' as const : 'fail' as const,
  detail:  i < 14 ? `v5.${i}.0 — auth OK`
           : i < 16 ? 'URL set but no API key configured'
           : 'URL not configured',
}))

const passing  = MOCK_AUDIT.filter(s => s.status === 'pass').length
const warnings = MOCK_AUDIT.filter(s => s.status === 'warn').length
const failing  = MOCK_AUDIT.filter(s => s.status === 'fail').length

export function SettingsDoctor() {
  return (
    <div className="space-y-6 max-w-[760px]">
      <div>
        <h1 className={cn(AURORA_DISPLAY_1, 'text-aurora-text-primary')}>Doctor</h1>
        <p className="mt-1 text-[13px] text-aurora-text-muted">
          Health audit — reachability, auth, and version checks for every configured service.
        </p>
      </div>

      {/* Summary cards */}
      <div className="grid grid-cols-4 gap-3">
        {[
          { label: 'Services Checked', value: MOCK_AUDIT.length, color: 'text-aurora-text-primary' },
          { label: 'Passing',          value: passing,            color: 'text-aurora-success'      },
          { label: 'Warnings',         value: warnings,           color: 'text-aurora-warn'         },
          { label: 'Failing',          value: failing,            color: 'text-aurora-error'        },
        ].map(card => (
          <div key={card.label} className={cn(AURORA_MEDIUM_PANEL, 'px-4 py-4 text-center')}>
            <p className={cn(AURORA_DISPLAY_NUMBER, 'text-[28px]', card.color)}>{card.value}</p>
            <p className={cn(AURORA_MUTED_LABEL, 'mt-1')}>{card.label}</p>
          </div>
        ))}
      </div>

      {/* Per-service audit list */}
      <div className="rounded-aurora-3 border border-aurora-border-strong overflow-hidden">
        <div className="px-4 py-2 bg-aurora-panel-strong border-b border-aurora-border-default">
          <p className={AURORA_MUTED_LABEL}>Service Audit</p>
        </div>
        {MOCK_AUDIT.map(svc => {
          const color = SERVICE_BRANDS[svc.id as ServiceKey] ?? '#29b6f6'
          const logo  = svc.slug ? SERVICE_LOGOS[svc.id as ServiceKey] : null
          return (
            <div key={svc.id} className="flex items-center gap-3 px-4 py-3 border-b border-aurora-border-default last:border-b-0 hover:bg-aurora-hover-bg transition-colors">
              <div
                className="size-[30px] rounded-[7px] flex items-center justify-center flex-shrink-0"
                style={{ background: `${color}22`, border: `1px solid ${color}44` }}
              >
                {logo ? (
                  <Image src={logo.replace('/png/', '/svg/').replace('.png', '.svg')} alt={svc.name} width={18} height={18} unoptimized onError={e => { (e.currentTarget as HTMLImageElement).src = logo! }} />
                ) : (
                  <span style={{ fontSize: 12, fontWeight: 800, color }}>{svc.name[0]}</span>
                )}
              </div>

              <div className="flex-1 min-w-0">
                <p className="text-[13px] font-medium text-aurora-text-primary">{svc.name}</p>
                <p className="text-[11px] text-aurora-text-muted truncate">{svc.detail}</p>
              </div>

              <div className="flex-shrink-0">
                {svc.status === 'pass' && <CheckCircle   className="size-4 text-aurora-success" />}
                {svc.status === 'warn' && <AlertTriangle className="size-4 text-aurora-warn"    />}
                {svc.status === 'fail' && <XCircle       className="size-4 text-aurora-error"   />}
              </div>

              <span className={cn('text-[11px] font-semibold flex-shrink-0 w-8 text-right',
                svc.status === 'pass' && 'text-aurora-success',
                svc.status === 'warn' && 'text-aurora-warn',
                svc.status === 'fail' && 'text-aurora-error',
              )}>
                {svc.status}
              </span>
            </div>
          )
        })}
      </div>
    </div>
  )
}
```

- [ ] **Commit:**

```bash
git add apps/gateway-admin/components/settings/panels/settings-doctor.tsx
git commit -m "feat(settings): Doctor panel with mock audit data"
```

---

## Task 6: Extract panel

**Files:**
- Create: `apps/gateway-admin/components/settings/panels/settings-extract.tsx`

- [ ] **Create:**

```tsx
// apps/gateway-admin/components/settings/panels/settings-extract.tsx
'use client'
import { useState } from 'react'
import { Search, Check } from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import {
  Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
} from '@/components/ui/table'
import { Checkbox } from '@/components/ui/checkbox'
import { Spinner } from '@/components/ui/spinner'
import { AURORA_DISPLAY_1, AURORA_MEDIUM_PANEL } from '@/components/aurora/tokens'
import { cn } from '@/lib/utils'

interface DiscoveredCred {
  key: string
  value: string
  source: string
  include: boolean
}

// Mock data representing extract.scan results
const MOCK_CREDS: DiscoveredCred[] = [
  { key: 'RADARR_URL',        value: 'http://100.64.0.11:7878', source: 'docker label', include: true  },
  { key: 'SONARR_URL',        value: 'https://sonarr.example.com',   source: 'docker label', include: true  },
  { key: 'RADARR_API_KEY',    value: '••••••••••••••••',           source: 'docker label', include: true  },
  { key: 'PLEX_TOKEN',        value: '••••••••••••••••',           source: '.env file',    include: true  },
  { key: 'UNRAID_API_KEY',    value: '••••••••••••••••',           source: 'config file',  include: false },
]

export function SettingsExtract() {
  const [scanning, setScanning] = useState(false)
  const [creds, setCreds] = useState<DiscoveredCred[] | null>(null)
  const [applying, setApplying] = useState(false)
  const [applied, setApplied] = useState(false)

  async function handleScan() {
    setScanning(true)
    setApplied(false)
    await new Promise(r => setTimeout(r, 2000))
    setCreds(MOCK_CREDS)
    setScanning(false)
  }

  async function handleApply() {
    setApplying(true)
    await new Promise(r => setTimeout(r, 800))
    // TODO: call extract.apply when lab-bg3e.3 ships
    console.log('[settings/extract] apply:', creds?.filter(c => c.include))
    toast.success('Credentials applied to ~/.labby/.env')
    setApplied(true)
    setApplying(false)
  }

  function toggleInclude(key: string) {
    setCreds(prev => prev?.map(c => c.key === key ? { ...c, include: !c.include } : c) ?? null)
  }

  return (
    <div className="space-y-6 max-w-[760px]">
      <div>
        <h1 className={cn(AURORA_DISPLAY_1, 'text-aurora-text-primary')}>Extract</h1>
        <p className="mt-1 text-[13px] text-aurora-text-muted">
          Scan local and SSH hosts for service credentials, then apply to{' '}
          <code className="text-[12px]">~/.labby/.env</code>.
        </p>
      </div>

      <div className={cn(AURORA_MEDIUM_PANEL, 'p-5 space-y-4')}>
        <div className="flex items-center gap-4 flex-wrap">
          <Button
            variant="secondary"
            onClick={handleScan}
            disabled={scanning}
          >
            {scanning ? <Spinner className="mr-2 size-4" /> : <Search className="size-4 mr-2" />}
            {scanning ? 'Scanning…' : 'Scan for Credentials'}
          </Button>
          <p className="text-[12px] text-aurora-text-muted">
            Searches Docker labels, config files, and .env files across configured hosts.
          </p>
        </div>

        {creds !== null && (
          <div className="space-y-3">
            <div className="flex items-center justify-between">
              <p className="text-[13px] font-semibold text-aurora-text-primary">
                Discovered Credentials
              </p>
              <span className="text-[11px] font-bold text-aurora-success bg-aurora-success/10 border border-aurora-success/25 px-2 py-0.5 rounded-full">
                {creds.length} found
              </span>
            </div>

            <div className="rounded-[8px] border border-aurora-border-strong overflow-hidden">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead className="text-[11px]">Key</TableHead>
                    <TableHead className="text-[11px]">Value</TableHead>
                    <TableHead className="text-[11px]">Source</TableHead>
                    <TableHead className="text-[11px] w-[60px]">Include</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {creds.map(cred => (
                    <TableRow key={cred.key}>
                      <TableCell className="text-[12px] font-mono">{cred.key}</TableCell>
                      <TableCell className="text-[12px] text-aurora-text-muted font-mono max-w-[180px] truncate">{cred.value}</TableCell>
                      <TableCell className="text-[11px] text-aurora-text-muted">{cred.source}</TableCell>
                      <TableCell>
                        <Checkbox
                          checked={cred.include}
                          onCheckedChange={() => toggleInclude(cred.key)}
                          className="data-[state=checked]:bg-aurora-accent-primary"
                        />
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>

            <div className="flex items-center gap-3">
              <Button
                onClick={handleApply}
                disabled={applying || applied}
                className="bg-aurora-accent-primary text-aurora-page-bg hover:bg-aurora-accent-strong"
              >
                {applying ? 'Applying…' : applied ? <><Check className="size-4 mr-1.5" /> Applied</> : 'Apply to ~/.labby/.env'}
              </Button>
              <Button variant="ghost" size="sm" className="text-aurora-text-muted">
                Preview Diff
              </Button>
            </div>
          </div>
        )}
      </div>
    </div>
  )
}
```

- [ ] **Commit:**

```bash
git add apps/gateway-admin/components/settings/panels/settings-extract.tsx
git commit -m "feat(settings): Extract panel with scan + apply flow"
```

---

## Task 7: v2 stub panel

**Files:**
- Create: `apps/gateway-admin/components/settings/panels/settings-stub.tsx`

- [ ] **Create:**

```tsx
// apps/gateway-admin/components/settings/panels/settings-stub.tsx
import { Monitor, Zap, LayoutGrid } from 'lucide-react'
import { AURORA_DISPLAY_1 } from '@/components/aurora/tokens'
import { cn } from '@/lib/utils'
import type { SettingsPanel } from '../settings-rail'

const STUB_CONFIG: Record<string, { title: string; icon: React.ReactNode; desc: string }> = {
  surfaces: {
    title: 'Surfaces',
    icon: <Monitor className="size-8 text-aurora-text-muted" />,
    desc: 'Configure which interfaces Labby exposes: CLI, MCP server, REST API, Web UI, and TUI. Coming in v2.',
  },
  features: {
    title: 'Features',
    icon: <Zap className="size-8 text-aurora-text-muted" />,
    desc: 'Enable or disable Labby features: Marketplace, Gateway, MCP Registry, ACP, Chat, Editor, Activity. Coming in v2.',
  },
  advanced: {
    title: 'Advanced',
    icon: <LayoutGrid className="size-8 text-aurora-text-muted" />,
    desc: 'Raw editor for ~/.labby/.env and ~/.labby/config.toml, log retention, workspace root, and other advanced settings. Coming in v2.',
  },
}

interface Props { panel: SettingsPanel }

export function SettingsStub({ panel }: Props) {
  const cfg = STUB_CONFIG[panel]
  if (!cfg) return null

  return (
    <div className="flex flex-col items-center justify-center text-center py-24 gap-4">
      {cfg.icon}
      <h1 className={cn(AURORA_DISPLAY_1, 'text-aurora-text-primary')}>{cfg.title}</h1>
      <p className="text-[14px] text-aurora-text-muted max-w-[420px] leading-[1.7]">{cfg.desc}</p>
      <span className="text-[11px] font-bold uppercase tracking-wide text-aurora-text-muted border border-aurora-border-strong bg-aurora-control-surface px-3 py-1 rounded-full">
        Coming in v2
      </span>
    </div>
  )
}
```

- [ ] **Commit:**

```bash
git add apps/gateway-admin/components/settings/panels/settings-stub.tsx
git commit -m "feat(settings): v2 stub panels (Surfaces, Features, Advanced)"
```

---

## Task 8: Wire the settings page

**Files:**
- Modify: `apps/gateway-admin/app/(admin)/settings/page.tsx`

- [ ] **Replace the existing page:**

```tsx
// apps/gateway-admin/app/(admin)/settings/page.tsx
import { SettingsLayout } from '@/components/settings/settings-layout'

export const metadata = { title: 'Settings — Labby' }

export default function SettingsPage() {
  return <SettingsLayout />
}
```

- [ ] **Build and verify:**

```bash
cd apps/gateway-admin && pnpm build 2>&1 | tail -20
```

Expected: `/settings` in route listing, no errors.

- [ ] **Type check:**

```bash
cd apps/gateway-admin && pnpm exec tsc --noEmit 2>&1 | head -40
```

Fix any type errors. Common issues:
- `ServiceKey` type narrowing for `SERVICE_BRANDS` lookups — use `as ServiceKey` where needed
- Image `src` type when `logo` could be `string | null` — add null check or non-null assertion
- Missing `'use client'` directive on components that use hooks

- [ ] **Manual smoke test:**

```bash
cd apps/gateway-admin && pnpm dev
```

Visit `http://localhost:3000/settings/`. Verify:
1. Settings rail renders with Config / System / v2 Stubs sections
2. Services panel (default) shows 21-service table with icons
3. Click "Configure" on Radarr → inline form expands with URL pre-populated from env
4. Click Core rail item → fields pre-populated from nodeinfo
5. Click Doctor → summary cards + audit list
6. Click Extract → Scan button → animated results table → Apply button
7. Click Surfaces (v2) → coming-soon placeholder

- [ ] **Commit:**

```bash
git add apps/gateway-admin/app/\(admin\)/settings/page.tsx
git commit -m "feat(settings): replace stub page with full settings rail implementation"
```

---

## Task 9: generateStaticParams for dynamic service routes (if added later)

> **Note:** The current implementation is a single `/settings/` page with client-side panel switching — no dynamic routes. If `/settings/services/[service]/page.tsx` is added later, it will need `generateStaticParams()` returning the 21 service slugs. Defer this until the design requires deep-linkable service URLs.

No action needed now.

---

## Deferred

These require lab-bg3e.3 (setup dispatch service) to ship:
- Real `setup.draft.set` / `setup.draft.commit` on Save buttons (currently console.log)
- Real `doctor.audit.full` SSE stream in Doctor panel (currently mock data)
- Real `extract.scan` + `extract.apply` in Extract panel (currently mock)
- Doctor panel "click failing service → navigate to /settings/services/{slug}?focus={field}" deep-link
- Draft-stale banner when another session has modified config
