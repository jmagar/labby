import test from 'node:test'
import assert from 'node:assert/strict'

import {
  initialGatewayAuthMode,
  normalizeProtectedPublicPath,
  protectedRouteForGateway,
  protectedRoutePathInputValue,
} from './gateway-protected-route'
import type { Gateway, ProtectedMcpRoute } from './types/gateway'

function gateway(overrides: Partial<Gateway> = {}): Gateway {
  return {
    id: 'repomix',
    name: 'repomix',
    transport: 'stdio',
    config: {},
    status: {
      healthy: true,
      connected: true,
      discovered_tool_count: 0,
      exposed_tool_count: 0,
      discovered_resource_count: 0,
      exposed_resource_count: 0,
      discovered_prompt_count: 0,
      exposed_prompt_count: 0,
    },
    discovery: { tools: [], resources: [], prompts: [] },
    warnings: [],
    created_at: '2026-05-10T00:00:00.000Z',
    updated_at: '2026-05-10T00:00:00.000Z',
    ...overrides,
  }
}

function route(overrides: Partial<ProtectedMcpRoute> = {}): ProtectedMcpRoute {
  return {
    name: 'repomix',
    enabled: true,
    public_host: 'mcp.tootie.tv',
    public_path: '/tools',
    upstream: 'repomix',
    backend_url: '',
    backend_mcp_path: '/mcp',
    scopes: ['mcp:read', 'mcp:write'],
    health_path: null,
    ...overrides,
  }
}

test('protectedRouteForGateway matches an enabled route by upstream and host', () => {
  const found = protectedRouteForGateway(
    gateway(),
    [
      route({ name: 'other', upstream: 'other', public_path: '/other' }),
      route(),
    ],
    'mcp.tootie.tv',
  )

  assert.equal(found?.public_path, '/tools')
  assert.equal(protectedRoutePathInputValue(found), 'tools')
})

test('protectedRouteForGateway falls back to route name for older route records', () => {
  const found = protectedRouteForGateway(
    gateway(),
    [route({ upstream: null })],
    'mcp.tootie.tv',
  )

  assert.equal(found?.name, 'repomix')
})

test('initialGatewayAuthMode shows OAuth for protected public routes', () => {
  assert.equal(initialGatewayAuthMode(gateway(), route()), 'oauth')
  assert.equal(initialGatewayAuthMode(gateway({ config: { bearer_token_env: 'TOKEN' } }), null), 'bearer')
  assert.equal(initialGatewayAuthMode(gateway(), null), 'none')
})

test('normalizeProtectedPublicPath accepts slugs and URLs', () => {
  assert.equal(normalizeProtectedPublicPath('tools'), '/tools')
  assert.equal(normalizeProtectedPublicPath('/tools/'), '/tools')
  assert.equal(normalizeProtectedPublicPath('https://mcp.tootie.tv/tools'), '/tools')
  assert.throws(() => normalizeProtectedPublicPath('/'), /non-root/)
})
