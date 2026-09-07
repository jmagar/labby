import test from 'node:test'
import assert from 'node:assert/strict'

import { authorityCacheKey, parseAuthoritySnapshot, selectAuthorityWorkspace } from './authority.ts'
import { __authorityContextStatsForTests, __resetAuthorityContextForTests, beginAuthorityRequest, invalidateAuthorityRequests, qualifyAuthorityCacheKey } from './authority-context.ts'

const payload = {
  owner: { kind: 'personal', id: 'principal-1' },
  organization_id: 'org-1',
  teams: [{ id: 'team-a', role: 'owner', membership_epoch: 2, policy_epoch: 4 }],
  projects: [{ id: 'project-a', role: 'manager' }],
  capabilities: ['scope.read', 'scope.read', 'scope.operate'],
  authority_generation: 7,
}

test('parses server authority and produces an opaque context-qualified cache key', () => {
  const snapshot = parseAuthoritySnapshot(payload)
  assert.equal(snapshot.principalId, 'principal-1')
  assert.deepEqual(snapshot.capabilities, ['scope.operate', 'scope.read'])
  const key = authorityCacheKey(snapshot, 'depot-east')
  assert.equal(key.includes('principal-1'), false)
  assert.deepEqual(qualifyAuthorityCacheKey('/skills', key).slice(0, 2), ['/skills', 'authority'])
})

test('workspace selectors only accept server-projected teams and projects', () => {
  const snapshot = parseAuthoritySnapshot(payload)
  assert.equal(selectAuthorityWorkspace(snapshot, { teamId: 'team-a' }).activeOwner.kind, 'team')
  assert.equal(selectAuthorityWorkspace(snapshot, { teamId: 'team-a', projectId: 'project-a' }).activeOwner.kind, 'project')
  assert.equal(selectAuthorityWorkspace(snapshot, {}).activeOwner.kind, 'personal')
  assert.throws(() => selectAuthorityWorkspace(snapshot, { teamId: 'team-b' }), /not available/)
  assert.throws(() => selectAuthorityWorkspace(snapshot, { projectId: 'project-b' }), /not available/)
})

test('malformed authority projections are rejected instead of partially trusted', () => {
  for (const broken of [
    { ...payload, organization_id: '' },
    { ...payload, authority_generation: -1 },
    { ...payload, capabilities: ['scope.read', 7] },
    { ...payload, teams: [{ id: 'team-a', role: 'owner' }] },
    { ...payload, owner: { kind: 'unknown', id: 'x' } },
  ]) assert.throws(() => parseAuthoritySnapshot(broken), /Malformed authority response/)
})

test('context invalidation aborts old requests but preserves the current generation', () => {
  __resetAuthorityContextForTests()
  const snapshot = parseAuthoritySnapshot(payload)
  const old = beginAuthorityRequest(snapshot, 10)
  const current = beginAuthorityRequest(snapshot, 11)
  invalidateAuthorityRequests(11)
  assert.equal(old.signal.aborted, true)
  assert.equal(current.signal.aborted, false)
  old.finish()
  current.finish()
})

test('authority context retains only a bounded generation history', () => {
  __resetAuthorityContextForTests()
  for (let generation = 1; generation <= 20; generation += 1) invalidateAuthorityRequests(generation)
  assert.equal(__authorityContextStatsForTests().retainedGenerations, 3)
})
