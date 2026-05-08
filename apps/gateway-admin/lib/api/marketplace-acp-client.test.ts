import test from 'node:test'
import assert from 'node:assert/strict'

import { installAcpAgent } from './marketplace-client'

test('installAcpAgent sends backend agent install parameter names', async () => {
  const originalFetch = globalThis.fetch
  const originalDispatchEvent = globalThis.dispatchEvent
  const seenEvents: string[] = []

  try {
    globalThis.fetch = (async (_input, init) => {
      const body = JSON.parse(String(init?.body))
      assert.equal(body.action, 'agent.install')
      assert.deepEqual(body.params, {
        id: 'codex-acp',
        node_ids: ['local'],
        confirm: true,
      })

      return new Response(
        JSON.stringify({
          agent_id: 'codex-acp',
          results: [{ node_id: 'local', ok: true, result: {} }],
        }),
        { status: 200 },
      )
    }) as typeof fetch
    globalThis.dispatchEvent = ((event: Event) => {
      seenEvents.push(event.type)
      return true
    }) as typeof dispatchEvent

    const result = await installAcpAgent({
      agent_id: 'codex-acp',
      device_ids: ['local'],
      scope: 'global',
    })

    assert.deepEqual(result.results, [
      { device_id: 'local', ok: true, message: 'Available in /chat' },
    ])
    assert.deepEqual(seenEvents, ['lab:acp-providers-changed'])
  } finally {
    globalThis.fetch = originalFetch
    globalThis.dispatchEvent = originalDispatchEvent
  }
})
