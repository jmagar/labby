import type {
  Marketplace,
  Plugin,
  Artifact,
  ArtifactLang,
  MarketplaceSource,
  MarketplaceRuntime,
  PluginComponent,
  PluginInstallState,
  PluginManifestSummary,
} from '../types/marketplace.js'
import { marketplaceActionUrl } from './gateway-config.ts'
import { performServiceAction, type ServiceActionError } from './service-action-client.ts'
import type {
  DeployPluginWorkspacePreviewResult,
  DeployPluginWorkspaceResult,
  PluginWorkspace,
  SavePluginWorkspaceFileInput,
  SavePluginWorkspaceFileResult,
} from '../editor/types.js'

const USE_MOCK_DATA = process.env.NEXT_PUBLIC_MOCK_DATA === 'true'

const MOCK_MARKETPLACES: Marketplace[] = [
  {
    id: 'official',
    name: 'Official',
    owner: 'Labby',
    description: 'Curated first-party plugins for gateway operations and observability.',
    autoUpdateEnabled: true,
    pluginCount: 3,
    lastUpdatedAt: '2026-04-21T16:00:00Z',
    githubOwner: 'labby',
    repository: 'labby/marketplace',
    source: 'github',
  },
  {
    id: 'community',
    name: 'Community',
    owner: 'Labby Community',
    description: 'Community-maintained plugins vetted for homelab workflows.',
    autoUpdateEnabled: false,
    pluginCount: 2,
    lastUpdatedAt: '2026-04-20T12:00:00Z',
    githubOwner: 'modelcontextprotocol',
    repository: 'modelcontextprotocol/servers',
    source: 'github',
  },
]

const MOCK_PLUGINS: Plugin[] = [
  {
    id: 'gateway-audit',
    name: 'Gateway Audit',
    marketplaceId: 'official',
    version: '1.4.0',
    description: 'Audit gateway exposure and policy drift from the admin UI.',
    tags: ['gateway', 'audit', 'security'],
    installed: true,
    updatedAt: '2026-04-21T17:00:00Z',
    installedAt: '2026-04-21T17:05:00Z',
  },
  {
    id: 'tail-helper',
    name: 'Tail Helper',
    marketplaceId: 'official',
    version: '0.9.2',
    description: 'Saved log views and compact tail presets for operators.',
    tags: ['logs', 'tail'],
    installed: false,
    updatedAt: '2026-04-18T11:00:00Z',
  },
  {
    id: 'oauth-tracer',
    name: 'OAuth Tracer',
    marketplaceId: 'official',
    version: '2.0.1',
    description: 'Trace upstream OAuth state and callback routing.',
    tags: ['oauth', 'auth'],
    installed: true,
    updatedAt: '2026-04-19T09:30:00Z',
    installedAt: '2026-04-19T09:45:00Z',
  },
  {
    id: 'registry-curator',
    name: 'Registry Curator',
    marketplaceId: 'community',
    version: '0.5.0',
    description: 'Annotate and review MCP registry servers with local metadata.',
    tags: ['registry', 'metadata'],
    installed: false,
    updatedAt: '2026-04-16T13:20:00Z',
  },
  {
    id: 'health-digest',
    name: 'Health Digest',
    marketplaceId: 'community',
    version: '1.1.3',
    description: 'Summaries for gateway health, warnings, and rollout readiness.',
    tags: ['health', 'summary'],
    installed: true,
    updatedAt: '2026-04-20T08:45:00Z',
    installedAt: '2026-04-20T08:50:00Z',
  },
]

const MOCK_ARTIFACTS: Record<string, Artifact[]> = {
  'gateway-audit': [
    {
      path: 'labby-gateway-audit/plugin.json',
      lang: 'json',
      content: '{\n  "name": "gateway-audit",\n  "entry": "index.ts"\n}',
    },
    {
      path: 'labby-gateway-audit/README.md',
      lang: 'markdown',
      content: '# Gateway Audit\n\nOperator-facing checks for tool, prompt, and resource exposure.',
    },
  ],
  'tail-helper': [
    {
      path: 'tail-helper/README.md',
      lang: 'markdown',
      content: '# Tail Helper\n\nCurated searches and timeline presets for the Logs page.',
    },
  ],
}

function cloneValue<T>(value: T): T {
  return structuredClone(value)
}

export class MarketplaceApiError extends Error implements ServiceActionError {
  status: number
  code?: string

  constructor(message: string, status: number, code?: string) {
    super(message)
    this.name = 'MarketplaceApiError'
    this.status = status
    this.code = code
  }
}

async function marketplaceAction<T>(
  action: string,
  params: object,
  signal?: AbortSignal,
): Promise<T> {
  return performServiceAction<T, MarketplaceApiError>({
    action,
    params,
    signal,
    serviceLabel: 'Marketplace',
    url: marketplaceActionUrl(),
    createError: (message, status, code) => new MarketplaceApiError(message, status, code),
  })
}

export function detectArtifactLang(path: string): ArtifactLang {
  const fileName = path.split('/').pop() ?? path
  if (path.endsWith('.json')) return 'json'
  if (path.endsWith('.yaml') || path.endsWith('.yml')) return 'yaml'
  if (path.endsWith('.md')) return 'markdown'
  if (path.endsWith('.sh') || path.endsWith('.bash') || fileName === '.bashrc' || fileName === '.zshrc') return 'bash'
  if (path.endsWith('.toml')) return 'toml'
  return 'text'
}

type RawMarketplace = Pick<Marketplace, 'id' | 'name' | 'owner'> & Partial<Marketplace>
type RawPlugin = Pick<Plugin, 'id' | 'name' | 'tags' | 'installed'> & Partial<Plugin>
type RawArtifact = Omit<Artifact, 'lang'> & { lang?: ArtifactLang | null }
type RawPluginComponent = Partial<PluginComponent> & Pick<PluginComponent, 'path' | 'name'>
type RawPluginInstallState = Partial<PluginInstallState> & Pick<PluginInstallState, 'installed'>
type RawPluginManifestSummary = Partial<PluginManifestSummary>

function normalizeMarketplace(raw: RawMarketplace): Marketplace {
  const source = (raw.source ?? 'local') as MarketplaceSource
  const githubOwner = source === 'github'
    ? (raw.githubOwner ?? raw.ghUser ?? raw.owner) || undefined
    : undefined
  const repository = raw.repository ?? raw.repo
  const remoteUrl = raw.remoteUrl ?? raw.url
  const localPath = raw.localPath ?? raw.path
  const description = raw.description ?? raw.desc ?? ''
  const autoUpdateEnabled = raw.autoUpdateEnabled ?? raw.autoUpdate ?? false
  const pluginCount = raw.pluginCount ?? raw.totalPlugins ?? 0
  const lastUpdatedAt = raw.lastUpdatedAt ?? raw.lastUpdated ?? ''

  return {
    ...raw,
    source,
    githubOwner,
    repository,
    remoteUrl,
    localPath,
    description,
    autoUpdateEnabled,
    pluginCount,
    lastUpdatedAt,
    ghUser: githubOwner,
    repo: repository,
    url: remoteUrl,
    path: localPath,
    desc: description,
    autoUpdate: autoUpdateEnabled,
    totalPlugins: pluginCount,
    lastUpdated: lastUpdatedAt,
  }
}

function normalizePlugin(raw: RawPlugin): Plugin {
  const marketplaceId = raw.marketplaceId ?? raw.mkt ?? ''
  const version = raw.version ?? raw.ver ?? ''
  const description = raw.description ?? raw.desc ?? ''
  const runtime = raw.runtime as MarketplaceRuntime | undefined
  const components = raw.components?.map((component) => normalizePluginComponent(component as RawPluginComponent))
  const installState = raw.installState
    ? normalizePluginInstallState(raw.installState as RawPluginInstallState)
    : undefined
  const manifest = raw.manifest
    ? normalizePluginManifest(raw.manifest as RawPluginManifestSummary)
    : undefined

  return {
    ...raw,
    marketplaceId,
    version,
    description,
    mkt: marketplaceId,
    ver: version,
    desc: description,
    runtime,
    components,
    installState,
    manifest,
  }
}

function normalizePluginComponent(raw: RawPluginComponent): PluginComponent {
  return {
    kind: (raw.kind ?? 'files') as PluginComponent['kind'],
    path: raw.path,
    name: raw.name,
    metadata: raw.metadata,
  }
}

function normalizePluginInstallState(raw: RawPluginInstallState): PluginInstallState {
  return {
    installed: raw.installed,
    enabled: raw.enabled,
    installedAt: raw.installedAt,
    updatedAt: raw.updatedAt,
  }
}

function normalizePluginManifest(raw: RawPluginManifestSummary): PluginManifestSummary {
  return {
    description: raw.description,
    version: raw.version,
    interface: raw.interface,
  }
}

export async function fetchMarketplaces(signal?: AbortSignal): Promise<Marketplace[]> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    return cloneValue(MOCK_MARKETPLACES).map(normalizeMarketplace)
  }
  const marketplaces = await marketplaceAction<RawMarketplace[]>('sources.list', {}, signal)
  return marketplaces.map(normalizeMarketplace)
}

export async function fetchPlugins(signal?: AbortSignal): Promise<Plugin[]> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    return cloneValue(MOCK_PLUGINS).map(normalizePlugin)
  }
  const plugins = await marketplaceAction<RawPlugin[]>('plugins.list', {}, signal)
  return plugins.map(normalizePlugin)
}

export async function getInstalledPluginIds(signal?: AbortSignal): Promise<Set<string>> {
  const plugins = await fetchPlugins(signal)
  return new Set(plugins.filter(p => p.installed).map(p => p.id))
}

export async function getArtifacts(pluginId: string, signal?: AbortSignal): Promise<Artifact[]> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    return cloneValue(MOCK_ARTIFACTS[pluginId] ?? [])
  }
  const artifacts = await marketplaceAction<RawArtifact[]>('plugin.artifacts', { id: pluginId }, signal)
  return artifacts.map((artifact) => ({
    ...artifact,
    lang: artifact.lang ?? detectArtifactLang(artifact.path),
  }))
}

export async function getPluginComponents(pluginId: string, signal?: AbortSignal): Promise<PluginComponent[]> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    return []
  }
  return marketplaceAction<PluginComponent[]>('plugin.components', { id: pluginId }, signal)
}

export async function installPlugin(pluginId: string, signal?: AbortSignal): Promise<void> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    return
  }
  await marketplaceAction<unknown>('plugin.install', { id: pluginId }, signal)
}

export async function uninstallPlugin(pluginId: string, signal?: AbortSignal): Promise<void> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    return
  }
  await marketplaceAction<unknown>('plugin.uninstall', { id: pluginId, confirm: true }, signal)
}

export async function getPluginWorkspace(pluginId: string, signal?: AbortSignal): Promise<PluginWorkspace> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    const artifacts = MOCK_ARTIFACTS[pluginId] ?? []
    return {
      pluginId,
      deployTarget: '~/.lab/plugins',
      files: artifacts.map((artifact) => ({
        path: artifact.path,
        lang: artifact.lang === 'text' ? 'markdown' : artifact.lang,
        content: artifact.content,
      })),
    }
  }
  return marketplaceAction<PluginWorkspace>('plugin.workspace', { id: pluginId }, signal)
}

export async function savePluginWorkspaceFile(
  input: SavePluginWorkspaceFileInput,
  signal?: AbortSignal,
): Promise<SavePluginWorkspaceFileResult> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    return { savedAt: new Date().toISOString() }
  }
  return marketplaceAction<SavePluginWorkspaceFileResult>(
    'plugin.save',
    { id: input.pluginId, path: input.path, content: input.content },
    signal,
  )
}

export async function deployPluginWorkspace(
  pluginId: string,
  signal?: AbortSignal,
): Promise<DeployPluginWorkspaceResult> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    return {
      ok: true,
      changed: [pluginId],
      skipped: [],
      removed: [],
      failed: [],
      target: '~/.lab/plugins',
    }
  }
  return marketplaceAction<DeployPluginWorkspaceResult>(
    'plugin.deploy',
    { id: pluginId, confirm: true },
    signal,
  )
}

export async function previewPluginWorkspaceDeploy(
  pluginId: string,
  signal?: AbortSignal,
): Promise<DeployPluginWorkspacePreviewResult> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    return {
      changed: [`${pluginId}/README.md`],
      skipped: [],
      removed: [],
      target: '~/.lab/plugins',
    }
  }
  return marketplaceAction<DeployPluginWorkspacePreviewResult>(
    'plugin.deploy.preview',
    { id: pluginId },
    signal,
  )
}

export interface ForkMarketplaceArtifactInput {
  pluginId: string
  artifacts?: string[]
}

export interface MarketplaceForkStatus {
  plugin_id: string
  component_id: string
  stash_workspace: string
  forked_artifacts: string[]
  status: 'clean' | 'dirty' | 'unknown'
}

export function forkMarketplaceArtifact(
  input: ForkMarketplaceArtifactInput,
  signal?: AbortSignal,
): Promise<unknown> {
  return marketplaceAction(
    'artifact.fork',
    {
      plugin_id: input.pluginId,
      ...(input.artifacts?.length ? { artifacts: input.artifacts } : {}),
      confirm: true,
    },
    signal,
  )
}

export function listMarketplaceForks(
  pluginId?: string,
  signal?: AbortSignal,
): Promise<MarketplaceForkStatus[]> {
  return marketplaceAction(
    'artifact.list',
    {
      ...(pluginId ? { plugin_id: pluginId } : {}),
    },
    signal,
  )
}

export function resetMarketplaceArtifact(
  input: ForkMarketplaceArtifactInput,
  signal?: AbortSignal,
): Promise<unknown> {
  return marketplaceAction(
    'artifact.reset',
    {
      plugin_id: input.pluginId,
      ...(input.artifacts?.length ? { artifacts: input.artifacts } : {}),
      confirm: true,
    },
    signal,
  )
}

export function unforkMarketplaceArtifact(
  input: ForkMarketplaceArtifactInput,
  signal?: AbortSignal,
): Promise<unknown> {
  return marketplaceAction(
    'artifact.unfork',
    {
      plugin_id: input.pluginId,
      ...(input.artifacts?.length ? { artifacts: input.artifacts } : {}),
      confirm: true,
    },
    signal,
  )
}

export interface AcpAgentInstallParams {
  agent_id: string
  device_ids: string[]
  scope: 'global' | 'project'
  project_path?: string
  env_vars?: Record<string, string>
}

export interface AcpAgentInstallDeviceResult {
  device_id: string
  ok: boolean
  message?: string
}

export interface AcpAgentInstallResult {
  results: AcpAgentInstallDeviceResult[]
}

type RawAcpAgentInstallDeviceResult = {
  device_id?: string
  node_id?: string
  ok: boolean
  message?: string
  error?: unknown
}

type RawAcpAgentInstallResult = {
  results?: RawAcpAgentInstallDeviceResult[]
}

function normalizeAcpInstallResult(raw: RawAcpAgentInstallResult): AcpAgentInstallResult {
  return {
    results: (raw.results ?? []).map((result) => {
      const message =
        result.message ??
        (typeof result.error === 'string' ? result.error : undefined) ??
        (result.ok && (result.device_id ?? result.node_id) === 'local'
          ? 'Available in /chat'
          : undefined)
      return {
        device_id: result.device_id ?? result.node_id ?? 'unknown',
        ok: result.ok,
        ...(message ? { message } : {}),
      }
    }),
  }
}

export async function installAcpAgent(
  params: AcpAgentInstallParams,
  signal?: AbortSignal,
): Promise<AcpAgentInstallResult> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    return {
      results: params.device_ids.map(device_id => ({ device_id, ok: true })),
    }
  }
  const raw = await marketplaceAction<RawAcpAgentInstallResult>(
    'agent.install',
    { id: params.agent_id, node_ids: params.device_ids, confirm: true },
    signal,
  )
  const result = normalizeAcpInstallResult(raw)
  if (result.results.some((item) => item.ok && item.device_id === 'local')) {
    globalThis.dispatchEvent?.(new CustomEvent('lab:acp-providers-changed'))
  }
  return result
}

// ── Cherry-pick ────────────────────────────────────────────────────────────

export interface CherryPickPluginParams {
  plugin_id: string
  /** Paths of the selected PluginComponent items */
  components: string[]
  device_ids: string[]
  scope: 'global' | 'project'
  project_path?: string
  confirm: true
}

export interface CherryPickPluginResult {
  /** RPC ID for SSE progress stream; absent when install completes synchronously */
  rpc_id?: string
}

export async function cherryPickPlugin(
  params: CherryPickPluginParams,
  signal?: AbortSignal,
): Promise<CherryPickPluginResult> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    return { rpc_id: `mock-rpc-${Date.now()}` }
  }
  return marketplaceAction<CherryPickPluginResult>('plugin.cherry_pick', params, signal)
}

export async function addMarketplace(
  input: { repo?: string; url?: string; name?: string; autoUpdate: boolean },
  signal?: AbortSignal,
): Promise<Marketplace> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    if (!input.repo && !input.url) {
      throw new MarketplaceApiError(
        'addMarketplace requires either `repo` or `url`',
        400,
        'missing_param',
      )
    }
    const target = (input.repo ?? input.url) as string
    return normalizeMarketplace({
      id: target,
      name: input.name ?? target,
      owner: input.repo?.split('/')[0] ?? 'custom',
      description: '',
      autoUpdateEnabled: input.autoUpdate,
      pluginCount: 0,
      lastUpdatedAt: new Date().toISOString(),
      githubOwner: input.repo?.split('/')[0] ?? undefined,
      repository: input.repo,
      remoteUrl: input.url,
      source: input.repo ? 'github' : 'git',
    })
  }
  const params: Record<string, unknown> = {}
  if (input.repo) params.repo = input.repo
  else if (input.url) params.url = input.url
  else {
    throw new MarketplaceApiError(
      'addMarketplace requires either `repo` or `url`',
      400,
      'missing_param',
    )
  }
  params.autoUpdate = input.autoUpdate
  await marketplaceAction<unknown>('sources.add', params, signal)
  const sources = await fetchMarketplaces(signal)
  const target = input.repo ?? input.url
  const found = sources.find(m =>
    m.repository === target || m.repo === target || m.remoteUrl === target || m.url === target || m.id === target
  )
  if (found) return found
  return normalizeMarketplace({
    id: input.repo ?? input.url ?? `custom-${Date.now()}`,
    name: input.name ?? input.repo ?? input.url ?? 'Custom',
    owner: input.repo?.split('/')[0] ?? '',
    description: '',
    autoUpdateEnabled: input.autoUpdate,
    pluginCount: 0,
    lastUpdatedAt: new Date().toISOString(),
    githubOwner: input.repo?.split('/')[0] ?? undefined,
    repo: input.repo,
    url: input.url,
    source: input.repo ? 'github' : 'git',
    desc: '',
    autoUpdate: input.autoUpdate,
    totalPlugins: 0,
    lastUpdated: new Date().toISOString(),
  })
}
