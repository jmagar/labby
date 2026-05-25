import test from 'node:test'
import assert from 'node:assert/strict'

import { filterVisibleRuns, isHiddenState } from './session-filters'
import type { ACPRun } from '@/components/chat/types'

function run(id: string, status: ACPRun['status']): ACPRun {
  return {
    id,
    projectId: 'p',
    agentId: 'a',
    provider: 'codex-acp',
    title: id,
    createdAt: new Date(),
    updatedAt: new Date(),
    status,
    providerSessionId: 'psid',
    cwd: '.',
  }
}

test('isHiddenState matches failed and closed only', () => {
  assert.equal(isHiddenState('failed'), true)
  assert.equal(isHiddenState('closed'), true)
  assert.equal(isHiddenState('cancelled'), false)
  assert.equal(isHiddenState('idle'), false)
  assert.equal(isHiddenState('completed'), false)
  assert.equal(isHiddenState(undefined), false)
})

test('filterVisibleRuns hides failed/closed by default', () => {
  const visible = filterVisibleRuns(
    [
      run('a', 'idle'),
      run('b', 'failed'),
      run('c', 'closed'),
      run('d', 'completed'),
      run('e', 'cancelled'),
    ],
    { includeHidden: false },
  )
  assert.deepEqual(
    visible.map((r) => r.id),
    ['a', 'd', 'e'],
  )
})

test('filterVisibleRuns passes everything when includeHidden=true', () => {
  const visible = filterVisibleRuns(
    [run('a', 'idle'), run('b', 'failed')],
    { includeHidden: true },
  )
  assert.equal(visible.length, 2)
})
