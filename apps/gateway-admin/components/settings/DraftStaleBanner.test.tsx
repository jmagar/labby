import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { act } from 'react'

import { installChatTestDom, renderClient } from '@/components/chat/test-utils'
import { DraftStaleBanner } from './DraftStaleBanner'

async function waitFor(assertion: () => void) {
  const deadline = Date.now() + 2_000
  let lastError: unknown
  while (Date.now() < deadline) {
    try {
      assertion()
      return
    } catch (error) {
      lastError = error
      await new Promise((resolve) => setTimeout(resolve, 20))
    }
  }
  throw lastError
}

test('DraftStaleBanner explains old drafts and can discard them', async () => {
  installChatTestDom()
  const actions: string[] = []
  globalThis.fetch = (async (_input, init) => {
    const payload = JSON.parse(String(init?.body ?? '{}')) as { action?: string }
    actions.push(payload.action ?? '')
    if (payload.action === 'state') {
      const discarded = actions.includes('draft.discard')
      return new Response(JSON.stringify({
        first_run: false,
        env_path: '/home/jmagar/.labby/.env',
        draft_path: '/home/jmagar/.labby/.env.draft',
        last_completed_step: 0,
        draft_stale: !discarded,
        has_draft: !discarded,
        draft_entry_count: discarded ? 0 : 1,
        env_mtime_unix_seconds: 1_780_000_000,
        draft_mtime_unix_seconds: discarded ? null : 1_779_000_000,
        state: { kind: 'ready' },
      }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    }
    if (payload.action === 'draft.discard') {
      return new Response(JSON.stringify({ removed: true }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    }
    return new Response(JSON.stringify({ kind: 'unknown_action', message: 'nope' }), {
      status: 400,
      headers: { 'content-type': 'application/json' },
    })
  }) as typeof fetch

  const view = await renderClient(<DraftStaleBanner />)

  await waitFor(() => assert.match(view.container.textContent ?? '', /Old draft detected/))
  assert.match(view.container.textContent ?? '', /with 1 value/)
  assert.doesNotMatch(view.container.textContent ?? '', /Another session has unsaved changes/)

  const button = view.container.querySelector('button')
  assert.ok(button)
  await act(async () => {
    button.dispatchEvent(new MouseEvent('click', { bubbles: true }))
  })

  await waitFor(() => assert.equal(view.container.textContent, ''))
  assert.deepEqual(actions, ['state', 'draft.discard', 'state'])

  await view.unmount()
})
