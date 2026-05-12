// Gateway data model types

export type TransportType = 'http' | 'stdio' | 'in_process'

export interface GatewayConfig {
  url?: string
  command?: string
  args?: string[]
  bearer_token_env?: string
  oauth_enabled?: boolean
  proxy_resources?: boolean
  proxy_prompts?: boolean
  expose_tools?: string[]
  expose_resources?: string[] | null
  expose_prompts?: string[] | null
}

/** Extended config for create/update payloads only. `bearer_token_value` is write-only and never returned by the API. */
export interface GatewayWriteConfig extends GatewayConfig {
  bearer_token_value?: string
  /** OAuth spec — write-only, never returned by the API. Set when auth mode is 'oauth'. */
  oauth?: { registration_strategy: string; scopes?: string[] }
}

export interface GatewayStatus {
  healthy: boolean
  connected: boolean
  last_error?: string
  discovered_tool_count: number
  exposed_tool_count: number
  discovered_resource_count: number
  exposed_resource_count: number
  discovered_prompt_count: number
  exposed_prompt_count: number
  likely_stale_count?: number
  pid?: number
  pgid?: number
  age_seconds?: number
  origin?: string
  owner?: {
    surface: string
    subject?: string
    request_id?: string
    session_id?: string
    client_name?: string
    raw?: string
  }
  runtime_state_path?: string
  reconciled_at?: string
}

export interface SurfaceState {
  enabled: boolean
  connected: boolean
}

export interface SurfaceStates {
  cli: SurfaceState
  api: SurfaceState
  mcp: SurfaceState
  webui: SurfaceState
}

export interface DiscoveredTool {
  name: string
  description?: string
  exposed: boolean
  matched_by: string | null
}

export interface DiscoveredResource {
  name: string
  uri: string
  description?: string
  exposed?: boolean
}

export interface DiscoveredPrompt {
  name: string
  description?: string
  exposed?: boolean
  arguments?: Array<{
    name: string
    description?: string
    required?: boolean
  }>
}

export interface GatewayDiscovery {
  tools: DiscoveredTool[]
  resources: DiscoveredResource[]
  prompts: DiscoveredPrompt[]
}

export interface GatewayWarning {
  code: string
  message: string
  timestamp: string
}

export interface Gateway {
  id: string
  name: string
  transport: TransportType
  source?: string
  configured?: boolean
  enabled?: boolean
  surfaces?: SurfaceStates
  config: GatewayConfig
  status: GatewayStatus
  discovery: GatewayDiscovery
  warnings: GatewayWarning[]
  created_at: string
  updated_at: string
}

export interface CreateGatewayInput {
  name: string
  transport: TransportType
  config: GatewayWriteConfig
}

export interface UpdateGatewayInput {
  name?: string
  transport?: TransportType
  config?: Partial<GatewayWriteConfig>
}

export interface TestGatewayResult {
  success: boolean
  severity?: 'success' | 'warning' | 'failure'
  message: string
  latency_ms?: number
  discovered_tools?: number
  discovered_resources?: number
  discovered_prompts?: number
  error?: string
  detail?: string
}

export interface ReloadGatewayResult {
  success: boolean
  message: string
  previous_tool_count: number
  new_tool_count: number
}

export interface GatewayCleanupResult {
  upstream: string
  aggressive: boolean
  dry_run?: boolean
  gateway_matched?: number
  local_matched?: number
  aggressive_matched?: number
  gateway_killed: number
  local_killed: number
  aggressive_killed: number
  gateway_matches?: Array<{ pattern: string; pids: number[] }>
  local_matches?: Array<{ pattern: string; pids: number[] }>
  aggressive_matches?: Array<{ pattern: string; pids: number[] }>
}

export interface SupportedServiceField {
  name: string
  description: string
  example: string
  secret: boolean
}

export interface SupportedService {
  key: string
  display_name: string
  category: string
  description: string
  required_env: SupportedServiceField[]
  optional_env: SupportedServiceField[]
  default_port?: number | null
}

export interface ServiceConfigField {
  name: string
  present: boolean
  secret: boolean
  value_preview?: string | null
}

export interface ServiceConfig {
  service: string
  configured: boolean
  fields: ServiceConfigField[]
}

export interface ServiceAction {
  name: string
  description: string
  destructive: boolean
}

export interface ToolSearchConfig {
  enabled: boolean
  top_k_default: number
  max_tools: number
}

export interface ToolSearchConfigInput {
  enabled: boolean
  top_k_default?: number
  max_tools?: number
}

export interface ProtectedMcpRoute {
  name: string
  enabled: boolean
  public_host: string
  public_path: string
  upstream?: string | null
  backend_url?: string
  backend_mcp_path?: string
  scopes: string[]
  health_path?: string | null
}

export type ProtectedMcpRouteInput = ProtectedMcpRoute

export interface ProtectedMcpRouteTestResult {
  ok: boolean
  route: ProtectedMcpRoute
  resource: string
  metadata_url: string
}

// Exposure policy types
export interface ExposurePolicy {
  mode: 'expose_all' | 'allowlist'
  patterns: string[]
}

export interface ExposurePolicyPreview {
  matched_tools: Array<{
    name: string
    matched_by: string
  }>
  unmatched_patterns: string[]
  filtered_tools: string[]
  exposed_count: number
  filtered_count: number
}
