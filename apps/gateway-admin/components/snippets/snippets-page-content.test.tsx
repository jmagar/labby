import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'

import { installChatTestDom, renderClient } from '@/components/chat/test-utils'
import { SidebarProvider } from '@/components/ui/sidebar'
import { SnippetsPageContent } from './snippets-page-content'

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

test('snippets page renders fetched snippets and typed inputs', async () => {
  installChatTestDom()
  globalThis.fetch = (async (_input, init) => {
    const payload = JSON.parse(String(init?.body ?? '{}')) as { action?: string }
    if (payload.action === 'snippets.list') {
      return new Response(JSON.stringify({
        snippets: [
          {
            name: 'homelab-readonly-pulse',
            description: 'Read-only homelab pulse',
            tags: ['homelab'],
            inputs: {
              host: {
                ty: 'string',
                required: false,
                default: 'dookie',
                description: 'Host alias',
              },
            },
            source: 'builtin',
            path: '/docs/snippets/homelab-readonly-pulse.md',
            shadowed: false,
          },
        ],
      }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    }
    return new Response(JSON.stringify({ valid: true, name: 'homelab-readonly-pulse', mode: 'existing' }), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    })
  }) as typeof fetch

  const view = await renderClient(
    <SidebarProvider>
      <SnippetsPageContent />
    </SidebarProvider>,
  )

  await waitFor(() => assert.match(view.container.textContent ?? '', /homelab-readonly-pulse/))
  assert.match(view.container.textContent ?? '', /Host alias/)
  assert.match(view.container.textContent ?? '', /default dookie/)

  await view.unmount()
})
