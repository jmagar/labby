import test from 'node:test'
import assert from 'node:assert/strict'

import { shouldShowWorkingAssistantBubble } from './message-thread'
import type { ACPMessage, ACPRun } from './types'

function run(status: ACPRun['status'] = 'running'): ACPRun {
  return {
    id: 'run-1',
    projectId: 'workspace',
    agentId: 'codex',
    provider: 'codex',
    title: 'Optimize mobile chat',
    createdAt: new Date('2026-05-05T00:00:00Z'),
    updatedAt: new Date('2026-05-05T00:00:00Z'),
    status,
    providerSessionId: 'provider-run-1',
    cwd: '/home/jmagar/workspace/lab',
  }
}

function message(overrides: Partial<ACPMessage> = {}): ACPMessage {
  return {
    id: 'message-1',
    runId: 'run-1',
    role: 'user',
    text: 'Please continue.',
    createdAt: new Date('2026-05-05T00:00:01Z'),
    isStreaming: false,
    thoughts: [],
    toolCalls: [],
    version: 1,
    ...overrides,
  }
}

test('shows working assistant bubble while run is running and no assistant stream exists', () => {
  assert.equal(
    shouldShowWorkingAssistantBubble(run('running'), [message()], 'open'),
    true,
  )
})

test('shows working assistant bubble during initial connecting state for a running run', () => {
  assert.equal(
    shouldShowWorkingAssistantBubble(run('running'), [message()], 'connecting'),
    true,
  )
})

test('does not show working assistant bubble when an assistant stream already exists', () => {
  assert.equal(
    shouldShowWorkingAssistantBubble(
      run('running'),
      [
        message(),
        message({
          id: 'assistant-stream',
          role: 'assistant',
          text: 'Working on it',
          isStreaming: true,
        }),
      ],
      'open',
    ),
    false,
  )
})

test('does not show working assistant bubble for waiting-for-permission', () => {
  assert.equal(
    shouldShowWorkingAssistantBubble(run('waiting_for_permission'), [message()], 'open'),
    false,
  )
})

test('does not show working assistant bubble for idle runs or errored streams', () => {
  assert.equal(
    shouldShowWorkingAssistantBubble(run('idle'), [message()], 'open'),
    false,
  )
  assert.equal(
    shouldShowWorkingAssistantBubble(run('running'), [message()], 'error'),
    false,
  )
})
