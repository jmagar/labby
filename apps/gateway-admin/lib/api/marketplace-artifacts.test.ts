import test from 'node:test'
import assert from 'node:assert/strict'

import {
  forkMarketplaceArtifact,
  listMarketplaceForks,
  resetMarketplaceArtifact,
  unforkMarketplaceArtifact,
} from './marketplace-client'

function installFetchMock() {
  const calls: Array<{ url: string; init?: RequestInit }> = []
  const originalFetch = globalThis.fetch
  globalThis.fetch = (async (url: string | URL | Request, init?: RequestInit) => {
    calls.push({ url: String(url), init })
    return new Response(JSON.stringify({ ok: true }), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    })
  }) as typeof fetch
  return {
    calls,
    restore() {
      globalThis.fetch = originalFetch
    },
  }
}

test('forkMarketplaceArtifact posts artifact.fork', async () => {
  const fetchMock = installFetchMock()
  try {
    await forkMarketplaceArtifact({ pluginId: 'demo@labby', artifacts: ['skills/demo/SKILL.md'] })
    const body = JSON.parse(String(fetchMock.calls[0]?.init?.body))
    assert.deepEqual(body, {
      action: 'artifact.fork',
      params: {
        plugin_id: 'demo@labby',
        artifacts: ['skills/demo/SKILL.md'],
        confirm: true,
      },
    })
  } finally {
    fetchMock.restore()
  }
})

test('write artifact helpers include confirm true', async () => {
  const fetchMock = installFetchMock()
  try {
    await forkMarketplaceArtifact({ pluginId: 'demo@labby', artifacts: ['skills/demo/SKILL.md'] })
    await resetMarketplaceArtifact({ pluginId: 'demo@labby', artifacts: ['skills/demo/SKILL.md'] })
    await unforkMarketplaceArtifact({ pluginId: 'demo@labby', artifacts: ['skills/demo/SKILL.md'] })
    const forkBody = JSON.parse(String(fetchMock.calls[0]?.init?.body))
    const resetBody = JSON.parse(String(fetchMock.calls[1]?.init?.body))
    const unforkBody = JSON.parse(String(fetchMock.calls[2]?.init?.body))
    assert.equal(forkBody.params.confirm, true)
    assert.equal(resetBody.params.confirm, true)
    assert.equal(unforkBody.params.confirm, true)
  } finally {
    fetchMock.restore()
  }
})

test('listMarketplaceForks posts artifact.list', async () => {
  const fetchMock = installFetchMock()
  try {
    await listMarketplaceForks('demo@labby')
    const body = JSON.parse(String(fetchMock.calls[0]?.init?.body))
    assert.deepEqual(body, {
      action: 'artifact.list',
      params: { plugin_id: 'demo@labby' },
    })
  } finally {
    fetchMock.restore()
  }
})
