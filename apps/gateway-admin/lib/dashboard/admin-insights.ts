import type { Gateway } from '@/lib/types/gateway'
import type { LogEvent, LogSubsystem } from '@/lib/types/logs'

export interface ActivityItem {
  id: string
  kind:
    | 'artifact'
    | 'device'
    | 'gateway'
    | 'marketplace'
    | 'oauth'
    | 'prompt'
    | 'resource'
    | 'session'
    | 'settings'
    | 'tool'
    | 'other'
  tone: 'success' | 'warning' | 'danger' | 'info'
  title: string
  detail: string
  timestamp: string
  event: LogEvent
}

/** Log events surfaced on the activity page, by subsystem. */
export const ACTIVITY_SUBSYSTEMS = [
  'gateway',
  'mcp_server',
  'mcp_client',
  'api',
  'web',
  'oauth_relay',
  'auth_webui',
  'auth_mcp',
  'auth_upstream',
  'core_runtime',
  'syslog',
] as const satisfies readonly LogSubsystem[]

interface ActionDescriptor {
  kind: ActivityItem['kind']
  label: string
}

const ACTION_LABELS: Record<string, ActionDescriptor> = {
  // Gateway + service management
  'gateway.add': { kind: 'gateway', label: 'Added gateway' },
  'gateway.remove': { kind: 'gateway', label: 'Removed gateway' },
  'gateway.update': { kind: 'gateway', label: 'Updated gateway' },
  'gateway.reload': { kind: 'gateway', label: 'Reloaded gateway' },
  'gateway.test': { kind: 'gateway', label: 'Tested gateway' },
  'gateway.mcp.enable': { kind: 'gateway', label: 'Enabled gateway' },
  'gateway.mcp.disable': { kind: 'gateway', label: 'Disabled gateway' },
  'gateway.virtual_server.enable': { kind: 'gateway', label: 'Enabled service' },
  'gateway.virtual_server.disable': { kind: 'gateway', label: 'Disabled service' },
  'gateway.virtual_server.set_surface': { kind: 'gateway', label: 'Changed service exposure' },
  'gateway.virtual_server.set_mcp_policy': { kind: 'gateway', label: 'Changed MCP exposure' },
  'gateway.service_config.set': { kind: 'settings', label: 'Changed service settings' },
  'gateway.server.install': { kind: 'gateway', label: 'Added registry server' },
  // MCP session lifecycle
  'session.initialized': { kind: 'session', label: 'MCP session connected' },
  // MCP tool / resource / prompt dispatch
  list_tools: { kind: 'tool', label: 'Listed tools' },
  call_tool: { kind: 'tool', label: 'Called tool' },
  list_resources: { kind: 'resource', label: 'Listed resources' },
  read_resource: { kind: 'resource', label: 'Read resource' },
  list_prompts: { kind: 'prompt', label: 'Listed prompts' },
  get_prompt: { kind: 'prompt', label: 'Fetched prompt' },
  // OAuth relay / upstream
  'oauth.relay.start': { kind: 'oauth', label: 'OAuth relay started' },
  probe: { kind: 'oauth', label: 'Probed upstream OAuth' },
  start: { kind: 'oauth', label: 'Started OAuth flow' },
  status: { kind: 'oauth', label: 'Checked OAuth status' },
  callback: { kind: 'oauth', label: 'OAuth callback' },
  clear: { kind: 'oauth', label: 'Cleared OAuth session' },
  // Chat / artifacts / marketplace / devices
  'session.created': { kind: 'session', label: 'Started chat session' },
  'artifact.created': { kind: 'artifact', label: 'Created artifact' },
  'artifact.removed': { kind: 'artifact', label: 'Removed artifact' },
  'artifact.edited': { kind: 'artifact', label: 'Edited artifact' },
  'artifact.forked': { kind: 'artifact', label: 'Forked artifact' },
  'artifact.patched': { kind: 'artifact', label: 'Patched artifact' },
  'marketplace.add': { kind: 'marketplace', label: 'Added marketplace' },
  'marketplace.remove': { kind: 'marketplace', label: 'Removed marketplace' },
  'marketplace.plugin.install': { kind: 'marketplace', label: 'Installed plugin' },
  'marketplace.plugin.remove': { kind: 'marketplace', label: 'Removed plugin' },
  'deploy.artifact': { kind: 'artifact', label: 'Deployed artifact' },
  'node.status': { kind: 'device', label: 'Device status changed' },
}

function describeAction(event: LogEvent): ActionDescriptor {
  if (event.action && ACTION_LABELS[event.action]) {
    return ACTION_LABELS[event.action]
  }
  if (event.action?.startsWith('gateway.')) {
    return { kind: 'gateway', label: humanizeAction(event.action) }
  }
  if (event.action?.startsWith('marketplace.')) {
    return { kind: 'marketplace', label: humanizeAction(event.action) }
  }
  if (event.action?.startsWith('artifact.')) {
    return { kind: 'artifact', label: humanizeAction(event.action) }
  }
  if (event.action?.startsWith('deploy.')) {
    return { kind: 'artifact', label: humanizeAction(event.action) }
  }
  if (event.subsystem === 'oauth_relay' || event.subsystem.startsWith('auth_')) {
    return { kind: 'oauth', label: event.action ?? 'OAuth event' }
  }
  if (event.subsystem === 'mcp_server') {
    return { kind: 'session', label: event.action ?? 'MCP event' }
  }
  if (event.subsystem === 'gateway') {
    return { kind: 'gateway', label: event.action ? humanizeAction(event.action) : 'Gateway activity' }
  }
  if (event.subsystem === 'api' || event.subsystem === 'web') {
    return { kind: 'settings', label: event.action ? humanizeAction(event.action) : 'App activity' }
  }
  if (event.subsystem === 'syslog' || event.subsystem === 'core_runtime') {
    return { kind: 'device', label: event.action ? humanizeAction(event.action) : 'Device activity' }
  }
  return { kind: 'other', label: event.action ?? event.subsystem }
}

function humanizeAction(action: string): string {
  return action
    .split('.')
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1).replaceAll('_', ' '))
    .join(' ')
}

function toneFor(event: LogEvent): ActivityItem['tone'] {
  if (event.level === 'error') return 'danger'
  if (event.level === 'warn') return 'warning'
  if (event.outcome_kind && event.outcome_kind !== 'ok') return 'warning'
  return 'success'
}

function formatDetail(event: LogEvent, descriptor: ActionDescriptor): string {
  const parts: string[] = []
  const fields = event.fields_json as Record<string, unknown> | undefined

  if (descriptor.kind === 'tool' && event.action === 'call_tool') {
    const name = fields?.tool_name ?? fields?.name ?? fields?.service
    if (typeof name === 'string' && name.length > 0) parts.push(name)
  }
  if (descriptor.kind === 'resource' && event.action === 'read_resource') {
    const uri = fields?.uri ?? fields?.resource_uri
    if (typeof uri === 'string' && uri.length > 0) parts.push(uri)
  }
  if (descriptor.kind === 'prompt' && event.action === 'get_prompt') {
    const name = fields?.prompt_name ?? fields?.name
    if (typeof name === 'string' && name.length > 0) parts.push(name)
  }
  for (const key of ['name', 'id', 'server', 'service', 'plugin', 'marketplace', 'artifact', 'device']) {
    const value = fields?.[key]
    if (typeof value === 'string' && value.length > 0 && !parts.includes(value)) {
      parts.push(value)
      break
    }
  }

  parts.push(event.message)
  if (event.instance) parts.push(`instance=${event.instance}`)
  if (event.source_node_id) {
    parts.push(`node=${event.source_node_id}`)
  }
  return parts.filter(Boolean).join(' · ')
}

export function buildActivityItemsFromLogs(events: LogEvent[]): ActivityItem[] {
  return events.map((event) => {
    const descriptor = describeAction(event)
    const parsed = new Date(event.ts)
    return {
      id: event.event_id,
      kind: descriptor.kind,
      tone: toneFor(event),
      title: descriptor.label,
      detail: formatDetail(event, descriptor),
      timestamp: Number.isNaN(parsed.getTime()) ? new Date(0).toISOString() : parsed.toISOString(),
      event,
    }
  })
}

export interface GatewayActivityItem {
  id: string
  gatewayId: string
  gatewayName: string
  kind: 'status' | 'warning'
  tone: 'success' | 'warning' | 'danger'
  title: string
  detail: string
  timestamp: string
}

export interface GatewaySettingsSnapshot {
  authModeLabel: 'Browser session' | 'API token'
  runtimeLabel: 'Live control plane' | 'Mock preview'
  totalGateways: number
  connectedGateways: number
  disconnectedGateways: number
  warningCount: number
  proxyResourceGateways: number
  bearerTokenGateways: number
}

export interface GatewayDocsSnapshot {
  totalGateways: number
  connectedGateways: number
  warningCount: number
  httpGateways: number
  stdioGateways: number
  supportedServices: number
  exposedTools: number
}

interface SettingsOptions {
  hasStandaloneBearerAuth: boolean
  hasMockData: boolean
}

export function buildGatewayActivityFeed(gateways: Gateway[]): GatewayActivityItem[] {
  const parseTimestamp = (value: string) => {
    const parsed = Date.parse(value)
    return Number.isNaN(parsed) ? 0 : parsed
  }

  return gateways
    .flatMap((gateway) => {
      const statusItem: GatewayActivityItem = gateway.status.connected && gateway.status.healthy
        ? {
            id: `${gateway.id}-status`,
            gatewayId: gateway.id,
            gatewayName: gateway.name,
            kind: 'status',
            tone: 'success',
            title: `${gateway.name} is healthy`,
            detail: `Probe completed successfully with ${gateway.status.discovered_tool_count} discovered tools over ${gateway.transport.toUpperCase()}.`,
            timestamp: gateway.updated_at ?? '',
          }
        : {
            id: `${gateway.id}-status`,
            gatewayId: gateway.id,
            gatewayName: gateway.name,
            kind: 'status',
            tone: 'danger',
            title: `${gateway.name} needs attention`,
            detail: gateway.status.last_error || 'Gateway is disconnected or not yet configured.',
            timestamp: gateway.updated_at ?? '',
          }

      const warningItems = gateway.warnings.map<GatewayActivityItem>((warning, index) => ({
        id: `${gateway.id}-warning-${index}`,
        gatewayId: gateway.id,
        gatewayName: gateway.name,
        kind: 'warning',
        tone: 'warning',
        title: `${warning.code} on ${gateway.name}`,
        detail: warning.message,
        timestamp: warning.timestamp,
      }))

      return [statusItem, ...warningItems]
    })
    .sort((left, right) => {
      const timestampDelta = parseTimestamp(right.timestamp) - parseTimestamp(left.timestamp)
      if (timestampDelta !== 0) {
        return timestampDelta
      }

      if (left.kind === right.kind) {
        return left.title.localeCompare(right.title)
      }

      return left.kind === 'status' ? -1 : 1
    })
}

export function buildGatewaySettingsSnapshot(
  gateways: Gateway[],
  options: SettingsOptions,
): GatewaySettingsSnapshot {
  return {
    authModeLabel: options.hasStandaloneBearerAuth ? 'API token' : 'Browser session',
    runtimeLabel: options.hasMockData ? 'Mock preview' : 'Live control plane',
    totalGateways: gateways.length,
    connectedGateways: gateways.filter((gateway) => gateway.status.connected).length,
    disconnectedGateways: gateways.filter((gateway) => !gateway.status.connected).length,
    warningCount: gateways.reduce((count, gateway) => count + gateway.warnings.length, 0),
    proxyResourceGateways: gateways.filter((gateway) => gateway.config.proxy_resources !== false).length,
    bearerTokenGateways: gateways.filter((gateway) => Boolean(gateway.config.bearer_token_env)).length,
  }
}

export function buildGatewayDocsSnapshot(
  gateways: Gateway[],
  supportedServices: number,
): GatewayDocsSnapshot {
  return {
    totalGateways: gateways.length,
    connectedGateways: gateways.filter((gateway) => gateway.status.connected).length,
    warningCount: gateways.reduce((count, gateway) => count + gateway.warnings.length, 0),
    httpGateways: gateways.filter((gateway) => gateway.transport === 'http').length,
    stdioGateways: gateways.filter((gateway) => gateway.transport === 'stdio').length,
    supportedServices,
    exposedTools: gateways.reduce((count, gateway) => count + gateway.status.exposed_tool_count, 0),
  }
}
