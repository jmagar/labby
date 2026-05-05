'use client'

/**
 * Shared ACP normalization helpers.
 *
 * Extracted from `chat-session-provider.tsx` and `use-chat-session-controller.ts`
 * to eliminate duplication. Both files import from here.
 */

import type { ACPRun } from '@/components/chat/types'
import type { BridgeSessionSummary, ProviderHealth } from '@/lib/acp/types'

// ---- Shared payload shape types ----

export type ProviderListPayload = {
  providers?: Array<{
    name?: string
    available?: boolean
    version?: string | null
    error?: string | null
    command?: string | null
    args?: string[] | null
    models?: Array<{ id: string; name: string; description?: string | null; fixed?: boolean }>
    defaultModelId?: string | null
    currentModelId?: string | null
    default_model_id?: string | null
    current_model_id?: string | null
  }>
  provider?: ProviderHealth
}

export type SessionCreatePayload = {
  session?: RawSessionSummary
} & RawSessionSummary

export type ErrorPayload = {
  message?: string
  error?: string
  kind?: string
}

export type RawSessionSummary = {
  id: string
  provider: string
  title: string
  cwd: string
  status?: string
  state?: string
  createdAt?: string
  updatedAt?: string
  providerSessionId?: string
  agentName?: string
  agentVersion?: string
  created_at?: string
  updated_at?: string
  provider_session_id?: string
  agent_name?: string
  agent_version?: string
  modelId?: string | null
  modelName?: string | null
  model_id?: string | null
  model_name?: string | null
}

// ---- Normalization helpers ----

export function normalizeSessionSummary(session: RawSessionSummary): BridgeSessionSummary {
  return {
    id: session.id,
    provider: session.provider,
    title: session.title,
    cwd: session.cwd,
    status: (session.status ?? session.state ?? 'idle') as BridgeSessionSummary['status'],
    createdAt: session.createdAt ?? session.created_at ?? '',
    updatedAt: session.updatedAt ?? session.updated_at ?? '',
    providerSessionId: session.providerSessionId ?? session.provider_session_id ?? '',
    agentName: session.agentName ?? session.agent_name ?? 'Codex',
    agentVersion: session.agentVersion ?? session.agent_version ?? 'unknown',
    modelId: session.modelId ?? session.model_id ?? null,
    modelName: session.modelName ?? session.model_name ?? null,
  }
}

export function toRun(session: RawSessionSummary): ACPRun {
  const normalized = normalizeSessionSummary(session)
  return {
    id: normalized.id,
    projectId: 'workspace',
    agentId: normalized.provider,
    provider: normalized.provider,
    title: normalized.title,
    createdAt: new Date(normalized.createdAt),
    updatedAt: new Date(normalized.updatedAt),
    status: normalized.status,
    providerSessionId: normalized.providerSessionId,
    cwd: normalized.cwd,
    modelId: normalized.modelId ?? null,
    modelName: normalized.modelName ?? null,
  }
}

export function normalizeProviderHealth(payload: ProviderListPayload): ProviderHealth {
  if (payload.provider) {
    return payload.provider
  }

  const provider = payload.providers?.[0]
  return {
    provider: provider?.name ?? 'codex',
    ready: Boolean(provider?.available),
    command: '',
    args: [],
    message: provider?.error ?? '',
    models: provider?.models ?? [],
    defaultModelId: provider?.defaultModelId ?? provider?.default_model_id ?? null,
    currentModelId: provider?.currentModelId ?? provider?.current_model_id ?? null,
  }
}

export function normalizeProviderList(payload: ProviderListPayload): ProviderHealth[] {
  if (payload.provider) {
    return [payload.provider]
  }

  return (payload.providers ?? []).map((provider) => ({
    provider: provider.name ?? 'codex-acp',
    ready: Boolean(provider.available),
    command: provider.command ?? '',
    args: provider.args ?? [],
    message: provider.error ?? '',
    models: provider.models ?? [],
    defaultModelId: provider.defaultModelId ?? provider.default_model_id ?? null,
    currentModelId: provider.currentModelId ?? provider.current_model_id ?? null,
  }))
}

export function extractCreatedSession(payload: SessionCreatePayload): RawSessionSummary {
  return payload.session ?? payload
}

export async function readJsonSafe<T>(response: Response): Promise<T | null> {
  const text = await response.text()
  if (!text) {
    return null
  }

  try {
    return JSON.parse(text) as T
  } catch {
    return null
  }
}

export function errorMessageFromPayload(payload: ErrorPayload | null, fallback: string): string {
  return payload?.message ?? payload?.error ?? fallback
}

export function sameProviderList(a: ProviderHealth[], b: ProviderHealth[]): boolean {
  if (a.length !== b.length) return false
  return a.every((item, i) => {
    const other = b[i]
    if (!other) return false
    const models = item.models ?? []
    const otherModels = other.models ?? []
    return (
      item.provider === other.provider &&
      item.ready === other.ready &&
      item.defaultModelId === other.defaultModelId &&
      item.currentModelId === other.currentModelId &&
      models.length === otherModels.length &&
      models.every((model, index) => {
        const otherModel = otherModels[index]
        return (
          model.id === otherModel?.id &&
          model.name === otherModel.name &&
          model.description === otherModel.description &&
          model.fixed === otherModel.fixed
        )
      })
    )
  })
}
