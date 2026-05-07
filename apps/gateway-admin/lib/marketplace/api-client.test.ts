import test from 'node:test'
import assert from 'node:assert/strict'

import { listAcpAgents, listMcpServers } from './api-client'

test('listAcpAgents accepts the backend array response from agent.list', async () => {
  const originalFetch = globalThis.fetch

  try {
    globalThis.fetch = (async (_input, init) => {
      const body = JSON.parse(String(init?.body))
      assert.equal(body.action, 'agent.list')

      return new Response(
        JSON.stringify([
          {
            id: 'codex-acp',
            name: 'Codex CLI',
            version: '0.12.0',
            distribution: { npx: { package: '@zed-industries/codex-acp', version: '0.12.0' } },
          },
        ]),
        { status: 200 },
      )
    }) as typeof fetch

    const agents = await listAcpAgents()

    assert.equal(agents.length, 1)
    assert.equal(agents[0]?.id, 'codex-acp')
    assert.equal(agents[0]?.name, 'Codex CLI')
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('listMcpServers requests a bounded registry page', async () => {
  const originalFetch = globalThis.fetch

  try {
    globalThis.fetch = (async (_input, init) => {
      const body = JSON.parse(String(init?.body))
      assert.equal(body.action, 'mcp.list')
      assert.deepEqual(body.params, { limit: 20 })

      return new Response(
        JSON.stringify({
          servers: [],
          metadata: { count: 0, nextCursor: null },
        }),
        { status: 200 },
      )
    }) as typeof fetch

    const servers = await listMcpServers()

    assert.equal(servers.length, 0)
  } finally {
    globalThis.fetch = originalFetch
  }
})
