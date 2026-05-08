import test from 'node:test'
import assert from 'node:assert/strict'

import {
  ensurePromptRunId,
  ensurePromptRunIdForProvider,
  integrateCreatedRun,
  providerDisplayName,
  retryMessageText,
  resolveSelectedAgent,
  resolveSelectedModel,
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

test('normalizes adapter-scoped model options and selected run model', () => {
  const agentsWithModels: ACPAgent[] = [
    {
      id: 'codex-acp',
      name: 'Codex ACP',
      description: 'codex-acp over local ACP bridge',
      version: 'live',
      capabilities: [],
      models: [
        { id: 'gpt-5', name: 'GPT-5' },
        { id: 'gpt-5-mini', name: 'GPT-5 Mini' },
      ],
      defaultModelId: 'gpt-5',
      currentModelId: 'gpt-5-mini',
    },
  ]

  const selected = resolveSelectedAgent(agentsWithModels, 'codex-acp', {
    ...run('run-codex'),
    provider: 'codex-acp',
    modelId: 'gpt-5-mini',
    modelName: 'GPT-5 Mini',
  })

  assert.equal(selected.id, 'codex-acp')
  assert.equal(selected.models?.length, 2)
  assert.equal(selected.currentModelId, 'gpt-5-mini')
})

test('resolveSelectedModel clears invalid model when adapter changes', () => {
  const codex: ACPAgent = {
    id: 'codex-acp',
    name: 'Codex ACP',
    description: '',
    version: 'live',
    capabilities: [],
    models: [
      { id: 'gpt-5-mini', name: 'GPT-5 Mini' },
      { id: 'gpt-5', name: 'GPT-5' },
    ],
    defaultModelId: 'gpt-5',
  }
  const claude: ACPAgent = {
    id: 'claude-acp',
    name: 'Claude ACP',
    description: '',
    version: 'live',
    capabilities: [],
    models: [{ id: 'sonnet-4.5', name: 'Sonnet 4.5' }],
    defaultModelId: 'sonnet-4.5',
  }

  assert.equal(resolveSelectedModel(codex, 'sonnet-4.5', null)?.id, 'gpt-5')
  assert.equal(resolveSelectedModel(claude, 'gpt-5', null)?.id, 'sonnet-4.5')
})

test('resolveSelectedModel falls through invalid requested ids before using list order', () => {
  const codex: ACPAgent = {
    id: 'codex-acp',
    name: 'Codex ACP',
    description: '',
    version: 'live',
    capabilities: [],
    models: [
      { id: 'gpt-5-mini', name: 'GPT-5 Mini' },
      { id: 'gpt-5', name: 'GPT-5' },
      { id: 'gpt-5-pro', name: 'GPT-5 Pro' },
    ],
    defaultModelId: 'gpt-5',
    currentModelId: 'gpt-5-pro',
  }

  assert.equal(resolveSelectedModel(codex, 'stale-model', null)?.id, 'gpt-5-pro')
  assert.equal(
    resolveSelectedModel(codex, 'stale-model', {
      ...run('run-codex'),
      provider: 'codex-acp',
      modelId: 'gpt-5',
    })?.id,
    'gpt-5',
  )
})

test('ensurePromptRunIdForProvider reuses the current run when selected provider differs', async () => {
  let createCalls = 0

  const runId = await ensurePromptRunIdForProvider(
    {
      ...run('run-codex'),
      provider: 'codex-acp',
    },
    'claude-acp',
    null,
    async () => {
      createCalls += 1
      return {
        ...run('run-claude'),
        provider: 'claude-acp',
      }
    },
    false,
  )

  assert.equal(runId, 'run-codex')
  assert.equal(createCalls, 0)
})

test('ensurePromptRunIdForProvider reuses run when provider selection already matches', async () => {
  let createCalls = 0

  const runId = await ensurePromptRunIdForProvider(
    {
      ...run('run-claude'),
      provider: 'claude-acp',
    },
    'claude-acp',
    null,
    async () => {
      createCalls += 1
      return run('run-created')
    },
    true,
  )

  assert.equal(runId, 'run-claude')
  assert.equal(createCalls, 0)
})

test('ensurePromptRunIdForProvider creates a run when no session is selected', async () => {
  let createCalls = 0
  let receivedOptions: { closeSessionPanel?: boolean; providerId?: string | null; modelId?: string | null } | undefined

  const runId = await ensurePromptRunIdForProvider(
    null,
    'claude-acp',
    'default',
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
  assert.deepEqual(receivedOptions, { closeSessionPanel: false, providerId: 'claude-acp', modelId: 'default' })
})

test('sendPromptForSelectedProvider switches provider inside the selected run and posts page context', async () => {
  const optimisticIds: string[] = []
  const requests: Array<{ path: string; body: unknown }> = []
  let refreshCalls = 0
  let createCalls = 0

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
    createSession: async () => {
      createCalls += 1
      return {
        ...run('run-claude'),
        provider: 'claude-acp',
      }
    },
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
  assert.equal(createCalls, 0)
  assert.deepEqual(requests, [
    {
      path: '/sessions/run-codex/prompt',
      body: {
        prompt: 'hello',
        provider: 'claude-acp',
        attachments: [{ kind: 'file', path: '/tmp/a.txt' }],
        pageContext: { route: '/gateways', entityType: 'gateway', entityId: 'local' },
      },
    },
  ])
})

test('sendPromptForSelectedProvider posts local attachment payloads without dropping prompt text', async () => {
  const requests: Array<{ path: string; body: unknown }> = []

  await sendPromptForSelectedProvider({
    payload: {
      text: 'summarize this file',
      attachments: [
        {
          kind: 'local',
          id: 'local-notes',
          name: 'notes.txt',
          mimeType: 'text/plain',
          size: 11,
          contentKind: 'text',
          text: 'hello world',
        },
      ],
    },
    selectedRun: {
      ...run('run-codex'),
      provider: 'codex-acp',
    },
    selectedProviderId: 'codex-acp',
    createSession: async () => run('unused'),
    isMobileViewport: false,
    fetchAcp: async (path, init) => {
      requests.push({ path, body: JSON.parse(String(init?.body)) })
      return new Response(JSON.stringify({ ok: true }), { status: 200 })
    },
    refreshSessions: async () => {},
    addOptimisticMessage: () => {},
    removeOptimisticMessage: () => {},
  })

  assert.deepEqual(requests, [
    {
      path: '/sessions/run-codex/prompt',
      body: {
        prompt: 'summarize this file',
        attachments: [
          {
            kind: 'local',
            id: 'local-notes',
            name: 'notes.txt',
            mimeType: 'text/plain',
            size: 11,
            contentKind: 'text',
            text: 'hello world',
          },
        ],
      },
    },
  ])
})

test('sendPromptForSelectedProvider includes selected model', async () => {
  const requests: Array<{ body: unknown }> = []

  await sendPromptForSelectedProvider({
    payload: { text: 'hello', attachments: [] },
    selectedRun: { ...run('run-codex'), provider: 'codex-acp' },
    selectedProviderId: 'codex-acp',
    selectedModelId: 'gpt-5-mini',
    createSession: async () => run('unused'),
    isMobileViewport: false,
    fetchAcp: async (_path, init) => {
      requests.push({ body: JSON.parse(String(init?.body)) })
      return new Response(JSON.stringify({ ok: true }), { status: 200 })
    },
    refreshSessions: async () => {},
    addOptimisticMessage: () => {},
    removeOptimisticMessage: () => {},
  })

  assert.deepEqual(requests[0]?.body, { prompt: 'hello', model: 'gpt-5-mini' })
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

test('retry payload preserves the original message text and attachments', async () => {
  const sent: unknown[] = []
  await retryMessageText(
    {
      text: 'retry this',
      attachments: [{ kind: 'file', path: '/tmp/original.txt' }],
    },
    async (payload) => {
      sent.push(payload)
    },
  )

  assert.deepEqual(sent, [{ text: 'retry this', attachments: [{ kind: 'file', path: '/tmp/original.txt' }] }])
})
