import { performServiceAction, isAbortError } from './service-action-client'
import { confirmGatewayParams, gatewayHeaders } from './gateway-request'
import { isStandaloneBearerAuthMode } from '@/lib/auth/auth-mode'
import { getMockGatewayFallback } from './mock-fallback'
import { RegistryApiError, normalizeServerJSON } from '@/lib/types/registry'
import type {
  LabRegistryMetadata,
  ListServersParams,
  RegistryLocalMetaDeleteResponse,
  RegistryLocalMetaResponse,
  RegistryMetaSetOptions,
  ServerListResponse,
  ServerResponse,
  ValidationResult,
  ServerJSON,
} from '@/lib/types/registry'

type RawServerResponse = Omit<ServerResponse, 'server'> & { server: ServerJSON }
type RestServerListRaw = { servers: RawServerResponse[]; next_cursor: string | null }
type RestServerVersionsRaw = { versions: RawServerResponse[] }

const USE_MOCK_DATA = process.env.NEXT_PUBLIC_MOCK_DATA === 'true'

const MOCK_REGISTRY_CONFIG: RegistryConfig = {
  url: 'https://registry.modelcontextprotocol.io',
}

const MOCK_REGISTRY_SERVERS: RawServerResponse[] = [
  {
    server: {
      name: 'io.labby.gateway-audit',
      title: 'Gateway Audit',
      description: 'Review gateway exposure, warnings, and operator drift from one MCP package.',
      version: '1.4.0',
      repository: { url: 'https://github.com/labby/gateway-audit' },
      remotes: [{ type: 'streamable-http', url: 'https://example.com/mcp' }],
      packages: [],
      icons: [],
      websiteUrl: 'https://github.com/labby/gateway-audit',
    },
    _meta: {
      'io.modelcontextprotocol.registry/official': {
        isLatest: true,
        publishedAt: '2026-04-20T12:00:00Z',
        status: 'active',
        statusChangedAt: '2026-04-20T12:00:00Z',
        updatedAt: '2026-04-21T10:00:00Z',
      },
      'tv.tootie.lab/registry': {
        curation: { featured: true, hidden: false, tags: ['audit', 'gateway'] },
        trust: { reviewed: true, source_verified: true, maintainer_known: true },
        ux: { recommended_for_homelab: true, works_in_lab: true, setup_difficulty: 'easy' },
      },
    },
  },
  {
    server: {
      name: 'io.labby.tail-helper',
      title: 'Tail Helper',
      description: 'Search presets and compact timeline helpers for log-heavy workflows.',
      version: '0.9.2',
      repository: { url: 'https://github.com/labby/tail-helper' },
      remotes: [{ type: 'stdio' }],
      packages: [],
      icons: [],
      websiteUrl: 'https://github.com/labby/tail-helper',
    },
    _meta: {
      'io.modelcontextprotocol.registry/official': {
        isLatest: true,
        publishedAt: '2026-04-18T12:00:00Z',
        status: 'active',
        statusChangedAt: '2026-04-18T12:00:00Z',
        updatedAt: '2026-04-18T12:00:00Z',
      },
      'tv.tootie.lab/registry': {
        curation: { featured: false, hidden: false, tags: ['logs'] },
        trust: { reviewed: true },
        ux: { recommended_for_homelab: false, works_in_lab: true, setup_difficulty: 'easy' },
      },
    },
  },
  {
    server: {
      name: 'io.community.registry-curator',
      title: 'Registry Curator',
      description: 'Attach local review metadata and curation notes to registry entries.',
      version: '0.5.0',
      repository: { url: 'https://github.com/modelcontextprotocol/servers' },
      remotes: [{ type: 'streamable-http', url: 'https://example.com/registry-curator' }],
      packages: [],
      icons: [],
      websiteUrl: 'https://github.com/modelcontextprotocol/servers',
    },
    _meta: {
      'io.modelcontextprotocol.registry/official': {
        isLatest: true,
        publishedAt: '2026-04-16T12:00:00Z',
        status: 'active',
        statusChangedAt: '2026-04-16T12:00:00Z',
        updatedAt: '2026-04-17T09:00:00Z',
      },
      'tv.tootie.lab/registry': {
        curation: { featured: false, hidden: false, tags: ['registry', 'metadata'] },
        trust: { reviewed: false },
        ux: { recommended_for_homelab: true, works_in_lab: true, setup_difficulty: 'medium' },
      },
    },
  },
]

const mockRegistryMetadata = new Map<string, LabRegistryMetadata | null>()

function cloneValue<T>(value: T): T {
  return structuredClone(value)
}

function normalizeResponse(raw: RawServerResponse): ServerResponse {
  return { ...raw, server: normalizeServerJSON(raw.server) }
}

function createRegistryError(message: string, status: number, code?: string): RegistryApiError {
  return new RegistryApiError(message, status, code)
}

function mergeMockMetadata(server: RawServerResponse): RawServerResponse {
  const metadata = mockRegistryMetadata.get(server.server.name)
  if (metadata === undefined) {
    return cloneValue(server)
  }

  return {
    ...cloneValue(server),
    _meta: {
      ...(server._meta ?? {}),
      'tv.tootie.lab/registry': metadata,
    },
  }
}

function listMockServers(): RawServerResponse[] {
  return MOCK_REGISTRY_SERVERS.map(mergeMockMetadata)
}

async function marketplaceMcpAction<T>(
  action: string,
  params: object,
  signal?: AbortSignal,
): Promise<T> {
  return performServiceAction<T, RegistryApiError>({
    action,
    params,
    signal,
    serviceLabel: 'McpRegistry',
    url: '/v1/marketplace',
    createError: createRegistryError,
  })
}

export interface RegistryConfig {
  url: string
}

async function registryRestGet<T>(url: string, fallbackMessage: string, signal?: AbortSignal): Promise<T> {
  const token = process.env.NEXT_PUBLIC_API_TOKEN
  const standaloneBearerAuth = isStandaloneBearerAuthMode(token)

  let response: Response
  try {
    response = await fetch(url, {
      headers: gatewayHeaders(token, standaloneBearerAuth),
      cache: 'no-store',
      credentials: standaloneBearerAuth ? 'omit' : 'include',
      signal,
    })
  } catch (error) {
    if (isAbortError(error)) throw error
    const msg = error instanceof Error ? error.message : 'unknown network error'
    throw createRegistryError(`${fallbackMessage}: ${msg}`, 502, 'backend_unreachable')
  }

  if (!response.ok) {
    const body = await (response.json() as Promise<{ message?: string; kind?: string }>).catch((): { message?: string; kind?: string } => ({}))
    throw createRegistryError(body.message ?? fallbackMessage, response.status, body.kind)
  }

  return response.json() as Promise<T>
}

function registryServerPath(name: string): string {
  return encodeURIComponent(name)
}

export async function getRegistryConfig(signal?: AbortSignal): Promise<RegistryConfig> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    return cloneValue(MOCK_REGISTRY_CONFIG)
  }

  return marketplaceMcpAction<RegistryConfig>('mcp.config', {}, signal)
}

export async function listServers(
  params: ListServersParams,
  signal?: AbortSignal,
): Promise<ServerListResponse> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()

    const filtered = listMockServers()
      .filter((response) => {
        if (!params.search) return true
        const haystack = `${response.server.name} ${response.server.title ?? ''} ${response.server.description}`.toLowerCase()
        return haystack.includes(params.search.toLowerCase())
      })
      .filter((response) => !params.version || response.server.version === params.version)
      .filter((response) => {
        if (!params.updated_since) return true
        const updatedAt = response._meta?.['io.modelcontextprotocol.registry/official']?.updatedAt
        return updatedAt ? updatedAt >= params.updated_since : true
      })
      .filter((response) => params.featured == null || Boolean(response._meta?.['tv.tootie.lab/registry']?.curation?.featured) === params.featured)
      .filter((response) => params.reviewed == null || Boolean(response._meta?.['tv.tootie.lab/registry']?.trust?.reviewed) === params.reviewed)
      .filter((response) => params.recommended == null || Boolean(response._meta?.['tv.tootie.lab/registry']?.ux?.recommended_for_homelab) === params.recommended)
      .filter((response) => params.hidden == null || Boolean(response._meta?.['tv.tootie.lab/registry']?.curation?.hidden) === params.hidden)
      .filter((response) => !params.tag || (response._meta?.['tv.tootie.lab/registry']?.curation?.tags ?? []).includes(params.tag))

    const limit = params.limit ?? 20
    const offset = params.cursor ? Number.parseInt(params.cursor, 10) || 0 : 0
    const page = filtered.slice(offset, offset + limit).map(normalizeResponse)
    const nextCursor = offset + limit < filtered.length ? String(offset + limit) : null

    return {
      servers: page,
      metadata: {
        count: page.length,
        nextCursor,
      },
    }
  }

  const qs = new URLSearchParams()
  if (params.search) qs.set('search', params.search)
  if (params.owner) qs.set('owner', params.owner)
  if (params.limit != null) qs.set('limit', String(params.limit))
  if (params.cursor) qs.set('cursor', params.cursor)
  if (params.version) qs.set('version', params.version)
  if (params.updated_since) qs.set('updated_since', params.updated_since)
  if (params.featured != null) qs.set('featured', String(params.featured))
  if (params.reviewed != null) qs.set('reviewed', String(params.reviewed))
  if (params.recommended != null) qs.set('recommended', String(params.recommended))
  if (params.hidden != null) qs.set('hidden', String(params.hidden))
  if (params.tag) qs.set('tag', params.tag)

  const qstr = qs.toString()
  const url = qstr ? `/v0.1/servers?${qstr}` : '/v0.1/servers'
  const raw = await registryRestGet<RestServerListRaw>(url, 'Failed to list servers', signal)
  const servers: ServerResponse[] = raw.servers.map(normalizeResponse)
  return { servers, metadata: { count: servers.length, nextCursor: raw.next_cursor } }
}

export async function getServer(
  name: string,
  signal?: AbortSignal,
): Promise<ServerResponse> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    const found = listMockServers().find((response) => response.server.name === name)
    if (!found) {
      throw createRegistryError('Failed to load server', 404, 'not_found')
    }
    return normalizeResponse(found)
  }

  const raw = await registryRestGet<RawServerResponse>(
    `/v0.1/servers/${registryServerPath(name)}/versions/latest`,
    'Failed to load server',
    signal,
  )
  return normalizeResponse(raw)
}

export async function listVersions(
  name: string,
  signal?: AbortSignal,
): Promise<ServerListResponse> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    const current = listMockServers().find((response) => response.server.name === name)
    if (!current) {
      throw createRegistryError('Failed to list server versions', 404, 'not_found')
    }
    return {
      servers: [normalizeResponse(current)],
      metadata: { count: 1, nextCursor: null },
    }
  }

  const raw = await registryRestGet<RestServerVersionsRaw>(
    `/v0.1/servers/${registryServerPath(name)}/versions`,
    'Failed to list server versions',
    signal,
  )
  const servers = raw.versions.map(normalizeResponse)
  return { servers, metadata: { count: servers.length, nextCursor: null } }
}

export async function validateServer(
  serverJson: ServerJSON,
  signal?: AbortSignal,
): Promise<ValidationResult> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    return { valid: true, issues: [] }
  }

  return marketplaceMcpAction<ValidationResult>('mcp.validate', { server_json: serverJson }, signal)
}

export async function getServerLocalMetadata(
  name: string,
  version?: string,
  signal?: AbortSignal,
): Promise<RegistryLocalMetaResponse> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    const server = listMockServers().find((response) => response.server.name === name)
    return {
      name,
      version: version ?? server?.server.version ?? 'latest',
      namespace: 'tv.tootie.lab/registry',
      metadata: server?._meta?.['tv.tootie.lab/registry'] ?? null,
    }
  }

  return marketplaceMcpAction<RegistryLocalMetaResponse>(
    'mcp.meta.get',
    { name, version },
    signal,
  )
}

export async function setServerLocalMetadata(
  name: string,
  metadata: LabRegistryMetadata,
  options?: RegistryMetaSetOptions,
  signal?: AbortSignal,
): Promise<RegistryLocalMetaResponse> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    const server = listMockServers().find((response) => response.server.name === name)
    mockRegistryMetadata.set(name, cloneValue(metadata))
    return {
      name,
      version: options?.version ?? server?.server.version ?? 'latest',
      namespace: 'tv.tootie.lab/registry',
      metadata: cloneValue(metadata),
    }
  }

  return marketplaceMcpAction<RegistryLocalMetaResponse>(
    'mcp.meta.set',
    { name, version: options?.version, updated_by: options?.updated_by, metadata },
    signal,
  )
}

export async function deleteServerLocalMetadata(
  name: string,
  version?: string,
  signal?: AbortSignal,
): Promise<RegistryLocalMetaDeleteResponse> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    const server = listMockServers().find((response) => response.server.name === name)
    mockRegistryMetadata.set(name, null)
    return {
      name,
      version: version ?? server?.server.version ?? 'latest',
      namespace: 'tv.tootie.lab/registry',
      deleted: true,
    }
  }

  return marketplaceMcpAction<RegistryLocalMetaDeleteResponse>(
    'mcp.meta.delete',
    { name, version },
    signal,
  )
}

export interface InstallServerParams {
  name: string
  gateway_name: string
  version: string
  bearer_token_env?: string
}

export interface InstallServerGatewayResult {
  gateway_id: string
  ok: boolean
  error?: string
  result?: unknown
}

export interface InstallServerResult {
  results: InstallServerGatewayResult[]
}

export async function installServer(
  params: InstallServerParams,
  signal?: AbortSignal,
): Promise<InstallServerResult> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    const gateway = getMockGatewayFallback('gw-2')
    if (!gateway) {
      throw createRegistryError('Failed to install server', 500, 'mock_gateway_missing')
    }
    return {
      results: [
        {
          gateway_id: params.gateway_name,
          ok: true,
          result: gateway,
        },
      ],
    }
  }

  return marketplaceMcpAction<InstallServerResult>(
    'mcp.install',
    confirmGatewayParams({
      name: params.name,
      gateway_ids: [params.gateway_name],
      version: params.version,
      bearer_token_env: params.bearer_token_env,
    }),
    signal,
  )
}
