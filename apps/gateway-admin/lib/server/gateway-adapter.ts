import type {
  CreateGatewayInput,
  ExposurePolicy,
  ExposurePolicyPreview,
  Gateway,
  GatewayImportSource,
  GatewayWarning,
  ServiceAction,
  TransportType,
  UpdateGatewayInput,
} from '../types/gateway'
import { EXPOSE_NONE_PATTERN, stripExposeNonePattern } from '../api/tool-exposure-draft.ts'
import { defaultGatewayBearerEnvName } from '../gateway-env.ts'

export interface BackendSurfaceStateView {
  enabled?: boolean
  connected?: boolean
}

export interface BackendSurfaceStatesView {
  cli?: BackendSurfaceStateView
  api?: BackendSurfaceStateView
  mcp?: BackendSurfaceStateView
  webui?: BackendSurfaceStateView
}

export interface BackendServerWarningView {
  code: string
  message: string
}

export interface BackendServerConfigSummaryView {
  transport?: string | null
  target?: string | null
}

export interface BackendServerView {
  id: string
  name: string
  source: string
  configured?: boolean
  enabled?: boolean
  connected?: boolean
  discovered_tool_count?: number
  exposed_tool_count?: number
  discovered_resource_count?: number
  exposed_resource_count?: number
  discovered_prompt_count?: number
  exposed_prompt_count?: number
  surfaces?: BackendSurfaceStatesView
  warnings?: BackendServerWarningView[]
  config_summary?: BackendServerConfigSummaryView
}

export interface BackendVirtualServiceDiscovery {
  tool_names?: string[]
  tools?: ServiceAction[]
  allowed_actions?: string[]
}

export interface BackendGatewayConfigView {
  name: string
  enabled?: boolean
  url?: string | null
  command?: string | null
  args?: string[]
  bearer_token_env?: string | null
  oauth_enabled?: boolean
  proxy_resources?: boolean
  proxy_prompts?: boolean
  expose_tools?: string[] | null
  expose_resources?: string[] | null
  expose_prompts?: string[] | null
  imported_from?: GatewayImportSource | null
}

export interface BackendGatewayRuntimeView {
  name: string
  tool_count: number
  resource_count: number
  prompt_count: number
  exposed_tool_count?: number
  exposed_resource_count?: number
  exposed_prompt_count?: number
  last_error?: string | null
}

export interface BackendGatewayMcpRuntimeView {
  name: string
  enabled?: boolean
  connected?: boolean
  discovered_tool_count?: number
  exposed_tool_count?: number
  discovered_resource_count?: number
  exposed_resource_count?: number
  discovered_prompt_count?: number
  exposed_prompt_count?: number
  likely_stale_count?: number
  pid?: number | null
  pgid?: number | null
  age_seconds?: number | null
  origin?: string | null
  owner?: {
    surface: string
    subject?: string | null
    request_id?: string | null
    session_id?: string | null
    client_name?: string | null
    raw?: string | null
  } | null
  transport?: string | null
  target?: string | null
  runtime_state_path?: string | null
  reconciled_at?: string | null
}

export interface BackendGatewayView {
  config: BackendGatewayConfigView
  runtime: BackendGatewayRuntimeView
}

export interface GatewayProbeStatus {
  connected: boolean
  healthy: boolean
  last_error?: string
}

export interface BackendGatewayToolRow {
  name: string
  description?: string | null
  exposed?: boolean
  matched_by?: string | null
}

export interface GatewayDiscoverySnapshot {
  tools: Array<string | BackendGatewayToolRow>
  resources: Array<string | { name?: string | null; uri: string; description?: string | null; exposed?: boolean | null }>
  prompts: Array<string | {
    name: string
    description?: string | null
    exposed?: boolean | null
    arguments?: Array<{ name: string; description?: string; required?: boolean }>
  }>
}

const NOW = () => new Date().toISOString()

const VALID_TRANSPORTS = ['http', 'stdio', 'in_process'] as const satisfies readonly TransportType[]
function isValidTransport(v: unknown): v is TransportType {
  return typeof v === 'string' && (VALID_TRANSPORTS as readonly string[]).includes(v)
}

function inferTransport(config: BackendGatewayConfigView): TransportType {
  return config.command ? 'stdio' : 'http'
}

function normalizeArgs(args?: string[]): string[] {
  return Array.isArray(args) ? args : []
}

function normalizeEnv(env?: Record<string, string>): Record<string, string> | undefined {
  if (!env) return undefined
  const entries = Object.entries(env)
    .map(([key, value]) => [key.trim(), value.trim()] as const)
    .filter(([key, value]) => key.length > 0 && value.length > 0)
  return entries.length > 0 ? Object.fromEntries(entries) : undefined
}

function matchPattern(toolName: string, pattern: string): boolean {
  if (pattern === '*') {
    return true
  }

  const parts = pattern.split('*')
  if (parts.length === 1) {
    return pattern === toolName
  }

  const anchoredStart = !pattern.startsWith('*')
  const anchoredEnd = !pattern.endsWith('*')
  const nonEmptyParts = parts.filter((part) => part.length > 0)

  if (nonEmptyParts.length === 0) {
    return true
  }

  let cursor = 0
  for (const [index, part] of nonEmptyParts.entries()) {
    if (index === 0 && anchoredStart) {
      if (!toolName.slice(cursor).startsWith(part)) {
        return false
      }
      cursor += part.length
      continue
    }

    const found = toolName.slice(cursor).indexOf(part)
    if (found === -1) {
      return false
    }
    cursor += found + part.length
  }

  if (anchoredEnd) {
    return toolName.endsWith(nonEmptyParts[nonEmptyParts.length - 1]!)
  }

  return true
}

function matchTool(toolName: string, patterns?: string[] | null): string | null {
  if (!patterns || patterns.length === 0) {
    return '*'
  }

  for (const pattern of patterns) {
    if (matchPattern(toolName, pattern)) {
      return pattern
    }
  }

  return null
}

function primitiveExposed(name: string, patterns: string[] | null | undefined, proxyEnabled: boolean): boolean {
  if (!proxyEnabled) {
    return false
  }

  if (!patterns || patterns.length === 0) {
    return true
  }

  return patterns.includes(name)
}

function matchVirtualServerAction(
  actionName: string,
  allowedActions: string[] | undefined,
  mcpEnabled: boolean,
): string | null {
  if (!mcpEnabled) {
    return null
  }

  if (!allowedActions || allowedActions.length === 0) {
    return '*'
  }

  return allowedActions.includes(actionName) ? actionName : null
}

function describeTarget(config: BackendGatewayConfigView): string {
  if (config.url) {
    return config.url
  }

  if (!config.command) {
    return config.name
  }

  if (config.command === 'env') {
    const actualCommand = (config.args ?? []).find((arg) => !arg.includes('='))
    return actualCommand ?? config.command
  }

  return config.command
}

function isMethodNotFound(lastError: string): boolean {
  return lastError.includes('Method not found') || lastError.includes('-32601')
}

function isNonEssentialCapabilityError(lastError: string): boolean {
  if (lastError.includes('does not implement MCP prompts discovery')) {
    return true
  }

  if (lastError.includes('does not implement MCP resource discovery')) {
    return true
  }

  if (lastError.includes('failed to list prompts from upstream:') && isMethodNotFound(lastError)) {
    return true
  }

  if (lastError.includes('failed to list resources from upstream:') && isMethodNotFound(lastError)) {
    return true
  }

  return false
}

function classifyWarning(lastError?: string) {
  if (!lastError) {
    return { code: 'connection_error', message: undefined }
  }

  if (lastError.includes('Authentication is required')) {
    return { code: 'auth_required', message: lastError }
  }

  if (lastError.toLowerCase().includes('timed out')) {
    return { code: 'timeout', message: lastError }
  }

  return { code: 'connection_error', message: lastError }
}

function extractReqwestUrl(error: string): string | undefined {
  return error.match(/url \(([^)]+)\)/)?.[1]
}

function humanizeLabServiceHealthError(message: string, serviceName: string): string | undefined {
  if (isNonEssentialCapabilityError(message)) {
    return undefined
  }

  if (message.includes('Auth required') || message.includes('Authentication')) {
    return `${serviceName} rejected the health check request. Verify the configured API key or token.`
  }

  const url = extractReqwestUrl(message)
  if (url) {
    return `Could not reach ${serviceName} at ${url}. Verify the service is running and the URL is correct.`
  }

  if (message.toLowerCase().includes('timed out')) {
    return `${serviceName} did not respond to the health check in time. The service may be overloaded or unreachable.`
  }

  return message
}

export function humanizeProbeError(lastError: string | undefined, config: BackendGatewayConfigView): string | undefined {
  if (!lastError) {
    return undefined
  }

  if (isNonEssentialCapabilityError(lastError)) {
    return undefined
  }

  const target = describeTarget(config)

  if (lastError.includes('Auth required')) {
    return `Authentication is required by ${target}. Configure \`bearer_token_env\` with a valid upstream token, then reload this gateway.`
  }

  const url = extractReqwestUrl(lastError)
  if (url) {
    return `Could not connect to ${url}. The upstream did not complete the MCP initialize request. Verify the server is running, reachable, and speaking MCP.`
  }

  if (lastError.includes('No such file or directory')) {
    return `The stdio command for ${config.name} could not start because a referenced file or path does not exist.`
  }

  if (lastError.includes('timed out')) {
    return `The gateway timed out while waiting for ${target} to respond during MCP initialization.`
  }

  return lastError
}

function buildWarnings(probe: GatewayProbeStatus): GatewayWarning[] {
  if (!probe.last_error || isNonEssentialCapabilityError(probe.last_error)) {
    return []
  }

  const warning = classifyWarning(probe.last_error)

  return [
    {
      code: warning.code,
      message: warning.message ?? probe.last_error,
      timestamp: NOW(),
    },
  ]
}

export function normalizeServerView(
  view: BackendServerView,
  discovery?: BackendVirtualServiceDiscovery,
  runtime?: BackendGatewayMcpRuntimeView,
): Gateway {
  const rawTransport = runtime?.transport ?? view.config_summary?.transport
  const transport: TransportType = isValidTransport(rawTransport) ? rawTransport : 'http'
  const target = runtime?.target ?? view.config_summary?.target ?? undefined
  const config: BackendGatewayConfigView = {
    name: view.name,
    ...(transport === 'http' ? { url: target } : {}),
    ...(transport === 'stdio' ? { command: target } : {}),
    proxy_resources: false,
    proxy_prompts: false,
  }
  const isLabService = view.source === 'in_process'
  const warnings = (view.warnings ?? []).map((warning) => {
    if (isNonEssentialCapabilityError(warning.message)) {
      return null
    }

    const message = isLabService
      ? (humanizeLabServiceHealthError(warning.message, view.name) ?? warning.message)
      : (humanizeProbeError(warning.message, config) ?? warning.message)
    const classified = classifyWarning(message)

    return {
      code: warning.code ?? classified.code,
      message,
      timestamp: NOW(),
    }
  }).filter((warning): warning is GatewayWarning => warning !== null)
  const lastError = warnings[0]?.message
  const tools = discovery?.tools
    ?? (discovery?.tool_names ?? []).map((name) => ({
      name,
      description: '',
      destructive: false,
    }))
  const defaultMcpEnabled = view.source === 'in_process' ? (view.enabled ?? true) : false
  const mcpEnabled = (view.enabled ?? true) && (view.surfaces?.mcp?.enabled ?? defaultMcpEnabled)

  return {
    id: view.id,
    name: view.name,
    source: view.source,
    configured: view.configured ?? true,
    enabled: view.enabled ?? true,
    surfaces: {
      cli: {
        enabled: view.surfaces?.cli?.enabled ?? false,
        connected: view.surfaces?.cli?.connected ?? false,
      },
      api: {
        enabled: view.surfaces?.api?.enabled ?? false,
        connected: view.surfaces?.api?.connected ?? false,
      },
      mcp: {
        enabled: view.surfaces?.mcp?.enabled ?? false,
        connected: view.surfaces?.mcp?.connected ?? false,
      },
      webui: {
        enabled: view.surfaces?.webui?.enabled ?? false,
        connected: view.surfaces?.webui?.connected ?? false,
      },
    },
    transport,
    config: {
      ...((transport === 'http' && target) ? { url: target } : {}),
      ...((transport === 'stdio' && target) ? { command: target } : {}),
      proxy_resources: config.proxy_resources,
      proxy_prompts: config.proxy_prompts,
    },
    status: {
      healthy: (view.connected ?? false) && warnings.length === 0,
      connected: view.connected ?? false,
      ...(lastError ? { last_error: lastError } : {}),
      discovered_tool_count: view.discovered_tool_count ?? tools.length,
      exposed_tool_count: view.exposed_tool_count ?? tools.length,
      discovered_resource_count: view.discovered_resource_count ?? 0,
      exposed_resource_count: view.exposed_resource_count ?? 0,
      discovered_prompt_count: view.discovered_prompt_count ?? 0,
      exposed_prompt_count: view.exposed_prompt_count ?? 0,
      likely_stale_count: runtime?.likely_stale_count,
      pid: runtime?.pid ?? undefined,
      pgid: runtime?.pgid ?? undefined,
      age_seconds: runtime?.age_seconds ?? undefined,
      origin: runtime?.origin ?? undefined,
      owner: runtime?.owner
        ? {
            surface: runtime.owner.surface,
            subject: runtime.owner.subject ?? undefined,
            request_id: runtime.owner.request_id ?? undefined,
            session_id: runtime.owner.session_id ?? undefined,
            client_name: runtime.owner.client_name ?? undefined,
            raw: runtime.owner.raw ?? undefined,
          }
        : undefined,
      runtime_state_path: runtime?.runtime_state_path ?? undefined,
      reconciled_at: runtime?.reconciled_at ?? undefined,
    },
    discovery: {
      tools: tools.map((tool) => {
        const matchedBy = view.source === 'in_process'
          ? matchVirtualServerAction(tool.name, discovery?.allowed_actions, mcpEnabled)
          : '*'

        return {
          name: tool.name,
          description: tool.description || undefined,
          exposed: matchedBy !== null,
          matched_by: matchedBy,
        }
      }),
      resources: [],
      prompts: [],
    },
    warnings,
    created_at: NOW(),
    updated_at: NOW(),
  }
}

export function normalizeGateway(
  view: BackendGatewayView,
  probe: GatewayProbeStatus,
  discovery: GatewayDiscoverySnapshot,
  runtime?: BackendGatewayMcpRuntimeView,
): Gateway {
  const config = view.config
  const humanizedError = humanizeProbeError(probe.last_error, config)
  const exposePatterns = config.expose_tools
  const tools = discovery.tools.map((tool) => {
    const name = typeof tool === 'string' ? tool : tool.name
    const matchedBy = matchTool(name, exposePatterns)
    return {
      name,
      description: typeof tool === 'string' ? undefined : tool.description ?? undefined,
      exposed: matchedBy !== null,
      matched_by: matchedBy,
    }
  })

  return {
    id: config.name,
    name: config.name,
    transport: inferTransport(config),
    source: 'custom_gateway',
    configured: true,
    enabled: config.enabled ?? true,
    surfaces: {
      cli: { enabled: false, connected: false },
      api: { enabled: false, connected: false },
      mcp: { enabled: config.enabled ?? true, connected: (config.enabled ?? true) && probe.connected },
      webui: { enabled: false, connected: false },
    },
    config: {
      url: config.url ?? undefined,
      command: config.command ?? undefined,
      args: normalizeArgs(config.args),
      bearer_token_env: config.bearer_token_env ?? undefined,
      oauth_enabled: config.oauth_enabled ?? false,
      proxy_resources: config.proxy_resources ?? false,
      proxy_prompts: config.proxy_prompts ?? true,
      expose_tools: exposePatterns ?? undefined,
      expose_resources: config.expose_resources ?? undefined,
      expose_prompts: config.expose_prompts ?? undefined,
      imported_from: config.imported_from ?? undefined,
    },
    status: {
      healthy: (config.enabled ?? true) && probe.healthy,
      connected: (config.enabled ?? true) && probe.connected,
      ...(humanizedError ? { last_error: humanizedError } : {}),
      discovered_tool_count: view.runtime.tool_count,
      exposed_tool_count: view.runtime.exposed_tool_count ?? tools.filter((tool) => tool.exposed).length,
      discovered_resource_count: view.runtime.resource_count,
      exposed_resource_count: view.runtime.exposed_resource_count ?? view.runtime.resource_count,
      discovered_prompt_count: view.runtime.prompt_count,
      exposed_prompt_count: view.runtime.exposed_prompt_count ?? view.runtime.prompt_count,
      likely_stale_count: runtime?.likely_stale_count,
      pid: runtime?.pid ?? undefined,
      pgid: runtime?.pgid ?? undefined,
      age_seconds: runtime?.age_seconds ?? undefined,
      origin: runtime?.origin ?? undefined,
      owner: runtime?.owner
        ? {
            surface: runtime.owner.surface,
            subject: runtime.owner.subject ?? undefined,
            request_id: runtime.owner.request_id ?? undefined,
            session_id: runtime.owner.session_id ?? undefined,
            client_name: runtime.owner.client_name ?? undefined,
            raw: runtime.owner.raw ?? undefined,
          }
        : undefined,
      runtime_state_path: runtime?.runtime_state_path ?? undefined,
      reconciled_at: runtime?.reconciled_at ?? undefined,
    },
    discovery: {
      tools,
      resources: discovery.resources.map((resource) => {
        const uri = typeof resource === 'string' ? resource : resource.uri
        const name = typeof resource === 'string' ? resource : resource.name ?? resource.uri
        return {
          name,
          uri,
          ...(typeof resource !== 'string' && resource.description ? { description: resource.description } : {}),
          exposed: typeof resource === 'string'
            ? primitiveExposed(name, config.expose_resources, config.proxy_resources ?? false)
            : resource.exposed ?? primitiveExposed(name, config.expose_resources, config.proxy_resources ?? false),
        }
      }),
      prompts: discovery.prompts.map((prompt) => {
        const name = typeof prompt === 'string' ? prompt : prompt.name
        return {
          name,
          ...(typeof prompt !== 'string' && prompt.description ? { description: prompt.description } : {}),
          exposed: typeof prompt === 'string'
            ? primitiveExposed(name, config.expose_prompts, config.proxy_prompts ?? true)
            : prompt.exposed ?? primitiveExposed(name, config.expose_prompts, config.proxy_prompts ?? true),
          ...(typeof prompt !== 'string' && prompt.arguments ? { arguments: prompt.arguments } : {}),
        }
      }),
    },
    warnings: buildWarnings({
      ...probe,
      last_error: humanizedError,
    }),
    created_at: NOW(),
    updated_at: NOW(),
  }
}

export function probeStatusFromRuntime(runtime: BackendGatewayRuntimeView): GatewayProbeStatus {
  const connectedCount = runtime.tool_count + runtime.resource_count + runtime.prompt_count
  const rawLastError = runtime.last_error?.trim() || undefined
  const lastError = rawLastError && !isNonEssentialCapabilityError(rawLastError)
    ? rawLastError
    : undefined

  if (connectedCount > 0) {
    return {
      connected: true,
      healthy: !lastError,
      ...(lastError ? { last_error: lastError } : {}),
    }
  }

  return {
    connected: false,
    healthy: false,
    last_error: lastError ?? 'No capabilities (tools, resources, or prompts) were discovered from this gateway.',
  }
}

export function gatewayInputToSpec(input: CreateGatewayInput) {
  const env = input.transport === 'stdio' ? normalizeEnv(input.config.env) : undefined
  const spec: Record<string, unknown> = {
    name: input.name,
    url: input.transport === 'http' ? input.config.url ?? null : null,
    command: input.transport === 'stdio' ? input.config.command ?? null : null,
    args: input.transport === 'stdio' ? normalizeArgs(input.config.args) : [],
    ...(env ? { env } : {}),
    bearer_token_env: input.config.bearer_token_env ?? null,
    proxy_resources: input.config.proxy_resources ?? false,
    proxy_prompts: input.config.proxy_prompts ?? true,
    expose_tools: input.config.expose_tools ?? null,
    expose_resources: input.config.expose_resources ?? null,
    expose_prompts: input.config.expose_prompts ?? null,
  }
  if (input.config.oauth) {
    spec.oauth = {
      mode: 'authorization_code_pkce',
      registration: { strategy: input.config.oauth.registration_strategy },
      scopes: input.config.oauth.scopes ?? null,
    }
  }
  return spec
}

export function buildGatewayCreatePayload(input: CreateGatewayInput) {
  const spec = gatewayInputToSpec({
    ...input,
    config: {
      ...input.config,
      bearer_token_env:
        input.config.bearer_token_env?.trim() ||
        (input.config.bearer_token_value?.trim()
          ? defaultGatewayBearerEnvName(input.name)
          : undefined),
    },
  })

  const payload: Record<string, unknown> = { spec }
  const bearerTokenValue = input.config.bearer_token_value?.trim()
  if (bearerTokenValue) {
    payload.bearer_token_value = bearerTokenValue
  }
  return payload
}

export function buildGatewayPatch(input: UpdateGatewayInput & { name?: string; transport?: TransportType }) {
  const config = input.config ?? {}
  const patch: Record<string, unknown> = {}

  if (input.name !== undefined) {
    patch.name = input.name
  }

  if (input.transport === 'http') {
    patch.url = config.url ?? null
    patch.command = null
    patch.args = []
  } else if (input.transport === 'stdio') {
    patch.url = null
    patch.command = config.command ?? null
    patch.args = normalizeArgs(config.args)
    if (config.env !== undefined) patch.env = normalizeEnv(config.env) ?? {}
  } else {
    if (config.url !== undefined) patch.url = config.url
    if (config.command !== undefined) patch.command = config.command
    if (config.args !== undefined) patch.args = normalizeArgs(config.args)
    if (config.env !== undefined) patch.env = normalizeEnv(config.env) ?? {}
  }

  if (config.bearer_token_env !== undefined) {
    patch.bearer_token_env = config.bearer_token_env?.trim() || null
  }

  if (config.proxy_resources !== undefined) {
    patch.proxy_resources = config.proxy_resources
  }

  if (config.proxy_prompts !== undefined) {
    patch.proxy_prompts = config.proxy_prompts
  }

  if (config.expose_tools !== undefined) {
    patch.expose_tools = config.expose_tools
  }

  if (config.expose_resources !== undefined) {
    patch.expose_resources = config.expose_resources
  }

  if (config.expose_prompts !== undefined) {
    patch.expose_prompts = config.expose_prompts
  }

  if (config.oauth !== undefined) {
    patch.oauth = {
      mode: 'authorization_code_pkce',
      registration: { strategy: config.oauth.registration_strategy },
      scopes: config.oauth.scopes ?? null,
    }
  }

  return patch
}

export function buildGatewayUpdatePayload(
  id: string,
  input: UpdateGatewayInput,
) {
  const patch = buildGatewayPatch({
    ...input,
    config: {
      ...input.config,
      bearer_token_env:
        input.config?.bearer_token_env !== undefined
          ? input.config.bearer_token_env?.trim()
          : input.config?.bearer_token_value?.trim()
            ? defaultGatewayBearerEnvName(input.name ?? id)
            : input.config?.bearer_token_env,
    },
  })

  const payload: Record<string, unknown> = {
    name: id,
    patch,
  }
  const bearerTokenValue = input.config?.bearer_token_value?.trim()
  if (bearerTokenValue) {
    payload.bearer_token_value = bearerTokenValue
  }
  return payload
}

export function exposurePolicyFromConfig(config: BackendGatewayConfigView): ExposurePolicy {
  const rawPatterns = config.expose_tools ?? []
  const patterns = stripExposeNonePattern(rawPatterns)
  if (rawPatterns.includes(EXPOSE_NONE_PATTERN)) {
    return { mode: 'allowlist', patterns: [] }
  }
  if (patterns.length === 0) {
    return { mode: 'expose_all', patterns: [] }
  }

  return { mode: 'allowlist', patterns }
}

export function previewExposurePolicy(
  toolNames: string[],
  patterns: string[],
): ExposurePolicyPreview {
  if (patterns.length === 0) {
    return {
      matched_tools: toolNames.map((name) => ({ name, matched_by: '*' })),
      unmatched_patterns: [],
      filtered_tools: [],
      exposed_count: toolNames.length,
      filtered_count: 0,
    }
  }

  const matched_tools: ExposurePolicyPreview['matched_tools'] = []
  const filtered_tools: string[] = []
  const usedPatterns = new Set<string>()

  for (const toolName of toolNames) {
    let matchedBy: string | null = null
    for (const pattern of patterns) {
      if (matchPattern(toolName, pattern)) {
        matchedBy = pattern
        usedPatterns.add(pattern)
        break
      }
    }

    if (matchedBy) {
      matched_tools.push({ name: toolName, matched_by: matchedBy })
    } else {
      filtered_tools.push(toolName)
    }
  }

  return {
    matched_tools,
    unmatched_patterns: patterns.filter((pattern) => !usedPatterns.has(pattern)),
    filtered_tools,
    exposed_count: matched_tools.length,
    filtered_count: filtered_tools.length,
  }
}
