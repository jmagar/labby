import test from 'node:test'
import assert from 'node:assert/strict'

import type { Gateway, ServiceAction, ServiceConfig, SupportedService } from '../types/gateway.ts'
import { mergeGatewayListWithSupportedServices, synthesizeLabGateway, upstreamMcpGateways } from './gateway-list-model.ts'

function makeGateway(partial: Partial<Gateway> & Pick<Gateway, 'id' | 'name'>): Gateway {
  return {
    id: partial.id,
    name: partial.name,
    transport: partial.transport ?? 'http',
    source: partial.source ?? 'custom_gateway',
    configured: partial.configured ?? true,
    enabled: partial.enabled ?? true,
    surfaces: partial.surfaces ?? {
      cli: { enabled: false, connected: false },
      api: { enabled: false, connected: false },
      mcp: { enabled: true, connected: true },
      webui: { enabled: false, connected: false },
    },
    config: partial.config ?? {},
    status: partial.status ?? {
      healthy: true,
      connected: true,
      discovered_tool_count: 0,
      exposed_tool_count: 0,
      discovered_resource_count: 0,
      exposed_resource_count: 0,
      discovered_prompt_count: 0,
      exposed_prompt_count: 0,
    },
    discovery: partial.discovery ?? { tools: [], resources: [], prompts: [] },
    warnings: partial.warnings ?? [],
    // created_at / updated_at are optional; only supply them when the test
    // explicitly needs them to reflect backend-provided data.
    ...(partial.created_at !== undefined ? { created_at: partial.created_at } : {}),
    ...(partial.updated_at !== undefined ? { updated_at: partial.updated_at } : {}),
  }
}

test('mergeGatewayListWithSupportedServices appends missing lab services as deactivated rows at the bottom', () => {
  const existing = [
    makeGateway({ id: 'custom-1', name: 'custom-1' }),
    makeGateway({
      id: 'plex',
      name: 'plex',
      transport: 'in_process',
      source: 'in_process',
      enabled: true,
      status: {
        healthy: true,
        connected: true,
        discovered_tool_count: 29,
        exposed_tool_count: 29,
        discovered_resource_count: 0,
        exposed_resource_count: 0,
        discovered_prompt_count: 0,
        exposed_prompt_count: 0,
      },
    }),
  ]

  const supported: SupportedService[] = [
    {
      key: 'plex',
      display_name: 'Plex',
      category: 'Media',
      description: 'Plex service',
      required_env: [],
      optional_env: [],
    },
    {
      key: 'radarr',
      display_name: 'Radarr',
      category: 'Media',
      description: 'Radarr service',
      required_env: [],
      optional_env: [],
    },
  ]

  const serviceConfigs = new Map<string, ServiceConfig>([
    ['radarr', { service: 'radarr', configured: false, fields: [] }],
  ])
  const serviceActions = new Map<string, ServiceAction[]>([
    ['radarr', [{ name: 'movie.search', description: 'Search', destructive: false }]],
  ])

  const merged = mergeGatewayListWithSupportedServices(existing, supported, serviceConfigs, serviceActions)

  assert.deepEqual(merged.map((gateway) => gateway.id), ['custom-1', 'plex', 'radarr'])
  assert.equal(merged[2]?.source, 'in_process')
  assert.equal(merged[2]?.enabled, false)
  assert.equal(merged[2]?.configured, false)
  assert.equal(merged[2]?.status.connected, false)
  assert.equal(merged[2]?.status.discovered_tool_count, 1)
  assert.equal(merged[2]?.status.exposed_tool_count, 0)
})

test('upstreamMcpGateways excludes lab-backed virtual service rows from the gateway surface', () => {
  const gateways = [
    makeGateway({ id: 'syslog', name: 'syslog', transport: 'http', source: 'custom' }),
    makeGateway({ id: 'deploy', name: 'deploy', transport: 'in_process', source: 'in_process', enabled: false }),
    makeGateway({ id: 'context7', name: 'context7', transport: 'http', source: 'custom_gateway' }),
  ]

  const filtered = upstreamMcpGateways(gateways)

  assert.deepEqual(filtered.map((gateway) => gateway.id), ['syslog', 'context7'])
})

test('mergeGatewayListWithSupportedServices preserves existing disabled lab rows and keeps them after active rows', () => {
  const existing = [
    makeGateway({ id: 'custom-1', name: 'custom-1' }),
    makeGateway({
      id: 'radarr',
      name: 'radarr',
      transport: 'in_process',
      source: 'in_process',
      enabled: false,
      configured: true,
      status: {
        healthy: false,
        connected: false,
        discovered_tool_count: 53,
        exposed_tool_count: 0,
        discovered_resource_count: 0,
        exposed_resource_count: 0,
        discovered_prompt_count: 0,
        exposed_prompt_count: 0,
      },
    }),
  ]

  const supported: SupportedService[] = [
    {
      key: 'radarr',
      display_name: 'Radarr',
      category: 'Media',
      description: 'Radarr service',
      required_env: [],
      optional_env: [],
    },
  ]

  const merged = mergeGatewayListWithSupportedServices(existing, supported, new Map(), new Map())

  assert.deepEqual(merged.map((gateway) => gateway.id), ['custom-1', 'radarr'])
  assert.equal(merged[1]?.enabled, false)
  assert.equal(merged[1]?.status.discovered_tool_count, 53)
})

test('synthesizeLabGateway does not fabricate created_at or updated_at', () => {
  const service: SupportedService = {
    key: 'sonarr',
    display_name: 'Sonarr',
    category: 'Media',
    description: 'Sonarr service',
    required_env: [],
    optional_env: [],
  }

  const gateway = synthesizeLabGateway(service, undefined, undefined)

  // Synthesized gateways have no backend-provided timestamps; they must be absent.
  assert.equal(gateway.created_at, undefined)
  assert.equal(gateway.updated_at, undefined)
})
