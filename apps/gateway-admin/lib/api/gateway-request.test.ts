import test from 'node:test'
import assert from 'node:assert/strict'

import { __setBrowserSessionStateForTests } from '../auth/session-store.ts'
import { captureGatewayAuthority, confirmGatewayParams, gatewayHeaders, gatewayRequestInit } from './gateway-request.ts'

test('gatewayRequestInit never sends bearer headers even if legacy bearer inputs are supplied', () => {
  const init = gatewayRequestInit('gateway.list', {}, 'dev-token', undefined, true)

  assert.equal(init.credentials, 'include')
  assert.equal((init.headers as Record<string, string>).Authorization, undefined)
  assert.equal(init.method, 'POST')
})

test('gatewayRequestInit keeps credentialed requests when a token is present without standalone bearer mode', () => {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'browser-user', email: 'browser@example.com' },
    expiresAt: 42,
    csrfToken: 'csrf-123',
  })
  const init = gatewayRequestInit('gateway.list', {}, 'dev-token', undefined, false)

  assert.equal(init.credentials, 'include')
  assert.equal((init.headers as Record<string, string>).Authorization, undefined)
  assert.equal((init.headers as Record<string, string>)['x-csrf-token'], 'csrf-123')
})

test('gatewayHeaders omits authorization when no token is provided', () => {
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  const headers = gatewayHeaders(undefined) as Record<string, string>

  assert.equal(headers['Content-Type'], 'application/json')
  assert.equal('Authorization' in headers, false)
})

test('gatewayRequestInit keeps credentialed requests for session-auth setups', () => {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'browser-user', email: 'browser@example.com' },
    expiresAt: 42,
    csrfToken: 'csrf-123',
  })
  const init = gatewayRequestInit('gateway.list', {}, undefined)

  assert.equal(init.credentials, 'include')
  assert.equal((init.headers as Record<string, string>)['Content-Type'], 'application/json')
  assert.equal((init.headers as Record<string, string>)['x-csrf-token'], 'csrf-123')
})

test('gatewayRequestInit forwards the project bound to the browser session', () => {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'browser-user', email: 'browser@example.com' },
    expiresAt: 42,
    csrfToken: 'csrf-123',
    projectId: 'project-42',
  })

  const init = gatewayRequestInit('artifacts.list', {})
  const headers = init.headers as Record<string, string>

  assert.equal(headers['x-labby-project-id'], 'project-42')
  assert.equal(headers['x-csrf-token'], 'csrf-123')
})

test('gateway headers and captured requests are qualified by explicit workspace context', () => {
  __setBrowserSessionStateForTests({
    status: 'authenticated', user: { sub: 'provider-sub' }, expiresAt: 42, csrfToken: 'csrf',
    authority: {
      schemaVersion: 1, compatibilityGeneration: 1, principalId: 'principal-1', organizationId: 'org-1',
      activeOwner: { kind: 'project', id: 'project-1' }, activeTeamId: 'team-1', activeProjectId: 'project-1',
      teams: [{ id: 'team-1', role: 'owner', membershipEpoch: 1, policyEpoch: 1 }], projects: [{ id: 'project-1', role: 'manager' }],
      capabilities: ['scope.read'], generation: 8,
    }, projectId: 'project-1',
  })
  const headers = gatewayHeaders() as Record<string, string>
  assert.equal(headers['x-labby-team-id'], 'team-1')
  assert.equal(headers['x-labby-project-id'], 'project-1')
  const captured = captureGatewayAuthority()
  assert.equal(captured.cacheKey.includes('principal-1'), false)
  captured.finish()
})

test('confirmGatewayParams marks destructive gateway mutations for explicit confirmation', () => {
  assert.deepEqual(confirmGatewayParams({ id: 'gateway_beta' }), {
    confirm: true,
    id: 'gateway_beta',
  })
})
