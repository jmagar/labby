import test from 'node:test'
import assert from 'node:assert/strict'

import {
  deleteServerLocalMetadata,
  getRegistryConfig,
  getServer,
  getServerLocalMetadata,
  installServer,
  listServers,
  listVersions,
  setServerLocalMetadata,
  validateServer,
} from './mcpregistry-client.ts'
import type { LabRegistryMetadata, ServerJSON } from '@/lib/types/registry'

const SERVER_NAME = 'io.github.lab/example-server'

const SERVER_JSON: ServerJSON = {
  name: SERVER_NAME,
  title: 'Example Server',
  description: 'Example MCP server',
  version: '1.2.3',
  packages: [],
  remotes: [{ type: 'streamable-http', url: 'https://example.com/mcp' }],
  icons: [],
}

const RAW_SERVER_RESPONSE = {
  server: SERVER_JSON,
  _meta: {
    'io.modelcontextprotocol.registry/official': {
      isLatest: true,
      publishedAt: '2026-05-01T00:00:00Z',
      status: 'active',
      statusChangedAt: '2026-05-01T00:00:00Z',
      updatedAt: '2026-05-02T00:00:00Z',
    },
  },
}

const LOCAL_METADATA: LabRegistryMetadata = {
  curation: {
    featured: true,
    hidden: false,
    tags: ['registry'],
  },
  trust: {
    reviewed: true,
  },
}

interface CapturedRequest {
  url: string
  init?: RequestInit
  body?: { action?: string; params?: Record<string, unknown> }
}

function jsonResponse(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  })
}

async function captureRequest(input: RequestInfo | URL, init?: RequestInit): Promise<CapturedRequest> {
  const rawBody = typeof init?.body === 'string' ? JSON.parse(init.body) as CapturedRequest['body'] : undefined
  return {
    url: String(input),
    init,
    body: rawBody,
  }
}

async function withMockFetch<T>(
  handler: (request: CapturedRequest) => Response | Promise<Response>,
  run: () => Promise<T>,
): Promise<{ result: T; requests: CapturedRequest[] }> {
  const originalFetch = globalThis.fetch
  const requests: CapturedRequest[] = []

  try {
    globalThis.fetch = (async (input, init) => {
      const request = await captureRequest(input, init)
      assert.notEqual(request.url, '/v1/mcpregistry')
      requests.push(request)
      return handler(request)
    }) as typeof fetch

    const result = await run()
    return { result, requests }
  } finally {
    globalThis.fetch = originalFetch
  }
}

function assertMarketplaceAction(
  request: CapturedRequest,
  action: string,
): Record<string, unknown> {
  assert.equal(request.url, '/v1/marketplace')
  assert.equal(request.init?.method, 'POST')
  assert.equal(request.init?.credentials, 'include')
  assert.equal(request.body?.action, action)
  return request.body?.params ?? {}
}

test('getRegistryConfig uses marketplace mcp.config action', async () => {
  const { result, requests } = await withMockFetch(
    (request) => {
      assertMarketplaceAction(request, 'mcp.config')
      return jsonResponse({ url: 'https://registry.modelcontextprotocol.io' })
    },
    () => getRegistryConfig(),
  )

  assert.equal(requests.length, 1)
  assert.equal(result.url, 'https://registry.modelcontextprotocol.io')
})

test('listServers uses /v0.1/servers with same-origin credentials', async () => {
  const { result, requests } = await withMockFetch(
    (request) => {
      assert.equal(request.url, '/v0.1/servers')
      assert.equal(request.init?.credentials, 'include')
      assert.equal(new Headers(request.init?.headers).get('Authorization'), null)
      return jsonResponse({
        servers: [],
        next_cursor: null,
      })
    },
    () => listServers({}),
  )

  assert.equal(requests.length, 1)
  assert.equal(result.servers.length, 0)
  assert.equal(result.metadata.nextCursor, null)
})

test('getServer uses /v0.1 latest-version REST route', async () => {
  const { result, requests } = await withMockFetch(
    (request) => {
      assert.equal(request.url, `/v0.1/servers/${encodeURIComponent(SERVER_NAME)}/versions/latest`)
      assert.equal(request.init?.credentials, 'include')
      return jsonResponse(RAW_SERVER_RESPONSE)
    },
    () => getServer(SERVER_NAME),
  )

  assert.equal(requests.length, 1)
  assert.equal(result.server.name, SERVER_NAME)
  assert.deepEqual(result.server.packages, [])
})

test('listVersions uses /v0.1 versions REST route', async () => {
  const { result, requests } = await withMockFetch(
    (request) => {
      assert.equal(request.url, `/v0.1/servers/${encodeURIComponent(SERVER_NAME)}/versions`)
      assert.equal(request.init?.credentials, 'include')
      return jsonResponse({ versions: [RAW_SERVER_RESPONSE] })
    },
    () => listVersions(SERVER_NAME),
  )

  assert.equal(requests.length, 1)
  assert.equal(result.servers.length, 1)
  assert.equal(result.metadata.count, 1)
  assert.equal(result.metadata.nextCursor, null)
})

test('validateServer uses marketplace mcp.validate action', async () => {
  const { result, requests } = await withMockFetch(
    (request) => {
      const params = assertMarketplaceAction(request, 'mcp.validate')
      assert.deepEqual(params.server_json, SERVER_JSON)
      return jsonResponse({ valid: true, issues: [] })
    },
    () => validateServer(SERVER_JSON),
  )

  assert.equal(requests.length, 1)
  assert.equal(result.valid, true)
})

test('getServerLocalMetadata uses marketplace mcp.meta.get action', async () => {
  const { result, requests } = await withMockFetch(
    (request) => {
      const params = assertMarketplaceAction(request, 'mcp.meta.get')
      assert.deepEqual(params, { name: SERVER_NAME, version: '1.2.3' })
      return jsonResponse({
        name: SERVER_NAME,
        version: '1.2.3',
        namespace: 'tv.tootie.lab/registry',
        metadata: LOCAL_METADATA,
      })
    },
    () => getServerLocalMetadata(SERVER_NAME, '1.2.3'),
  )

  assert.equal(requests.length, 1)
  assert.deepEqual(result.metadata, LOCAL_METADATA)
})

test('setServerLocalMetadata uses marketplace mcp.meta.set action', async () => {
  const { result, requests } = await withMockFetch(
    (request) => {
      const params = assertMarketplaceAction(request, 'mcp.meta.set')
      assert.deepEqual(params, {
        name: SERVER_NAME,
        version: '1.2.3',
        updated_by: 'tester',
        metadata: LOCAL_METADATA,
      })
      return jsonResponse({
        name: SERVER_NAME,
        version: '1.2.3',
        namespace: 'tv.tootie.lab/registry',
        metadata: LOCAL_METADATA,
      })
    },
    () => setServerLocalMetadata(SERVER_NAME, LOCAL_METADATA, { version: '1.2.3', updated_by: 'tester' }),
  )

  assert.equal(requests.length, 1)
  assert.deepEqual(result.metadata, LOCAL_METADATA)
})

test('deleteServerLocalMetadata uses marketplace mcp.meta.delete action', async () => {
  const { result, requests } = await withMockFetch(
    (request) => {
      const params = assertMarketplaceAction(request, 'mcp.meta.delete')
      assert.deepEqual(params, { name: SERVER_NAME, version: '1.2.3' })
      return jsonResponse({
        name: SERVER_NAME,
        version: '1.2.3',
        namespace: 'tv.tootie.lab/registry',
        deleted: true,
      })
    },
    () => deleteServerLocalMetadata(SERVER_NAME, '1.2.3'),
  )

  assert.equal(requests.length, 1)
  assert.equal(result.deleted, true)
})

test('installServer uses marketplace mcp.install action with gateway_ids', async () => {
  const { result, requests } = await withMockFetch(
    (request) => {
      const params = assertMarketplaceAction(request, 'mcp.install')
      assert.deepEqual(params, {
        name: SERVER_NAME,
        gateway_ids: ['homelab'],
        version: '1.2.3',
        bearer_token_env: 'EXAMPLE_TOKEN',
        confirm: true,
      })
      return jsonResponse({
        results: [
          {
            gateway_id: 'homelab',
            ok: true,
          },
        ],
      })
    },
    () => installServer({
      name: SERVER_NAME,
      gateway_name: 'homelab',
      version: '1.2.3',
      bearer_token_env: 'EXAMPLE_TOKEN',
    }),
  )

  assert.equal(requests.length, 1)
  assert.deepEqual(result.results, [{ gateway_id: 'homelab', ok: true }])
})

test('mcpregistry client exports do not call removed /v1/mcpregistry endpoint', async () => {
  const { requests } = await withMockFetch(
    (request) => {
      switch (request.url) {
        case '/v1/marketplace': {
          switch (request.body?.action) {
            case 'mcp.config':
              return jsonResponse({ url: 'https://registry.modelcontextprotocol.io' })
            case 'mcp.validate':
              return jsonResponse({ valid: true, issues: [] })
            case 'mcp.meta.get':
            case 'mcp.meta.set':
              return jsonResponse({
                name: SERVER_NAME,
                version: '1.2.3',
                namespace: 'tv.tootie.lab/registry',
                metadata: LOCAL_METADATA,
              })
            case 'mcp.meta.delete':
              return jsonResponse({
                name: SERVER_NAME,
                version: '1.2.3',
                namespace: 'tv.tootie.lab/registry',
                deleted: true,
              })
            case 'mcp.install':
              return jsonResponse({ results: [{ gateway_id: 'homelab', ok: true }] })
            default:
              throw new Error(`unexpected marketplace action ${request.body?.action}`)
          }
        }
        case '/v0.1/servers':
          return jsonResponse({ servers: [RAW_SERVER_RESPONSE], next_cursor: null })
        case `/v0.1/servers/${encodeURIComponent(SERVER_NAME)}/versions/latest`:
          return jsonResponse(RAW_SERVER_RESPONSE)
        case `/v0.1/servers/${encodeURIComponent(SERVER_NAME)}/versions`:
          return jsonResponse({ versions: [RAW_SERVER_RESPONSE] })
        default:
          throw new Error(`unexpected URL ${request.url}`)
      }
    },
    async () => {
      await getRegistryConfig()
      await listServers({})
      await getServer(SERVER_NAME)
      await listVersions(SERVER_NAME)
      await validateServer(SERVER_JSON)
      await getServerLocalMetadata(SERVER_NAME, '1.2.3')
      await setServerLocalMetadata(SERVER_NAME, LOCAL_METADATA, { version: '1.2.3' })
      await deleteServerLocalMetadata(SERVER_NAME, '1.2.3')
      await installServer({ name: SERVER_NAME, gateway_name: 'homelab', version: '1.2.3' })
    },
  )

  assert.equal(requests.some((request) => request.url === '/v1/mcpregistry'), false)
})
