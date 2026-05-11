import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { act } from 'react'
import { SWRConfig } from 'swr'
import { Window } from 'happy-dom'

import type { CreateGatewayInput, Gateway, ProtectedMcpRoute, UpdateGatewayInput } from '@/lib/types/gateway'

test('new custom URL auto-switches to OAuth and shows blocked popup fallback', async () => {
  const window = installGatewayDialogDom()
  let openCalls = 0
  Object.defineProperty(window, 'open', {
    configurable: true,
    value: () => {
      openCalls += 1
      return null
    },
  })

  const originalFetch = globalThis.fetch
  globalThis.fetch = (async (input, init) => {
    const path = String(input)
    if (path === '/v1/gateway' && init?.method === 'POST') {
      return new Response(JSON.stringify([]), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      })
    }
    if (path === '/v1/gateway/oauth/probe') {
      return new Response(JSON.stringify({
        upstream: 'github',
        url: 'https://github.example/mcp',
        oauth_discovered: true,
        issuer: 'https://github.example',
        scopes: ['mcp:read'],
        registration_strategy: 'dynamic',
      }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      })
    }
    if (path === '/v1/gateway/oauth/start') {
      return new Response(JSON.stringify({
        authorization_url: 'https://github.example/oauth/authorize',
      }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      })
    }
    return new Response(JSON.stringify([]), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    })
  }) as typeof fetch

  try {
    const view = await renderOpenGatewayDialog()
    const urlInput = document.querySelector('#url') as HTMLInputElement | null
    assert.ok(urlInput)

    await act(async () => {
      const setValue = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set
      setValue?.call(urlInput, 'https://github.example/mcp')
      urlInput.dispatchEvent(new window.InputEvent('input', { bubbles: true, data: 'https://github.example/mcp' }) as unknown as Event)
      await new Promise((resolve) => setTimeout(resolve, 700))
    })

    await waitFor(() => {
      assert.match(document.body.textContent ?? '', /OAuth \(MCP\)/)
      assert.match(document.body.textContent ?? '', /OAuth detected\. Click to authorize; the browser blocked the automatic popup\./)
      assert.match(document.body.textContent ?? '', /Click to authorize/)
      assert.equal(openCalls, 1)
    })

    await view.unmount()
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('stale delayed OAuth probe cannot auto-connect the previous URL', async () => {
  const window = installGatewayDialogDom()
  const openCalls: string[] = []
  Object.defineProperty(window, 'open', {
    configurable: true,
    value: (url?: string | URL) => {
      openCalls.push(String(url))
      return null
    },
  })

  const originalFetch = globalThis.fetch
  let firstProbeResolve: ((response: Response) => void) | null = null
  globalThis.fetch = (async (input, init) => {
    const path = String(input)
    if (path === '/v1/gateway' && init?.method === 'POST') {
      return gatewayActionResponse(init, {})
    }
    if (path === '/v1/gateway/oauth/probe') {
      const body = JSON.parse(String(init?.body ?? '{}')) as { url: string }
      if (body.url === 'https://old.example/mcp') {
        return new Promise<Response>((resolve) => {
          firstProbeResolve = resolve
        })
      }
      return jsonResponse({
        upstream: 'new',
        url: body.url,
        oauth_discovered: false,
      })
    }
    throw new Error(`unexpected fetch ${path}`)
  }) as typeof fetch

  try {
    const view = await renderOpenGatewayDialog()
    const urlInput = document.querySelector('#url') as HTMLInputElement | null
    assert.ok(urlInput)

    await setInputValue(window, urlInput, 'https://old.example/mcp')
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 650))
    })
    await setInputValue(window, urlInput, 'https://new.example/mcp')
    await act(async () => {
      firstProbeResolve?.(jsonResponse({
        upstream: 'old',
        url: 'https://old.example/mcp',
        oauth_discovered: true,
        issuer: 'https://old.example',
      }))
      await new Promise((resolve) => setTimeout(resolve, 700))
    })

    await waitFor(() => {
      assert.doesNotMatch(document.body.textContent ?? '', /OAuth \(MCP\)/)
      assert.deepEqual(openCalls, [])
    })

    await view.unmount()
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('auto OAuth opens authorization URL when popups are allowed', async () => {
  const window = installGatewayDialogDom()
  const openCalls: string[] = []
  Object.defineProperty(window, 'open', {
    configurable: true,
    value: (url?: string | URL) => {
      openCalls.push(String(url))
      return { closed: false, location: { href: '' }, close: () => {} }
    },
  })

  const originalFetch = globalThis.fetch
  globalThis.fetch = (async (input, init) => {
    const path = String(input)
    if (path === '/v1/gateway' && init?.method === 'POST') {
      return gatewayActionResponse(init, {})
    }
    if (path === '/v1/gateway/oauth/probe') {
      return jsonResponse({
        upstream: 'github',
        url: 'https://github.example/mcp',
        oauth_discovered: true,
        issuer: 'https://github.example',
        scopes: ['mcp:read'],
        registration_strategy: 'dynamic',
      })
    }
    if (path === '/v1/gateway/oauth/start') {
      return jsonResponse({ authorization_url: 'https://github.example/oauth/authorize' })
    }
    throw new Error(`unexpected fetch ${path}`)
  }) as typeof fetch

  try {
    const view = await renderOpenGatewayDialog()
    const urlInput = document.querySelector('#url') as HTMLInputElement | null
    assert.ok(urlInput)

    await setInputValue(window, urlInput, 'https://github.example/mcp')

    await waitFor(() => {
      assert.deepEqual(openCalls, ['https://github.example/oauth/authorize'])
      assert.match(document.body.textContent ?? '', /Waiting\.\.\./)
    })

    await view.unmount()
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('blocked OAuth popup can be retried from a user click', async () => {
  const window = installGatewayDialogDom()
  const tabs: Array<{ closed: boolean; location: { href: string }; close: () => void }> = []
  let openCalls = 0
  Object.defineProperty(window, 'open', {
    configurable: true,
    value: (url?: string | URL) => {
      openCalls += 1
      if (openCalls === 1) return null
      const tab = { closed: false, location: { href: String(url) }, close: () => {} }
      tabs.push(tab)
      return tab
    },
  })

  const originalFetch = globalThis.fetch
  globalThis.fetch = (async (input, init) => {
    const path = String(input)
    if (path === '/v1/gateway' && init?.method === 'POST') {
      return gatewayActionResponse(init, {})
    }
    if (path === '/v1/gateway/oauth/probe') {
      return jsonResponse({
        upstream: 'github',
        url: 'https://github.example/mcp',
        oauth_discovered: true,
        issuer: 'https://github.example',
        scopes: ['mcp:read'],
        registration_strategy: 'dynamic',
      })
    }
    if (path === '/v1/gateway/oauth/start') {
      return jsonResponse({ authorization_url: 'https://github.example/oauth/authorize' })
    }
    throw new Error(`unexpected fetch ${path}`)
  }) as typeof fetch

  try {
    const view = await renderOpenGatewayDialog()
    const urlInput = document.querySelector('#url') as HTMLInputElement | null
    assert.ok(urlInput)
    await setInputValue(window, urlInput, 'https://github.example/mcp')

    await waitFor(() => {
      assert.match(document.body.textContent ?? '', /Click to authorize/)
    })

    const retry = [...document.querySelectorAll('button')]
      .find((button) => button.textContent?.includes('Click to authorize')) as HTMLButtonElement | undefined
    assert.ok(retry)
    await act(async () => {
      retry.click()
      await new Promise((resolve) => setTimeout(resolve, 0))
    })

    await waitFor(() => {
      assert.equal(openCalls, 2)
      assert.equal(tabs[0]?.location.href, 'https://github.example/oauth/authorize')
      assert.match(document.body.textContent ?? '', /Waiting\.\.\./)
    })

    await view.unmount()
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('saving a changed protected route updates before deleting the new route', async () => {
  const window = installGatewayDialogDom()
  const actions: string[] = []
  const originalFetch = globalThis.fetch
  globalThis.fetch = (async (input, init) => {
    const path = String(input)
    if (path === '/v1/gateway' && init?.method === 'POST') {
      return gatewayActionResponse(init, {
        protectedRoutes: [protectedRouteFixture('old-route', '/old', 'tools')],
        onAction: (action) => actions.push(action),
      })
    }
    throw new Error(`unexpected fetch ${path}`)
  }) as typeof fetch

  try {
    const onSaveInputs: unknown[] = []
    const view = await renderOpenGatewayDialog(gatewayFixture('tools'), async (input) => {
      onSaveInputs.push(input)
    })

    const pathInput = document.querySelector('#protected-public-path') as HTMLInputElement | null
    assert.ok(pathInput)
    await waitFor(() => assert.equal(pathInput.value, 'old'))
    await setInputValue(window, pathInput, 'new')

    await clickSave()

    await waitFor(() => {
      assert.deepEqual(onSaveInputs.map(() => 'save'), ['save'])
      assert.deepEqual(actions.filter((action) => action.startsWith('gateway.protected_route.')), [
        'gateway.protected_route.list',
        'gateway.protected_route.update',
      ])
    })

    await view.unmount()
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('clearing a protected route removes the existing route after saving', async () => {
  const window = installGatewayDialogDom()
  const actions: string[] = []
  const originalFetch = globalThis.fetch
  globalThis.fetch = (async (input, init) => {
    const path = String(input)
    if (path === '/v1/gateway' && init?.method === 'POST') {
      return gatewayActionResponse(init, {
        protectedRoutes: [protectedRouteFixture('old-route', '/old', 'tools')],
        onAction: (action) => actions.push(action),
      })
    }
    throw new Error(`unexpected fetch ${path}`)
  }) as typeof fetch

  try {
    const onSaveInputs: unknown[] = []
    const view = await renderOpenGatewayDialog(gatewayFixture('tools'), async (input) => {
      onSaveInputs.push(input)
    })

    const pathInput = document.querySelector('#protected-public-path') as HTMLInputElement | null
    assert.ok(pathInput)
    await waitFor(() => assert.equal(pathInput.value, 'old'))
    await setInputValue(window, pathInput, '')

    await clickSave()

    await waitFor(() => {
      assert.deepEqual(onSaveInputs.map(() => 'save'), ['save'])
      assert.deepEqual(actions.filter((action) => action.startsWith('gateway.protected_route.')), [
        'gateway.protected_route.list',
        'gateway.protected_route.remove',
      ])
    })

    await view.unmount()
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('shouldAutoConnectOauth only allows new HTTP no-auth OAuth discoveries', async () => {
  const { shouldAutoConnectOauth } = await import('./gateway-form-dialog')

  assert.equal(shouldAutoConnectOauth({
    open: true,
    isEditing: false,
    transport: 'http',
    authMode: 'none',
    oauthDiscovered: true,
    upstream: 'github',
  }), true)

  assert.equal(shouldAutoConnectOauth({
    open: true,
    isEditing: false,
    transport: 'http',
    authMode: 'bearer',
    oauthDiscovered: true,
    upstream: 'github',
  }), false)

  assert.equal(shouldAutoConnectOauth({
    open: true,
    isEditing: true,
    transport: 'http',
    authMode: 'none',
    oauthDiscovered: true,
    upstream: 'github',
  }), false)

  assert.equal(shouldAutoConnectOauth({
    open: true,
    isEditing: false,
    transport: 'stdio',
    authMode: 'none',
    oauthDiscovered: true,
    upstream: 'github',
  }), false)

  assert.equal(shouldAutoConnectOauth({
    open: true,
    isEditing: false,
    transport: 'http',
    authMode: 'none',
    oauthDiscovered: true,
    upstream: '',
  }), false)
})

test('oauthConnectButtonLabel exposes blocked popup recovery copy', async () => {
  const { oauthConnectButtonLabel } = await import('./gateway-form-dialog')

  assert.equal(oauthConnectButtonLabel({ kind: 'blocked', upstream: 'github' }), 'Click to authorize')
  assert.equal(oauthConnectButtonLabel({ kind: 'probing' }), 'Detecting OAuth...')
  assert.equal(oauthConnectButtonLabel({ kind: 'authorizing', upstream: 'github' }), 'Waiting...')
  assert.equal(oauthConnectButtonLabel({ kind: 'idle' }), 'Connect via OAuth')
})

test('manual bearer mode prevents OAuth auto-connect even when probe discovers OAuth', async () => {
  const { shouldAutoConnectOauth } = await import('./gateway-form-dialog')

  assert.equal(shouldAutoConnectOauth({
    open: true,
    isEditing: false,
    transport: 'http',
    authMode: 'bearer',
    oauthDiscovered: true,
    upstream: 'github',
  }), false)
})

test('editing an existing gateway prevents OAuth auto-connect', async () => {
  const { shouldAutoConnectOauth } = await import('./gateway-form-dialog')

  assert.equal(shouldAutoConnectOauth({
    open: true,
    isEditing: true,
    transport: 'http',
    authMode: 'none',
    oauthDiscovered: true,
    upstream: 'github',
  }), false)
})

function installGatewayDialogDom() {
  const window = new Window()
  Object.defineProperty(globalThis, 'window', { configurable: true, value: window })
  Object.defineProperty(globalThis, 'document', { configurable: true, value: window.document })
  Object.defineProperty(globalThis, 'navigator', { configurable: true, value: window.navigator })
  Object.defineProperty(globalThis, 'DOMException', { configurable: true, value: window.DOMException })
  Object.defineProperty(globalThis, 'Node', { configurable: true, value: window.Node })
  Object.defineProperty(globalThis, 'NodeFilter', { configurable: true, value: window.NodeFilter })
  Object.defineProperty(globalThis, 'MouseEvent', { configurable: true, value: window.MouseEvent })
  Object.defineProperty(globalThis, 'PointerEvent', { configurable: true, value: window.PointerEvent })
  Object.defineProperty(globalThis, 'KeyboardEvent', { configurable: true, value: window.KeyboardEvent })
  Object.defineProperty(globalThis, 'Event', { configurable: true, value: window.Event })
  Object.defineProperty(globalThis, 'CustomEvent', { configurable: true, value: window.CustomEvent })
  Object.defineProperty(globalThis, 'InputEvent', { configurable: true, value: window.InputEvent })
  Object.defineProperty(globalThis, 'HTMLElement', { configurable: true, value: window.HTMLElement })
  Object.defineProperty(globalThis, 'HTMLInputElement', { configurable: true, value: window.HTMLInputElement })
  Object.defineProperty(globalThis, 'Element', { configurable: true, value: window.Element })
  Object.defineProperty(globalThis, 'DocumentFragment', { configurable: true, value: window.DocumentFragment })
  Object.defineProperty(globalThis, 'MutationObserver', { configurable: true, value: window.MutationObserver })
  Object.defineProperty(globalThis, 'getComputedStyle', { configurable: true, value: window.getComputedStyle.bind(window) })
  Object.defineProperty(globalThis, 'requestAnimationFrame', {
    configurable: true,
    value: (callback: FrameRequestCallback) => window.setTimeout(() => callback(Date.now()), 0),
  })
  Object.defineProperty(globalThis, 'cancelAnimationFrame', {
    configurable: true,
    value: (handle: ReturnType<typeof window.setTimeout>) => window.clearTimeout(handle),
  })
  Object.defineProperty(globalThis, 'IS_REACT_ACT_ENVIRONMENT', { configurable: true, value: true })
  return window
}

async function renderDialog(element: React.ReactElement) {
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

async function renderOpenGatewayDialog(
  gateway: Gateway | null = null,
  onSave: (input: CreateGatewayInput | UpdateGatewayInput) => Promise<void> = async () => {},
) {
  const { GatewayFormDialog } = await import('./gateway-form-dialog')

  return renderDialog(
    <SWRConfig value={{ provider: () => new Map(), dedupingInterval: 0 }}>
      <GatewayFormDialog
        open
        onOpenChange={() => {}}
        gateway={gateway}
        onSave={onSave}
      />
    </SWRConfig>,
  )
}

async function setInputValue(window: Window, input: HTMLInputElement, value: string) {
  await act(async () => {
    const setValue = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set
    setValue?.call(input, value)
    input.dispatchEvent(new window.InputEvent('input', { bubbles: true, data: value }) as unknown as Event)
    await new Promise((resolve) => setTimeout(resolve, 0))
  })
}

async function clickSave() {
  const save = [...document.querySelectorAll('button')]
    .find((button) => /^(Add Server|Save Changes)$/.test(button.textContent?.trim() ?? '')) as HTMLButtonElement | undefined
  assert.ok(save)
  await act(async () => {
    save.click()
    await new Promise((resolve) => setTimeout(resolve, 0))
  })
}

function jsonResponse(value: unknown, status = 200) {
  return new Response(JSON.stringify(value), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

function gatewayActionResponse(
  init: RequestInit | undefined,
  options: {
    protectedRoutes?: ProtectedMcpRoute[]
    onAction?: (action: string, params: Record<string, unknown>) => void
  },
) {
  const body = JSON.parse(String(init?.body ?? '{}')) as { action?: string; params?: Record<string, unknown> }
  const action = body.action ?? ''
  const params = body.params ?? {}
  options.onAction?.(action, params)

  switch (action) {
    case 'gateway.supported_services':
      return jsonResponse([])
    case 'gateway.protected_route.list':
      return jsonResponse(options.protectedRoutes ?? [])
    case 'gateway.protected_route.add':
    case 'gateway.protected_route.update':
      return jsonResponse(params.route ?? {})
    case 'gateway.protected_route.remove': {
      const removed = (options.protectedRoutes ?? []).find((route) => route.name === params.name)
      return jsonResponse(removed ?? {})
    }
    default:
      return jsonResponse([])
  }
}

function gatewayFixture(name: string): Gateway {
  return {
    id: name,
    name,
    transport: 'http',
    source: 'custom_gateway',
    configured: true,
    enabled: true,
    config: {
      url: `https://${name}.example/mcp`,
      proxy_resources: true,
      proxy_prompts: true,
    },
    status: {
      healthy: true,
      connected: true,
      discovered_tool_count: 0,
      exposed_tool_count: 0,
      discovered_resource_count: 0,
      exposed_resource_count: 0,
      discovered_prompt_count: 0,
      exposed_prompt_count: 0,
    },
    discovery: { tools: [], resources: [], prompts: [] },
    warnings: [],
    created_at: '2026-05-11T00:00:00Z',
    updated_at: '2026-05-11T00:00:00Z',
  }
}

function protectedRouteFixture(name: string, publicPath: string, upstream: string): ProtectedMcpRoute {
  return {
    name,
    enabled: true,
    public_host: 'mcp.tootie.tv',
    public_path: publicPath,
    upstream,
    backend_url: '',
    backend_mcp_path: '/mcp',
    scopes: ['mcp:read', 'mcp:write'],
    health_path: null,
  }
}
