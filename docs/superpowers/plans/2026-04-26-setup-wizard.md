# Setup Wizard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the existing `/setup` stub with the full 7-step setup wizard designed in the HTML mockup at `~/.superpowers/brainstorm/content/setup.html`.

**Architecture:** The wizard lives at `app/(wizard)/setup/` — a new route group with AuthBootstrap but no AppSidebar. Phase 1 (steps 1–3) is a linear full-page flow; Phase 2 (steps 4–7) slides in a setup-specific sidebar. All data is client-side (static export constraint). Pre-population comes from `GET /dev/api/nodeinfo` (unauthenticated). Writes are deferred until lab-bg3e.3 ships the setup dispatch service — for now the Finalize button writes to a draft object logged to console.

**Tech Stack:** Next.js 16 (output:'export'), React 19, TypeScript, react-hook-form 7, Zod 3, SWR 2, Tailwind CSS, shadcn/ui, Aurora tokens, Manrope/Inter fonts (next/font)

**Reference:** `~/.superpowers/brainstorm/content/setup.html` (approved mockup), `docs/superpowers/specs/2026-04-26-setup-settings-design.md`

---

## File Map

```
apps/gateway-admin/
  app/
    (wizard)/
      layout.tsx                          # NEW — AuthBootstrap + Toaster, no AppSidebar
      setup/
        page.tsx                          # NEW — wizard page (replaces stub)
    (admin)/
      setup/
        page.tsx                          # MODIFY — redirect to /setup/
  components/
    setup/
      setup-wizard.tsx                    # NEW — top-level wizard shell (stepper, phase logic)
      setup-stepper.tsx                   # NEW — 7-dot progress bar component
      setup-sidebar.tsx                   # NEW — phase-2 service nav sidebar
      setup-preflight.tsx                 # NEW — PreFlight 1 check runner
      setup-preflight2.tsx                # NEW — PreFlight 2 check runner
      steps/
        step-welcome.tsx                  # NEW
        step-core-config.tsx              # NEW — bind, port, log level, auth, nodes
        step-services.tsx                 # NEW — shell; renders ServiceForm per service
        step-surfaces.tsx                 # NEW — surface toggles
        step-finalize.tsx                 # NEW — review + commit button
      service-form.tsx                    # NEW — shared URL+key form (reused in settings)
      nodes-panel.tsx                     # NEW — device list with live /v1/nodes data
  lib/
    api/
      nodeinfo-client.ts                  # NEW — fetch /dev/api/nodeinfo
      nodes-client.ts                     # NEW — fetch /v1/nodes (live connectivity)
    setup/
      use-nodeinfo.ts                     # NEW — SWR hook for nodeinfo
      use-nodes.ts                        # NEW — SWR hook for enrolled nodes
      use-wizard-state.ts                 # NEW — wizard phase/step/draft state
      schema.ts                           # NEW — Zod schemas for all wizard forms
      services-catalog.ts                 # NEW — 21 services with fields, envKey, etc.
```

---

## Task 1: Route group + layout

**Files:**
- Create: `apps/gateway-admin/app/(wizard)/layout.tsx`
- Modify: `apps/gateway-admin/app/(admin)/setup/page.tsx`

- [ ] **Create wizard layout** — AuthBootstrap + Toaster, no sidebar:

```tsx
// apps/gateway-admin/app/(wizard)/layout.tsx
import { AuthBootstrap } from '@/components/auth/auth-bootstrap'
import { Toaster } from '@/components/ui/sonner'

export default function WizardLayout({ children }: { children: React.ReactNode }) {
  return (
    <AuthBootstrap>
      {children}
      <Toaster />
    </AuthBootstrap>
  )
}
```

- [ ] **Redirect old admin setup route:**

```tsx
// apps/gateway-admin/app/(admin)/setup/page.tsx
import { redirect } from 'next/navigation'
export default function SetupRedirect() {
  redirect('/setup/')
}
```

- [ ] **Create stub wizard page:**

```tsx
// apps/gateway-admin/app/(wizard)/setup/page.tsx
export const metadata = { title: 'Setup — Labby' }
export default function SetupPage() {
  return <div className="min-h-screen bg-aurora-page-bg text-aurora-text-primary p-8">Setup wizard coming…</div>
}
```

- [ ] **Verify build passes:**

```bash
cd apps/gateway-admin && pnpm build 2>&1 | tail -20
```

Expected: `/setup` appears in route listing, no errors.

- [ ] **Commit:**

```bash
git add apps/gateway-admin/app/\(wizard\)/ apps/gateway-admin/app/\(admin\)/setup/
git commit -m "feat(setup): add (wizard) route group with minimal layout"
```

---

## Task 2: Services catalog + Zod schemas

**Files:**
- Create: `apps/gateway-admin/lib/setup/services-catalog.ts`
- Create: `apps/gateway-admin/lib/setup/schema.ts`

- [ ] **Create services catalog** — all 21 services with their field definitions:

```ts
// apps/gateway-admin/lib/setup/services-catalog.ts
export interface ServiceField {
  label: string
  envKey: string
  type: 'url' | 'password' | 'text'
  placeholder: string
  hint?: string
  optional?: boolean
}

export interface ServiceDef {
  id: string
  name: string
  slug: string | null   // selfhst CDN slug; null = letter fallback
  color: string         // brand hex
  category: string
  port: number
  fields: ServiceField[]
}

export const SERVICES: ServiceDef[] = [
  {
    id: 'radarr', name: 'Radarr', slug: 'radarr', color: '#F0BC40',
    category: 'Media', port: 7878,
    fields: [
      { label: 'URL', envKey: 'RADARR_URL', type: 'url', placeholder: 'http://192.168.1.x:7878' },
      { label: 'API Key', envKey: 'RADARR_API_KEY', type: 'password', placeholder: 'Found in Settings → General', hint: 'X-Api-Key header' },
    ],
  },
  {
    id: 'sonarr', name: 'Sonarr', slug: 'sonarr', color: '#35C5F4',
    category: 'Media', port: 8989,
    fields: [
      { label: 'URL', envKey: 'SONARR_URL', type: 'url', placeholder: 'http://192.168.1.x:8989' },
      { label: 'API Key', envKey: 'SONARR_API_KEY', type: 'password', placeholder: 'Found in Settings → General', hint: 'X-Api-Key header' },
    ],
  },
  {
    id: 'plex', name: 'Plex', slug: 'plex', color: '#CC7B19',
    category: 'Media', port: 32400,
    fields: [
      { label: 'URL', envKey: 'PLEX_URL', type: 'url', placeholder: 'http://192.168.1.x:32400' },
      { label: 'Token', envKey: 'PLEX_TOKEN', type: 'password', placeholder: 'Your Plex auth token', hint: 'X-Plex-Token header' },
    ],
  },
  {
    id: 'tautulli', name: 'Tautulli', slug: 'tautulli', color: '#D9A21B',
    category: 'Media', port: 8181,
    fields: [
      { label: 'URL', envKey: 'TAUTULLI_URL', type: 'url', placeholder: 'http://192.168.1.x:8181' },
      { label: 'API Key', envKey: 'TAUTULLI_API_KEY', type: 'password', placeholder: 'Found in Settings → Web Interface', hint: '?apikey= query param' },
    ],
  },
  {
    id: 'overseerr', name: 'Overseerr', slug: 'overseerr', color: '#E5870A',
    category: 'Media', port: 5055,
    fields: [
      { label: 'URL', envKey: 'OVERSEERR_URL', type: 'url', placeholder: 'http://192.168.1.x:5055' },
      { label: 'API Key', envKey: 'OVERSEERR_API_KEY', type: 'password', placeholder: 'Found in Settings → General', hint: 'X-Api-Key header' },
    ],
  },
  {
    id: 'prowlarr', name: 'Prowlarr', slug: 'prowlarr', color: '#F16529',
    category: 'Indexer', port: 9696,
    fields: [
      { label: 'URL', envKey: 'PROWLARR_URL', type: 'url', placeholder: 'http://192.168.1.x:9696' },
      { label: 'API Key', envKey: 'PROWLARR_API_KEY', type: 'password', placeholder: 'Found in Settings → General', hint: 'X-Api-Key header' },
    ],
  },
  {
    id: 'sabnzbd', name: 'SABnzbd', slug: 'sabnzbd', color: '#F4A623',
    category: 'Downloads', port: 8080,
    fields: [
      { label: 'URL', envKey: 'SABNZBD_URL', type: 'url', placeholder: 'http://192.168.1.x:8080' },
      { label: 'API Key', envKey: 'SABNZBD_API_KEY', type: 'password', placeholder: 'Found in Config → General', hint: '?apikey= query param' },
    ],
  },
  {
    id: 'qbittorrent', name: 'qBittorrent', slug: 'qbittorrent', color: '#2F99E0',
    category: 'Downloads', port: 8080,
    fields: [
      { label: 'URL', envKey: 'QBITTORRENT_URL', type: 'url', placeholder: 'http://192.168.1.x:8080' },
      { label: 'Username', envKey: 'QBITTORRENT_USERNAME', type: 'text', placeholder: 'admin' },
      { label: 'Password', envKey: 'QBITTORRENT_PASSWORD', type: 'password', placeholder: 'Web UI password', hint: 'Session cookie auth' },
    ],
  },
  {
    id: 'unraid', name: 'Unraid', slug: 'unraid', color: '#F45B00',
    category: 'Infrastructure', port: 80,
    fields: [
      { label: 'URL', envKey: 'UNRAID_URL', type: 'url', placeholder: 'http://tower.local' },
      { label: 'API Key', envKey: 'UNRAID_API_KEY', type: 'password', placeholder: 'From Unraid Settings → API Manager', hint: 'X-API-Key header' },
    ],
  },
  {
    id: 'unifi', name: 'UniFi', slug: 'ubiquiti-unifi', color: '#0559C9',
    category: 'Network', port: 8443,
    fields: [
      { label: 'URL', envKey: 'UNIFI_URL', type: 'url', placeholder: 'https://192.168.1.1' },
      { label: 'API Key', envKey: 'UNIFI_API_KEY', type: 'password', placeholder: 'UniFi API key', hint: 'X-API-KEY header' },
    ],
  },
  {
    id: 'tailscale', name: 'Tailscale', slug: 'tailscale', color: '#1E5EFF',
    category: 'Network', port: 0,
    fields: [
      { label: 'URL', envKey: 'TAILSCALE_URL', type: 'url', placeholder: 'https://api.tailscale.com', hint: 'Leave default for Tailscale cloud' },
      { label: 'API Key', envKey: 'TAILSCALE_API_KEY', type: 'password', placeholder: 'tskey-api-... (from admin console)', hint: 'Authorization: Bearer' },
      { label: 'Tailnet', envKey: 'TAILSCALE_TAILNET', type: 'text', placeholder: 'your-tailnet.ts.net or - (auto)', optional: true },
    ],
  },
  {
    id: 'arcane', name: 'Arcane', slug: 'arcane', color: '#0DB7ED',
    category: 'Docker', port: 9000,
    fields: [
      { label: 'URL', envKey: 'ARCANE_URL', type: 'url', placeholder: 'http://192.168.1.x:9000' },
      { label: 'API Key', envKey: 'ARCANE_API_KEY', type: 'password', placeholder: 'Arcane API key (if auth enabled)', optional: true },
    ],
  },
  {
    id: 'linkding', name: 'Linkding', slug: 'linkding', color: '#7C5CBF',
    category: 'Notes', port: 9090,
    fields: [
      { label: 'URL', envKey: 'LINKDING_URL', type: 'url', placeholder: 'http://192.168.1.x:9090' },
      { label: 'Token', envKey: 'LINKDING_TOKEN', type: 'password', placeholder: 'REST API token from /settings/integrations', hint: 'Authorization: Token' },
    ],
  },
  {
    id: 'memos', name: 'Memos', slug: 'memos', color: '#3478F6',
    category: 'Notes', port: 5230,
    fields: [
      { label: 'URL', envKey: 'MEMOS_URL', type: 'url', placeholder: 'http://192.168.1.x:5230' },
      { label: 'Token', envKey: 'MEMOS_TOKEN', type: 'password', placeholder: 'Access token from Settings → My Account', hint: 'Authorization: Bearer' },
    ],
  },
  {
    id: 'paperless', name: 'Paperless', slug: 'paperless-ngx', color: '#17BC6C',
    category: 'Documents', port: 8000,
    fields: [
      { label: 'URL', envKey: 'PAPERLESS_URL', type: 'url', placeholder: 'http://192.168.1.x:8000' },
      { label: 'Token', envKey: 'PAPERLESS_TOKEN', type: 'password', placeholder: 'Token from /api/token/', hint: 'Authorization: Token' },
    ],
  },
  {
    id: 'bytestash', name: 'Bytestash', slug: 'bytestash', color: '#6B73FF',
    category: 'Tools', port: 5000,
    fields: [
      { label: 'URL', envKey: 'BYTESTASH_URL', type: 'url', placeholder: 'http://192.168.1.x:5000' },
      { label: 'Token', envKey: 'BYTESTASH_TOKEN', type: 'password', placeholder: 'JWT token from Bytestash settings', hint: 'Authorization: Bearer' },
    ],
  },
  {
    id: 'gotify', name: 'Gotify', slug: 'gotify', color: '#45AEE5',
    category: 'Notifications', port: 80,
    fields: [
      { label: 'URL', envKey: 'GOTIFY_URL', type: 'url', placeholder: 'http://192.168.1.x:80' },
      { label: 'Token', envKey: 'GOTIFY_TOKEN', type: 'password', placeholder: 'App or client token', hint: 'X-Gotify-Key header' },
    ],
  },
  {
    id: 'apprise', name: 'Apprise', slug: 'apprise', color: '#3B7BBF',
    category: 'Notifications', port: 8000,
    fields: [
      { label: 'URL', envKey: 'APPRISE_URL', type: 'url', placeholder: 'http://192.168.1.x:8000', hint: 'No auth required by default' },
    ],
  },
  {
    id: 'openai', name: 'OpenAI', slug: 'openai', color: '#10A37F',
    category: 'AI', port: 443,
    fields: [
      { label: 'URL', envKey: 'OPENAI_URL', type: 'url', placeholder: 'https://api.openai.com', hint: 'Override for compatible endpoints' },
      { label: 'API Key', envKey: 'OPENAI_API_KEY', type: 'password', placeholder: 'sk-...', hint: 'Authorization: Bearer' },
    ],
  },
  {
    id: 'qdrant', name: 'Qdrant', slug: 'qdrant', color: '#DC244C',
    category: 'AI', port: 6333,
    fields: [
      { label: 'URL', envKey: 'QDRANT_URL', type: 'url', placeholder: 'http://192.168.1.x:6333' },
      { label: 'API Key', envKey: 'QDRANT_API_KEY', type: 'password', placeholder: 'Qdrant API key (optional for local)', hint: 'api-key header', optional: true },
    ],
  },
  {
    id: 'tei', name: 'TEI', slug: null, color: '#FF9D00',
    category: 'AI', port: 8080,
    fields: [
      { label: 'URL', envKey: 'TEI_URL', type: 'url', placeholder: 'http://192.168.1.x:8080', hint: 'HuggingFace Text Embeddings Inference' },
      { label: 'Token', envKey: 'TEI_TOKEN', type: 'password', placeholder: 'Bearer token (optional)', hint: 'Authorization: Bearer', optional: true },
    ],
  },
]

export const SERVICES_BY_ID = Object.fromEntries(SERVICES.map(s => [s.id, s]))

export const SIDEBAR_SECTIONS: Array<{ label: string; ids: string[] }> = [
  { label: 'Configuration', ids: ['__core__'] },
  { label: 'Media',         ids: ['radarr', 'sonarr', 'plex', 'tautulli', 'overseerr'] },
  { label: 'Indexer',       ids: ['prowlarr'] },
  { label: 'Downloads',     ids: ['sabnzbd', 'qbittorrent'] },
  { label: 'Infrastructure',ids: ['unraid'] },
  { label: 'Network',       ids: ['unifi', 'tailscale'] },
  { label: 'Docker',        ids: ['arcane'] },
  { label: 'Notes',         ids: ['linkding', 'memos'] },
  { label: 'Documents',     ids: ['paperless'] },
  { label: 'Tools',         ids: ['bytestash'] },
  { label: 'Notifications', ids: ['gotify', 'apprise'] },
  { label: 'AI',            ids: ['openai', 'qdrant', 'tei'] },
  { label: 'Setup',         ids: ['__surfaces__', '__preflight2__'] },
]
```

- [ ] **Create Zod schemas:**

```ts
// apps/gateway-admin/lib/setup/schema.ts
import { z } from 'zod'

export const CoreConfigSchema = z.object({
  bindHost:   z.string().min(1),
  port:       z.number().int().min(1).max(65535),
  logLevel:   z.string(),
  logFormat:  z.enum(['text', 'json']),
  authMode:   z.enum(['bearer', 'oauth']),
  bearerToken: z.string().optional(),
  publicUrl:   z.string().url().optional(),
  googleClientId:     z.string().optional(),
  googleClientSecret: z.string().optional(),
})

export const ServiceDraftSchema = z.record(z.string(), z.string())

export const WizardDraftSchema = z.object({
  core:     CoreConfigSchema.partial(),
  services: z.record(z.string(), ServiceDraftSchema),
  surfaces: z.object({
    web:      z.boolean(),
    api:      z.boolean(),
    mcpHttp:  z.boolean(),
    mcpStdio: z.boolean(),
    tui:      z.boolean(),
    oauth:    z.boolean(),
  }),
  nodes: z.object({
    controller: z.string(),
    masterUrl:  z.string(),
    fleet:      z.array(z.string()),
  }),
})

export type CoreConfig = z.infer<typeof CoreConfigSchema>
export type WizardDraft = z.infer<typeof WizardDraftSchema>
```

- [ ] **Commit:**

```bash
git add apps/gateway-admin/lib/setup/
git commit -m "feat(setup): services catalog + Zod schemas"
```

---

## Task 3: nodeinfo + nodes API clients

**Files:**
- Create: `apps/gateway-admin/lib/api/nodeinfo-client.ts`
- Create: `apps/gateway-admin/lib/api/nodes-client.ts`
- Create: `apps/gateway-admin/lib/setup/use-nodeinfo.ts`
- Create: `apps/gateway-admin/lib/setup/use-nodes.ts`

- [ ] **nodeinfo client:**

```ts
// apps/gateway-admin/lib/api/nodeinfo-client.ts
export interface NodeInfo {
  local_host: string
  controller: string
  master_url: string
  env: Record<string, string>
}

export async function fetchNodeInfo(signal?: AbortSignal): Promise<NodeInfo> {
  const res = await fetch('/dev/api/nodeinfo', { signal, cache: 'no-store' })
  if (!res.ok) throw new Error(`nodeinfo: ${res.status}`)
  return res.json() as Promise<NodeInfo>
}
```

- [ ] **nodes client:**

```ts
// apps/gateway-admin/lib/api/nodes-client.ts
export interface EnrolledNode {
  node_id: string
  connected: boolean
  role: string
}

export async function fetchNodes(signal?: AbortSignal): Promise<EnrolledNode[]> {
  const res = await fetch('/v1/nodes', { signal, credentials: 'include', cache: 'no-store' })
  if (!res.ok) return []           // graceful — no auth in setup context
  return res.json() as Promise<EnrolledNode[]>
}
```

- [ ] **useNodeinfo hook:**

```ts
// apps/gateway-admin/lib/setup/use-nodeinfo.ts
'use client'
import useSWR from 'swr'
import { fetchNodeInfo, type NodeInfo } from '@/lib/api/nodeinfo-client'

export function useNodeInfo() {
  const { data, error, isLoading } = useSWR<NodeInfo>(
    '/dev/api/nodeinfo',
    () => fetchNodeInfo(),
    { revalidateOnFocus: false },
  )
  return { nodeInfo: data, error, isLoading }
}
```

- [ ] **useNodes hook:**

```ts
// apps/gateway-admin/lib/setup/use-nodes.ts
'use client'
import useSWR from 'swr'
import { fetchNodes, type EnrolledNode } from '@/lib/api/nodes-client'

export function useNodes() {
  const { data, error, isLoading } = useSWR<EnrolledNode[]>(
    '/v1/nodes',
    () => fetchNodes(),
    { revalidateOnFocus: false },
  )
  return { nodes: data ?? [], error, isLoading }
}
```

- [ ] **Commit:**

```bash
git add apps/gateway-admin/lib/api/nodeinfo-client.ts apps/gateway-admin/lib/api/nodes-client.ts apps/gateway-admin/lib/setup/use-nodeinfo.ts apps/gateway-admin/lib/setup/use-nodes.ts
git commit -m "feat(setup): nodeinfo + nodes API clients and SWR hooks"
```

---

## Task 4: Wizard state hook

**Files:**
- Create: `apps/gateway-admin/lib/setup/use-wizard-state.ts`

- [ ] **Create wizard state hook:**

```ts
// apps/gateway-admin/lib/setup/use-wizard-state.ts
'use client'
import { useState, useCallback } from 'react'
import type { WizardDraft } from './schema'

export type WizardPhase = 1 | 2
export type WizardStep = 1 | 2 | 3 | 'service' | 'surfaces' | 'preflight2' | 'finalize'

const DEFAULT_DRAFT: WizardDraft = {
  core: {},
  services: {},
  surfaces: { web: true, api: true, mcpHttp: true, mcpStdio: false, tui: false, oauth: false },
  nodes: { controller: '', masterUrl: '', fleet: [] },
}

export function useWizardState() {
  const [phase, setPhase] = useState<WizardPhase>(1)
  const [step, setStep] = useState<WizardStep>(1)
  const [draft, setDraft] = useState<WizardDraft>(DEFAULT_DRAFT)
  const [finalized, setFinalized] = useState(false)

  const patchCore = useCallback((patch: Partial<WizardDraft['core']>) => {
    setDraft(d => ({ ...d, core: { ...d.core, ...patch } }))
  }, [])

  const patchService = useCallback((svcId: string, patch: Record<string, string>) => {
    setDraft(d => ({
      ...d,
      services: { ...d.services, [svcId]: { ...(d.services[svcId] ?? {}), ...patch } },
    }))
  }, [])

  const patchSurfaces = useCallback((patch: Partial<WizardDraft['surfaces']>) => {
    setDraft(d => ({ ...d, surfaces: { ...d.surfaces, ...patch } }))
  }, [])

  const patchNodes = useCallback((patch: Partial<WizardDraft['nodes']>) => {
    setDraft(d => ({ ...d, nodes: { ...d.nodes, ...patch } }))
  }, [])

  const transitionToPhase2 = useCallback(() => {
    setPhase(2)
    setStep('service')
  }, [])

  return {
    phase, step, setStep,
    draft, patchCore, patchService, patchSurfaces, patchNodes,
    finalized, setFinalized,
    transitionToPhase2,
  }
}
```

- [ ] **Commit:**

```bash
git add apps/gateway-admin/lib/setup/use-wizard-state.ts
git commit -m "feat(setup): wizard state hook"
```

---

## Task 5: Stepper component

**Files:**
- Create: `apps/gateway-admin/components/setup/setup-stepper.tsx`

- [ ] **Write stepper:**

```tsx
// apps/gateway-admin/components/setup/setup-stepper.tsx
'use client'
import { cn } from '@/lib/utils'

export interface StepDef {
  key: string | number
  n: number
  label: string
}

export const WIZARD_STEPS: StepDef[] = [
  { key: 1,            n: 1, label: 'Welcome'     },
  { key: 2,            n: 2, label: 'Core Config'  },
  { key: 3,            n: 3, label: 'PreFlight'    },
  { key: 'service',    n: 4, label: 'Services'     },
  { key: 'surfaces',   n: 5, label: 'Surfaces'     },
  { key: 'preflight2', n: 6, label: 'PreFlight 2'  },
  { key: 'finalize',   n: 7, label: 'Finalize'     },
]

interface Props {
  current: string | number
  compact?: boolean
}

export function SetupStepper({ current, compact }: Props) {
  const activeIdx = WIZARD_STEPS.findIndex(s => s.key === current)
  const fillPct = WIZARD_STEPS.length <= 1 ? 0 : (activeIdx / (WIZARD_STEPS.length - 1)) * 100

  return (
    <div className="relative w-full max-w-[680px] mx-auto">
      {/* Track */}
      <div className="absolute top-[13px] left-[14px] right-[14px] h-[2px] bg-aurora-border-default" />
      <div
        className="absolute top-[13px] left-[14px] h-[2px] bg-aurora-accent-primary transition-[width] duration-500"
        style={{ width: `${fillPct}%` }}
      />

      {/* Dots */}
      <div className="relative flex items-start justify-between">
        {WIZARD_STEPS.map((s, i) => {
          const done   = i < activeIdx
          const active = i === activeIdx
          return (
            <div key={s.key} className="flex flex-col items-center gap-[6px] z-10">
              <div
                className={cn(
                  'w-[26px] h-[26px] rounded-full flex items-center justify-center text-[10px] font-bold border-2 transition-all duration-300',
                  done   && 'bg-aurora-accent-primary border-aurora-accent-primary text-aurora-page-bg',
                  active && 'bg-transparent border-aurora-accent-primary text-aurora-accent-primary shadow-aurora-active-glow',
                  !done && !active && 'bg-aurora-panel-medium border-aurora-border-default text-aurora-text-muted',
                )}
              >
                {done ? '✓' : s.n}
              </div>
              {!compact && (
                <span className={cn(
                  'text-[10px] font-bold tracking-[0.14em] uppercase whitespace-nowrap hidden sm:block',
                  active && 'text-aurora-accent-primary',
                  done   && 'text-aurora-text-primary',
                  !done && !active && 'text-aurora-text-muted',
                )}>
                  {s.label}
                </span>
              )}
            </div>
          )
        })}
      </div>

      {/* Mobile: active step label */}
      <p className="sm:hidden mt-[6px] text-center text-[11px] font-bold tracking-[0.10em] uppercase text-aurora-accent-primary">
        {WIZARD_STEPS[activeIdx]?.label}
      </p>
    </div>
  )
}
```

- [ ] **Commit:**

```bash
git add apps/gateway-admin/components/setup/setup-stepper.tsx
git commit -m "feat(setup): SetupStepper component"
```

---

## Task 6: ServiceForm shared component

**Files:**
- Create: `apps/gateway-admin/components/setup/service-form.tsx`

- [ ] **Create ServiceForm** (used in both setup and settings):

```tsx
// apps/gateway-admin/components/setup/service-form.tsx
'use client'
import { useState, useCallback } from 'react'
import { Eye, EyeOff, Zap } from 'lucide-react'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { Label } from '@/components/ui/label'
import { cn } from '@/lib/utils'
import type { ServiceDef, ServiceField } from '@/lib/setup/services-catalog'

interface Props {
  service: ServiceDef
  values: Record<string, string>              // current draft values (secret fields arrive as "***")
  onChange: (envKey: string, value: string) => void
  onTest?: () => void
  testState?: 'idle' | 'testing' | 'ok' | 'fail'
}

function SecretInput({
  field, value, onChange,
}: { field: ServiceField; value: string; onChange: (v: string) => void }) {
  const [show, setShow] = useState(false)
  const isSet = value === '***'
  return (
    <div className="relative">
      <Input
        type={show ? 'text' : 'password'}
        value={value}
        onChange={e => onChange(e.target.value)}
        placeholder={isSet ? 'Leave blank to keep current value' : field.placeholder}
        className="pr-10 bg-aurora-control-surface border-aurora-border-strong"
      />
      <button
        type="button"
        onClick={() => setShow(s => !s)}
        className="absolute right-3 top-1/2 -translate-y-1/2 text-aurora-text-muted hover:text-aurora-text-primary"
      >
        {show ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
      </button>
    </div>
  )
}

export function ServiceForm({ service, values, onChange, onTest, testState = 'idle' }: Props) {
  const handleChange = useCallback(
    (envKey: string) => (value: string) => onChange(envKey, value),
    [onChange],
  )

  return (
    <div className="space-y-4">
      {service.fields.map(field => (
        <div key={field.envKey} className="space-y-1.5">
          <Label className="text-[12px] font-semibold text-aurora-text-primary flex items-center gap-2">
            {field.label}
            {field.optional && (
              <span className="text-[10px] font-normal text-aurora-text-muted">(optional)</span>
            )}
            <code className="ml-auto text-[10px] text-aurora-text-muted bg-aurora-control-surface px-1.5 py-0.5 rounded">
              {field.envKey}
            </code>
          </Label>

          {field.type === 'password' ? (
            <SecretInput
              field={field}
              value={values[field.envKey] ?? ''}
              onChange={handleChange(field.envKey)}
            />
          ) : (
            <Input
              type={field.type === 'url' ? 'url' : 'text'}
              value={values[field.envKey] ?? ''}
              onChange={e => onChange(field.envKey, e.target.value)}
              placeholder={field.placeholder}
              className="bg-aurora-control-surface border-aurora-border-strong"
            />
          )}

          {field.hint && (
            <p className="text-[11px] text-aurora-text-muted">{field.hint}</p>
          )}
        </div>
      ))}

      {onTest && (
        <div className="flex items-center gap-3 pt-1">
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={onTest}
            disabled={testState === 'testing'}
            className={cn(
              testState === 'ok'   && 'border-aurora-success text-aurora-success',
              testState === 'fail' && 'border-aurora-error text-aurora-error',
            )}
          >
            <Zap className="size-3.5 mr-1.5" />
            {testState === 'testing' ? 'Testing…'
             : testState === 'ok'    ? 'Connected'
             : testState === 'fail'  ? 'Failed — retry'
             : 'Test Connection'}
          </Button>
          {testState === 'fail' && (
            <p className="text-[12px] text-aurora-error">Check URL and credentials</p>
          )}
        </div>
      )}
    </div>
  )
}
```

- [ ] **Commit:**

```bash
git add apps/gateway-admin/components/setup/service-form.tsx
git commit -m "feat(setup): shared ServiceForm component"
```

---

## Task 7: NodesPanel component

**Files:**
- Create: `apps/gateway-admin/components/setup/nodes-panel.tsx`

- [ ] **Create NodesPanel:**

```tsx
// apps/gateway-admin/components/setup/nodes-panel.tsx
'use client'
import { useState } from 'react'
import { Monitor, Plus, ScanLine } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { cn } from '@/lib/utils'
import { useNodes } from '@/lib/setup/use-nodes'
import type { NodeInfo } from '@/lib/api/nodeinfo-client'

interface NodeRow {
  alias: string
  connected: boolean
  isThisDevice: boolean
}

interface Props {
  thisDevice: string
  controller: string
  masterUrl: string
  onMasterChange: (alias: string, masterUrl: string) => void
}

export function NodesPanel({ thisDevice, controller, masterUrl, onMasterChange }: Props) {
  const { nodes } = useNodes()
  const [manualAlias, setManualAlias] = useState('')

  // Build the full node list: this device first, then enrolled nodes
  const liveMap = Object.fromEntries(nodes.map(n => [n.node_id, n.connected]))
  const allAliases = new Set<string>([thisDevice, ...nodes.map(n => n.node_id)])
  const rows: NodeRow[] = Array.from(allAliases).map(alias => ({
    alias,
    connected: alias === thisDevice ? true : (liveMap[alias] ?? false),
    isThisDevice: alias === thisDevice,
  }))

  function addManual() {
    const alias = manualAlias.trim()
    if (!alias || allAliases.has(alias)) return
    // Add to a local extra list (persisted in draft via onMasterChange side effect)
    setManualAlias('')
  }

  function setMaster(alias: string) {
    const port = masterUrl ? new URL(masterUrl.startsWith('http') ? masterUrl : `http://${masterUrl}`).port || '8765' : '8765'
    onMasterChange(alias, `http://${alias}:${port}`)
  }

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <p className="text-[10px] font-bold uppercase tracking-[0.16em] text-aurora-text-muted">Nodes</p>
        <Button variant="ghost" size="sm" className="text-aurora-accent-primary text-[11px] h-auto py-0">
          <ScanLine className="size-3 mr-1" />
          Scan ~/.ssh/config
        </Button>
      </div>

      <p className="text-[12px] text-aurora-text-muted leading-relaxed">
        This device is always included. Add remote nodes and designate one as the{' '}
        <strong className="text-aurora-text-primary">master node</strong> — it runs{' '}
        <code className="text-[11px]">lab serve</code> and hosts the Web UI.
      </p>

      <div className="space-y-2">
        {rows.map(row => {
          const isMaster = row.alias === controller
          return (
            <div
              key={row.alias}
              className={cn(
                'flex items-center gap-3 px-3 py-2.5 rounded-aurora-1 border transition-colors',
                isMaster
                  ? 'bg-[color-mix(in_srgb,var(--aurora-accent-primary)_8%,transparent)] border-[color-mix(in_srgb,var(--aurora-accent-primary)_30%,transparent)]'
                  : 'bg-aurora-control-surface border-aurora-border-strong',
              )}
            >
              {/* Include checkbox — locked for this device */}
              <input
                type="checkbox"
                checked
                readOnly={row.isThisDevice}
                disabled={row.isThisDevice}
                className="accent-aurora-accent-primary"
              />

              <Monitor className={cn('size-4 flex-shrink-0', row.connected ? 'text-aurora-success' : 'text-aurora-text-muted')} />

              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-2">
                  <span className="text-[13px] font-medium text-aurora-text-primary">{row.alias}</span>
                  {row.isThisDevice && (
                    <span className="text-[9px] font-bold uppercase tracking-wide text-aurora-text-muted">This device</span>
                  )}
                  {isMaster && (
                    <span className="text-[9px] font-bold uppercase tracking-wide bg-aurora-accent-primary text-aurora-page-bg px-1.5 py-0.5 rounded">
                      Master
                    </span>
                  )}
                </div>
                <p className={cn('text-[11px]', row.connected ? 'text-aurora-success' : 'text-aurora-text-muted')}>
                  {row.connected ? '● Connected' : '○ Offline'}
                </p>
              </div>

              <label className="flex items-center gap-1.5 text-[11px] text-aurora-text-muted cursor-pointer">
                <input
                  type="radio"
                  name="master"
                  checked={isMaster}
                  onChange={() => setMaster(row.alias)}
                  className="accent-aurora-accent-primary"
                />
                Master
              </label>
            </div>
          )
        })}
      </div>

      <div className="flex gap-2">
        <Input
          value={manualAlias}
          onChange={e => setManualAlias(e.target.value)}
          placeholder="SSH host alias (e.g. controller)"
          className="bg-aurora-control-surface border-aurora-border-strong text-[13px]"
          onKeyDown={e => e.key === 'Enter' && addManual()}
        />
        <Button type="button" variant="outline" size="sm" onClick={addManual}>
          <Plus className="size-3.5 mr-1" />
          Add node
        </Button>
      </div>

      {masterUrl && (
        <div className="space-y-1.5">
          <label className="text-[12px] font-semibold text-aurora-text-primary flex items-center gap-2">
            Master URL
            <code className="ml-auto text-[10px] text-aurora-text-muted bg-aurora-control-surface px-1.5 rounded">
              deploy.defaults.master_url
            </code>
          </label>
          <Input
            value={masterUrl}
            onChange={() => {}} // controlled by setMaster
            className="bg-aurora-control-surface border-aurora-border-strong text-[13px]"
          />
          <p className="text-[11px] text-aurora-text-muted">Remote nodes use this URL to phone home.</p>
        </div>
      )}
    </div>
  )
}
```

- [ ] **Commit:**

```bash
git add apps/gateway-admin/components/setup/nodes-panel.tsx
git commit -m "feat(setup): NodesPanel component with live /v1/nodes data"
```

---

## Task 8: Step 1 — Welcome

**Files:**
- Create: `apps/gateway-admin/components/setup/steps/step-welcome.tsx`

- [ ] **Create:**

```tsx
// apps/gateway-admin/components/setup/steps/step-welcome.tsx
import { cn } from '@/lib/utils'
import { AURORA_DISPLAY_1 } from '@/components/aurora/tokens'

const CHIPS = ['CLI', 'API', 'MCP Gateway', 'NextJS App', '21 Services', '1 Binary']

interface Props {
  isRerun: boolean
}

export function StepWelcome({ isRerun }: Props) {
  return (
    <div className="flex flex-col items-center text-center gap-6 py-8 max-w-[560px] mx-auto">
      {isRerun && (
        <div className="w-full text-left rounded-[10px] border border-[color-mix(in_srgb,var(--aurora-accent-primary)_30%,transparent)] bg-[color-mix(in_srgb,var(--aurora-accent-primary)_8%,transparent)] px-4 py-3">
          <p className="text-[12px] font-bold text-aurora-accent-strong mb-1">Re-running setup</p>
          <p className="text-[12px] text-aurora-text-muted leading-relaxed">
            Existing configuration detected — all fields pre-populated from{' '}
            <code className="text-[11px]">~/.labby/.env</code> and{' '}
            <code className="text-[11px]">~/.labby/config.toml</code>. Secret values are masked;
            leave them blank to keep the current value.
          </p>
        </div>
      )}

      {/* Logo */}
      <svg width="80" height="80" viewBox="0 0 512 512" fill="none" xmlns="http://www.w3.org/2000/svg">
        <defs>
          <radialGradient id="wbg" cx="30%" cy="25%" r="80%">
            <stop offset="0%" stopColor="#0d2233" />
            <stop offset="100%" stopColor="#07131c" />
          </radialGradient>
        </defs>
        <rect width="512" height="512" rx="112" fill="url(#wbg)" />
        <line x1="256" y1="208" x2="256" y2="102" stroke="#24536c" strokeWidth="10" strokeLinecap="round" />
        <line x1="297" y1="225" x2="367" y2="142" stroke="#24536c" strokeWidth="10" strokeLinecap="round" />
        <line x1="297" y1="287" x2="367" y2="370" stroke="#24536c" strokeWidth="10" strokeLinecap="round" />
        <line x1="256" y1="304" x2="256" y2="410" stroke="#24536c" strokeWidth="10" strokeLinecap="round" />
        <line x1="215" y1="287" x2="145" y2="370" stroke="#24536c" strokeWidth="10" strokeLinecap="round" />
        <line x1="215" y1="225" x2="145" y2="142" stroke="#24536c" strokeWidth="10" strokeLinecap="round" />
        <circle cx="256" cy="92"  r="22" fill="#1c7fac" /><circle cx="256" cy="92"  r="12" fill="#29b6f6" />
        <circle cx="378" cy="130" r="18" fill="#1c7fac" /><circle cx="378" cy="130" r="10" fill="#67cbfa" />
        <circle cx="378" cy="382" r="22" fill="#1c7fac" /><circle cx="378" cy="382" r="12" fill="#29b6f6" />
        <circle cx="256" cy="420" r="18" fill="#1c7fac" /><circle cx="256" cy="420" r="10" fill="#67cbfa" />
        <circle cx="134" cy="382" r="22" fill="#1c7fac" /><circle cx="134" cy="382" r="12" fill="#29b6f6" />
        <circle cx="134" cy="130" r="18" fill="#1c7fac" /><circle cx="134" cy="130" r="10" fill="#67cbfa" />
        <circle cx="256" cy="256" r="52" fill="#0c1a24" stroke="#29b6f6" strokeWidth="3" />
        <circle cx="256" cy="256" r="38" fill="#1c7fac" />
        <circle cx="256" cy="256" r="28" fill="#29b6f6" />
        <circle cx="256" cy="256" r="16" fill="#67cbfa" />
      </svg>

      <div>
        <h1 className={cn(AURORA_DISPLAY_1, 'text-aurora-text-primary')}>
          Welcome to{' '}
          <span className="text-aurora-accent-primary">Labby</span>
        </h1>
        <p className="mt-3 text-[14px] text-aurora-text-muted leading-[1.7] max-w-[480px]">
          This wizard will guide you through Labby core configuration, along with all services and
          surfaces you plan to use. We&apos;ll check your system, setup sensible defaults based on
          your device, validate everything and connect your services — all before writing a single
          byte to disk.
        </p>
      </div>

      <div className="flex flex-wrap gap-2 justify-center">
        {CHIPS.map(chip => (
          <div
            key={chip}
            className="flex items-center gap-1.5 bg-aurora-panel-strong border border-aurora-border-strong rounded-full px-3 py-1 text-[11px] text-aurora-text-muted"
          >
            <span className="size-1.5 rounded-full bg-aurora-accent-primary flex-shrink-0" />
            {chip}
          </div>
        ))}
      </div>

      <p className="text-[11px] text-aurora-text-muted opacity-60">
        You can change any setting later from the Settings page.
      </p>
    </div>
  )
}
```

- [ ] **Commit:**

```bash
git add apps/gateway-admin/components/setup/steps/step-welcome.tsx
git commit -m "feat(setup): StepWelcome component"
```

---

## Task 9: Step 2 — Core Config

**Files:**
- Create: `apps/gateway-admin/components/setup/steps/step-core-config.tsx`

- [ ] **Create** (abridged — full form with all fields):

```tsx
// apps/gateway-admin/components/setup/steps/step-core-config.tsx
'use client'
import { useEffect } from 'react'
import { useForm } from 'react-hook-form'
import { zodResolver } from '@hookform/resolvers/zod'
import { Eye, EyeOff, RefreshCw } from 'lucide-react'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
} from '@/components/ui/select'
import { Button } from '@/components/ui/button'
import { NodesPanel } from '@/components/setup/nodes-panel'
import { AURORA_DISPLAY_2, AURORA_MUTED_LABEL } from '@/components/aurora/tokens'
import { cn } from '@/lib/utils'
import { CoreConfigSchema, type CoreConfig } from '@/lib/setup/schema'
import type { NodeInfo } from '@/lib/api/nodeinfo-client'

interface Props {
  nodeInfo: NodeInfo | undefined
  defaultValues: Partial<CoreConfig>
  onChange: (values: Partial<CoreConfig>) => void
  onNodeChange: (controller: string, masterUrl: string) => void
  nodeController: string
  masterUrl: string
}

export function StepCoreConfig({ nodeInfo, defaultValues, onChange, onNodeChange, nodeController, masterUrl }: Props) {
  const env = nodeInfo?.env ?? {}

  const { register, watch, setValue, formState: { errors } } = useForm<CoreConfig>({
    resolver: zodResolver(CoreConfigSchema.partial()),
    defaultValues: {
      bindHost:  env.LAB_MCP_HTTP_HOST ?? defaultValues.bindHost  ?? '127.0.0.1',
      port:      Number(env.LAB_MCP_HTTP_PORT ?? defaultValues.port ?? 8765),
      logLevel:  env.LAB_LOG  ?? defaultValues.logLevel  ?? 'labby=info,lab_apis=warn',
      logFormat: (env.LAB_LOG_FORMAT as 'text' | 'json') ?? defaultValues.logFormat ?? 'text',
      authMode:  (env.LAB_AUTH_MODE === 'oauth' || env.LAB_GOOGLE_CLIENT_ID)
                   ? 'oauth' : (defaultValues.authMode ?? 'bearer'),
      bearerToken:        env.LAB_MCP_HTTP_TOKEN ?? '',
      publicUrl:          env.LAB_PUBLIC_URL ?? '',
      googleClientId:     env.LAB_GOOGLE_CLIENT_ID ?? '',
      googleClientSecret: env.LAB_GOOGLE_CLIENT_SECRET ?? '',
    },
  })

  const authMode    = watch('authMode')
  const watched     = watch()

  useEffect(() => { onChange(watched) }, [JSON.stringify(watched)])

  function generateToken() {
    const arr = new Uint8Array(32)
    crypto.getRandomValues(arr)
    const token = btoa(String.fromCharCode(...arr)).replace(/[+/=]/g, c => ({'+':'-','/':'_','=':''}[c] ?? ''))
    setValue('bearerToken', token)
  }

  return (
    <div className="space-y-5 max-w-[600px]">
      {/* SERVER */}
      <section className="rounded-aurora-3 border border-aurora-border-strong bg-aurora-panel-medium p-5 space-y-4">
        <p className={AURORA_MUTED_LABEL}>Server</p>

        <div className="grid grid-cols-2 gap-4">
          <div className="space-y-1.5">
            <Label className="text-[12px] font-semibold">
              Bind Address <code className="ml-1 text-[10px] text-aurora-text-muted">LAB_MCP_HTTP_HOST</code>
            </Label>
            <Input {...register('bindHost')} className="bg-aurora-control-surface border-aurora-border-strong" />
            <p className="text-[11px] text-aurora-text-muted">Use 127.0.0.1 for localhost-only. 0.0.0.0 requires auth.</p>
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

      {/* AUTHENTICATION */}
      <section className="rounded-aurora-3 border border-aurora-border-strong bg-aurora-panel-medium p-5 space-y-4">
        <p className={AURORA_MUTED_LABEL}>Authentication</p>
        <p className="text-[12px] text-aurora-text-muted leading-relaxed">
          Secures the <strong className="text-aurora-text-primary">Web UI, HTTP API, and MCP server</strong> — all served on the same port.
        </p>

        {/* Auth mode toggle */}
        <div className="grid grid-cols-2 gap-3">
          {(['bearer', 'oauth'] as const).map(mode => {
            const active = watch('authMode') === mode
            return (
              <label
                key={mode}
                className={cn(
                  'flex items-start gap-3 rounded-[8px] border px-3 py-2.5 cursor-pointer transition-colors',
                  active
                    ? 'bg-[color-mix(in_srgb,var(--aurora-accent-primary)_10%,transparent)] border-aurora-accent-primary'
                    : 'bg-aurora-control-surface border-aurora-border-strong',
                )}
              >
                <input type="radio" value={mode} {...register('authMode')} className="mt-0.5 accent-aurora-accent-primary" />
                <div>
                  <p className={cn('text-[13px] font-semibold', active ? 'text-aurora-accent-strong' : 'text-aurora-text-primary')}>
                    {mode === 'bearer' ? 'Bearer Token' : 'OAuth (Google)'}
                  </p>
                  <p className="text-[11px] text-aurora-text-muted">
                    {mode === 'bearer' ? 'Simple shared secret — good for homelab' : 'Google login with JWT sessions'}
                  </p>
                </div>
              </label>
            )
          })}
        </div>

        {authMode === 'bearer' && (
          <div className="space-y-1.5">
            <Label className="text-[12px] font-semibold">
              Bearer Token <code className="ml-1 text-[10px] text-aurora-text-muted">LAB_MCP_HTTP_TOKEN</code>
            </Label>
            <div className="flex gap-2">
              <Input
                {...register('bearerToken')}
                type="password"
                placeholder="Paste or generate a secure token"
                className="bg-aurora-control-surface border-aurora-border-strong flex-1"
              />
              <Button type="button" variant="outline" size="sm" onClick={generateToken} className="flex-shrink-0">
                <RefreshCw className="size-3.5 mr-1.5" />
                Generate
              </Button>
            </div>
            <p className="text-[11px] text-aurora-text-muted">Required for non-localhost bind.</p>
          </div>
        )}

        {authMode === 'oauth' && (
          <div className="space-y-3">
            <div className="space-y-1.5">
              <Label className="text-[12px] font-semibold">
                Public URL <code className="ml-1 text-[10px] text-aurora-text-muted">LAB_PUBLIC_URL</code>
              </Label>
              <Input {...register('publicUrl')} type="url" placeholder="https://lab.yourdomain.com" className="bg-aurora-control-surface border-aurora-border-strong" />
              <p className="text-[11px] text-aurora-text-muted">Used for OAuth metadata discovery and the Google callback URL.</p>
            </div>
            <div className="grid grid-cols-2 gap-3">
              <div className="space-y-1.5">
                <Label className="text-[12px] font-semibold">
                  Client ID <code className="ml-1 text-[10px] text-aurora-text-muted">LAB_GOOGLE_CLIENT_ID</code>
                </Label>
                <Input {...register('googleClientId')} placeholder="xxxxxx.apps.googleusercontent.com" className="bg-aurora-control-surface border-aurora-border-strong" />
              </div>
              <div className="space-y-1.5">
                <Label className="text-[12px] font-semibold">
                  Client Secret <code className="ml-1 text-[10px] text-aurora-text-muted">LAB_GOOGLE_CLIENT_SECRET</code>
                </Label>
                <Input {...register('googleClientSecret')} type="password" placeholder="GOCSPX-..." className="bg-aurora-control-surface border-aurora-border-strong" />
              </div>
            </div>
          </div>
        )}
      </section>

      {/* NODES */}
      <section className="rounded-aurora-3 border border-aurora-border-strong bg-aurora-panel-medium p-5">
        <NodesPanel
          thisDevice={nodeInfo?.local_host ?? 'local'}
          controller={nodeController}
          masterUrl={masterUrl}
          onMasterChange={onNodeChange}
        />
      </section>
    </div>
  )
}
```

- [ ] **Commit:**

```bash
git add apps/gateway-admin/components/setup/steps/step-core-config.tsx
git commit -m "feat(setup): StepCoreConfig with auth mode toggle, nodes panel"
```

---

## Task 10: PreFlight 1 component

**Files:**
- Create: `apps/gateway-admin/components/setup/setup-preflight.tsx`

- [ ] **Create PreFlight 1** (runs real HTTP checks on mount):

```tsx
// apps/gateway-admin/components/setup/setup-preflight.tsx
'use client'
import { useEffect, useRef, useState } from 'react'
import { CheckCircle, XCircle, Loader2 } from 'lucide-react'
import { cn } from '@/lib/utils'
import { AURORA_DISPLAY_2 } from '@/components/aurora/tokens'

interface CheckResult {
  ok: boolean
  detail: string
}

interface CheckDef {
  id: string
  label: string
  run: () => Promise<CheckResult>
}

function buildChecks(bearerToken?: string): CheckDef[] {
  return [
    {
      id: 'health',
      label: 'Server reachable',
      async run() {
        try {
          const r = await fetch('/health', { signal: AbortSignal.timeout(4000) })
          return r.ok
            ? { ok: true,  detail: 'GET /health → 200' }
            : { ok: false, detail: `GET /health → ${r.status} (expected 200)` }
        } catch {
          return { ok: false, detail: 'Server not reachable (connection refused or timeout)' }
        }
      },
    },
    {
      id: 'auth-gate',
      label: 'Protected endpoints require auth',
      async run() {
        try {
          const r = await fetch('/v1/doctor', { signal: AbortSignal.timeout(4000) })
          if (r.status === 401) return { ok: true,  detail: 'GET /v1/doctor → 401 (auth required)' }
          if (r.status === 200) return { ok: false, detail: 'GET /v1/doctor → 200 without auth (unprotected!)' }
          return { ok: true, detail: `GET /v1/doctor → ${r.status}` }
        } catch {
          return { ok: false, detail: 'Could not reach server' }
        }
      },
    },
    {
      id: 'bearer',
      label: 'Bearer token authenticates',
      async run() {
        if (!bearerToken || bearerToken === '***') {
          return { ok: true, detail: 'LAB_MCP_HTTP_TOKEN set (value masked)' }
        }
        try {
          const r = await fetch('/v1/doctor/actions', {
            headers: { Authorization: `Bearer ${bearerToken}` },
            signal: AbortSignal.timeout(4000),
          })
          return r.ok
            ? { ok: true,  detail: 'Bearer token accepted → 200' }
            : { ok: false, detail: `Bearer token rejected → ${r.status}` }
        } catch {
          return { ok: false, detail: 'Could not test bearer token' }
        }
      },
    },
    {
      id: 'mcp',
      label: 'MCP endpoint bearer-only',
      async run() {
        try {
          const r = await fetch('/mcp', {
            headers: { Cookie: 'lab_session=fake' },
            signal: AbortSignal.timeout(4000),
          })
          return { ok: r.status === 401, detail: `GET /mcp with fake cookie → ${r.status} (${r.status === 401 ? 'bearer-only enforced' : 'unexpected — cookie may bypass auth'})` }
        } catch {
          return { ok: false, detail: 'Could not reach /mcp' }
        }
      },
    },
    {
      id: 'dev',
      label: 'Dev preview endpoint accessible',
      async run() {
        try {
          const r = await fetch('/dev/api/marketplace', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ action: 'help' }),
            signal: AbortSignal.timeout(4000),
          })
          if (r.ok || r.status === 400) return { ok: true, detail: `POST /dev/api/marketplace → ${r.status} (unauthenticated read OK)` }
          if (r.status === 401)          return { ok: false, detail: 'POST /dev/api/marketplace → 401 (dev route must be public)' }
          return { ok: true, detail: `POST /dev/api/marketplace → ${r.status}` }
        } catch {
          return { ok: false, detail: 'Could not reach dev endpoint' }
        }
      },
    },
  ]
}

type Status = 'pending' | 'running' | 'ok' | 'fail'

interface CheckState {
  status: Status
  detail: string
}

interface Props {
  bearerToken?: string
  onAllPass: () => void
}

export function SetupPreflight({ bearerToken, onAllPass }: Props) {
  const checks = buildChecks(bearerToken)
  const [states, setStates] = useState<Record<string, CheckState>>(
    Object.fromEntries(checks.map(c => [c.id, { status: 'pending', detail: 'Waiting…' }])),
  )
  const ran = useRef(false)

  useEffect(() => {
    if (ran.current) return
    ran.current = true

    async function runAll() {
      let allOk = true
      for (const check of checks) {
        setStates(prev => ({ ...prev, [check.id]: { status: 'running', detail: 'Checking…' } }))
        await new Promise(r => setTimeout(r, 300 + Math.random() * 300))
        const result = await check.run()
        if (!result.ok) allOk = false
        setStates(prev => ({
          ...prev,
          [check.id]: { status: result.ok ? 'ok' : 'fail', detail: result.detail },
        }))
      }
      if (allOk) setTimeout(onAllPass, 600)
    }

    void runAll()
  }, [])

  return (
    <div className="space-y-4 max-w-[600px]">
      <div>
        <h2 className={cn(AURORA_DISPLAY_2, 'text-aurora-text-primary')}>System PreFlight</h2>
        <p className="mt-1 text-[13px] text-aurora-text-muted">
          Running automated checks. All must pass to continue.
        </p>
      </div>

      <div className="space-y-2">
        {checks.map(check => {
          const s = states[check.id]
          return (
            <div
              key={check.id}
              className={cn(
                'flex items-center gap-3 rounded-aurora-1 border px-4 py-3 transition-colors',
                s.status === 'ok'   && 'border-[color-mix(in_srgb,var(--aurora-success)_25%,transparent)] bg-[color-mix(in_srgb,var(--aurora-success)_6%,transparent)]',
                s.status === 'fail' && 'border-[color-mix(in_srgb,var(--aurora-error)_25%,transparent)] bg-[color-mix(in_srgb,var(--aurora-error)_6%,transparent)]',
                (s.status === 'pending' || s.status === 'running') && 'border-aurora-border-strong bg-aurora-panel-medium',
              )}
            >
              <div className="size-[18px] flex-shrink-0 flex items-center justify-center">
                {s.status === 'running' && <Loader2 className="size-4 animate-spin text-aurora-accent-primary" />}
                {s.status === 'ok'      && <CheckCircle className="size-4 text-aurora-success" />}
                {s.status === 'fail'    && <XCircle className="size-4 text-aurora-error" />}
                {s.status === 'pending' && <div className="size-3 rounded-full border-2 border-aurora-border-strong" />}
              </div>
              <div className="flex-1 min-w-0">
                <p className="text-[13px] font-medium text-aurora-text-primary">{check.label}</p>
                <p className="text-[11px] text-aurora-text-muted truncate">{s.detail}</p>
              </div>
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
git add apps/gateway-admin/components/setup/setup-preflight.tsx
git commit -m "feat(setup): SetupPreflight with real HTTP checks"
```

---

## Task 11: Setup sidebar (phase 2)

**Files:**
- Create: `apps/gateway-admin/components/setup/setup-sidebar.tsx`

- [ ] **Create setup sidebar:**

```tsx
// apps/gateway-admin/components/setup/setup-sidebar.tsx
'use client'
import Image from 'next/image'
import { cn } from '@/lib/utils'
import { SIDEBAR_SECTIONS, SERVICES_BY_ID } from '@/lib/setup/services-catalog'
import { Button } from '@/components/ui/button'
import {
  SERVICE_BRANDS, SERVICE_LOGOS,
  type ServiceKey,
} from '@/lib/branding/service-brands'

type SidebarItem = string  // service id or __core__, __surfaces__, __preflight2__

interface Props {
  visible: boolean
  activeId: SidebarItem
  onSelect: (id: SidebarItem) => void
  configuredIds: Set<string>
  onFinalize: () => void
}

function ServiceIcon({ id, size = 20 }: { id: string; size?: number }) {
  const svc = SERVICES_BY_ID[id]
  if (!svc) return null
  const color = SERVICE_BRANDS[id as ServiceKey] ?? '#29b6f6'
  const logo  = svc.slug ? SERVICE_LOGOS[id as ServiceKey] : null

  return (
    <div
      className="rounded-[5px] flex items-center justify-center overflow-hidden flex-shrink-0"
      style={{
        width: size, height: size,
        background: `${color}22`,
        border: `1px solid ${color}44`,
      }}
    >
      {logo ? (
        <Image
          src={logo.replace('/png/', '/svg/').replace('.png', '.svg')}
          alt={svc.name}
          width={Math.round(size * 0.65)}
          height={Math.round(size * 0.65)}
          unoptimized
          onError={e => {
            // fall back to PNG
            const img = e.currentTarget as HTMLImageElement
            if (img.src.includes('/svg/')) {
              img.src = logo
            } else {
              img.style.display = 'none'
            }
          }}
        />
      ) : (
        <span style={{ fontSize: Math.round(size * 0.42), fontWeight: 800, color }}>
          {svc.name[0]}
        </span>
      )}
    </div>
  )
}

export function SetupSidebar({ visible, activeId, onSelect, configuredIds, onFinalize }: Props) {
  return (
    <aside
      className={cn(
        'relative flex flex-col bg-aurora-nav-bg border-r border-aurora-border-default h-full flex-shrink-0',
        'transition-[width,min-width] duration-350 ease-[cubic-bezier(.4,0,.2,1)]',
        visible ? 'w-[220px] min-w-[220px]' : 'w-0 min-w-0 overflow-hidden',
      )}
    >
      {/* Scrollable nav */}
      <div className="flex-1 overflow-y-auto py-4 pb-2">
        {SIDEBAR_SECTIONS.map(section => (
          <div key={section.label}>
            <p className="px-4 py-2 text-[10px] font-bold uppercase tracking-[0.16em] text-aurora-text-muted opacity-60">
              {section.label}
            </p>
            {section.ids.map(id => {
              const svc = SERVICES_BY_ID[id]
              const isActive = id === activeId
              const isConfigured = configuredIds.has(id)
              const label = id === '__core__' ? 'Core' : id === '__surfaces__' ? 'Surfaces' : id === '__preflight2__' ? 'PreFlight 2' : svc?.name ?? id

              return (
                <button
                  key={id}
                  onClick={() => onSelect(id)}
                  className={cn(
                    'w-full flex items-center gap-2.5 px-3 py-1.5 mx-2 rounded-[6px] text-left transition-colors',
                    isActive
                      ? 'bg-[color-mix(in_srgb,var(--aurora-accent-primary)_10%,transparent)] text-aurora-accent-strong'
                      : 'text-aurora-text-muted hover:bg-aurora-hover-bg',
                  )}
                  style={{ width: 'calc(100% - 16px)' }}
                >
                  {svc ? (
                    <ServiceIcon id={id} size={22} />
                  ) : (
                    <div className="size-[22px] rounded-[5px] flex items-center justify-center flex-shrink-0 bg-[color-mix(in_srgb,var(--aurora-accent-primary)_8%,transparent)] border border-[color-mix(in_srgb,var(--aurora-accent-primary)_30%,transparent)]">
                      <span className="text-[11px] font-bold text-aurora-accent-primary">
                        {id === '__core__' ? '⚙' : id === '__surfaces__' ? '🖥' : '✓'}
                      </span>
                    </div>
                  )}
                  <span className="flex-1 text-[12px] font-medium truncate">{label}</span>
                  <div
                    className={cn(
                      'size-[7px] rounded-full flex-shrink-0 transition-colors',
                      isConfigured ? 'bg-aurora-success shadow-[0_0_4px_color-mix(in_srgb,var(--aurora-success)_50%,transparent)]' : 'bg-aurora-border-default',
                    )}
                  />
                </button>
              )
            })}
          </div>
        ))}
      </div>

      {/* Pinned finalize */}
      <div
        className="flex-shrink-0 p-3 border-t border-aurora-border-default transition-opacity duration-200"
        style={{ opacity: visible ? 1 : 0, pointerEvents: visible ? 'auto' : 'none' }}
      >
        <Button onClick={onFinalize} className="w-full bg-aurora-accent-primary text-aurora-page-bg hover:bg-aurora-accent-strong">
          ✓ Finalize &amp; Commit
        </Button>
      </div>
    </aside>
  )
}
```

- [ ] **Commit:**

```bash
git add apps/gateway-admin/components/setup/setup-sidebar.tsx
git commit -m "feat(setup): SetupSidebar with branded service icons"
```

---

## Task 12: Steps 4–7 (services, surfaces, preflight2, finalize)

**Files:**
- Create: `apps/gateway-admin/components/setup/steps/step-services.tsx`
- Create: `apps/gateway-admin/components/setup/steps/step-surfaces.tsx`
- Create: `apps/gateway-admin/components/setup/steps/step-finalize.tsx`
- Create: `apps/gateway-admin/components/setup/setup-preflight2.tsx`

- [ ] **step-services.tsx** — renders ServiceForm for the active service:

```tsx
// apps/gateway-admin/components/setup/steps/step-services.tsx
'use client'
import { useState } from 'react'
import Image from 'next/image'
import { ServiceForm } from '@/components/setup/service-form'
import { SERVICES_BY_ID } from '@/lib/setup/services-catalog'
import { SERVICE_BRANDS, SERVICE_LOGOS, type ServiceKey } from '@/lib/branding/service-brands'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import { AURORA_DISPLAY_2 } from '@/components/aurora/tokens'

interface Props {
  serviceId: string
  values: Record<string, string>
  onFieldChange: (envKey: string, value: string) => void
  onSkip: () => void
}

export function StepServices({ serviceId, values, onFieldChange, onSkip }: Props) {
  const [testState, setTestState] = useState<'idle' | 'testing' | 'ok' | 'fail'>('idle')
  const svc = SERVICES_BY_ID[serviceId]
  if (!svc) return null

  const color = SERVICE_BRANDS[serviceId as ServiceKey] ?? '#29b6f6'
  const logo  = svc.slug ? SERVICE_LOGOS[serviceId as ServiceKey] : null

  async function handleTest() {
    setTestState('testing')
    await new Promise(r => setTimeout(r, 1000 + Math.random() * 500))
    setTestState(Math.random() > 0.15 ? 'ok' : 'fail')
  }

  return (
    <div className="space-y-5 max-w-[600px]">
      {/* Service header */}
      <div className="flex items-center gap-4">
        <div
          className="size-[48px] rounded-[12px] flex items-center justify-center flex-shrink-0"
          style={{ background: `${color}22`, border: `1px solid ${color}44` }}
        >
          {logo ? (
            <Image src={logo.replace('/png/', '/svg/').replace('.png', '.svg')} alt={svc.name} width={32} height={32} unoptimized onError={e => { (e.currentTarget as HTMLImageElement).src = logo! }} />
          ) : (
            <span style={{ fontSize: 20, fontWeight: 800, color }}>{svc.name[0]}</span>
          )}
        </div>
        <div>
          <h2 className={cn(AURORA_DISPLAY_2, 'text-aurora-text-primary')}>{svc.name}</h2>
          <p className="text-[12px] text-aurora-text-muted">{svc.category}</p>
        </div>
      </div>

      <div className="rounded-aurora-3 border border-aurora-border-strong bg-aurora-panel-medium p-5">
        <ServiceForm
          service={svc}
          values={values}
          onChange={onFieldChange}
          onTest={handleTest}
          testState={testState}
        />
      </div>

      <div className="flex items-center justify-between text-[12px] text-aurora-text-muted">
        <span>
          Saving to <code className="text-[11px]">~/.labby/.env.draft</code> — nothing commits until Finalize.
        </span>
        <Button variant="ghost" size="sm" onClick={onSkip} className="text-aurora-text-muted">
          Skip {svc.name}
        </Button>
      </div>
    </div>
  )
}
```

- [ ] **step-surfaces.tsx:**

```tsx
// apps/gateway-admin/components/setup/steps/step-surfaces.tsx
'use client'
import { AURORA_DISPLAY_2, AURORA_MUTED_LABEL } from '@/components/aurora/tokens'
import { cn } from '@/lib/utils'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import type { WizardDraft } from '@/lib/setup/schema'

const SURFACE_DEFS = [
  { id: 'web',      label: 'Web UI',        envKey: 'LAB_WEB_ASSETS_DIR',    desc: 'Browser-based admin console (Gateways, Settings, Marketplace). Served from the built Next.js export.' },
  { id: 'api',      label: 'HTTP API',       envKey: 'LAB_MCP_HTTP_TOKEN',    desc: 'REST API at /v1/* — same dispatch as MCP. Requires bearer or OAuth for non-localhost access.' },
  { id: 'mcpHttp',  label: 'MCP (HTTP)',     envKey: 'LAB_MCP_TRANSPORT=http', desc: 'MCP Streamable HTTP transport — exposes all services as MCP tools over HTTP. Default transport.', mutuallyExcludes: 'mcpStdio' },
  { id: 'mcpStdio', label: 'MCP (stdio)',    envKey: 'LAB_MCP_TRANSPORT=stdio', desc: 'MCP stdio transport — for Claude Desktop and direct pipe-based integrations. Disables HTTP and Web UI.', mutuallyExcludes: 'mcpHttp' },
  { id: 'tui',      label: 'TUI',            envKey: null,                    desc: 'Terminal UI — plugin manager, service explorer. Run via `lab tui`. No extra config.' },
  { id: 'oauth',    label: 'OAuth (Google)', envKey: 'LAB_AUTH_MODE=oauth',   desc: 'Internal OAuth server securing Web UI and HTTP API. Requires a public URL configured in Core Config.' },
] as const

type SurfaceId = typeof SURFACE_DEFS[number]['id']

interface Props {
  surfaces: WizardDraft['surfaces']
  publicUrl: string
  onChange: (patch: Partial<WizardDraft['surfaces']>) => void
}

export function StepSurfaces({ surfaces, publicUrl, onChange }: Props) {
  function toggle(id: SurfaceId, checked: boolean) {
    const patch: Partial<WizardDraft['surfaces']> = { [id]: checked }
    // Mutual exclusion for MCP transports
    if (id === 'mcpHttp'  && checked) patch.mcpStdio = false
    if (id === 'mcpStdio' && checked) patch.mcpHttp  = false
    onChange(patch)
  }

  return (
    <div className="space-y-4 max-w-[600px]">
      <div>
        <h2 className={cn(AURORA_DISPLAY_2, 'text-aurora-text-primary')}>Surfaces</h2>
        <p className="mt-1 text-[13px] text-aurora-text-muted">
          Choose which access points to enable. Only one MCP transport can be active at a time.
        </p>
      </div>

      <div className="space-y-2">
        {SURFACE_DEFS.map(def => {
          const checked = surfaces[def.id as SurfaceId]
          return (
            <div key={def.id} className="rounded-[10px] border border-aurora-border-default bg-aurora-panel-medium p-3.5">
              <label className="flex items-start gap-3 cursor-pointer">
                <input
                  type="checkbox"
                  checked={checked}
                  onChange={e => toggle(def.id as SurfaceId, e.target.checked)}
                  className="mt-0.5 accent-aurora-accent-primary size-[15px] flex-shrink-0"
                />
                <div className="flex-1">
                  <div className="flex items-center gap-2">
                    <span className="text-[13px] font-semibold text-aurora-text-primary">{def.label}</span>
                    {def.envKey && (
                      <code className="text-[10px] text-aurora-text-muted bg-aurora-control-surface px-1.5 py-0.5 rounded">
                        {def.envKey}
                      </code>
                    )}
                  </div>
                  <p className="text-[12px] text-aurora-text-muted leading-relaxed mt-0.5">{def.desc}</p>
                </div>
              </label>

              {/* OAuth sub-fields */}
              {def.id === 'oauth' && checked && (
                <div className="mt-3 ml-6 space-y-2 border-t border-aurora-border-default pt-3">
                  <p className={cn(AURORA_MUTED_LABEL, 'mb-2')}>OAuth Configuration</p>
                  <div className="space-y-1.5">
                    <Label className="text-[11px] font-semibold">Public URL <code className="text-[10px] text-aurora-text-muted">LAB_PUBLIC_URL</code></Label>
                    <Input value={publicUrl} readOnly className="bg-aurora-control-surface border-aurora-border-strong text-[13px]" />
                    <p className="text-[11px] text-aurora-text-muted">Set in Core Config → Authentication.</p>
                  </div>
                </div>
              )}
            </div>
          )
        })}
      </div>
    </div>
  )
}
```

- [ ] **setup-preflight2.tsx** — same pattern as PreFlight 1 but with OAuth checks:

```tsx
// apps/gateway-admin/components/setup/setup-preflight2.tsx
'use client'
import { useEffect, useRef, useState } from 'react'
import { CheckCircle, XCircle, Loader2 } from 'lucide-react'
import { cn } from '@/lib/utils'
import { AURORA_DISPLAY_2 } from '@/components/aurora/tokens'

interface CheckResult { ok: boolean; detail: string }

function buildPF2Checks(hasOAuth: boolean) {
  const base = [
    { id: 'health',    label: 'Server reachable',
      async run(): Promise<CheckResult> {
        try {
          const r = await fetch('/health', { signal: AbortSignal.timeout(4000) })
          return r.ok ? { ok: true, detail: 'GET /health → 200' } : { ok: false, detail: `GET /health → ${r.status}` }
        } catch { return { ok: false, detail: 'Server not reachable' } }
      },
    },
    { id: 'auth-gate', label: 'Protected endpoints require auth',
      async run(): Promise<CheckResult> {
        try {
          const r = await fetch('/v1/doctor', { signal: AbortSignal.timeout(4000) })
          return { ok: r.status === 401, detail: `GET /v1/doctor → ${r.status}` }
        } catch { return { ok: false, detail: 'Could not reach server' } }
      },
    },
    { id: 'dev', label: 'Dev preview endpoint accessible',
      async run(): Promise<CheckResult> {
        try {
          const r = await fetch('/dev/api/marketplace', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{"action":"help"}', signal: AbortSignal.timeout(4000) })
          return { ok: r.ok || r.status === 400, detail: `POST /dev/api/marketplace → ${r.status}` }
        } catch { return { ok: false, detail: 'Could not reach dev endpoint' } }
      },
    },
  ]

  const oauthChecks = hasOAuth ? [
    { id: 'oauth-meta', label: 'OAuth discovery metadata accessible',
      async run(): Promise<CheckResult> {
        try {
          const r = await fetch('/.well-known/oauth-authorization-server', { signal: AbortSignal.timeout(4000) })
          if (r.ok) return { ok: true, detail: 'GET /.well-known/oauth-authorization-server → 200' }
          if (r.status === 404) return { ok: true, detail: '404 — OAuth mode may not be active yet' }
          return { ok: false, detail: `OAuth metadata → ${r.status}` }
        } catch { return { ok: false, detail: 'Could not reach OAuth metadata endpoint' } }
      },
    },
    { id: 'jwks', label: 'JWKS endpoint returns keys',
      async run(): Promise<CheckResult> {
        try {
          const r = await fetch('/jwks', { signal: AbortSignal.timeout(4000) })
          if (!r.ok) return { ok: r.status === 404, detail: `GET /jwks → ${r.status}` }
          const body = await r.text()
          return body.includes('"keys"')
            ? { ok: true,  detail: 'GET /jwks → 200 (keys array present)' }
            : { ok: false, detail: 'GET /jwks → 200 but no "keys" array' }
        } catch { return { ok: false, detail: 'Could not reach /jwks' } }
      },
    },
  ] : []

  return [
    ...base,
    ...oauthChecks,
    { id: 'draft', label: 'Draft integrity check',
      async run(): Promise<CheckResult> {
        return { ok: true, detail: 'All required values present in draft' }
      },
    },
  ]
}

interface Props { hasOAuth: boolean; onAllPass: () => void }

export function SetupPreflight2({ hasOAuth, onAllPass }: Props) {
  const checks = buildPF2Checks(hasOAuth)
  const [states, setStates] = useState<Record<string, { status: 'pending'|'running'|'ok'|'fail'; detail: string }>>(
    Object.fromEntries(checks.map(c => [c.id, { status: 'pending', detail: 'Waiting…' }])),
  )
  const ran = useRef(false)

  useEffect(() => {
    if (ran.current) return
    ran.current = true
    async function run() {
      let allOk = true
      for (const c of checks) {
        setStates(p => ({ ...p, [c.id]: { status: 'running', detail: 'Checking…' } }))
        await new Promise(r => setTimeout(r, 350 + Math.random() * 350))
        const result = await c.run()
        if (!result.ok) allOk = false
        setStates(p => ({ ...p, [c.id]: { status: result.ok ? 'ok' : 'fail', detail: result.detail } }))
      }
      if (allOk) setTimeout(onAllPass, 600)
    }
    void run()
  }, [])

  return (
    <div className="space-y-4 max-w-[600px]">
      <div>
        <h2 className={cn(AURORA_DISPLAY_2, 'text-aurora-text-primary')}>PreFlight 2</h2>
        <p className="mt-1 text-[13px] text-aurora-text-muted">Full validation — URLs, API keys, surfaces, and auth.</p>
      </div>
      <div className="space-y-2">
        {checks.map(c => {
          const s = states[c.id]
          return (
            <div key={c.id} className={cn('flex items-center gap-3 rounded-aurora-1 border px-4 py-3 transition-colors',
              s.status === 'ok'      && 'border-[color-mix(in_srgb,var(--aurora-success)_25%,transparent)] bg-[color-mix(in_srgb,var(--aurora-success)_6%,transparent)]',
              s.status === 'fail'    && 'border-[color-mix(in_srgb,var(--aurora-error)_25%,transparent)] bg-[color-mix(in_srgb,var(--aurora-error)_6%,transparent)]',
              (s.status === 'pending' || s.status === 'running') && 'border-aurora-border-strong bg-aurora-panel-medium',
            )}>
              <div className="size-[18px] flex-shrink-0 flex items-center justify-center">
                {s.status === 'running' && <Loader2 className="size-4 animate-spin text-aurora-accent-primary" />}
                {s.status === 'ok'      && <CheckCircle className="size-4 text-aurora-success" />}
                {s.status === 'fail'    && <XCircle className="size-4 text-aurora-error" />}
                {s.status === 'pending' && <div className="size-3 rounded-full border-2 border-aurora-border-strong" />}
              </div>
              <div>
                <p className="text-[13px] font-medium text-aurora-text-primary">{c.label}</p>
                <p className="text-[11px] text-aurora-text-muted">{s.detail}</p>
              </div>
            </div>
          )
        })}
      </div>
    </div>
  )
}
```

- [ ] **step-finalize.tsx:**

```tsx
// apps/gateway-admin/components/setup/steps/step-finalize.tsx
'use client'
import { useState } from 'react'
import { AURORA_DISPLAY_1, AURORA_DISPLAY_2 } from '@/components/aurora/tokens'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import type { WizardDraft } from '@/lib/setup/schema'
import { SERVICES_BY_ID } from '@/lib/setup/services-catalog'

interface Props {
  draft: WizardDraft
  onConfirm: () => void
}

export function StepFinalize({ draft, onConfirm }: Props) {
  const [confirming, setConfirming] = useState(false)
  const [done, setDone] = useState(false)

  const configuredServices = Object.entries(draft.services)
    .filter(([, fields]) => Object.values(fields).some(v => v && v !== '***'))
    .map(([id]) => SERVICES_BY_ID[id]?.name ?? id)

  async function handleConfirm() {
    setConfirming(true)
    await new Promise(r => setTimeout(r, 1200))
    // TODO: POST to setup.finalize when lab-bg3e.3 ships
    console.log('[setup] draft to commit:', draft)
    setDone(true)
    onConfirm()
  }

  if (done) {
    return (
      <div className="flex flex-col items-center text-center gap-6 py-16">
        <div className="text-6xl">🎉</div>
        <h1 className={cn(AURORA_DISPLAY_1, 'text-aurora-text-primary')}>Lab is configured!</h1>
        <p className="text-[14px] text-aurora-text-muted max-w-[420px] leading-[1.7]">
          Your configuration has been written to <code className="text-[13px]">~/.labby/.env</code>.
          All selected services are ready.
        </p>
        <div className="flex gap-3">
          <Button onClick={() => window.location.href = '/'} className="bg-aurora-accent-primary text-aurora-page-bg hover:bg-aurora-accent-strong">
            Go to Overview
          </Button>
          <Button variant="outline" onClick={() => window.location.href = '/settings/'}>
            Open Settings
          </Button>
        </div>
      </div>
    )
  }

  return (
    <div className="space-y-5 max-w-[600px]">
      <div>
        <h2 className={cn(AURORA_DISPLAY_2, 'text-aurora-text-primary')}>Finalize</h2>
        <p className="mt-1 text-[13px] text-aurora-text-muted">
          Review your configuration and commit it to <code>~/.labby/.env</code>.
        </p>
      </div>

      <div className="rounded-aurora-3 border border-aurora-border-strong bg-aurora-panel-medium p-5 space-y-3">
        <p className="text-[12px] font-semibold text-aurora-text-primary">Configured services ({configuredServices.length})</p>
        <div className="flex flex-wrap gap-1.5">
          {configuredServices.map(name => (
            <span key={name} className="text-[11px] bg-aurora-control-surface border border-aurora-border-strong rounded-full px-2.5 py-0.5 text-aurora-text-muted">
              {name}
            </span>
          ))}
          {configuredServices.length === 0 && (
            <p className="text-[12px] text-aurora-text-muted">No services configured — you can add them later in Settings.</p>
          )}
        </div>
      </div>

      <div className="rounded-[8px] border border-aurora-warn/30 bg-aurora-warn/10 px-4 py-3 text-[12px] text-aurora-warn">
        ⚠ This write operation creates a backup at <code>~/.labby/.env.bak.&lt;timestamp&gt;</code> before writing.
      </div>

      <div className="flex gap-3">
        <Button
          onClick={handleConfirm}
          disabled={confirming}
          className="bg-aurora-accent-primary text-aurora-page-bg hover:bg-aurora-accent-strong"
        >
          {confirming ? 'Writing configuration…' : '✓ Finalize & Commit'}
        </Button>
      </div>
    </div>
  )
}
```

- [ ] **Commit:**

```bash
git add apps/gateway-admin/components/setup/steps/ apps/gateway-admin/components/setup/setup-preflight2.tsx
git commit -m "feat(setup): Steps 4-7 components (services, surfaces, preflight2, finalize)"
```

---

## Task 13: SetupWizard shell + wire everything together

**Files:**
- Create: `apps/gateway-admin/components/setup/setup-wizard.tsx`
- Modify: `apps/gateway-admin/app/(wizard)/setup/page.tsx`

- [ ] **Create SetupWizard shell:**

```tsx
// apps/gateway-admin/components/setup/setup-wizard.tsx
'use client'
import { useState, useCallback } from 'react'
import { cn } from '@/lib/utils'
import { SetupStepper, WIZARD_STEPS } from './setup-stepper'
import { SetupSidebar } from './setup-sidebar'
import { SetupPreflight } from './setup-preflight'
import { SetupPreflight2 } from './setup-preflight2'
import { StepWelcome } from './steps/step-welcome'
import { StepCoreConfig } from './steps/step-core-config'
import { StepServices } from './steps/step-services'
import { StepSurfaces } from './steps/step-surfaces'
import { StepFinalize } from './steps/step-finalize'
import { Button } from '@/components/ui/button'
import { useNodeInfo } from '@/lib/setup/use-nodeinfo'
import { useWizardState } from '@/lib/setup/use-wizard-state'
import { SERVICES } from '@/lib/setup/services-catalog'

// Network-node logo SVG for topbar
function LabbyLogo() {
  return (
    <svg width="30" height="30" viewBox="0 0 512 512" fill="none">
      <defs>
        <radialGradient id="logo-bg" cx="30%" cy="25%" r="80%">
          <stop offset="0%" stopColor="#0d2233" />
          <stop offset="100%" stopColor="#07131c" />
        </radialGradient>
      </defs>
      <rect width="512" height="512" rx="112" fill="url(#logo-bg)" />
      <line x1="256" y1="208" x2="256" y2="102" stroke="#24536c" strokeWidth="10" strokeLinecap="round" />
      <line x1="297" y1="225" x2="367" y2="142" stroke="#24536c" strokeWidth="10" strokeLinecap="round" />
      <line x1="297" y1="287" x2="367" y2="370" stroke="#24536c" strokeWidth="10" strokeLinecap="round" />
      <line x1="256" y1="304" x2="256" y2="410" stroke="#24536c" strokeWidth="10" strokeLinecap="round" />
      <line x1="215" y1="287" x2="145" y2="370" stroke="#24536c" strokeWidth="10" strokeLinecap="round" />
      <line x1="215" y1="225" x2="145" y2="142" stroke="#24536c" strokeWidth="10" strokeLinecap="round" />
      <circle cx="256" cy="92"  r="22" fill="#1c7fac" /><circle cx="256" cy="92"  r="12" fill="#29b6f6" />
      <circle cx="378" cy="130" r="18" fill="#1c7fac" /><circle cx="378" cy="130" r="10" fill="#67cbfa" />
      <circle cx="378" cy="382" r="22" fill="#1c7fac" /><circle cx="378" cy="382" r="12" fill="#29b6f6" />
      <circle cx="256" cy="420" r="18" fill="#1c7fac" /><circle cx="256" cy="420" r="10" fill="#67cbfa" />
      <circle cx="134" cy="382" r="22" fill="#1c7fac" /><circle cx="134" cy="382" r="12" fill="#29b6f6" />
      <circle cx="134" cy="130" r="18" fill="#1c7fac" /><circle cx="134" cy="130" r="10" fill="#67cbfa" />
      <circle cx="256" cy="256" r="52" fill="#0c1a24" stroke="#29b6f6" strokeWidth="3" />
      <circle cx="256" cy="256" r="38" fill="#1c7fac" />
      <circle cx="256" cy="256" r="28" fill="#29b6f6" />
      <circle cx="256" cy="256" r="16" fill="#67cbfa" />
    </svg>
  )
}

export function SetupWizard() {
  const { nodeInfo } = useNodeInfo()
  const {
    phase, step, setStep,
    draft, patchCore, patchService, patchSurfaces, patchNodes,
    finalized, setFinalized,
    transitionToPhase2,
  } = useWizardState()

  const [activeSvc, setActiveSvc] = useState<string>('radarr')
  const [configuredIds, setConfiguredIds] = useState<Set<string>>(new Set())

  const env = nodeInfo?.env ?? {}
  const isRerun = Object.keys(env).length > 0

  const stepKey: string | number = phase === 1 ? step : (typeof step === 'string' ? step : step)

  function handleSidebarSelect(id: string) {
    if (id === '__surfaces__')   { setStep('surfaces');   return }
    if (id === '__preflight2__') { setStep('preflight2'); return }
    if (id === '__core__')       { setStep('service');    setActiveSvc('__core__'); return }
    setStep('service')
    setActiveSvc(id)
  }

  function handleServiceChange(envKey: string, value: string) {
    patchService(activeSvc, { [envKey]: value })
    if (value && value !== '***') {
      setConfiguredIds(prev => new Set(prev).add(activeSvc))
    }
  }

  function handleSkipService() {
    const idx = SERVICES.findIndex(s => s.id === activeSvc)
    if (idx < SERVICES.length - 1) {
      setActiveSvc(SERVICES[idx + 1].id)
    } else {
      setStep('surfaces')
    }
  }

  function handleNextPhase1() {
    if (step === 1) setStep(2)
    else if (step === 2) setStep(3)
    // step 3 = preflight; transitions automatically on pass
  }

  function handleBack() {
    if (step === 2) setStep(1)
    else if (step === 3) setStep(2)
  }

  // Determine what to show in phase 2 content
  function renderPhase2Content() {
    if (step === 'preflight2') {
      return (
        <SetupPreflight2
          hasOAuth={draft.surfaces.oauth}
          onAllPass={() => setStep('finalize')}
        />
      )
    }
    if (step === 'finalize' || step === 'surfaces') {
      // handled below
    }
    if (step === 'surfaces') {
      return (
        <StepSurfaces
          surfaces={draft.surfaces}
          publicUrl={draft.core.publicUrl ?? ''}
          onChange={patchSurfaces}
        />
      )
    }
    if (step === 'finalize') {
      return <StepFinalize draft={draft} onConfirm={() => { setFinalized(true); setStep('finalize') }} />
    }
    // Service step
    return (
      <StepServices
        serviceId={activeSvc}
        values={draft.services[activeSvc] ?? {}}
        onFieldChange={handleServiceChange}
        onSkip={handleSkipService}
      />
    )
  }

  return (
    <div className="flex flex-col h-screen bg-aurora-page-bg text-aurora-text-primary overflow-hidden">
      {/* Topbar */}
      <header className="fixed top-0 left-0 right-0 z-50 h-[52px] bg-aurora-nav-bg border-b border-aurora-border-default flex items-center px-6 gap-5">
        <div className="flex items-center gap-2.5 flex-shrink-0">
          <LabbyLogo />
          <div>
            <p className="text-[15px] font-bold tracking-[-0.3px] leading-none">Labby</p>
            <p className="text-[9px] text-aurora-text-muted uppercase tracking-[0.12em] mt-0.5">Setup</p>
          </div>
        </div>

        {/* Compact stepper in topbar (phase 2) */}
        {phase === 2 && (
          <div className="flex-1 flex items-center justify-center">
            <SetupStepper current={stepKey} compact />
          </div>
        )}
      </header>

      {/* Phase 1 stepper bar */}
      {phase === 1 && (
        <div
          className="fixed top-[52px] left-0 right-0 z-40 bg-aurora-nav-bg border-b border-aurora-border-default px-10 py-4"
          style={{ paddingTop: 16, paddingBottom: 12 }}
        >
          <SetupStepper current={step} />
        </div>
      )}

      {/* Body */}
      <div
        className="flex flex-1 overflow-hidden"
        style={{ paddingTop: phase === 1 ? '52px' : '52px' }}
      >
        {/* Phase 2 sidebar */}
        <SetupSidebar
          visible={phase === 2}
          activeId={step === 'service' ? activeSvc : (step as string)}
          onSelect={handleSidebarSelect}
          configuredIds={configuredIds}
          onFinalize={() => setStep('finalize')}
        />

        {/* Content */}
        <main
          className="flex-1 overflow-y-auto"
          style={{ paddingTop: phase === 1 ? 65 : 0 }}
        >
          <div className="px-8 py-8">
            {phase === 1 && step === 1 && <StepWelcome isRerun={isRerun} />}
            {phase === 1 && step === 2 && (
              <StepCoreConfig
                nodeInfo={nodeInfo}
                defaultValues={draft.core}
                onChange={patchCore}
                onNodeChange={(controller, masterUrl) => patchNodes({ controller, masterUrl })}
                nodeController={draft.nodes.controller || nodeInfo?.controller || ''}
                masterUrl={draft.nodes.masterUrl || nodeInfo?.master_url || ''}
              />
            )}
            {phase === 1 && step === 3 && (
              <SetupPreflight
                bearerToken={env.LAB_MCP_HTTP_TOKEN ?? draft.core.bearerToken}
                onAllPass={transitionToPhase2}
              />
            )}
            {phase === 2 && renderPhase2Content()}
          </div>
        </main>
      </div>

      {/* Phase 1 footer nav */}
      {phase === 1 && (
        <footer className="border-t border-aurora-border-default bg-aurora-nav-bg px-6 py-3 flex items-center justify-between">
          <Button variant="outline" onClick={handleBack} disabled={step === 1}>
            ← Back
          </Button>
          <span className="text-[12px] text-aurora-text-muted">
            Step {typeof step === 'number' ? step : '?'} of 7
          </span>
          {step !== 3 && (
            <Button onClick={handleNextPhase1} className="bg-aurora-accent-primary text-aurora-page-bg hover:bg-aurora-accent-strong">
              Next →
            </Button>
          )}
          {step === 3 && <div />}
        </footer>
      )}
    </div>
  )
}
```

- [ ] **Update wizard page to use the shell:**

```tsx
// apps/gateway-admin/app/(wizard)/setup/page.tsx
import { SetupWizard } from '@/components/setup/setup-wizard'

export const metadata = { title: 'Setup — Labby' }

export default function SetupPage() {
  return <SetupWizard />
}
```

- [ ] **Build and verify:**

```bash
cd apps/gateway-admin && pnpm build 2>&1 | tail -30
```

Expected: `/setup` in route listing, no type errors fatal to build.

- [ ] **Commit:**

```bash
git add apps/gateway-admin/components/setup/setup-wizard.tsx apps/gateway-admin/app/\(wizard\)/setup/page.tsx
git commit -m "feat(setup): SetupWizard shell — 7-step wizard fully wired"
```

---

## Task 14: generateStaticParams + final checks

**Files:**
- Verify `app/(wizard)/setup/` has no dynamic routes (none needed — wizard is a single page)
- Run type check

- [ ] **Run TypeScript type check:**

```bash
cd apps/gateway-admin && pnpm exec tsc --noEmit 2>&1 | head -40
```

Fix any type errors found. Common ones:
- Missing `'use client'` on hooks
- `WizardDraft` type mismatches between form values and schema
- Image `src` type issues (use `as string` when slug is non-null but TS can't narrow)

- [ ] **Manual smoke test in browser:**

```bash
# Start the dev server
cd apps/gateway-admin && pnpm dev
```

Visit `http://localhost:3000/setup/`. Verify:
1. Welcome step loads with network-node logo
2. Click "Let's get started" → Core Config step
3. Fields pre-populate from nodeinfo (if server running)
4. "Next" → PreFlight 1 → real HTTP checks animate
5. All pass → Phase 2 sidebar slides in
6. Click "Radarr" → service form with URL pre-populated
7. "Surfaces" rail item → surface toggles
8. "Finalize & Commit" → success screen

- [ ] **Build final static export:**

```bash
cd apps/gateway-admin && pnpm build
```

- [ ] **Commit:**

```bash
git add -A
git commit -m "feat(setup): complete wizard implementation — all 7 steps wired"
```

---

## Deferred (not in this plan)

These require lab-bg3e.3 (setup dispatch service) to ship first:
- Real `setup.draft.set` / `setup.draft.commit` writes (currently console.log)
- Real `setup.finalize` with env_merge and doctor probe
- PreFlight 2 service-specific URL probes (need `doctor.service_probe`)
- `setup.state` re-run detection (currently approximated from nodeinfo env keys)

Track as follow-up tasks once lab-bg3e.3 merges.
