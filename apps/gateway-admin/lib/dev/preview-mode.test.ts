import test from 'node:test'
import assert from 'node:assert/strict'

import {
  DevPreviewReadOnlyError,
  assertDevPreviewCanRunAction,
  devPreviewActionUrl,
  isDevPreviewReadOnlyAction,
  isDevPreviewRoute,
} from './preview-mode'

// ──────────────────────────────────────────────────────────────
// isDevPreviewReadOnlyAction
// ──────────────────────────────────────────────────────────────

test('isDevPreviewReadOnlyAction: returns true for every whitelisted action', () => {
  const whitelist = [
    'help',
    'schema',
    'sources.list',
    'plugins.list',
    'plugin.get',
    'plugin.artifacts',
    'plugin.workspace',
    'plugin.components',
    'plugin.deploy.preview',
    'artifact.list',
    'agent.list',
    'agent.get',
    'mcp.config',
    'mcp.list',
    'mcp.get',
    'mcp.versions',
    'mcp.meta.get',
  ]
  for (const action of whitelist) {
    assert.equal(
      isDevPreviewReadOnlyAction(action),
      true,
      `expected ${action} to be whitelisted`,
    )
  }
})

test('isDevPreviewReadOnlyAction: returns false for mutating actions', () => {
  const mutating = [
    'plugin.install',
    'plugin.uninstall',
    'sources.add',
    'sources.remove',
    'plugin.deploy',
    'plugin.workspace.save',
  ]
  for (const action of mutating) {
    assert.equal(
      isDevPreviewReadOnlyAction(action),
      false,
      `expected ${action} to not be whitelisted`,
    )
  }
})

// ──────────────────────────────────────────────────────────────
// isDevPreviewRoute
// ──────────────────────────────────────────────────────────────

test('isDevPreviewRoute: returns false in non-browser environments', () => {
  // window is undefined in Node.js
  assert.equal(isDevPreviewRoute(), false)
})

test('isDevPreviewRoute: returns true for /dev and /dev/* when window is present', () => {
  const original = globalThis.window

  try {
    // Minimal window mock — only location.pathname is needed
    Object.defineProperty(globalThis, 'window', {
      value: { location: { pathname: '/dev' } },
      writable: true,
      configurable: true,
    })
    assert.equal(isDevPreviewRoute(), true)

    globalThis.window = { location: { pathname: '/dev/gateway-policy' } } as Window & typeof globalThis
    assert.equal(isDevPreviewRoute(), true)

    globalThis.window = { location: { pathname: '/dev/gateway-policy' } } as Window & typeof globalThis
    assert.equal(isDevPreviewRoute(), true)
  } finally {
    if (original === undefined) {
      // @ts-expect-error — restoring undefined
      delete globalThis.window
    } else {
      globalThis.window = original
    }
  }
})

test('isDevPreviewRoute: returns false for non-dev routes', () => {
  const original = globalThis.window

  try {
    const nonDevPaths = ['/marketplace', '/gateways', '/devtools', '/developer', '/']
    for (const pathname of nonDevPaths) {
      globalThis.window = { location: { pathname } } as Window & typeof globalThis
      assert.equal(isDevPreviewRoute(), false, `expected false for ${pathname}`)
    }
  } finally {
    if (original === undefined) {
      // @ts-expect-error — restoring undefined
      delete globalThis.window
    } else {
      globalThis.window = original
    }
  }
})

// ──────────────────────────────────────────────────────────────
// assertDevPreviewCanRunAction
// ──────────────────────────────────────────────────────────────

test('assertDevPreviewCanRunAction: does not throw for read-only actions outside /dev', () => {
  // window undefined → not a dev route, nothing should throw
  assert.doesNotThrow(() => assertDevPreviewCanRunAction('plugin.install'))
})

test('assertDevPreviewCanRunAction: does not throw for whitelisted actions in /dev', () => {
  const original = globalThis.window

  try {
    globalThis.window = { location: { pathname: '/dev/gateway-policy' } } as Window & typeof globalThis
    assert.doesNotThrow(() => assertDevPreviewCanRunAction('plugins.list'))
    assert.doesNotThrow(() => assertDevPreviewCanRunAction('plugin.components'))
  } finally {
    if (original === undefined) {
      // @ts-expect-error — restoring undefined
      delete globalThis.window
    } else {
      globalThis.window = original
    }
  }
})

test('assertDevPreviewCanRunAction: throws DevPreviewReadOnlyError for mutating actions in /dev', () => {
  const original = globalThis.window

  try {
    globalThis.window = { location: { pathname: '/dev/gateway-policy' } } as Window & typeof globalThis

    const mutating = ['plugin.install', 'plugin.uninstall', 'sources.add']
    for (const action of mutating) {
      assert.throws(
        () => assertDevPreviewCanRunAction(action),
        (err: unknown) => {
          assert.ok(err instanceof DevPreviewReadOnlyError, 'should be DevPreviewReadOnlyError')
          assert.equal((err as DevPreviewReadOnlyError).status, 403)
          assert.equal((err as DevPreviewReadOnlyError).code, 'dev_preview_read_only')
          assert.ok(
            (err as DevPreviewReadOnlyError).message.includes(action),
            'message should include the blocked action name',
          )
          return true
        },
        `expected ${action} to throw in /dev route`,
      )
    }
  } finally {
    if (original === undefined) {
      // @ts-expect-error — restoring undefined
      delete globalThis.window
    } else {
      globalThis.window = original
    }
  }
})

// ──────────────────────────────────────────────────────────────
// devPreviewActionUrl
// ──────────────────────────────────────────────────────────────

test('devPreviewActionUrl: returns url unchanged outside /dev routes', () => {
  // window undefined → not a dev route
  assert.equal(devPreviewActionUrl('/v1/marketplace'), '/v1/marketplace')
  assert.equal(devPreviewActionUrl('/v1/gateway'), '/v1/gateway')
})

test('devPreviewActionUrl: routes /v1/marketplace to /dev/api/marketplace inside /dev', () => {
  const original = globalThis.window

  try {
    globalThis.window = { location: { pathname: '/dev/gateway-policy' } } as Window & typeof globalThis
    assert.equal(devPreviewActionUrl('/v1/marketplace'), '/dev/api/marketplace')
  } finally {
    if (original === undefined) {
      // @ts-expect-error — restoring undefined
      delete globalThis.window
    } else {
      globalThis.window = original
    }
  }
})

test('devPreviewActionUrl: leaves non-marketplace URLs unchanged inside /dev', () => {
  const original = globalThis.window

  try {
    globalThis.window = { location: { pathname: '/dev/gateway-policy' } } as Window & typeof globalThis
    // Non-marketplace endpoints pass through unchanged
    assert.equal(devPreviewActionUrl('/v1/gateway'), '/v1/gateway')
    assert.equal(devPreviewActionUrl('/v1/acp'), '/v1/acp')
  } finally {
    if (original === undefined) {
      // @ts-expect-error — restoring undefined
      delete globalThis.window
    } else {
      globalThis.window = original
    }
  }
})
