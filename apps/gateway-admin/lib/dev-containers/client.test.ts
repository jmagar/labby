import assert from 'node:assert/strict'
import test from 'node:test'
import { __setBrowserSessionStateForTests } from '../auth/session-store.ts'
import { createDevContainer, listDevContainers } from './client.ts'

const authority = { schemaVersion: 1, compatibilityGeneration: 1, principalId: 'principal-1', organizationId: 'org-1', activeOwner: { kind: 'team' as const, id: 'team-1' }, activeTeamId: 'team-1', teams: [{ id: 'team-1', role: 'admin', membershipEpoch: 1, policyEpoch: 1 }], projects: [], capabilities: ['scope.read', 'scope.create'], generation: 7 } as const
test('Dev Container requests use selected authoritative owner and transport headers', async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'subject' }, expiresAt: 1, csrfToken: 'csrf', authority })
  const original = globalThis.fetch; const calls: Array<{ body: Record<string, unknown>; headers: Headers }> = []
  globalThis.fetch = async (_input, init) => { calls.push({ body: JSON.parse(String(init?.body)), headers: new Headers(init?.headers) }); return new Response(JSON.stringify(calls.length === 1 ? { instances: [] } : { instance_id: 'dev-1' }), { status: 200 }) }
  try { await listDevContainers(); await createDevContainer('dev-1', 'rust') } finally { globalThis.fetch = original }
  assert.equal(calls[0].headers.get('x-labby-team-id'), 'team-1')
  assert.deepEqual(calls[1].body, { action: 'dev_containers.create', params: { instance_id: 'dev-1', template_id: 'rust', owner_kind: 'team', owner_id: 'team-1' } })
})
