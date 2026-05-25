import {
  type ErrorPayload,
  readJsonSafe,
  errorMessageFromPayload,
} from './acp-normalizers'
import type { ACPAgent, ACPMessage, ACPRun } from '@/components/chat/types'
import type { PromptAttachmentRef } from '@/lib/fs/types'

export const ACP_AGENT: ACPAgent = {
  id: 'codex',
  name: 'Codex ACP',
  description: 'codex-acp over local ACP bridge',
  version: 'live',
  capabilities: ['tool_use', 'streaming', 'permissions', 'plans'],
}

export type SessionCreationIntent = 'bootstrap' | 'manual' | 'send'
export type CreateSessionOptions = {
  closeSessionPanel?: boolean
  providerId?: string | null
  modelId?: string | null
}
export type CreateSessionFn = (options?: CreateSessionOptions) => Promise<ACPRun>

export function providerDisplayName(providerId: string): string {
  const normalized = providerId.trim().toLowerCase()
  switch (normalized) {
    case 'codex':
    case 'codex-acp':
      return 'Codex ACP'
    case 'claude':
    case 'claude-acp':
      return 'Claude ACP'
    case 'gemini':
      return 'Gemini'
    case 'gemini-acp':
      return 'Gemini ACP'
    default:
      return providerId
  }
}

export function shouldAutoCreateInitialRun(
  providerReady: boolean,
  runCount: number,
  selectedRunId: string | null,
) {
  return providerReady && runCount === 0 && !selectedRunId
}

export function integrateCreatedRun(current: ACPRun[], run: ACPRun) {
  return [run, ...current.filter((existing) => existing.id !== run.id)]
}

export function sessionCreationOptionsForIntent(
  intent: SessionCreationIntent,
  isMobileViewport: boolean,
): CreateSessionOptions | undefined {
  if (intent === 'bootstrap') {
    return undefined
  }

  return { closeSessionPanel: isMobileViewport }
}

export async function createSessionForIntent(
  createSession: CreateSessionFn,
  intent: SessionCreationIntent,
  isMobileViewport: boolean,
) {
  return createSession(sessionCreationOptionsForIntent(intent, isMobileViewport))
}

export async function ensurePromptRunId(
  selectedRunId: string | null,
  createSession: CreateSessionFn,
  isMobileViewport: boolean,
) {
  if (selectedRunId) {
    return selectedRunId
  }

  const run = await createSessionForIntent(createSession, 'send', isMobileViewport)
  return run.id
}

export async function ensurePromptRunIdForProvider(
  selectedRun: ACPRun | null,
  selectedProviderId: string | null,
  selectedModelId: string | null | undefined,
  createSession: CreateSessionFn,
  isMobileViewport: boolean,
) {
  if (selectedRun) {
    return selectedRun.id
  }

  const run = await createSession({
    ...sessionCreationOptionsForIntent('send', isMobileViewport),
    providerId: selectedProviderId,
    modelId: selectedModelId,
  })
  return run.id
}

export function resolveSelectedAgent(
  agents: ACPAgent[],
  selectedProviderId: string | null,
  selectedRun: ACPRun | null,
): ACPAgent {
  const selectedProviderAgent = selectedProviderId
    ? agents.find((agent) => agent.id === selectedProviderId)
    : undefined
  if (selectedProviderAgent) {
    return selectedProviderAgent
  }

  if (selectedRun) {
    return (
      agents.find((agent) => agent.id === selectedRun.provider) ?? {
        ...ACP_AGENT,
        id: selectedRun.provider,
        name: providerDisplayName(selectedRun.provider),
      }
    )
  }

  return agents[0] ?? ACP_AGENT
}

export function resolveSelectedModel(
  agent: ACPAgent | null,
  requestedModelId: string | null,
  selectedRun: ACPRun | null,
) {
  const models = agent?.models ?? []
  if (models.length === 0) return null
  const runModel = selectedRun && agent && selectedRun.provider === agent.id ? selectedRun.modelId : null
  for (const candidate of [requestedModelId, runModel, agent?.currentModelId, agent?.defaultModelId]) {
    const model = candidate ? models.find((option) => option.id === candidate) : null
    if (model) return model
  }
  return models[0] ?? null
}

export type PromptPayload = {
  text: string
  attachments: PromptAttachmentRef[]
}

export type SendPromptForSelectedProviderOptions = {
  payload: PromptPayload
  selectedRun: ACPRun | null
  selectedProviderId: string | null
  selectedModelId?: string | null
  createSession: CreateSessionFn
  isMobileViewport: boolean
  fetchAcp: (path: string, init?: RequestInit) => Promise<Response>
  refreshSessions: () => Promise<void>
  addOptimisticMessage: (message: ACPMessage) => void
  removeOptimisticMessage: (messageId: string) => void
  pageContext?: unknown
  includePageContext?: boolean
}

type StartAndPromptResult = {
  session_id: string
  provider_session_id?: string
  model_id?: string
  provider: string
  title: string
  stream_ticket?: string
}

export async function sendPromptForSelectedProvider({
  payload,
  selectedRun,
  selectedProviderId,
  selectedModelId,
  createSession,
  isMobileViewport,
  fetchAcp,
  refreshSessions,
  addOptimisticMessage,
  removeOptimisticMessage,
  pageContext,
  includePageContext = false,
}: SendPromptForSelectedProviderOptions) {
  // When no session is selected, fire one atomic action instead of two
  // sequential calls. This collapses the prior orphan-row race: any failure
  // here is server-side atomic — either the session exists with its first
  // prompt queued, or nothing was created.
  if (!selectedRun) {
    const optimisticRunId = `pending-${Date.now()}`
    const optimisticId = `optimistic-${optimisticRunId}-${Date.now()}`
    addOptimisticMessage({
      id: optimisticId,
      runId: optimisticRunId,
      role: 'user' as const,
      text: payload.text,
      createdAt: new Date(),
      thoughts: [],
      toolCalls: [],
      version: 0,
    })

    const orchestratorBody = {
      action: 'session.start_and_prompt',
      params: {
        prompt: payload.text,
        ...(selectedProviderId && { provider: selectedProviderId }),
        ...(selectedModelId && { model: selectedModelId }),
        ...(payload.attachments.length > 0 && { attachments: payload.attachments }),
        ...(includePageContext && pageContext !== null && pageContext !== undefined && { page_context: pageContext }),
      },
    }

    const response = await fetchAcp('', {
      method: 'POST',
      body: JSON.stringify(orchestratorBody),
    })
    if (!response.ok) {
      removeOptimisticMessage(optimisticId)
      const errorPayload = await readJsonSafe<ErrorPayload>(response)
      throw new Error(
        errorMessageFromPayload(errorPayload, 'Failed to start ACP session.'),
      )
    }
    // Drop the optimistic placeholder — refreshSessions() below will surface
    // the materialized run with its real id, and the SSE stream will replay
    // the user message we just queued server-side.
    removeOptimisticMessage(optimisticId)
    const _result = (await readJsonSafe(response)) as StartAndPromptResult | null
    // The server-side action atomically queued the prompt; surfacing the new
    // run via the standard refresh is sufficient.
    void _result
    await refreshSessions()
    return
  }

  const runId = await ensurePromptRunIdForProvider(
    selectedRun,
    selectedProviderId,
    selectedModelId,
    createSession,
    isMobileViewport,
  )

  const optimisticId = `optimistic-${runId}-${Date.now()}`
  addOptimisticMessage({
    id: optimisticId,
    runId,
    role: 'user' as const,
    text: payload.text,
    createdAt: new Date(),
    thoughts: [],
    toolCalls: [],
    version: 0,
  })

  const body = {
    prompt: payload.text,
    ...(selectedRun &&
      selectedProviderId &&
      selectedProviderId !== selectedRun.provider && { provider: selectedProviderId }),
    ...(selectedModelId && { model: selectedModelId }),
    ...(payload.attachments.length > 0 && { attachments: payload.attachments }),
    ...(includePageContext && pageContext !== null && pageContext !== undefined && { pageContext }),
  }

  const response = await fetchAcp(`/sessions/${runId}/prompt`, {
    method: 'POST',
    body: JSON.stringify(body),
  })
  if (!response.ok) {
    removeOptimisticMessage(optimisticId)
    const errorPayload = await readJsonSafe<ErrorPayload>(response)
    throw new Error(errorMessageFromPayload(errorPayload, 'Failed to send prompt to ACP session.'))
  }

  await refreshSessions()
}
