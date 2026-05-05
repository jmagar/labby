import test from 'node:test'
import assert from 'node:assert/strict'

import {
  ensurePromptRunId,
  ensurePromptRunIdForProvider,
  integrateCreatedRun,
  providerDisplayName,
  retryMessageText,
  resolveSelectedAgent,
  sendPromptForSelectedProvider,
  sessionCreationOptionsForIntent,
  shouldAutoCreateInitialRun,
} from './chat-shell'
import type { ACPAgent, ACPRun } from './types'

function run(id: string, title = id): ACPRun {
  return {
    id,
    projectId: 'workspace',
    agentId: 'codex',
    provider: 'codex',
    title,
    createdAt: new Date('2026-04-23T00:00:00Z'),
    updatedAt: new Date('2026-04-23T00:00:00Z'),
    status: 'idle',
    providerSessionId: `provider-${id}`,
    cwd: '/tmp/workspace',
  }
}

const agents: ACPAgent[] = [
  {
    id: 'codex-acp',
    name: 'Codex ACP',
    description: 'codex-acp over local ACP bridge',
    version: 'live',
    capabilities: [],
  },
  {
    id: 'claude-acp',
    name: 'claude-acp',
    description: 'codex-acp over local ACP bridge',
    version: 'live',
    capabilities: [],
  },
]

test('shouldAutoCreateInitialRun only allows bootstrap when provider is ready and no run is selected', () => {
  assert.equal(shouldAutoCreateInitialRun(false, 0, null), false)
  assert.equal(shouldAutoCreateInitialRun(true, 1, null), false)
  assert.equal(shouldAutoCreateInitialRun(true, 0, 'run-1'), false)
  assert.equal(shouldAutoCreateInitialRun(true, 0, null), true)
})

test('integrateCreatedRun prepends the new run and removes stale duplicates', () => {
  const created = run('run-2', 'Fresh session')
  const current = [run('run-1', 'Older session'), run('run-2', 'Stale duplicate')]

  assert.deepEqual(integrateCreatedRun(current, created), [created, current[0]!])
})

test('providerDisplayName normalizes known ACP provider ids', () => {
  assert.equal(providerDisplayName('codex-acp'), 'Codex ACP')
  assert.equal(providerDisplayName('claude-acp'), 'Claude ACP')
  assert.equal(providerDisplayName('gemini'), 'Gemini')
  assert.equal(providerDisplayName('custom-provider'), 'custom-provider')
})

test('sessionCreationOptionsForIntent only closes the panel for non-bootstrap mobile flows', () => {
  assert.equal(sessionCreationOptionsForIntent('bootstrap', true), undefined)
  assert.deepEqual(sessionCreationOptionsForIntent('manual', true), { closeSessionPanel: true })
  assert.deepEqual(sessionCreationOptionsForIntent('send', false), { closeSessionPanel: false })
})

test('ensurePromptRunId reuses the selected run without creating a new session', async () => {
  let createCalls = 0

  const runId = await ensurePromptRunId(
    'run-7',
    async () => {
      createCalls += 1
      return run('run-created')
    },
    true,
  )

  assert.equal(runId, 'run-7')
  assert.equal(createCalls, 0)
})

test('ensurePromptRunId creates a mobile send session when no run is selected', async () => {
  let receivedOptions: { closeSessionPanel?: boolean } | undefined

  const runId = await ensurePromptRunId(
    null,
    async (options) => {
      receivedOptions = options
      return run('run-3', 'Prompt bootstrap')
    },
    true,
  )

  assert.equal(runId, 'run-3')
  assert.deepEqual(receivedOptions, { closeSessionPanel: true })
})

test('resolveSelectedAgent prefers the selected provider over the selected run', () => {
  const selected = resolveSelectedAgent(agents, 'claude-acp', {
    ...run('run-codex'),
    provider: 'codex-acp',
  })

  assert.equal(selected.id, 'claude-acp')
  assert.equal(selected.name, 'claude-acp')
})

test('ensurePromptRunIdForProvider creates a run when selected provider differs from selected run', async () => {
  let createCalls = 0
  let receivedOptions: { closeSessionPanel?: boolean } | undefined

  const runId = await ensurePromptRunIdForProvider(
    {
      ...run('run-codex'),
      provider: 'codex-acp',
    },
    'claude-acp',
    async (options) => {
      createCalls += 1
      receivedOptions = options
      return {
        ...run('run-claude'),
        provider: 'claude-acp',
      }
    },
    false,
  )

  assert.equal(runId, 'run-claude')
  assert.equal(createCalls, 1)
  assert.deepEqual(receivedOptions, { closeSessionPanel: false })
})

test('ensurePromptRunIdForProvider reuses run when provider selection already matches', async () => {
  let createCalls = 0

  const runId = await ensurePromptRunIdForProvider(
    {
      ...run('run-claude'),
      provider: 'claude-acp',
    },
    'claude-acp',
    async () => {
      createCalls += 1
      return run('run-created')
    },
    true,
  )

  assert.equal(runId, 'run-claude')
  assert.equal(createCalls, 0)
})

test('sendPromptForSelectedProvider creates provider-matched run and posts page context', async () => {
  const optimisticIds: string[] = []
  const requests: Array<{ path: string; body: unknown }> = []
  let refreshCalls = 0

  await sendPromptForSelectedProvider({
    payload: {
      text: 'hello',
      attachments: [{ kind: 'file', path: '/tmp/a.txt' }],
    },
    selectedRun: {
      ...run('run-codex'),
      provider: 'codex-acp',
    },
    selectedProviderId: 'claude-acp',
    createSession: async () => ({
      ...run('run-claude'),
      provider: 'claude-acp',
    }),
    isMobileViewport: false,
    fetchAcp: async (path, init) => {
      requests.push({
        path,
        body: JSON.parse(String(init?.body)),
      })
      return new Response(JSON.stringify({ ok: true }), { status: 200 })
    },
    refreshSessions: async () => {
      refreshCalls += 1
    },
    addOptimisticMessage: (message) => {
      optimisticIds.push(message.id)
    },
    removeOptimisticMessage: (messageId) => {
      optimisticIds.splice(optimisticIds.indexOf(messageId), 1)
    },
    pageContext: { route: '/gateways', entityType: 'gateway', entityId: 'local' },
    includePageContext: true,
  })

  assert.equal(refreshCalls, 1)
  assert.equal(optimisticIds.length, 1)
  assert.deepEqual(requests, [
    {
      path: '/sessions/run-claude/prompt',
      body: {
        prompt: 'hello',
        attachments: [{ kind: 'file', path: '/tmp/a.txt' }],
        pageContext: { route: '/gateways', entityType: 'gateway', entityId: 'local' },
      },
    },
  ])
})

test('sendPromptForSelectedProvider removes optimistic message and throws normalized backend errors', async () => {
  const optimisticIds: string[] = []

  await assert.rejects(
    sendPromptForSelectedProvider({
      payload: {
        text: 'hello',
        attachments: [],
      },
      selectedRun: {
        ...run('run-claude'),
        provider: 'claude-acp',
      },
      selectedProviderId: 'claude-acp',
      createSession: async () => run('run-created'),
      isMobileViewport: true,
      fetchAcp: async () =>
        new Response(JSON.stringify({ message: 'provider failed' }), { status: 500 }),
      refreshSessions: async () => {
        throw new Error('refresh should not run')
      },
      addOptimisticMessage: (message) => {
        optimisticIds.push(message.id)
      },
      removeOptimisticMessage: (messageId) => {
        optimisticIds.splice(optimisticIds.indexOf(messageId), 1)
      },
    }),
    /provider failed/,
  )

  assert.deepEqual(optimisticIds, [])
})

test('retry payload uses selected message text without inventing attachments', async () => {
  const sent: unknown[] = []
  await retryMessageText(
    {
      text: 'retry this',
    },
    async (payload) => {
      sent.push(payload)
    },
  )

  assert.deepEqual(sent, [{ text: 'retry this', attachments: [] }])
})
