'use client'

/**
 * ChatSessionProvider — 4-context React provider for shared chat session state.
 *
 * Owns the shared chat session state for both the /chat route and the
 * floating chat popover.
 *
 * 4-context split for re-render isolation:
 * - ChatSessionDataContext: runs, selectedRunId, providers, etc. (changes per interaction)
 * - ChatSessionActionsContext: stable callbacks (almost never re-renders)
 * - ChatSessionConnectionContext: connectionState, sessionStatus (2-3x per session)
 * - ChatSessionStreamContext: messages, events, activity (per token — only chat surfaces)
 *
 * Non-chat pages consume NONE of these contexts — zero re-render cost.
 *
 * Stream lifecycle:
 * SSE stream opens whenever selectedRunId is non-null.
 */

import * as React from 'react'
import { createAcpFetcher } from '@/lib/acp/fetch'
import {
  toProjects,
  appendSessionEvent,
  resolveLastSessionEventSeq,
  resolveSessionStatusFromEvents,
  deriveTranscriptAndActivity,
} from './session-events'
import {
  consumeSessionEventBuffer,
} from './use-session-events'
import { sessionEventCache, sessionLastSeqCache } from './session-event-cache'
import {
  type ProviderListPayload,
  type SessionCreatePayload,
  type ErrorPayload,
  type RawSessionSummary,
  toRun,
  normalizeProviderHealth,
  normalizeProviderList,
  extractCreatedSession,
  readJsonSafe,
  errorMessageFromPayload,
  sameProviderList,
} from './acp-normalizers'
import type { ACPAgent, ACPRun, ACPProject, ACPMessage, ACPModelOption } from '@/components/chat/types'
import type { BridgeSessionSummary, ProviderHealth, BridgeEvent } from '@/lib/acp/types'
import type { SessionEventConnectionState } from './use-session-events'
import {
  ACP_AGENT,
  integrateCreatedRun,
  providerDisplayName,
  resolveSelectedAgent,
  resolveSelectedModel,
  sendPromptForSelectedProvider,
  shouldAutoCreateInitialRun,
  type PromptPayload,
  type CreateSessionFn,
  type CreateSessionOptions,
} from './use-chat-session-controller'

const USE_MOCK_DATA = process.env.NEXT_PUBLIC_MOCK_DATA === 'true'

// ---- PageContext type ----

/** Optional page context slot — off by default. */
export type PageContext = {
  route: string
  entityType?: string
  entityId?: string
} | null

// ---- Context value types ----

export type ChatSessionDataContextValue = {
  runs: ACPRun[]
  selectedRunId: string | null
  selectedRun: ACPRun | null
  providerHealth: ProviderHealth | null
  providers: ProviderHealth[]
  selectedProviderId: string | null
  selectedAgent: ACPAgent
  selectedModel: ACPModelOption | null
  agents: ACPAgent[]
  projects: ACPProject[]
  sessionsLoaded: boolean
  pageContext: PageContext
}

export type ChatSessionActionsContextValue = {
  createSession: CreateSessionFn
  selectRun: (runId: string) => void
  sendPrompt: (
    payload: PromptPayload,
    options?: { includePageContext?: boolean; pageContext?: unknown; providerId?: string | null },
  ) => Promise<void>
  refreshSessions: () => Promise<void>
  refreshProvider: () => Promise<void>
  selectAgent: (providerId: string) => void
  selectModel: (providerId: string, modelId: string) => void
  setPageContext: (ctx: PageContext) => void
}

export type ChatSessionConnectionContextValue = {
  connectionState: SessionEventConnectionState
  sessionStatus: BridgeSessionSummary['status']
}

export type ChatSessionStreamContextValue = {
  messages: ReturnType<typeof deriveTranscriptAndActivity>['messages']
  events: BridgeEvent[]
  activity: ReturnType<typeof deriveTranscriptAndActivity>['activity']
}

// ---- Contexts ----

const ChatSessionDataContext = React.createContext<ChatSessionDataContextValue | null>(null)
const ChatSessionActionsContext = React.createContext<ChatSessionActionsContextValue | null>(null)
const ChatSessionConnectionContext = React.createContext<ChatSessionConnectionContextValue | null>(null)
const ChatSessionStreamContext = React.createContext<ChatSessionStreamContextValue | null>(null)

// ---- Hooks ----

export function useChatSessionData(): ChatSessionDataContextValue {
  const ctx = React.useContext(ChatSessionDataContext)
  if (!ctx) throw new Error('useChatSessionData must be used within ChatSessionProvider')
  return ctx
}

export function useChatSessionActions(): ChatSessionActionsContextValue {
  const ctx = React.useContext(ChatSessionActionsContext)
  if (!ctx) throw new Error('useChatSessionActions must be used within ChatSessionProvider')
  return ctx
}

export function useChatSessionConnection(): ChatSessionConnectionContextValue {
  const ctx = React.useContext(ChatSessionConnectionContext)
  if (!ctx) throw new Error('useChatSessionConnection must be used within ChatSessionProvider')
  return ctx
}

export function useChatSessionStream(): ChatSessionStreamContextValue {
  const ctx = React.useContext(ChatSessionStreamContext)
  if (!ctx) throw new Error('useChatSessionStream must be used within ChatSessionProvider')
  return ctx
}

// ---- Provider props ----

export type ChatSessionProviderProps = {
  children: React.ReactNode
  isMobileViewport?: boolean
  onSessionPanelClose?: () => void
  /**
   * When true, the provider will auto-create an initial session if none exists.
   * Defaults to false so visiting admin pages with the chat closed does not
   * create empty sessions on the backend. Set to true when the chat surface is
   * actually visible (popover open or `/chat` route).
   */
  autoBootstrap?: boolean
}

// ---- Main provider ----

export function ChatSessionProvider({
  children,
  isMobileViewport = false,
  onSessionPanelClose,
  autoBootstrap = false,
}: ChatSessionProviderProps) {
  // ---- State ----
  const [runs, setRuns] = React.useState<ACPRun[]>([])
  const [sessionsLoaded, setSessionsLoaded] = React.useState(false)
  const [selectedRunId, setSelectedRunId] = React.useState<string | null>(() => {
    try {
      const id = typeof localStorage !== 'undefined'
        ? localStorage.getItem('lab.chat.last-session-id')
        : null
      return id && /^[a-zA-Z0-9_-]{8,64}$/.test(id) ? id : null
    } catch {
      return null
    }
  })
  const [providerHealth, setProviderHealth] = React.useState<ProviderHealth | null>(null)
  const [providers, setProviders] = React.useState<ProviderHealth[]>([])
  const [selectedProviderId, setSelectedProviderId] = React.useState<string | null>(null)
  const [selectedModelByProvider, setSelectedModelByProvider] = React.useState<Record<string, string>>({})
  const [optimisticMessages, setOptimisticMessages] = React.useState<ACPMessage[]>([])
  const [pageContext, setPageContext] = React.useState<PageContext>(null)

  // SSE state
  const [events, setEvents] = React.useState<BridgeEvent[]>([])
  const [connectionState, setConnectionState] = React.useState<SessionEventConnectionState>('idle')
  const lastSeqRef = React.useRef(0)

  // Mutex for createSession
  const isCreatingRef = React.useRef(false)

  // Forward-declared ref so createSession (defined before refreshProvider)
  // can refresh the provider catalog after a new session is bootstrapped.
  const refreshProviderRef = React.useRef<() => Promise<void>>(async () => {})

  // ---- ACP fetch utility (re-derived per call, not stored in state) ----
  const fetchAcpRef = React.useRef(createAcpFetcher())
  // Re-derive on each render in case config changed (stable ref via useCallback)
  const fetchAcp = React.useCallback(
    (path: string, init?: RequestInit) => fetchAcpRef.current(path, init),
    [],
  )

  // ---- Derived values ----
  const selectedRun = React.useMemo(
    () => runs.find((run) => run.id === selectedRunId) ?? null,
    [runs, selectedRunId],
  )

  const projects = React.useMemo(
    () =>
      toProjects(
        runs.map((run) => ({
          id: run.id,
          providerSessionId: run.providerSessionId,
          provider: run.provider,
          title: run.title,
          cwd: run.cwd,
          createdAt: run.createdAt.toISOString(),
          updatedAt: run.updatedAt.toISOString(),
          status: run.status,
          agentName: providerDisplayName(run.provider),
          agentVersion: ACP_AGENT.version,
        })),
      ),
    [runs],
  )

  const agents = React.useMemo<ACPAgent[]>(
    () =>
      providers.map((provider) => ({
        ...ACP_AGENT,
        id: provider.provider,
        name: providerDisplayName(provider.provider),
        version: ACP_AGENT.version,
        models: provider.models,
        defaultModelId: provider.defaultModelId,
        currentModelId: provider.currentModelId,
      })),
    [providers],
  )

  const selectedAgent = React.useMemo(
    () => resolveSelectedAgent(agents, selectedProviderId, selectedRun),
    [agents, selectedProviderId, selectedRun],
  )

  const selectedModel = React.useMemo(
    () =>
      resolveSelectedModel(
        selectedAgent,
        selectedAgent ? selectedModelByProvider[selectedAgent.id] ?? null : null,
        selectedRun,
      ),
    [selectedAgent, selectedModelByProvider, selectedRun],
  )

  const sessionStatus = React.useMemo(
    () => resolveSessionStatusFromEvents(events),
    [events],
  )

  const derived = React.useMemo(() => deriveTranscriptAndActivity(events), [events])

  // ---- Actions ----

  const refreshSessions = React.useCallback(async () => {
    const response = await fetchAcp('/sessions')
    if (!response.ok) {
      setRuns([])
      setSelectedRunId(null)
      setSessionsLoaded(true)
      return
    }
    const payload = await readJsonSafe<{ sessions: RawSessionSummary[] }>(response)
    if (!payload) {
      setRuns([])
      setSelectedRunId(null)
      setSessionsLoaded(true)
      return
    }
    const nextRuns = payload.sessions.map(toRun)
    setRuns(nextRuns)
    setSelectedRunId((current) => {
      if (current && nextRuns.some((run) => run.id === current)) return current
      return nextRuns[0]?.id ?? null
    })
    setSessionsLoaded(true)
  }, [fetchAcp])

  const refreshProvider = React.useCallback(async () => {
    const response = await fetchAcp('/provider')
    if (!response.ok) {
      const unavailable: ProviderHealth = {
        provider: 'codex-acp',
        ready: false,
        command: '',
        args: [],
        message: 'ACP provider unavailable.',
      }
      setProviders([unavailable])
      setSelectedProviderId('codex-acp')
      setProviderHealth(unavailable)
      return
    }
    const payload = (await response.json()) as ProviderListPayload
    const nextProviders = normalizeProviderList(payload)
    const selected = nextProviders.find((provider) => provider.ready) ?? nextProviders[0] ?? normalizeProviderHealth(payload)
    setProviders((prev) => sameProviderList(prev, nextProviders) ? prev : (nextProviders.length > 0 ? nextProviders : [selected]))
    setSelectedProviderId((current) =>
      current && nextProviders.some((provider) => provider.provider === current)
        ? current
        : selected.provider,
    )
    setProviderHealth(selected)
  }, [fetchAcp])

  // Keep the ref in sync so createSession can call the latest closure.
  React.useEffect(() => {
    refreshProviderRef.current = refreshProvider
  }, [refreshProvider])

  const createSession = React.useCallback<CreateSessionFn>(
    async (createOptions?: CreateSessionOptions) => {
      if (isCreatingRef.current) {
        // Mutex guard: prevent duplicate concurrent session creation — throw immediately to signal the caller
        throw new Error('Session creation already in progress')
      }
      isCreatingRef.current = true
      try {
        const response = await fetchAcp('/sessions', {
          method: 'POST',
          body: JSON.stringify({
            provider: selectedProviderId ?? providerHealth?.provider ?? 'codex-acp',
            ...(selectedModel?.id && { model: selectedModel.id }),
          }),
        })
        const payload = await readJsonSafe<SessionCreatePayload | ErrorPayload>(response)
        if (!response.ok || !payload) {
          const message = errorMessageFromPayload(
            payload as ErrorPayload | null,
            'Failed to create ACP session.',
          )
          setProviderHealth((current) => ({
            provider: current?.provider ?? 'codex-acp',
            ready: false,
            command: current?.command ?? '',
            args: current?.args ?? [],
            message,
          }))
          throw new Error(message)
        }
        // Narrow: if payload has an `id` field it is a SessionCreatePayload; otherwise ErrorPayload
        const isSessionPayload = 'id' in payload || ('session' in payload && payload.session != null)
        if (!isSessionPayload) {
          const message = errorMessageFromPayload(payload as ErrorPayload, 'Failed to create ACP session.')
          throw new Error(message)
        }
        const run = toRun(extractCreatedSession(payload as SessionCreatePayload))
        setRuns((current) => integrateCreatedRun(current, run))
        setSelectedRunId(run.id)
        if (createOptions?.closeSessionPanel) {
          onSessionPanelClose?.()
        }
        // Refresh provider list so model picker reflects models the agent
        // returned during session bootstrap. Most ACP providers only populate
        // their model catalog after a session has been created — without this
        // refresh the picker stays empty until the user manually reloads.
        void refreshProviderRef.current?.()
        return run
      } finally {
        isCreatingRef.current = false
      }
    },
    [fetchAcp, onSessionPanelClose, providerHealth?.provider, selectedModel?.id, selectedProviderId],
  )

  const selectRun = React.useCallback(
    (runId: string) => {
      setSelectedRunId(runId)
      const run = runs.find((candidate) => candidate.id === runId)
      if (run) {
        setSelectedProviderId(run.provider)
      }
      try {
        localStorage.setItem('lab.chat.last-session-id', runId)
      } catch {
        // localStorage may be unavailable
      }
      if (isMobileViewport) {
        onSessionPanelClose?.()
      }
    },
    [isMobileViewport, onSessionPanelClose, runs],
  )

  const selectAgent = React.useCallback((providerId: string) => {
    setSelectedProviderId(providerId)
  }, [])

  const selectModel = React.useCallback((providerId: string, modelId: string) => {
    const provider = providers.find((candidate) => candidate.provider === providerId)
    if (!provider?.models?.some((model) => model.id === modelId)) return
    setSelectedModelByProvider((current) => ({ ...current, [providerId]: modelId }))
  }, [providers])

  const sendPrompt = React.useCallback<ChatSessionActionsContextValue['sendPrompt']>(
    async (payload, options) => {
      const providerId = options?.providerId ?? selectedProviderId
      const selectedModelId =
        providerId === selectedAgent.id ? selectedModel?.id ?? null : selectedModelByProvider[providerId ?? ''] ?? null
      try {
        await sendPromptForSelectedProvider({
          payload,
          selectedRun,
          selectedProviderId: providerId,
          selectedModelId,
          createSession,
          isMobileViewport,
          fetchAcp,
          refreshSessions,
          addOptimisticMessage: (message) => setOptimisticMessages((prev) => [...prev, message]),
          removeOptimisticMessage: (messageId) =>
            setOptimisticMessages((prev) => prev.filter((message) => message.id !== messageId)),
          includePageContext: options?.includePageContext,
          pageContext: options?.pageContext,
        })
      } catch (error) {
        const message = error instanceof Error ? error.message : 'Failed to send prompt to ACP session.'
        setProviderHealth((current) => ({
          provider: current?.provider ?? selectedProviderId ?? 'codex-acp',
          ready: false,
          command: current?.command ?? '',
          args: current?.args ?? [],
          message,
        }))
        throw error
      }
    },
    [
      createSession,
      fetchAcp,
      isMobileViewport,
      refreshSessions,
      selectedProviderId,
      selectedAgent.id,
      selectedModel?.id,
      selectedModelByProvider,
      selectedRun,
    ],
  )

  const setPageContextStable = React.useCallback((ctx: PageContext) => {
    setPageContext(ctx)
  }, [])

  // ---- Initialization effects ----

  React.useEffect(() => {
    if (USE_MOCK_DATA) {
      setProviderHealth({
        provider: 'codex-acp',
        ready: false,
        command: '',
        args: [],
        message: 'ACP unavailable in mock preview.',
      })
      setProviders([])
      setSelectedProviderId(null)
      setRuns([])
      setSelectedRunId(null)
      setSessionsLoaded(true)
      return
    }

    void refreshProvider()
    void refreshSessions()
  }, [refreshProvider, refreshSessions])

  // Sync providerHealth when selectedProviderId or providers list changes
  React.useEffect(() => {
    if (!selectedProviderId) return
    const selected = providers.find((provider) => provider.provider === selectedProviderId)
    if (selected) {
      setProviderHealth(selected)
    }
  }, [providers, selectedProviderId])

  // Auto-bootstrap first session — after sessions have been loaded so we don't race with refreshSessions.
  // Gated on `autoBootstrap`: the provider mounts on every admin page, but we only want to mint a
  // session when the user has actually surfaced the chat (popover open or /chat route). Otherwise
  // every dashboard view would silently create an empty session + SSE stream on the backend.
  // NOTE(phase-2): wire AbortController once createSession accepts a signal.
  React.useEffect(() => {
    if (!autoBootstrap) return
    if (!sessionsLoaded) return
    if (!shouldAutoCreateInitialRun(Boolean(providerHealth?.ready), runs.length, selectedRunId)) return

    void (async () => {
      try {
        await createSession()
      } catch {
        // providerHealth.message carries the failure detail
      }
    })()
  }, [autoBootstrap, createSession, providerHealth?.ready, runs.length, selectedRunId, sessionsLoaded])

  // Update run status from session events (bail-out setter for re-render efficiency)
  React.useEffect(() => {
    if (!selectedRunId || events.length === 0) return

    setRuns((current) =>
      current.map((run) => {
        if (run.id !== selectedRunId) return run
        if (run.status === sessionStatus) return run
        return { ...run, status: sessionStatus, updatedAt: new Date() }
      }),
    )
  }, [selectedRunId, sessionStatus, events.length])

  // ---- SSE stream management (active whenever selectedRunId is non-null) ----

  React.useEffect(() => {
    if (!selectedRunId) {
      setEvents([])
      setConnectionState('idle')
      lastSeqRef.current = 0
      return
    }

    if (USE_MOCK_DATA) {
      setEvents([])
      setConnectionState('idle')
      lastSeqRef.current = 0
      return
    }

    // Rehydrate from shared cache
    const cachedEvents = sessionEventCache.get(selectedRunId) ?? []
    const cachedLastSeq = resolveLastSessionEventSeq(
      cachedEvents,
      sessionLastSeqCache.get(selectedRunId) ?? 0,
    )

    setEvents(cachedEvents)
    setConnectionState('connecting')
    lastSeqRef.current = cachedLastSeq

    const abortController = new AbortController()

    void (async () => {
      try {
        // Obtain a short-lived HMAC ticket before opening the SSE stream.
        const ticketRes = await fetchAcpRef.current(`/sessions/${selectedRunId}/subscribe_ticket`, {
          method: 'POST',
          signal: abortController.signal,
        })
        if (!ticketRes.ok) {
          setConnectionState('error')
          return
        }
        const { ticket } = await ticketRes.json() as { ticket: string }

        const response = await fetchAcpRef.current(
          `/sessions/${selectedRunId}/events?since=${lastSeqRef.current}&ticket=${encodeURIComponent(ticket)}`,
          { signal: abortController.signal },
        )

        if (!response.ok || !response.body) {
          setConnectionState('error')
          return
        }

        setConnectionState('open')

        const reader = response.body.getReader()
        const decoder = new TextDecoder()
        let buffer = ''

        const applyBuffer = () => {
          const consumed = consumeSessionEventBuffer(buffer, lastSeqRef.current)
          buffer = consumed.buffer

          if (consumed.events.length === 0) return

          // Batch: apply all events from this chunk in one setEvents call
          setEvents((current) => {
            let next = current
            for (const event of consumed.events) {
              lastSeqRef.current = event.seq
              sessionLastSeqCache.set(selectedRunId, event.seq)
              next = appendSessionEvent(next, event)
            }
            sessionEventCache.set(selectedRunId, next)
            return next
          })
        }

        while (true) {
          const { done, value } = await reader.read()
          if (done) break
          buffer += decoder.decode(value, { stream: true })
          applyBuffer()
        }

        buffer += decoder.decode()
        applyBuffer()
      } catch {
        if (!abortController.signal.aborted) {
          setConnectionState('error')
        }
      }
    })()

    return () => {
      abortController.abort()
    }
  }, [selectedRunId])

  // ---- Context values ----

  const dataValue = React.useMemo<ChatSessionDataContextValue>(
    () => ({
      runs,
      selectedRunId,
      selectedRun,
      providerHealth,
      providers,
      selectedProviderId,
      selectedAgent,
      selectedModel,
      agents,
      projects,
      sessionsLoaded,
      pageContext,
    }),
    [runs, selectedRunId, selectedRun, providerHealth, providers, selectedProviderId, selectedAgent, selectedModel, agents, projects, sessionsLoaded, pageContext],
  )

  const actionsValue = React.useMemo<ChatSessionActionsContextValue>(
    () => ({
      createSession,
      selectRun,
      sendPrompt,
      refreshSessions,
      refreshProvider,
      selectAgent,
      selectModel,
      setPageContext: setPageContextStable,
    }),
    [createSession, selectRun, sendPrompt, refreshSessions, refreshProvider, selectAgent, selectModel, setPageContextStable],
  )

  const connectionValue = React.useMemo<ChatSessionConnectionContextValue>(
    () => ({
      connectionState,
      sessionStatus,
    }),
    [connectionState, sessionStatus],
  )

  const streamValue = React.useMemo<ChatSessionStreamContextValue>(
    () => ({
      messages:
        optimisticMessages.length === 0
          ? derived.messages
          : [...derived.messages, ...optimisticMessages.filter((message) => message.runId === selectedRunId)].sort(
              (a, b) => a.createdAt.getTime() - b.createdAt.getTime(),
            ),
      events,
      activity: derived.activity,
    }),
    [derived.messages, events, derived.activity, optimisticMessages, selectedRunId],
  )

  return (
    <ChatSessionDataContext.Provider value={dataValue}>
      <ChatSessionActionsContext.Provider value={actionsValue}>
        <ChatSessionConnectionContext.Provider value={connectionValue}>
          <ChatSessionStreamContext.Provider value={streamValue}>
            {children}
          </ChatSessionStreamContext.Provider>
        </ChatSessionConnectionContext.Provider>
      </ChatSessionActionsContext.Provider>
    </ChatSessionDataContext.Provider>
  )
}
