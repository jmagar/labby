import type { Marketplace, MarketplaceRuntime, Plugin, PluginComponent, PluginComponentKind } from '@/lib/types/marketplace'
import type { AcpAgent, McpServer } from '@/lib/marketplace/types'

export type MarketplaceCatalogKind =
  | 'plugin'
  | 'agent'
  | 'skill'
  | 'command'
  | 'mcp_server'
  | 'lsp_server'
  | 'acp_agent'
  | 'app'
  | 'hook'
  | 'channel'
  | 'executable'
  | 'theme'
  | 'asset'
  | 'file'
  | 'config'
  | 'settings'
  | 'monitor'
  | 'output_style'
  | 'source'
export type MarketplaceCatalogLens = 'all' | 'installed' | 'plugins' | 'agents' | 'skills' | 'commands' | 'mcp_servers' | 'acp_agents' | 'sources'
export type MarketplaceInstallFacet = 'installed' | 'not_installed' | 'update_available' | 'builtin'
export type MarketplaceSort = 'name' | 'source' | 'installed' | 'updated'

export interface MarketplaceCatalogItem {
  id: string
  kind: MarketplaceCatalogKind
  name: string
  subtitle: string
  description: string
  version?: string
  sourceId?: string
  sourceName?: string
  distribution?: string
  ecosystem: string
  installed: boolean
  hasUpdate: boolean
  builtin: boolean
  updatedAt?: string
  avatar?: MarketplaceCatalogAvatar
  tags: string[]
  raw: unknown
  searchText: string
  updatedAtMs: number
}

export interface MarketplaceCatalogAvatar {
  kind: 'github'
  owner: string
}

export interface MarketplaceCatalogFilterState {
  lens: MarketplaceCatalogLens
  search: string
  types: MarketplaceCatalogKind[]
  installStates: MarketplaceInstallFacet[]
  ecosystems: string[]
  sourceIds: string[]
  distributions: string[]
  sort: MarketplaceSort
}

export interface MarketplaceCatalogSummary {
  all: number
  installed: number
  plugins: number
  agents: number
  skills: number
  commands: number
  mcpServers: number
  lspServers: number
  acpAgents: number
  sources: number
  updates: number
}

export interface PluginComponentCatalogRaw {
  plugin: Plugin
  component: PluginComponent
}

interface BuildMarketplaceCatalogItemsInput {
  plugins: Plugin[]
  sources: Marketplace[]
  mcpServers: Array<McpServer | McpRegistryEnvelope>
  acpAgents: AcpAgent[]
}

interface McpPackage {
  registryType?: string
  identifier?: string
  transport?: { type?: string }
}

interface McpRemote {
  type?: string
}

interface McpRegistryEnvelope {
  server?: McpServer & {
    title?: string
    packages?: McpPackage[]
    remotes?: McpRemote[]
    repository?: { source?: string; url?: string }
    websiteUrl?: string
  }
  _meta?: {
    'io.modelcontextprotocol.registry/official'?: {
      updatedAt?: string
      publishedAt?: string
      isLatest?: boolean
      status?: string
    }
  }
}

const RUNTIME_LABELS: Record<MarketplaceRuntime, string> = {
  claude: 'Claude',
  codex: 'Codex',
  gemini: 'Gemini',
}

const CATALOG_COLLATOR = new Intl.Collator(undefined, { sensitivity: 'base' })
const SEARCH_KIND_BOOST: Partial<Record<MarketplaceCatalogKind, number>> = {
  plugin: 40,
  mcp_server: 20,
  acp_agent: 20,
  source: 10,
}

export const MCP_REGISTRY_SOURCE_ID = 'mcp-registry'
export const MCP_REGISTRY_SOURCE_NAME = 'MCP Registry'
export const ACP_REGISTRY_SOURCE_ID = 'acp-registry'
export const ACP_REGISTRY_SOURCE_NAME = 'ACP Registry'

function sourceDisplayName(source: Marketplace): string {
  return source.name || source.id
}

function sourceSubtitle(source: Marketplace): string {
  if (source.source === 'github') return source.repository ?? source.repo ?? source.githubOwner ?? 'GitHub source'
  if (source.source === 'git') return source.remoteUrl ?? source.url ?? 'Git source'
  return source.localPath ?? source.path ?? 'Local source'
}

function githubOwnerFromRepository(repository?: string): string | undefined {
  if (!repository) return undefined
  const trimmed = repository.trim()
  if (!trimmed) return undefined

  const githubUrlMatch = trimmed.match(/github\.com[:/]([^/\s]+)\/[^/\s#?]+/i)
  if (githubUrlMatch?.[1]) return githubUrlMatch[1]

  const slugMatch = trimmed.match(/^([^/\s]+)\/[^/\s]+$/)
  return slugMatch?.[1]
}

function githubOwnerFromSource(source?: Marketplace): string | undefined {
  if (!source) return undefined
  if (source.githubOwner) return source.githubOwner
  if (source.ghUser) return source.ghUser
  return githubOwnerFromRepository(source.repository ?? source.repo ?? source.remoteUrl ?? source.url)
}

function githubAvatar(owner?: string): MarketplaceCatalogAvatar | undefined {
  const normalized = owner?.trim()
  if (!normalized) return undefined
  return { kind: 'github', owner: normalized }
}

function normalizeMcpServer(entry: McpServer | McpRegistryEnvelope): {
  server: McpRegistryEnvelope['server'] & McpServer
  updatedAt?: string
  isLatest?: boolean
  raw: McpServer | McpRegistryEnvelope
} {
  const envelope = entry as McpRegistryEnvelope
  const server = (envelope.server ?? entry) as McpRegistryEnvelope['server'] & McpServer
  const officialMeta = envelope._meta?.['io.modelcontextprotocol.registry/official']
  return {
    server,
    updatedAt: officialMeta?.updatedAt ?? officialMeta?.publishedAt,
    isLatest: officialMeta?.isLatest,
    raw: entry,
  }
}

function mcpDisplayName(server: McpRegistryEnvelope['server'] & McpServer): string {
  return server.title ?? server.name ?? server.packages?.[0]?.identifier ?? 'Unknown MCP server'
}

function mcpIdentifier(server: McpRegistryEnvelope['server'] & McpServer): string {
  return server.name ?? server.packages?.[0]?.identifier ?? mcpDisplayName(server)
}

function githubOwnerFromMcpServer(server: McpRegistryEnvelope['server'] & McpServer): string | undefined {
  const identifier = mcpIdentifier(server)
  const registryNameMatch = identifier.match(/^io\.github\.([^/]+)\//i)
  if (registryNameMatch?.[1]) return registryNameMatch[1]
  return githubOwnerFromRepository(server.repository?.url ?? server.websiteUrl)
}

function mcpSubtitle(server: McpRegistryEnvelope['server'] & McpServer): string {
  return server.package ?? server.packages?.[0]?.identifier ?? server.repository?.url ?? 'MCP Registry'
}

function mcpDistribution(server: McpRegistryEnvelope['server'] & McpServer): string {
  const packageName = server.package ?? server.packages?.[0]?.identifier ?? ''
  const registryType = server.packages?.[0]?.registryType?.toLowerCase()
  const transport = server.transport?.[0] ?? server.packages?.[0]?.transport?.type ?? server.remotes?.[0]?.type
  if (registryType === 'npm' || packageName.startsWith('@') || packageName.includes('npm')) return 'npm'
  if (registryType === 'pypi' || packageName.includes('uvx') || packageName.toLowerCase().includes('python')) return 'Python'
  if (registryType === 'oci') return 'Docker'
  return transport ?? 'MCP'
}

function agentDistribution(agent: AcpAgent): string {
  const distribution = agent.distribution ?? {}
  if (distribution.npx !== undefined) return 'npm'
  if (distribution.uvx !== undefined) return 'Python'
  if (distribution.binary !== undefined) return 'Binary'
  return 'ACP'
}

function githubOwnerFromAcpAgent(agent: AcpAgent): string | undefined {
  return githubOwnerFromRepository(agent.repository ?? agent.website)
}

function pluginEcosystem(plugin: Plugin): string {
  return plugin.runtime ? RUNTIME_LABELS[plugin.runtime] : 'Generic'
}

function componentCatalogKind(kind: PluginComponentKind): MarketplaceCatalogKind {
  if (kind === 'agents') return 'agent'
  if (kind === 'skills') return 'skill'
  if (kind === 'commands') return 'command'
  if (kind === 'mcp_servers' || kind === 'mcp-config') return 'mcp_server'
  if (kind === 'lsp_servers' || kind === 'lsp-config') return 'lsp_server'
  if (kind === 'apps') return 'app'
  if (kind === 'hooks') return 'hook'
  if (kind === 'channels') return 'channel'
  if (kind === 'monitors') return 'monitor'
  if (kind === 'bin') return 'executable'
  if (kind === 'settings') return 'settings'
  if (kind === 'themes') return 'theme'
  if (kind === 'output-styles' || kind === 'output_styles') return 'output_style'
  if (kind === 'files') return 'file'
  return 'asset'
}

function componentDistribution(kind: MarketplaceCatalogKind): string {
  if (kind === 'agent') return 'Agent'
  if (kind === 'skill') return 'Skill'
  if (kind === 'command') return 'Command'
  if (kind === 'mcp_server') return 'MCP server'
  if (kind === 'lsp_server') return 'LSP server'
  if (kind === 'app') return 'App'
  if (kind === 'hook') return 'Hook'
  if (kind === 'channel') return 'Channel'
  if (kind === 'monitor') return 'Monitor'
  if (kind === 'executable') return 'Executable'
  if (kind === 'theme') return 'Theme'
  if (kind === 'settings') return 'Settings'
  if (kind === 'output_style') return 'Output style'
  if (kind === 'config') return 'Config'
  if (kind === 'file') return 'File'
  return 'Asset'
}

function componentDescription(component: PluginComponent, plugin: Plugin): string {
  const metadata = component.metadata ?? {}
  const description = metadata.description
  return typeof description === 'string' && description.trim() ? description : plugin.description || plugin.desc || ''
}

type MarketplaceCatalogItemInput = Omit<MarketplaceCatalogItem, 'searchText' | 'updatedAtMs'>

function catalogSearchText(item: MarketplaceCatalogItemInput): string {
  return [
    item.name,
    item.subtitle,
    item.description,
    item.sourceName ?? '',
    item.distribution ?? '',
    item.ecosystem,
    ...item.tags,
  ]
    .join(' ')
    .toLowerCase()
}

function catalogUpdatedAtMs(updatedAt?: string): number {
  const timestamp = new Date(updatedAt ?? 0).getTime()
  return Number.isFinite(timestamp) ? timestamp : 0
}

function catalogItem(item: MarketplaceCatalogItemInput): MarketplaceCatalogItem {
  return {
    ...item,
    searchText: catalogSearchText(item),
    updatedAtMs: catalogUpdatedAtMs(item.updatedAt),
  }
}

function mcpEntryTimestamp(entry: ReturnType<typeof normalizeMcpServer>): number {
  return catalogUpdatedAtMs(entry.updatedAt)
}

function dedupeMcpServers(entries: Array<McpServer | McpRegistryEnvelope>): Array<ReturnType<typeof normalizeMcpServer>> {
  const byIdentifier = new Map<string, ReturnType<typeof normalizeMcpServer>>()
  for (const entry of entries) {
    const normalized = normalizeMcpServer(entry)
    const identifier = mcpIdentifier(normalized.server)
    const current = byIdentifier.get(identifier)
    if (
      !current
      || (normalized.isLatest && !current.isLatest)
      || (normalized.isLatest === current.isLatest && mcpEntryTimestamp(normalized) > mcpEntryTimestamp(current))
    ) {
      byIdentifier.set(identifier, normalized)
    }
  }
  return [...byIdentifier.values()]
}

function pluginComponentItems(
  plugin: Plugin,
  sourceNames: Map<string, string>,
  sourceAvatars: Map<string, MarketplaceCatalogAvatar>,
): MarketplaceCatalogItem[] {
  return (plugin.components ?? []).map((component): MarketplaceCatalogItem => {
    const kind = componentCatalogKind(component.kind)
    return catalogItem({
      id: `component:${plugin.id}:${component.kind}:${component.path}`,
      kind,
      name: component.name || component.path,
      subtitle: `${plugin.name} / ${component.path}`,
      description: componentDescription(component, plugin),
      version: plugin.version || plugin.ver,
      sourceId: plugin.marketplaceId,
      sourceName: sourceNames.get(plugin.marketplaceId) ?? plugin.marketplaceId,
      distribution: componentDistribution(kind),
      ecosystem: pluginEcosystem(plugin),
      installed: plugin.installed,
      hasUpdate: Boolean(plugin.hasUpdate),
      builtin: false,
      updatedAt: plugin.updatedAt,
      avatar: sourceAvatars.get(plugin.marketplaceId),
      tags: [plugin.name, component.path, ...(plugin.tags ?? [])],
      raw: { plugin, component } satisfies PluginComponentCatalogRaw,
    })
  })
}

export function isPluginCatalogItem(item: MarketplaceCatalogItem | null): item is MarketplaceCatalogItem & { raw: Plugin } {
  return item?.kind === 'plugin'
}

export function isMcpServerCatalogItem(item: MarketplaceCatalogItem | null): item is MarketplaceCatalogItem & { raw: McpServer | McpRegistryEnvelope } {
  return item?.kind === 'mcp_server'
}

export function isAcpAgentCatalogItem(item: MarketplaceCatalogItem | null): item is MarketplaceCatalogItem & { raw: AcpAgent } {
  return item?.kind === 'acp_agent'
}

export function isPluginComponentCatalogItem(item: MarketplaceCatalogItem | null): item is MarketplaceCatalogItem & { raw: PluginComponentCatalogRaw } {
  return Boolean(
    item
      && item.kind !== 'plugin'
      && item.kind !== 'mcp_server'
      && item.kind !== 'acp_agent'
      && item.kind !== 'source'
      && typeof item.raw === 'object'
      && item.raw !== null
      && 'plugin' in item.raw
      && 'component' in item.raw,
  )
}

export function catalogItemMcpServer(item: MarketplaceCatalogItem): McpServer | null {
  if (!isMcpServerCatalogItem(item)) return null
  const envelope = item.raw as McpRegistryEnvelope
  return (envelope.server ?? item.raw) as McpServer
}

export function buildMarketplaceCatalogItems({
  plugins,
  sources,
  mcpServers,
  acpAgents,
}: BuildMarketplaceCatalogItemsInput): MarketplaceCatalogItem[] {
  const sourceNames = new Map(sources.map((source) => [source.id, sourceDisplayName(source)]))
  const sourceAvatars = new Map(
    sources.flatMap((source) => {
      const avatar = githubAvatar(githubOwnerFromSource(source))
      return avatar ? [[source.id, avatar] as const] : []
    }),
  )

  return [
    ...plugins.flatMap((plugin): MarketplaceCatalogItem[] => [
      catalogItem({
        id: `plugin:${plugin.id}`,
        kind: 'plugin',
        name: plugin.name,
        subtitle: plugin.marketplaceId,
        description: plugin.description || plugin.desc || '',
        version: plugin.version || plugin.ver,
        sourceId: plugin.marketplaceId,
        sourceName: sourceNames.get(plugin.marketplaceId) ?? plugin.marketplaceId,
        distribution: 'Plugin package',
        ecosystem: pluginEcosystem(plugin),
        installed: plugin.installed,
        hasUpdate: Boolean(plugin.hasUpdate),
        builtin: false,
        updatedAt: plugin.updatedAt,
        avatar: sourceAvatars.get(plugin.marketplaceId),
        tags: plugin.tags ?? [],
        raw: plugin,
      }),
      ...pluginComponentItems(plugin, sourceNames, sourceAvatars),
    ]),
    ...dedupeMcpServers(mcpServers).map(({ server, updatedAt, raw }): MarketplaceCatalogItem => {
      return catalogItem({
        id: `mcp:${mcpIdentifier(server)}`,
        kind: 'mcp_server',
        name: mcpDisplayName(server),
        subtitle: mcpSubtitle(server),
        description: server.description ?? '',
        version: server.version,
        sourceId: MCP_REGISTRY_SOURCE_ID,
        sourceName: MCP_REGISTRY_SOURCE_NAME,
        distribution: mcpDistribution(server),
        ecosystem: 'MCP',
        installed: false,
        hasUpdate: false,
        builtin: false,
        updatedAt,
        avatar: githubAvatar(githubOwnerFromMcpServer(server)),
        tags: [server.transport?.[0], server.packages?.[0]?.registryType, server.remotes?.[0]?.type].filter(Boolean) as string[],
        raw,
      })
    }),
    ...acpAgents.map((agent): MarketplaceCatalogItem => catalogItem({
      id: `agent:${agent.id}`,
      kind: 'acp_agent',
      name: agent.name,
      subtitle: agent.id,
      description: agent.description ?? '',
      version: agent.version,
      sourceId: ACP_REGISTRY_SOURCE_ID,
      sourceName: ACP_REGISTRY_SOURCE_NAME,
      distribution: agentDistribution(agent),
      ecosystem: 'ACP',
      installed: Boolean(agent.installed),
      hasUpdate: false,
      builtin: Boolean(agent.builtin),
      updatedAt: agent.installedAt,
      avatar: githubAvatar(githubOwnerFromAcpAgent(agent)),
      tags: [agent.license, ...(agent.authors ?? [])].filter(Boolean) as string[],
      raw: agent,
    })),
    ...sources.map((source): MarketplaceCatalogItem => catalogItem({
      id: `source:${source.id}`,
      kind: 'source',
      name: sourceDisplayName(source),
      subtitle: sourceSubtitle(source),
      description: source.description || source.desc || '',
      version: undefined,
      sourceId: source.id,
      sourceName: sourceDisplayName(source),
      distribution: source.source === 'github' ? 'GitHub' : source.source === 'git' ? 'git' : 'local',
      ecosystem: 'Source',
      installed: false,
      hasUpdate: false,
      builtin: false,
      updatedAt: source.lastUpdatedAt || source.lastUpdated,
      avatar: sourceAvatars.get(source.id),
      tags: [source.source, source.autoUpdateEnabled || source.autoUpdate ? 'auto-update' : 'manual'],
      raw: source,
    })),
  ]
}

export function marketplaceCatalogSummary(items: MarketplaceCatalogItem[]): MarketplaceCatalogSummary {
  const acc: MarketplaceCatalogSummary = {
    all: items.length,
    installed: 0,
    plugins: 0,
    agents: 0,
    skills: 0,
    commands: 0,
    mcpServers: 0,
    lspServers: 0,
    acpAgents: 0,
    sources: 0,
    updates: 0,
  }
  for (const item of items) {
    if (item.installed) acc.installed++
    if (item.hasUpdate) acc.updates++
    if (item.kind === 'plugin') acc.plugins++
    else if (item.kind === 'agent') acc.agents++
    else if (item.kind === 'skill') acc.skills++
    else if (item.kind === 'command') acc.commands++
    else if (item.kind === 'mcp_server') acc.mcpServers++
    else if (item.kind === 'lsp_server') acc.lspServers++
    else if (item.kind === 'acp_agent') acc.acpAgents++
    else if (item.kind === 'source') acc.sources++
  }
  return acc
}

function matchesLens(item: MarketplaceCatalogItem, lens: MarketplaceCatalogLens): boolean {
  if (lens === 'all') return true
  if (lens === 'installed') return item.installed
  if (lens === 'plugins') return item.kind === 'plugin'
  if (lens === 'agents') return item.kind === 'agent'
  if (lens === 'skills') return item.kind === 'skill'
  if (lens === 'commands') return item.kind === 'command'
  if (lens === 'mcp_servers') return item.kind === 'mcp_server'
  if (lens === 'acp_agents') return item.kind === 'acp_agent'
  return item.kind === 'source'
}

function matchesInstallFacets(item: MarketplaceCatalogItem, facets: MarketplaceInstallFacet[]): boolean {
  if (facets.length === 0) return true

  const actual = new Set<MarketplaceInstallFacet>()
  if (item.installed) actual.add('installed')
  if (!item.installed && item.kind !== 'source') actual.add('not_installed')
  if (item.hasUpdate) actual.add('update_available')
  if (item.builtin) actual.add('builtin')

  return facets.some((facet) => actual.has(facet))
}

function matchesSearch(item: MarketplaceCatalogItem, normalizedSearch: string): boolean {
  if (!normalizedSearch) return true
  return item.searchText.includes(normalizedSearch)
}

function matchesAny<T extends string>(selected: T[], actual?: T): boolean {
  return selected.length === 0 || (actual !== undefined && selected.includes(actual))
}

function searchRank(item: MarketplaceCatalogItem, normalizedSearch: string): number {
  if (!normalizedSearch) return 0

  const kindBoost = SEARCH_KIND_BOOST[item.kind] ?? 0
  const name = item.name.toLowerCase()
  const subtitle = item.subtitle.toLowerCase()
  const sourceName = item.sourceName?.toLowerCase() ?? ''
  const distribution = item.distribution?.toLowerCase() ?? ''
  const tags = item.tags.map((tag) => tag.toLowerCase())

  if (name === normalizedSearch) return 700 + kindBoost
  if (name.startsWith(normalizedSearch)) return 600 + kindBoost
  if (name.includes(normalizedSearch)) return 500 + kindBoost
  if (subtitle.includes(normalizedSearch)) return 400 + kindBoost
  if (sourceName.includes(normalizedSearch)) return 350 + kindBoost
  if (distribution.includes(normalizedSearch)) return 300 + kindBoost
  if (tags.some((tag) => tag === normalizedSearch || tag.startsWith(normalizedSearch))) return 250 + kindBoost
  if (tags.some((tag) => tag.includes(normalizedSearch))) return 200 + kindBoost
  return 100
}

export function filterMarketplaceCatalogItems(
  items: MarketplaceCatalogItem[],
  state: MarketplaceCatalogFilterState,
): MarketplaceCatalogItem[] {
  const normalizedSearch = state.search.trim().toLowerCase()
  return items.filter((item) => {
    if (!matchesLens(item, state.lens)) return false
    if (!matchesSearch(item, normalizedSearch)) return false
    if (!matchesAny(state.types, item.kind)) return false
    if (!matchesInstallFacets(item, state.installStates)) return false
    if (!matchesAny(state.ecosystems, item.ecosystem)) return false
    if (!matchesAny(state.sourceIds, item.sourceId)) return false
    if (!matchesAny(state.distributions, item.distribution)) return false
    return true
  })
}

export function sortMarketplaceCatalogItems(
  items: MarketplaceCatalogItem[],
  sort: MarketplaceSort,
  search = '',
): MarketplaceCatalogItem[] {
  const normalizedSearch = search.trim().toLowerCase()
  return [...items].sort((left, right) => {
    if (normalizedSearch) {
      const bySearch = searchRank(right, normalizedSearch) - searchRank(left, normalizedSearch)
      if (bySearch !== 0) return bySearch
    }

    if (sort === 'installed') {
      if (left.installed !== right.installed) return left.installed ? -1 : 1
      if (left.hasUpdate !== right.hasUpdate) return left.hasUpdate ? -1 : 1
    }

    if (sort === 'updated') {
      if (left.kind === 'source' && right.kind !== 'source') return 1
      if (right.kind === 'source' && left.kind !== 'source') return -1
      const leftTime = left.updatedAtMs
      const rightTime = right.updatedAtMs
      if (leftTime !== rightTime) return rightTime - leftTime
    }

    if (sort === 'source') {
      const bySource = CATALOG_COLLATOR.compare(
        left.sourceName ?? left.sourceId ?? left.subtitle,
        right.sourceName ?? right.sourceId ?? right.subtitle,
      )
      if (bySource !== 0) return bySource
    }

    return CATALOG_COLLATOR.compare(left.name, right.name)
  })
}
