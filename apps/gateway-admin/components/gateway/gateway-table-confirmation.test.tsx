import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { act } from 'react'
import { Window } from 'happy-dom'

import type { Gateway } from '@/lib/types/gateway'

function installDom() {
  const window = new Window()
  Object.defineProperty(globalThis, 'window', { configurable: true, value: window })
  Object.defineProperty(globalThis, 'self', { configurable: true, value: window })
  Object.defineProperty(globalThis, 'document', { configurable: true, value: window.document })
  Object.defineProperty(globalThis, 'navigator', { configurable: true, value: window.navigator })
  Object.defineProperty(globalThis, 'HTMLElement', { configurable: true, value: window.HTMLElement })
  Object.defineProperty(globalThis, 'HTMLButtonElement', { configurable: true, value: window.HTMLButtonElement })
  Object.defineProperty(globalThis, 'Node', { configurable: true, value: window.Node })
  Object.defineProperty(globalThis, 'Event', { configurable: true, value: window.Event })
  Object.defineProperty(globalThis, 'MouseEvent', { configurable: true, value: window.MouseEvent })
  Object.defineProperty(globalThis, 'PointerEvent', { configurable: true, value: window.PointerEvent })
  Object.defineProperty(globalThis, 'KeyboardEvent', { configurable: true, value: window.KeyboardEvent })
  Object.defineProperty(globalThis, 'CustomEvent', { configurable: true, value: window.CustomEvent })
  Object.defineProperty(globalThis, 'DOMException', { configurable: true, value: window.DOMException })
  Object.defineProperty(globalThis, 'MutationObserver', {
    configurable: true,
    value: window.MutationObserver,
  })
  Object.defineProperty(globalThis, 'requestAnimationFrame', {
    configurable: true,
    value: (callback: FrameRequestCallback) => window.setTimeout(() => callback(Date.now()), 0),
  })
  Object.defineProperty(globalThis, 'cancelAnimationFrame', {
    configurable: true,
    value: (handle: number) => window.clearTimeout(handle as unknown as Parameters<typeof window.clearTimeout>[0]),
  })
  Object.defineProperty(globalThis, 'getComputedStyle', {
    configurable: true,
    value: window.getComputedStyle.bind(window),
  })
  Object.defineProperty(globalThis, 'IS_REACT_ACT_ENVIRONMENT', { configurable: true, value: true })
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
    rerender: async (next: React.ReactElement) => {
      await act(async () => {
        root.render(next)
      })
    },
    unmount: async () => {
      await act(async () => root.unmount())
      container.remove()
    },
  }
}

function click(element: Element | null) {
  assert.ok(element)
  act(() => {
    element.dispatchEvent(new MouseEvent('click', { bubbles: true }))
  })
}

const gateway: Gateway = {
  id: 'gw-http',
  name: 'github-server',
  transport: 'http',
  source: 'custom',
  configured: true,
  enabled: true,
  config: { url: 'https://github.example.test/mcp' },
  status: {
    healthy: true,
    connected: true,
    discovered_tool_count: 2,
    exposed_tool_count: 2,
    discovered_resource_count: 0,
    exposed_resource_count: 0,
    discovered_prompt_count: 0,
    exposed_prompt_count: 0,
  },
  discovery: { tools: [], resources: [], prompts: [] },
  warnings: [],
  created_at: '2026-06-01T12:00:00Z',
  updated_at: '2026-06-01T12:00:00Z',
}

test('gateway table asks before disabling an enabled server', async () => {
  installDom()
  const { GatewayTable } = await import('./gateway-table')
  let disableCalls = 0
  const view = await renderClient(
    <GatewayTable
      gateways={[gateway]}
      density="comfortable"
      onEdit={() => {}}
      onTest={() => {}}
      onReload={() => {}}
      onCleanup={() => {}}
      onClearCleanupHistory={() => {}}
      onToggleEnabled={() => { disableCalls += 1 }}
      onDelete={() => {}}
    />,
  )

  const disableButton = [...view.container.querySelectorAll('button')]
    .find((button) => button.textContent?.includes('Disable server'))
  click(disableButton ?? null)

  assert.equal(disableCalls, 0)
  assert.match(document.body.textContent ?? '', /Disable server\?/)
  assert.match(document.body.textContent ?? '', /Connected clients should no longer have access/)

  const dialog = document.body.querySelector('[data-slot="alert-dialog-content"]')
  assert.ok(dialog)
  const confirmButton = [...dialog.querySelectorAll('button')]
    .find((button) => button.textContent?.trim() === 'Disable server')
  click(confirmButton ?? null)

  assert.equal(disableCalls, 1)
  await view.unmount()
})

test('gateway table does not re-enable a server that changes state while disable confirmation is open', async () => {
  installDom()
  const { GatewayTable } = await import('./gateway-table')
  let toggleCalls = 0
  const props = {
    density: 'comfortable' as const,
    onEdit: () => {},
    onTest: () => {},
    onReload: () => {},
    onCleanup: () => {},
    onClearCleanupHistory: () => {},
    onToggleEnabled: () => {
      toggleCalls += 1
    },
    onDelete: () => {},
  }
  const view = await renderClient(<GatewayTable gateways={[gateway]} {...props} />)

  const disableButton = [...view.container.querySelectorAll('button')]
    .find((button) => button.textContent?.includes('Disable server'))
  click(disableButton ?? null)

  await view.rerender(<GatewayTable gateways={[{ ...gateway, enabled: false }]} {...props} />)

  const dialog = document.body.querySelector('[data-slot="alert-dialog-content"]')
  assert.ok(dialog)
  const confirmButton = [...dialog.querySelectorAll('button')]
    .find((button) => button.textContent?.trim() === 'Disable server')
  click(confirmButton ?? null)

  assert.equal(toggleCalls, 0)
  await view.unmount()
})
