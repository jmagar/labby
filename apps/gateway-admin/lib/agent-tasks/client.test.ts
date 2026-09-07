import assert from 'node:assert/strict'
import test from 'node:test'

import { listAgents, listTasks } from './client.ts'
import { __setBrowserSessionStateForTests } from '../auth/session-store.ts'

const authority = { schemaVersion: 1, compatibilityGeneration: 1, principalId: 'principal-1', organizationId: 'org-1', activeOwner: { kind: 'team' as const, id: 'team-1' }, activeTeamId: 'team-1', teams: [{ id: 'team-1', role: 'member', membershipEpoch: 1, policyEpoch: 1 }], projects: [], capabilities: ['scope.read'], generation: 1 } as const
function authenticate() { __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'principal-1' }, expiresAt: Date.now() + 10_000, csrfToken: 'csrf', authority }) }

test('agent and task lists use authenticated authoritative action endpoints', async () => {
  authenticate()
  const requests: Request[] = []
  globalThis.fetch = async (input, init) => {
    const request = new Request(new URL(String(input), 'http://labby.test'), init)
    requests.push(request)
    return Response.json(request.url.includes('/agents/') ? { agents: [{ agent_id: 'a-1' }] } : { tasks: [{ task_id: 't-1' }] })
  }
  assert.equal((await listAgents())[0]?.agent_id, 'a-1')
  assert.equal((await listTasks())[0]?.task_id, 't-1')
  assert.deepEqual(requests.map(request => new URL(request.url).pathname), ['/v1/agents/', '/v1/tasks/'])
  assert.ok(requests.every(request => request.method === 'POST' && request.credentials === 'include'))
  assert.deepEqual(JSON.parse(await requests[0]!.text()), { action: 'agents.list', params: {} })
  assert.deepEqual(JSON.parse(await requests[1]!.text()), { action: 'tasks.list', params: {} })
})

test('denials do not become empty authoritative lists', async () => {
  authenticate()
  globalThis.fetch = async () => Response.json({ message: 'access denied' }, { status: 403 })
  await assert.rejects(listAgents(), /failed \(403\)/)
  await assert.rejects(listTasks(), /failed \(403\)/)
})
