import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { act } from 'react'
import { SWRConfig } from 'swr'
import { Window } from 'happy-dom'

import { MARKETPLACE_VIEW_MODE_STORAGE_KEY } from './marketplace-view-preference'

function installMarketplaceDom({ desktop = true } = {}) {
  const window = new Window()
  Object.defineProperty(globalThis, 'window', { configurable: true, value: window })
  Object.defineProperty(globalThis, 'document', { configurable: true, value: window.document })
  Object.defineProperty(globalThis, 'navigator', { configurable: true, value: window.navigator })
  Object.defineProperty(globalThis, 'DOMException', { configurable: true, value: window.DOMException })
  Object.defineProperty(globalThis, 'Node', { configurable: true, value: window.Node })
  Object.defineProperty(globalThis, 'MouseEvent', { configurable: true, value: window.MouseEvent })
  Object.defineProperty(globalThis, 'PointerEvent', { configurable: true, value: window.PointerEvent })
  Object.defineProperty(globalThis, 'KeyboardEvent', { configurable: true, value: window.KeyboardEvent })
  Object.defineProperty(globalThis, 'Event', { configurable: true, value: window.Event })
  Object.defineProperty(globalThis, 'InputEvent', { configurable: true, value: window.InputEvent })
  Object.defineProperty(globalThis, 'HTMLElement', { configurable: true, value: window.HTMLElement })
  Object.defineProperty(globalThis, 'HTMLInputElement', { configurable: true, value: window.HTMLInputElement })
  Object.defineProperty(globalThis, 'IS_REACT_ACT_ENVIRONMENT', { configurable: true, value: true })
  Object.defineProperty(window, 'matchMedia', {
    configurable: true,
    value: (query: string) => ({
      matches: query.includes('min-width') ? desktop : !desktop,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    }),
  })
  Object.defineProperty(globalThis, 'matchMedia', { configurable: true, value: window.matchMedia })
  return window
}

async function renderClient(element: React.ReactElement) {
  const { createRoot } = await import('react-dom/client')
  const container = document.createElement('div')
  document.body.appendChild(container)
  const root = createRoot(container)

  await act(async () => {
    root.render(element)
  })

  return {
    container,
    unmount: async () => {
      await act(async () => root.unmount())
      container.remove()
    },
  }
}

function installMarketplaceFetch() {
  globalThis.fetch = (async (_input, init) => {
    const payload = JSON.parse(String(init?.body ?? '{}')) as { action?: string }
    let body: unknown
    switch (payload.action) {
      case 'sources.list':
        body = [{ id: 'official', name: 'Official', owner: 'Labby', source: 'local', path: '/tmp/marketplace' }]
        break
      case 'plugins.list':
        body = [
          {
            id: 'codex-helper',
            name: 'Codex Helper',
            marketplaceId: 'official',
            version: '1.0.0',
            description: 'Codex workflow helpers',
            tags: ['codex'],
            installed: false,
          },
          {
            id: 'monitor-pack',
            name: 'Monitor Pack',
            marketplaceId: 'official',
            version: '1.0.0',
            description: 'Codex-adjacent monitoring helpers',
            tags: ['monitoring'],
            installed: false,
          },
        ]
        break
      case 'mcp.list':
        body = { servers: [] }
        break
      case 'agent.list':
        body = {
          agents: [
            {
              id: 'codex-cli',
              name: 'Codex CLI',
              description: 'Codex ACP agent',
              version: '1.0.0',
              distribution: { binary: 'codex' },
            },
          ],
        }
        break
      default:
        body = []
    }
    return new Response(JSON.stringify(body), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    })
  }) as typeof fetch
}

async function waitFor(assertion: () => void) {
  const deadline = Date.now() + 2_000
  let lastError: unknown
  while (Date.now() < deadline) {
    try {
      assertion()
      return
    } catch (error) {
      lastError = error
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 20))
      })
    }
  }
  throw lastError
}

test('marketplace persisted view preference controls rendered view and search ranks strong matches first', async () => {
  const window = installMarketplaceDom({ desktop: true })
  installMarketplaceFetch()
  const [{ SidebarProvider }, { MarketplaceListContent }] = await Promise.all([
    import('@/components/ui/sidebar'),
    import('./marketplace-list-content'),
  ])
  window.localStorage.setItem(MARKETPLACE_VIEW_MODE_STORAGE_KEY, 'table')

  const view = await renderClient(
    <SWRConfig value={{ provider: () => new Map(), dedupingInterval: 0 }}>
      <SidebarProvider>
        <MarketplaceListContent readOnlyPreview />
      </SidebarProvider>
    </SWRConfig>,
  )

  await waitFor(() => assert.match(view.container.textContent ?? '', /Codex Helper/))
  assert.equal(view.container.querySelector('table') !== null, true)

  const search = view.container.querySelector('input[name="marketplace-search"]') as HTMLInputElement | null
  assert.ok(search)
  await act(async () => {
    const setValue = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set
    setValue?.call(search, 'codex')
    search.dispatchEvent(new window.InputEvent('input', { bubbles: true, data: 'codex' }) as unknown as Event)
  })

  await waitFor(() => {
    const firstRow = view.container.querySelector('tbody tr')
    assert.ok(firstRow)
    assert.match(firstRow.textContent ?? '', /Codex Helper/)
  })

  await view.unmount()
})
