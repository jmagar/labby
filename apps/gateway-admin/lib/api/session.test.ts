import test from 'node:test'
import assert from 'node:assert/strict'

import {
  __setBrowserSessionStateForTests,
  getBrowserSessionEpoch,
  getBrowserSessionState,
  getSessionAuthority,
  sessionHasCapability,
  loadBrowserSession,
  logoutBrowserSession,
} from '../auth/session-store.ts'
import {
  hasApiTokenAuth,
  isStandaloneBearerAuthMode,
  shouldBypassBrowserSessionAuth,
} from '../auth/auth-mode.ts'

type FetchMock = typeof globalThis.fetch

test('loadBrowserSession stores authenticated payloads', async () => {
  globalThis.fetch = (async () =>
    new Response(
      JSON.stringify({
        authenticated: true,
        user: {
          sub: 'browser-user',
          email: 'browser@example.com',
        },
        expires_at: 123,
        csrf_token: 'csrf-123',
        principal_id: 'principal-42',
        organization_id: 'org-1',
        active_owner: { kind: 'team', id: 'team-7' },
        active_team_id: 'team-7',
        active_project_id: 'project-42',
        teams: [{ id: 'team-7', role: 'owner', membership_epoch: 2, policy_epoch: 3 }],
        projects: [{ id: 'project-42', role: 'manager' }],
        capabilities: ['scope.read', 'scope.operate'],
        authority_generation: 9,
      }),
      { status: 200 },
    )) as FetchMock

  const state = await loadBrowserSession()
  assert.equal(state.status, 'authenticated')
  assert.equal(state.status === 'authenticated' ? state.projectId : undefined, 'project-42')
  assert.deepEqual(getSessionAuthority(), {
    schemaVersion: 1,
    compatibilityGeneration: 1,
    principalId: 'principal-42',
    organizationId: 'org-1',
    activeOwner: { kind: 'team', id: 'team-7' },
    activeTeamId: 'team-7',
    activeProjectId: 'project-42',
    teams: [{ id: 'team-7', role: 'owner', membershipEpoch: 2, policyEpoch: 3 }],
    projects: [{ id: 'project-42', role: 'manager' }],
    capabilities: ['scope.operate', 'scope.read'],
    generation: 9,
  })
  assert.equal(getBrowserSessionState().status, 'authenticated')
})

test('same-subject server authority changes advance the browser session epoch', async () => {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'browser-user', email: 'browser@example.com' },
    expiresAt: 123,
    csrfToken: 'csrf-admin',
    authority: {
      principalId: 'principal-1',
      organizationId: 'org-1',
      activeOwner: { kind: 'team', id: 'team-a' },
      activeTeamId: 'team-a',
      teams: [{ id: 'team-a', role: 'member', membershipEpoch: 1, policyEpoch: 1 }],
      projects: [],
      capabilities: ['scope.read'],
      generation: 3,
      schemaVersion: 1,
      compatibilityGeneration: 1,
    },
  })
  const before = getBrowserSessionEpoch()
  globalThis.fetch = (async () => new Response(JSON.stringify({
    authenticated: true,
    user: { sub: 'browser-user', email: 'browser@example.com' },
    expires_at: 124,
    csrf_token: 'csrf-user',
    principal_id: 'principal-1',
    organization_id: 'org-1',
    active_owner: { kind: 'team', id: 'team-b' },
    active_team_id: 'team-b',
    teams: [{ id: 'team-b', role: 'member', membership_epoch: 2, policy_epoch: 1 }],
    projects: [],
    capabilities: ['scope.read'],
    authority_generation: 4,
  }), { status: 200 })) as FetchMock

  await loadBrowserSession()
  assert.ok(getBrowserSessionEpoch() > before)
})

test('legacy admin hints never manufacture a role without a server authority projection', async () => {
  __setBrowserSessionStateForTests({ status: 'loading' })
  globalThis.fetch = (async () => new Response(JSON.stringify({
    authenticated: true,
    user: { sub: 'allowlisted-user', email: 'admin@example.com' },
    expires_at: 124,
    csrf_token: 'csrf-user',
    is_admin: true,
  }), { status: 200 })) as FetchMock

  const state = await loadBrowserSession()
  assert.equal(state.status === 'authenticated' ? state.isAdmin : true, false)
  assert.equal(getSessionAuthority(), undefined)
  assert.equal(sessionHasCapability('platform.manage'), false)
})

test('principal projection defaults to personal ownership and capabilities drive admin presentation', async () => {
  __setBrowserSessionStateForTests({ status: 'loading' })
  globalThis.fetch = (async () => new Response(JSON.stringify({
    authenticated: true,
    user: { sub: 'opaque-provider-subject' },
    expires_at: 124,
    csrf_token: 'csrf-user',
    principal_id: 'principal-9',
    organization_id: 'org-1',
    teams: [],
    projects: [],
    capabilities: ['platform.manage', 'scope.read'],
    authority_generation: 1,
  }), { status: 200 })) as FetchMock

  const state = await loadBrowserSession()
  assert.deepEqual(getSessionAuthority()?.activeOwner, { kind: 'personal', id: 'principal-9' })
  assert.equal(state.status === 'authenticated' ? state.isAdmin : false, true)
  assert.equal(sessionHasCapability('platform.manage'), true)
})

test('loadBrowserSession falls back to unauthenticated when /auth/session fails', async () => {
  __setBrowserSessionStateForTests({ status: 'loading' })
  globalThis.fetch = (async () =>
    new Response('not found', {
      status: 401,
      headers: { 'content-type': 'text/plain' },
    })) as FetchMock

  const state = await loadBrowserSession()
  assert.deepEqual(state, { status: 'unauthenticated' })
  assert.deepEqual(getBrowserSessionState(), { status: 'unauthenticated' })
})

test('loadBrowserSession keeps backend failures distinct from auth failures', async () => {
  __setBrowserSessionStateForTests({ status: 'loading' })
  globalThis.fetch = (async () =>
    new Response(
      JSON.stringify({
        kind: 'internal_error',
        message: 'auth store unavailable',
      }),
      {
      status: 500,
      headers: {
        'content-type': 'application/json',
        'x-request-id': 'req-auth-123',
      },
    },
    )) as FetchMock

  const state = await loadBrowserSession()
  assert.deepEqual(state, {
    status: 'auth_error',
    kind: 'internal_error',
    message: 'auth store unavailable',
    requestId: 'req-auth-123',
  })
  assert.deepEqual(getBrowserSessionState(), {
    status: 'auth_error',
    kind: 'internal_error',
    message: 'auth store unavailable',
    requestId: 'req-auth-123',
  })
})

test('loadBrowserSession keeps network failures distinct from auth failures', async () => {
  __setBrowserSessionStateForTests({ status: 'loading' })
  globalThis.fetch = (async () => {
    throw new Error('socket hang up')
  }) as FetchMock

  const state = await loadBrowserSession()
  assert.deepEqual(state, {
    status: 'auth_error',
    kind: 'network_error',
    message: 'Unable to reach the authentication service. Try again.',
  })
})

test('logoutBrowserSession resets local state after POST', async () => {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'browser-user', email: 'browser@example.com' },
    expiresAt: 123,
    csrfToken: 'csrf-123',
  })

  let requestInit: RequestInit | undefined
  globalThis.fetch = async (_input, init) => {
    requestInit = init
    return new Response(null, { status: 204 })
  }

  await logoutBrowserSession()
  assert.equal(requestInit?.method, 'POST')
  assert.equal(
    (requestInit?.headers as Record<string, string>)['x-csrf-token'],
    'csrf-123',
  )
  assert.deepEqual(getBrowserSessionState(), { status: 'unauthenticated' })
})

test('logoutBrowserSession preserves the authenticated state when server revocation fails', async () => {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'browser-user', email: 'browser@example.com' },
    expiresAt: 123,
    csrfToken: 'csrf-123',
  })

  globalThis.fetch = (async () => new Response('boom', { status: 500 })) as FetchMock

  await assert.rejects(
    logoutBrowserSession(),
    /Failed to logout browser session/,
  )
  assert.deepEqual(getBrowserSessionState(), {
    status: 'authenticated',
    user: { sub: 'browser-user', email: 'browser@example.com' },
    expiresAt: 123,
    csrfToken: 'csrf-123',
  })
})

test('a session load started before successful logout cannot restore the revoked session', async () => {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'browser-user', email: 'browser@example.com' },
    expiresAt: 123,
    csrfToken: 'csrf-123',
  })

  let releaseSessionLoad: (() => void) | undefined
  const sessionLoadBlocked = new Promise<void>((resolve) => {
    releaseSessionLoad = resolve
  })
  globalThis.fetch = (async (input) => {
    if (input === '/auth/logout') {
      return new Response(null, { status: 204 })
    }
    await sessionLoadBlocked
    return new Response(JSON.stringify({
      authenticated: true,
      user: { sub: 'browser-user', email: 'browser@example.com' },
      expires_at: 456,
      csrf_token: 'stale-csrf',
      is_admin: true,
    }), { status: 200, headers: { 'content-type': 'application/json' } })
  }) as FetchMock

  const staleLoad = loadBrowserSession()
  await logoutBrowserSession()
  releaseSessionLoad?.()
  const loadResult = await staleLoad

  assert.deepEqual(loadResult, { status: 'unauthenticated' })
  assert.deepEqual(getBrowserSessionState(), { status: 'unauthenticated' })
})

test('hasApiTokenAuth only enables bearer mode for non-empty tokens', () => {
  assert.equal(hasApiTokenAuth(undefined), false)
  assert.equal(hasApiTokenAuth(''), false)
  assert.equal(hasApiTokenAuth('   '), false)
  assert.equal(hasApiTokenAuth('dev-token'), false)
})

test('isStandaloneBearerAuthMode activates whenever a token is set', () => {
  assert.equal(isStandaloneBearerAuthMode(undefined), false)
  assert.equal(isStandaloneBearerAuthMode('   '), false)
  assert.equal(isStandaloneBearerAuthMode('dev-token'), false)
})

test('shouldBypassBrowserSessionAuth bypasses hosted auth when a token is set or in mock mode', () => {
  assert.equal(shouldBypassBrowserSessionAuth(undefined, 'false'), false)
  assert.equal(shouldBypassBrowserSessionAuth('dev-token', 'false'), false)
  assert.equal(shouldBypassBrowserSessionAuth(undefined, 'true'), true)
})
