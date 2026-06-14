import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { act } from 'react'

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
  const requests: Array<{ action?: string; params?: Record<string, unknown> }> = []
  globalThis.fetch = (async (_input, init) => {
    const payload = JSON.parse(String(init?.body ?? '{}')) as { action?: string; params?: Record<string, unknown> }
    requests.push(payload)
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
    if (payload.action === 'snippets.get') {
      return new Response(JSON.stringify({
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
        body: [
          '---',
          'name: homelab-readonly-pulse',
          'description: Read-only homelab pulse',
          '---',
          '',
          '# Homelab Pulse',
          '',
          'Use **read-only** checks before changing anything.',
          '',
          '<script>alert("nope")</script>',
          '![tracking pixel](https://example.com/pixel.png)',
          '[bad link](javascript:alert("nope"))',
          '',
          '```js',
          'async () => ({ ok: true })',
          '```',
        ].join('\n'),
      }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    }
    if (payload.action === 'snippets.test') {
      return new Response(JSON.stringify({ name: 'homelab-readonly-pulse', passed: false }), {
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
  await waitFor(() => {
    const headings = Array.from(view.container.querySelectorAll('h1')).map((node) => node.textContent)
    assert.ok(headings.includes('Homelab Pulse'))
  })
  assert.deepEqual(requests.slice(0, 2).map((request) => request.action), ['snippets.list', 'snippets.get'])
  assert.deepEqual(requests[1]?.params, { name: 'homelab-readonly-pulse' })
  assert.equal(view.container.querySelector('script'), null)
  assert.equal(view.container.querySelector('img'), null)
  assert.equal(
    Array.from(view.container.querySelectorAll('a')).some((link) =>
      link.getAttribute('href')?.startsWith('javascript:'),
    ),
    false,
  )

  const testButton = Array.from(view.container.querySelectorAll('button')).find(
    (button) => button.textContent?.trim() === 'Test',
  )
  assert.ok(testButton, 'expected Test button')
  await act(async () => {
    testButton.dispatchEvent(new MouseEvent('click', { bubbles: true }))
  })
  await waitFor(() => assert.match(view.container.textContent ?? '', /Test failed/))

  await view.unmount()
})
