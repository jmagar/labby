import test from 'node:test'
import assert from 'node:assert/strict'

import { __setBrowserSessionStateForTests, getBrowserSessionState } from '../auth/session-store.ts'
import { GatewayApiError, gatewayApi } from './gateway-client.ts'
import { EXPOSE_NONE_PATTERN } from './tool-exposure-draft.ts'

type RecordedRequest = {
  action: string
  params: Record<string, unknown>
}

const standardGatewayView = {
  config: {
    name: 'gateway-1',
    url: 'http://gateway.example',
    command: null,
    args: [],
    bearer_token_env: null,
    proxy_resources: false,
    expose_tools: null,
  },
  runtime: {
    name: 'gateway-1',
    tool_count: 1,
    resource_count: 1,
    prompt_count: 1,
    exposed_tool_count: 1,
    exposed_resource_count: 1,
    exposed_prompt_count: 1,
    last_error: null,
  },
}

function jsonResponse(body: unknown) {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: {
      'content-type': 'application/json',
    },
  })
}

async function withGatewayFetch(
  handlers: Record<string, (params: Record<string, unknown>) => Promise<unknown> | unknown>,
  run: (requests: RecordedRequest[]) => Promise<void>,
) {
  const originalFetch = globalThis.fetch
  const requests: RecordedRequest[] = []
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })

  globalThis.fetch = (async (_input, init) => {
    const body = JSON.parse(String(init?.body ?? '{}')) as RecordedRequest
    requests.push(body)

    const handler = handlers[body.action]
    if (!handler) {
      throw new Error(`unexpected action: ${body.action}`)
    }

    const result = await handler(body.params)
    return result instanceof Response ? result : jsonResponse(result)
  }) as typeof fetch

  try {
    await run(requests)
  } finally {
    globalThis.fetch = originalFetch
  }
}

test('gatewayApi.create sends confirm=true with destructive gateway adds', async () => {
  await withGatewayFetch(
    {
      'gateway.add': () => standardGatewayView,
      'gateway.mcp.list': () => [],
      'gateway.test': () => standardGatewayView.runtime,
      'gateway.discovered_tools': () => ['tool.alpha'],
      'gateway.discovered_resources': () => ['lab://resource.alpha'],
      'gateway.discovered_prompts': () => ['prompt.alpha'],
    },
    async (requests) => {
      await gatewayApi.create({
        name: 'gateway-1',
        transport: 'http',
        config: {
          url: 'http://gateway.example',
        },
      } as never)

      assert.equal(
        requests.find((request) => request.action === 'gateway.add')?.params.confirm,
        true,
      )
    },
  )
})

test('gatewayApi.create sends pasted bearer tokens as a separate payload field', async () => {
  await withGatewayFetch(
    {
      'gateway.add': () => standardGatewayView,
      'gateway.mcp.list': () => [],
      'gateway.test': () => standardGatewayView.runtime,
      'gateway.discovered_tools': () => ['tool.alpha'],
      'gateway.discovered_resources': () => ['lab://resource.alpha'],
      'gateway.discovered_prompts': () => ['prompt.alpha'],
    },
    async (requests) => {
      await gatewayApi.create({
        name: 'github',
        transport: 'http',
        config: {
          url: 'https://api.githubcopilot.com/mcp/',
          bearer_token_value: 'ghp_secret',
        },
      } as never)

      assert.deepEqual(
        requests.find((request) => request.action === 'gateway.add')?.params.spec,
        {
          name: 'github',
          url: 'https://api.githubcopilot.com/mcp/',
          command: null,
          args: [],
          bearer_token_env: 'LAB_GW_GITHUB_AUTH_HEADER',
          proxy_resources: false,
          proxy_prompts: true,
          expose_tools: null,
          expose_resources: null,
          expose_prompts: null,
        },
      )
      assert.equal(
        requests.find((request) => request.action === 'gateway.add')?.params.bearer_token_value,
        'ghp_secret',
      )
    },
  )
})

test('gatewayApi.update sends confirm=true with destructive gateway updates', async () => {
  await withGatewayFetch(
    {
      'gateway.update': () => standardGatewayView,
      'gateway.mcp.list': () => [],
      'gateway.test': () => standardGatewayView.runtime,
      'gateway.discovered_tools': () => ['tool.alpha'],
      'gateway.discovered_resources': () => ['lab://resource.alpha'],
      'gateway.discovered_prompts': () => ['prompt.alpha'],
    },
    async (requests) => {
      await gatewayApi.update(
        'gateway-1',
        {
          name: 'gateway-1',
          transport: 'http',
          config: {
            url: 'http://gateway-updated.example',
          },
        } as never,
      )

      assert.equal(
        requests.find((request) => request.action === 'gateway.update')?.params.confirm,
        true,
      )
    },
  )
})

test('gatewayApi.update sends pasted bearer tokens as a separate payload field', async () => {
  await withGatewayFetch(
    {
      'gateway.update': () => standardGatewayView,
      'gateway.mcp.list': () => [],
      'gateway.test': () => standardGatewayView.runtime,
      'gateway.discovered_tools': () => ['tool.alpha'],
      'gateway.discovered_resources': () => ['lab://resource.alpha'],
      'gateway.discovered_prompts': () => ['prompt.alpha'],
    },
    async (requests) => {
      await gatewayApi.update(
        'github',
        {
          name: 'github',
          transport: 'http',
          config: {
            url: 'https://api.githubcopilot.com/mcp/',
            bearer_token_value: 'ghp_secret',
          },
        } as never,
      )

      assert.equal(
        requests.find((request) => request.action === 'gateway.update')?.params.bearer_token_value,
        'ghp_secret',
      )
    },
  )
})

test('gatewayApi.remove sends confirm=true with destructive gateway removals', async () => {
  await withGatewayFetch(
    {
      'gateway.remove': () => ({ ok: true }),
    },
    async (requests) => {
      await gatewayApi.remove('gateway-1')

      assert.equal(
        requests.find((request) => request.action === 'gateway.remove')?.params.confirm,
        true,
      )
    },
  )
})

test('gatewayApi.removeVirtualServer sends confirm=true with virtual-server removals', async () => {
  await withGatewayFetch(
    {
      'gateway.virtual_server.remove': () => ({ id: 'stale-registry' }),
    },
    async (requests) => {
      await gatewayApi.removeVirtualServer('stale-registry')

      const request = requests.find((request) => request.action === 'gateway.virtual_server.remove')
      assert.equal(request?.params.id, 'stale-registry')
      assert.equal(request?.params.confirm, true)
    },
  )
})

test('gatewayApi.reload sends confirm=true with destructive gateway reloads', async () => {
  await withGatewayFetch(
    {
      'gateway.get': () => standardGatewayView,
      'gateway.reload': () => ({ ok: true }),
    },
    async (requests) => {
      await gatewayApi.reload('gateway-1')

      assert.equal(
        requests.find((request) => request.action === 'gateway.reload')?.params.confirm,
        true,
      )
    },
  )
})

test('gatewayApi.getToolSearchConfig reads gateway-wide tool search settings', async () => {
  await withGatewayFetch(
    {
      'gateway.tool_search.get': () => ({
        enabled: true,
        top_k_default: 12,
        max_tools: 8000,
      }),
    },
    async (requests) => {
      const config = await gatewayApi.getToolSearchConfig()

      assert.deepEqual(config, {
        enabled: true,
        top_k_default: 12,
        max_tools: 8000,
      })
      assert.equal(requests[0]?.action, 'gateway.tool_search.get')
      assert.deepEqual(requests[0]?.params, {})
    },
  )
})

test('gatewayApi.setToolSearchConfig sends confirm=true for gateway-wide updates', async () => {
  await withGatewayFetch(
    {
      'gateway.tool_search.set': (params) => params,
    },
    async (requests) => {
      const config = await gatewayApi.setToolSearchConfig({
        enabled: true,
        top_k_default: 20,
        max_tools: 10_000,
      })

      assert.equal(config.enabled, true)
      assert.equal(config.top_k_default, 20)
      assert.equal(config.max_tools, 10_000)
      assert.equal(requests[0]?.action, 'gateway.tool_search.set')
      assert.equal(requests[0]?.params.confirm, true)
    },
  )
})

test('gatewayApi protected route actions use gateway service action payloads', async () => {
  const route = {
    name: 'tools',
    enabled: true,
    public_host: 'mcp.example.com',
    public_path: '/tools',
    backend_url: 'http://localhost:3100',
    backend_mcp_path: '/mcp',
    scopes: ['mcp:read'],
    health_path: '/health',
  }

  await withGatewayFetch(
    {
      'gateway.protected_route.list': () => [route],
      'gateway.protected_route.get': () => route,
      'gateway.protected_route.test': () => ({
        ok: true,
        route,
        resource: 'https://mcp.example.com/tools',
        metadata_url: 'https://mcp.example.com/.well-known/oauth-protected-resource/tools',
      }),
      'gateway.protected_route.add': () => route,
      'gateway.protected_route.update': () => route,
      'gateway.protected_route.remove': () => route,
    },
    async (requests) => {
      assert.deepEqual(await gatewayApi.listProtectedRoutes(), [route])
      assert.deepEqual(await gatewayApi.getProtectedRoute('tools'), route)
      assert.equal((await gatewayApi.testProtectedRoute(route)).ok, true)
      await gatewayApi.addProtectedRoute(route)
      await gatewayApi.updateProtectedRoute('tools', route)
      await gatewayApi.removeProtectedRoute('tools')

      assert.deepEqual(requests.map((request) => request.action), [
        'gateway.protected_route.list',
        'gateway.protected_route.get',
        'gateway.protected_route.test',
        'gateway.protected_route.add',
        'gateway.protected_route.update',
        'gateway.protected_route.remove',
      ])
      assert.deepEqual(requests[1]?.params, { name: 'tools' })
      assert.deepEqual(requests[2]?.params, { route })
      assert.equal(requests[3]?.params.confirm, true)
      assert.equal(requests[4]?.params.confirm, true)
      assert.equal(requests[5]?.params.confirm, true)
    },
  )
})

test('gatewayApi.setExposurePolicy sends confirm=true when updating a gateway config', async () => {
  await withGatewayFetch(
    {
      'gateway.server.get': () => ({
        id: 'gateway-1',
        name: 'gateway-1',
        source: 'custom_gateway',
      }),
      'gateway.update': () => ({ ok: true }),
    },
    async (requests) => {
      await gatewayApi.setExposurePolicy('gateway-1', {
        mode: 'allowlist',
        patterns: ['tool.alpha'],
      })

      assert.equal(
        requests.find((request) => request.action === 'gateway.update')?.params.confirm,
        true,
      )
    },
  )
})

test('gatewayApi.getExposurePolicy preserves expose-none sentinel as empty allowlist', async () => {
  await withGatewayFetch(
    {
      'gateway.server.get': () => ({
        id: 'github-chat',
        name: 'github-chat',
        source: 'in_process',
      }),
      'gateway.virtual_server.get_mcp_policy': () => ({
        allowed_actions: [EXPOSE_NONE_PATTERN],
      }),
    },
    async () => {
      const policy = await gatewayApi.getExposurePolicy('github-chat')

      assert.deepEqual(policy, {
        mode: 'allowlist',
        patterns: [],
      })
    },
  )
})

test('gatewayApi.list does not refresh browser session for non-csrf validation errors', async () => {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'browser-user', email: 'browser@example.com' },
    expiresAt: 123,
    csrfToken: 'csrf-old',
  })

  const urls: string[] = []
  globalThis.fetch = (async (input) => {
    const url = String(input)
    urls.push(url)

    if (url === '/v1/gateway') {
      return new Response(
        JSON.stringify({
          kind: 'validation_failed',
          message: 'missing required parameter `name`',
        }),
        {
          status: 422,
          headers: {
            'content-type': 'application/json',
            'x-request-id': 'req-gateway-validation-1',
          },
        },
      )
    }

    throw new Error(`unexpected fetch: ${url}`)
  }) as typeof fetch

  await assert.rejects(
    gatewayApi.list(),
    (error: unknown) => {
      assert.ok(error instanceof GatewayApiError)
      assert.equal(error.code, 'validation_failed')
      return true
    },
  )

  assert.deepEqual(getBrowserSessionState(), {
    status: 'authenticated',
    user: { sub: 'browser-user', email: 'browser@example.com' },
    expiresAt: 123,
    csrfToken: 'csrf-old',
  })
  assert.deepEqual(urls, ['/v1/gateway', '/v1/gateway'])
})

test('gatewayApi.list keeps loading when a stale in-process service has no action catalog', async () => {
  const originalWarn = console.warn
  console.warn = () => {}
  try {
    await withGatewayFetch(
      {
        'gateway.list': () => ([
          {
            id: 'mcpregistry',
            name: 'mcpregistry',
            source: 'in_process',
            configured: true,
            enabled: false,
            connected: false,
            discovered_tool_count: 0,
            exposed_tool_count: 0,
            discovered_resource_count: 0,
            exposed_resource_count: 0,
            discovered_prompt_count: 0,
            exposed_prompt_count: 0,
            surfaces: {
              cli: { enabled: false, connected: false },
              api: { enabled: false, connected: false },
              mcp: { enabled: false, connected: false },
              webui: { enabled: false, connected: false },
            },
            warnings: [],
            config_summary: {
              transport: 'in_process',
              target: 'mcpregistry',
            },
          },
        ]),
        'gateway.mcp.list': () => [],
        'gateway.service_actions': () => new Response(
          JSON.stringify({
            kind: 'invalid_param',
            message: 'unknown service `mcpregistry`',
            param: 'service',
          }),
          {
            status: 422,
            headers: { 'content-type': 'application/json' },
          },
        ),
        'gateway.virtual_server.get_mcp_policy': () => ({ allowed_actions: [] }),
      },
      async () => {
        const gateways = await gatewayApi.list()

        assert.equal(gateways.length, 1)
        assert.equal(gateways[0]?.id, 'mcpregistry')
        assert.equal(gateways[0]?.discovery.tools.length, 0)
        assert.match(gateways[0]?.warnings[0]?.message ?? '', /unknown service `mcpregistry`/)
      },
    )
  } finally {
    console.warn = originalWarn
  }
})

test('gatewayApi.list logs degraded gateway row warning counts once', async () => {
  const originalWarn = console.warn
  const warnings: unknown[][] = []
  console.warn = (...args: unknown[]) => {
    warnings.push(args)
  }

  try {
    await withGatewayFetch(
      {
        'gateway.list': () => ([
          {
            id: 'mcpregistry',
            name: 'mcpregistry',
            source: 'in_process',
            configured: true,
            enabled: false,
            connected: false,
            discovered_tool_count: 0,
            exposed_tool_count: 0,
            discovered_resource_count: 0,
            exposed_resource_count: 0,
            discovered_prompt_count: 0,
            exposed_prompt_count: 0,
            surfaces: {
              cli: { enabled: false, connected: false },
              api: { enabled: false, connected: false },
              mcp: { enabled: false, connected: false },
              webui: { enabled: false, connected: false },
            },
            warnings: [
              {
                code: 'unknown_service',
                message: 'service `mcpregistry` is not registered in this lab binary',
              },
            ],
            config_summary: {
              transport: 'in_process',
              target: 'mcpregistry',
            },
          },
        ]),
        'gateway.mcp.list': () => [],
        'gateway.service_actions': () => new Response(
          JSON.stringify({
            kind: 'invalid_param',
            message: 'unknown service `mcpregistry`',
            param: 'service',
          }),
          {
            status: 422,
            headers: { 'content-type': 'application/json' },
          },
        ),
        'gateway.virtual_server.get_mcp_policy': () => ({ allowed_actions: [] }),
      },
      async () => {
        await gatewayApi.list()

        assert.equal(warnings.length, 1)
        assert.equal(warnings[0][0], '[gateway] degraded gateway rows')
        assert.deepEqual(warnings[0][1], {
          unknown_service: 1,
          service_catalog_unavailable: 1,
        })
      },
    )
  } finally {
    console.warn = originalWarn
  }
})

test('gatewayApi.list rethrows aborts instead of degrading rows', async () => {
  const originalWarn = console.warn
  const warnings: unknown[][] = []
  console.warn = (...args: unknown[]) => {
    warnings.push(args)
  }
  const controller = new AbortController()

  try {
    await withGatewayFetch(
      {
        'gateway.list': () => ([
          {
            id: 'plex',
            name: 'plex',
            source: 'in_process',
            configured: true,
            enabled: true,
            connected: false,
            discovered_tool_count: 0,
            exposed_tool_count: 0,
            discovered_resource_count: 0,
            exposed_resource_count: 0,
            discovered_prompt_count: 0,
            exposed_prompt_count: 0,
            surfaces: {
              cli: { enabled: false, connected: false },
              api: { enabled: false, connected: false },
              mcp: { enabled: true, connected: false },
              webui: { enabled: false, connected: false },
            },
            warnings: [],
            config_summary: {
              transport: 'in_process',
              target: 'plex',
            },
          },
        ]),
        'gateway.mcp.list': () => [],
        'gateway.service_actions': () => {
          controller.abort()
          throw new DOMException('Aborted', 'AbortError')
        },
        'gateway.virtual_server.get_mcp_policy': () => ({ allowed_actions: [] }),
      },
      async () => {
        await assert.rejects(
          () => gatewayApi.list(controller.signal),
          (error: unknown) => error instanceof DOMException && error.name === 'AbortError',
        )
        assert.equal(warnings.length, 0)
      },
    )
  } finally {
    console.warn = originalWarn
  }
})

test('gatewayApi destructive mutations send confirm=true', async () => {
  const actions: Array<{ action: string; params: Record<string, unknown> }> = []

  globalThis.fetch = (async (input, init) => {
    const url = String(input)
    if (url !== '/v1/gateway') {
      throw new Error(`unexpected fetch: ${url}`)
    }

    const payload = JSON.parse(String(init?.body ?? '{}')) as {
      action: string
      params: Record<string, unknown>
    }
    actions.push(payload)

    if (payload.action === 'gateway.get') {
      return new Response(
        JSON.stringify({
          config: { name: 'plex', proxy_resources: false },
          runtime: { tool_count: 1 },
        }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      )
    }

    if (payload.action === 'gateway.mcp.list') {
      return new Response('[]', {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    }

    if (payload.action === 'gateway.add' || payload.action === 'gateway.update') {
      return new Response(
        JSON.stringify({
          config: {
            name: 'plex',
            url: 'https://lab.example.com/mcp',
            proxy_resources: false,
          },
          runtime: {
            tool_count: 1,
            resource_count: 0,
            prompt_count: 0,
            exposed_tool_count: 1,
            exposed_resource_count: 0,
            exposed_prompt_count: 0,
          },
        }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      )
    }

    if (payload.action === 'gateway.remove' || payload.action === 'gateway.reload') {
      return new Response('null', {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    }

    if (
      payload.action === 'gateway.discovered_tools' ||
      payload.action === 'gateway.discovered_resources' ||
      payload.action === 'gateway.discovered_prompts'
    ) {
      return new Response('[]', {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    }

    throw new Error(`unexpected action: ${payload.action}`)
  }) as typeof fetch

  await gatewayApi.create({
    name: 'plex',
    transport: 'http',
    config: { url: 'https://lab.example.com/mcp' },
  })
  await gatewayApi.update('plex', { name: 'plex-updated' })
  await gatewayApi.remove('plex')
  await gatewayApi.reload('plex')

  const destructiveActions = actions.filter(({ action }) =>
    ['gateway.add', 'gateway.update', 'gateway.remove', 'gateway.reload'].includes(action),
  )

  assert.equal(destructiveActions.length, 4)
  for (const action of destructiveActions) {
    assert.equal(action.params.confirm, true)
  }
})

test('gatewayApi.get applies virtual-server MCP policy to in-process tool exposure', async () => {
  await withGatewayFetch(
    {
      'gateway.server.get': () => ({
        id: 'github-chat',
        name: 'github-chat',
        source: 'in_process',
        configured: true,
        enabled: true,
        connected: true,
        discovered_tool_count: 2,
        exposed_tool_count: 1,
        discovered_resource_count: 0,
        exposed_resource_count: 0,
        discovered_prompt_count: 0,
        exposed_prompt_count: 0,
        surfaces: {
          cli: { enabled: false, connected: false },
          api: { enabled: false, connected: false },
          mcp: { enabled: true, connected: true },
          webui: { enabled: false, connected: false },
        },
        warnings: [],
        config_summary: {
          transport: 'in_process',
          target: 'github-chat',
        },
      }),
      'gateway.service_config.get': () => ({
        service: 'github-chat',
        configured: true,
        fields: [],
      }),
      'gateway.service_actions': () => ([
        { name: 'index_repository', description: 'Index a GitHub repository', destructive: false },
        { name: 'query_repository', description: 'Query a GitHub repository', destructive: false },
      ]),
      'gateway.virtual_server.get_mcp_policy': () => ({
        allowed_actions: ['query_repository'],
      }),
    },
    async (requests) => {
      const gateway = await gatewayApi.get('github-chat')

      assert.deepEqual(
        gateway.discovery.tools.map((tool) => ({
          name: tool.name,
          exposed: tool.exposed,
          matched_by: tool.matched_by,
        })),
        [
          { name: 'index_repository', exposed: false, matched_by: null },
          { name: 'query_repository', exposed: true, matched_by: 'query_repository' },
        ],
      )

      assert.deepEqual(
        requests.map((request) => request.action),
        [
          'gateway.server.get',
          'gateway.service_config.get',
          'gateway.service_actions',
          'gateway.virtual_server.get_mcp_policy',
        ],
      )
    },
  )
})
