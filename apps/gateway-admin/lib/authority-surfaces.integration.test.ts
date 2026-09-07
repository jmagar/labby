import assert from 'node:assert/strict'
import test from 'node:test'

import { listAgents, listTasks } from './agent-tasks/client.ts'
import { listProjects } from './projects/client.ts'
import { __setBrowserSessionStateForTests, selectSessionWorkspace } from './auth/session-store.ts'

const authority = {
  schemaVersion: 1, compatibilityGeneration: 1, principalId: 'principal-1', organizationId: 'org-1',
  activeOwner: { kind: 'team' as const, id: 'team-owner' }, activeTeamId: 'team-owner',
  teams: [
    { id: 'team-owner', role: 'owner', membershipEpoch: 1, policyEpoch: 1 },
    { id: 'team-admin', role: 'admin', membershipEpoch: 1, policyEpoch: 1 },
    { id: 'team-member', role: 'member', membershipEpoch: 1, policyEpoch: 1 },
  ], projects: [], capabilities: ['scope.read'], generation: 7,
} as const

function authenticate() {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'principal-1' }, expiresAt: Date.now() + 10_000, csrfToken: 'csrf', authority })
}

test('project results preserve server-derived lifecycle authority for platform, owner, admin, and member rows', async () => {
  authenticate()
  globalThis.fetch = async () => Response.json([
    { project_id: 'platform', team_id: 'team-owner', can_manage: true },
    { project_id: 'owner', team_id: 'team-owner', can_manage: true },
    { project_id: 'admin', team_id: 'team-admin', can_manage: true },
    { project_id: 'member', team_id: 'team-member', can_manage: false },
  ])
  const rows = await listProjects()
  assert.deepEqual(rows.map(row => [row.project_id, row.can_manage]), [['platform', true], ['owner', true], ['admin', true], ['member', false]])
})

test('personal and outsider denials remain errors across project, agent, and task surfaces', async () => {
  authenticate()
  globalThis.fetch = async () => Response.json({ kind: 'not_found' }, { status: 404 })
  await assert.rejects(listProjects(), /failed \(404\)/)
  await assert.rejects(listAgents(), /failed \(404\)/)
  await assert.rejects(listTasks(), /failed \(404\)/)
})

test('workspace switching aborts stale project, agent, and task requests', async () => {
  for (const load of [listProjects, listAgents, listTasks]) {
    authenticate()
    globalThis.fetch = async (_input, init) => new Promise<Response>((_resolve, reject) => {
      const signal = init?.signal
      signal?.addEventListener('abort', () => reject(signal.reason), { once: true })
    })
    const pending = load()
    selectSessionWorkspace({ teamId: 'team-admin' })
    await assert.rejects(pending, error => error instanceof DOMException && error.name === 'AbortError')
  }
})

test('runtime unavailable responses remain explicit failures', async () => {
  authenticate()
  globalThis.fetch = async () => Response.json({ kind: 'runtime_unavailable' }, { status: 503 })
  await assert.rejects(listAgents(), /failed \(503\)/)
  await assert.rejects(listTasks(), /failed \(503\)/)
})
