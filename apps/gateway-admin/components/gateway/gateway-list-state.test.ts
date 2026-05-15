import test from 'node:test'
import assert from 'node:assert/strict'

import type { Gateway } from '@/lib/types/gateway'
import {
  aggregateToolsFromGateways,
  filterGateways,
  filterTools,
  matchesGatewayStatusFacet,
  sortToolRows,
  type ToolInventoryRow,
} from './gateway-list-state'

function buildGateway(overrides: Partial<Gateway> = {}): Gateway {
  return {
    id: 'gw_default',
    name: 'Gateway Default',
    transport: 'stdio',
    source: 'in_process',
    configured: true,
    enabled: true,
    config: {
      command: 'lab service mcp',
      url: undefined,
    },
    status: {
      healthy: true,
      connected: true,
      discovered_tool_count: 1,
      exposed_tool_count: 1,
      discovered_resource_count: 0,
      exposed_resource_count: 0,
      discovered_prompt_count: 0,
      exposed_prompt_count: 0,
      last_error: undefined,
    },
    discovery: {
      tools: [],
      resources: [],
      prompts: [],
    },
    warnings: [],
    created_at: '2026-04-16T12:00:00Z',
    updated_at: '2026-04-17T12:00:00Z',
    ...overrides,
  }
}

const configuredHealthyGateway = buildGateway({
  id: 'gw_lab',
  name: 'Lab Gateway',
  transport: 'stdio',
  source: 'in_process',
  discovery: {
    tools: [
      { name: 'gateway', description: 'Manage gateways', exposed: true, matched_by: null },
      { name: 'unifi', description: 'UniFi Network Application local API', exposed: true, matched_by: null },
    ],
    resources: [],
    prompts: [],
  },
})

const disconnectedGateway = buildGateway({
  id: 'gw_http',
  name: 'HTTP Gateway',
  transport: 'http',
  source: 'custom',
  config: {
    url: 'https://plex.example.com/mcp',
  },
  status: {
    healthy: false,
    connected: false,
    discovered_tool_count: 1,
    exposed_tool_count: 0,
    discovered_resource_count: 0,
    exposed_resource_count: 0,
    discovered_prompt_count: 0,
    exposed_prompt_count: 0,
    last_error: 'Disconnected',
  },
  discovery: {
    tools: [{ name: 'search', description: 'Search library', exposed: false, matched_by: null }],
    resources: [],
    prompts: [],
  },
})

const disabledGateway = buildGateway({
  id: 'gw_disabled',
  name: 'Disabled Gateway',
  enabled: false,
  discovery: { tools: [], resources: [], prompts: [] },
})

const fixtures = [configuredHealthyGateway, disconnectedGateway]

test('configured primary lens returns configured gateways before secondary filters', () => {
  const result = filterGateways(fixtures, {
    primaryLens: 'configured',
    search: '',
    status: [],
    source: [],
    transport: [],
  })

  assert.deepEqual(result.map((gateway) => gateway.id), ['gw_lab', 'gw_http'])
})

test('tools aggregation produces one row per tool per gateway', () => {
  const rows = aggregateToolsFromGateways(fixtures)

  assert.equal(rows.length, 3)
  assert.deepEqual(rows.map((row) => [row.gatewayId, row.toolName]), [
    ['gw_lab', 'gateway'],
    ['gw_lab', 'unifi'],
    ['gw_http', 'search'],
  ])
})

test('tools filters combine search with gateway and exposure filters', () => {
  const toolFixtures: ToolInventoryRow[] = aggregateToolsFromGateways(fixtures)
  const rows = filterTools(toolFixtures, {
    search: 'uni',
    gatewayIds: ['gw_lab'],
    exposure: 'exposed',
    source: [],
    transport: [],
  })

  assert.deepEqual(rows.map((row) => row.toolName), ['unifi'])
})

test('gateway status facets map configured healthy disconnected enabled and disabled correctly', () => {
  assert.equal(matchesGatewayStatusFacet(configuredHealthyGateway, ['configured']), true)
  assert.equal(matchesGatewayStatusFacet(configuredHealthyGateway, ['healthy']), true)
  assert.equal(matchesGatewayStatusFacet(disconnectedGateway, ['disconnected']), true)
  assert.equal(matchesGatewayStatusFacet(disabledGateway, ['disabled']), true)
  assert.equal(matchesGatewayStatusFacet(disabledGateway, ['enabled']), false)
  assert.equal(matchesGatewayStatusFacet(disabledGateway, ['disconnected']), false)
})

test('disconnected primary lens excludes disabled gateways', () => {
  const result = filterGateways([configuredHealthyGateway, disconnectedGateway, disabledGateway], {
    primaryLens: 'disconnected',
    search: '',
    status: [],
    source: [],
    transport: [],
  })

  assert.deepEqual(result.map((gateway) => gateway.id), ['gw_http'])
})

test('tool rows sort alphabetically by tool name', () => {
  const rows = sortToolRows(aggregateToolsFromGateways(fixtures))
  assert.deepEqual(rows.map((row) => row.toolName), ['gateway', 'search', 'unifi'])
})
