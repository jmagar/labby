import type { AcpEvent, BridgeEvent, BridgeEventStatus, BridgeSessionStatus, BridgeSessionSummary } from '@/lib/acp/types'
import type { ACPMessage, ActivityItem, TranscriptTerminal, TranscriptToolCall } from '@/components/chat/types'

// ---------------------------------------------------------------------------
// ACP Terminal chunk eviction constants (C1/R3/O5)
//
// All size limits are in UTF-16 code units (String.prototype.length), not
// actual bytes. For ASCII/BMP terminal output (the common case) these are
// equivalent. Non-BMP characters (emoji, etc.) count as two code units.
// ---------------------------------------------------------------------------

/** Per-chunk pre-push cap (in UTF-16 code units). Chunks larger than this are sliced to the tail. */
const MAX_CHUNK_BYTES = 64 * 1024 // 64 KiB

/** Per-terminal cap (in UTF-16 code units). Oldest chunks are evicted when totalBytes exceeds this. */
const MAX_TOTAL_BYTES = 1 * 1024 * 1024 // 1 MiB

/** On terminal_exit, compact chunks to this tail size (in UTF-16 code units). */
const TERMINAL_RENDER_TAIL_BYTES = 256 * 1024 // 256 KiB

/** Max terminal_id length. Events with longer IDs are dropped. */
const MAX_TERMINAL_ID_LENGTH = 128

export const MAX_SESSION_EVENTS = 500

function toDate(value: string) {
  return new Date(value)
}

type ToolCallPatch = {
  id: string
  title?: string
  status?: TranscriptToolCall['status']
  kind?: string | null
  input?: unknown
  output?: unknown
  content?: unknown[] | null
  locations?: string[]
  permissionOptions?: Array<{ optionId: string; name: string; kind: string }>
  permissionSelection?: string | null
  /** Terminal patch: describes the kind of terminal update to apply. */
  terminalPatch?: TerminalPatch | null
}

// ---------------------------------------------------------------------------
// Terminal patch types
// ---------------------------------------------------------------------------

type TerminalPatch =
  | { kind: 'info'; terminalId: string }
  | { kind: 'output'; terminalId: string; data: string }
  | { kind: 'exit'; terminalId: string; exitCode: number | null }

/**
 * Parse ACP terminal metadata from a tool_call_update output payload.
 *
 * ACP terminal ordering invariant: terminal_info MUST arrive before
 * terminal_output for the same terminal_id. Orphan output (output before info)
 * is logged at warn and dropped.
 *
 * R4: terminal_id is capped at MAX_TERMINAL_ID_LENGTH chars; longer IDs are
 * treated as malformed and the event is dropped.
 *
 * Signal is NOT stored (F3 resolution: drop signal in Phase 1, re-add in
 * lab-lffl when server-derived signal is available).
 */
function readTerminalPatch(rawOutput: unknown): TerminalPatch | null {
  if (!isRecord(rawOutput)) return null
  const meta = rawOutput['_meta']
  if (!isRecord(meta)) return null

  // terminal_info
  if (isRecord(meta['terminal_info'])) {
    const info = meta['terminal_info']
    const terminalId = typeof info['terminal_id'] === 'string' ? info['terminal_id'] : null
    if (!terminalId) return null
    if (terminalId.length > MAX_TERMINAL_ID_LENGTH) {
      console.warn('[acp] terminal_info terminal_id exceeds max length, dropping event')
      return null
    }
    return { kind: 'info', terminalId }
  }

  // terminal_output
  if (isRecord(meta['terminal_output'])) {
    const out = meta['terminal_output']
    const terminalId = typeof out['terminal_id'] === 'string' ? out['terminal_id'] : null
    const data = typeof out['data'] === 'string' ? out['data'] : ''
    if (!terminalId) return null
    if (terminalId.length > MAX_TERMINAL_ID_LENGTH) {
      console.warn('[acp] terminal_output terminal_id exceeds max length, dropping event')
      return null
    }
    return { kind: 'output', terminalId, data }
  }

  // terminal_exit
  if (isRecord(meta['terminal_exit'])) {
    const exit = meta['terminal_exit']
    const terminalId = typeof exit['terminal_id'] === 'string' ? exit['terminal_id'] : null
    if (!terminalId) return null
    if (terminalId.length > MAX_TERMINAL_ID_LENGTH) {
      console.warn('[acp] terminal_exit terminal_id exceeds max length, dropping event')
      return null
    }
    const exitCode = typeof exit['exit_code'] === 'number' ? exit['exit_code'] : null
    return { kind: 'exit', terminalId, exitCode }
  }

  return null
}

/**
 * Apply a terminal patch to an existing TranscriptTerminal (or create one).
 *
 * Chunk eviction rules (C1/R3/O5):
 * - MAX_CHUNK_BYTES: slice chunk to tail before push (R2 security cap)
 * - MAX_TOTAL_BYTES: FIFO evict oldest chunks when exceeded (C1/R3)
 * - terminal_exit: compact-and-freeze to TERMINAL_RENDER_TAIL_BYTES (O5)
 *
 * Returns a new TranscriptTerminal object (immutable reducer pattern).
 */
function applyTerminalPatch(
  existing: TranscriptTerminal | null | undefined,
  patch: TerminalPatch,
): TranscriptTerminal {
  const base: TranscriptTerminal = existing ?? {
    rawChunks: [],
    totalBytes: 0,
    truncated: false,
    exitCode: null,
  }

  if (patch.kind === 'info') {
    // terminal_info creates the terminal entry; no chunk data yet.
    return base
  }

  if (patch.kind === 'output') {
    let data = patch.data
    // R2: per-chunk size cap — slice to tail if oversized.
    if (data.length > MAX_CHUNK_BYTES) {
      data = data.slice(data.length - MAX_CHUNK_BYTES)
    }

    const rawChunks = [...base.rawChunks, data]
    let totalBytes = base.totalBytes + data.length
    let { truncated } = base

    // C1/R3: FIFO eviction when total exceeds MAX_TOTAL_BYTES.
    while (totalBytes > MAX_TOTAL_BYTES && rawChunks.length > 0) {
      const evicted = rawChunks.shift()!
      totalBytes -= evicted.length
      truncated = true
    }

    return { rawChunks, totalBytes, truncated, exitCode: base.exitCode }
  }

  if (patch.kind === 'exit') {
    // O5: compact-and-freeze on exit — keep only TERMINAL_RENDER_TAIL_BYTES.
    const joined = base.rawChunks.join('')
    let finalText = joined
    let truncated = base.truncated
    if (joined.length > TERMINAL_RENDER_TAIL_BYTES) {
      finalText = joined.slice(joined.length - TERMINAL_RENDER_TAIL_BYTES)
      truncated = true
    }
    return {
      rawChunks: finalText ? [finalText] : [],
      totalBytes: finalText.length,
      truncated,
      exitCode: patch.exitCode,
    }
  }

  return base
}

const INTERNAL_EVENT_KINDS = new Set(['tool_call_metadata'])
const SESSION_STATUSES = new Set<BridgeSessionStatus>([
  'idle',
  'running',
  'waiting_for_permission',
  'completed',
  'failed',
  'cancelled',
  'closed',
])

// Keep transcript derivation resilient to providers that emit unstable chunk ids.
function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value)
}

function isToolCallStatus(value: unknown): TranscriptToolCall['status'] | undefined {
  switch (value) {
    case 'pending':
    case 'in_progress':
    case 'completed':
    case 'failed':
    case 'idle':
    case 'running':
    case 'cancelled':
      return value
    default:
      return undefined
  }
}

function readToolMetadata(raw: unknown) {
  if (!isRecord(raw)) {
    return null
  }

  const hasMetadataShape =
    'tool_kind' in raw ||
    'kind' in raw ||
    'locations' in raw ||
    'content' in raw ||
    'raw_output' in raw

  if (!hasMetadataShape) {
    return null
  }

  return {
    title: typeof raw.title === 'string' ? raw.title : undefined,
    kind: typeof raw.tool_kind === 'string' ? raw.tool_kind : typeof raw.kind === 'string' ? raw.kind : null,
    status: isToolCallStatus(raw.status),
    content: Array.isArray(raw.content) ? raw.content : undefined,
    locations: Array.isArray(raw.locations)
      ? raw.locations.filter((value): value is string => typeof value === 'string')
      : undefined,
    output: raw.raw_output,
  }
}

function providerRawType(raw: unknown) {
  if (!isRecord(raw) || typeof raw.type !== 'string') {
    return null
  }
  return raw.type
}

function isBridgeSessionStatus(value: unknown): value is BridgeSessionStatus {
  return typeof value === 'string' && SESSION_STATUSES.has(value as BridgeSessionStatus)
}

function bridgeBaseEvent(event: AcpEvent, kind: BridgeEvent['kind'], provider = 'codex'): BridgeEvent {
  return {
    id: event.id,
    seq: event.seq,
    sessionId: event.session_id,
    provider,
    kind,
    createdAt: event.created_at,
  }
}

export function bridgeEventFromAcpEvent(event: AcpEvent): BridgeEvent {
  switch (event.kind) {
    case 'message_chunk':
      return {
        ...bridgeBaseEvent(event, 'message.chunk'),
        role: event.role,
        text: event.text,
        messageId: event.message_id,
      }
    case 'reasoning_chunk':
      return {
        ...bridgeBaseEvent(event, 'message.chunk'),
        role: 'thinking',
        text: event.text,
      }
    case 'tool_call_start':
      return {
        ...bridgeBaseEvent(event, 'tool.call'),
        title: event.name,
        toolCallId: event.tool_call_id,
        rawInput: event.input,
      }
    case 'tool_call_update':
      return {
        ...bridgeBaseEvent(event, 'tool.update'),
        title: 'Tool call updated',
        toolCallId: event.tool_call_id,
        status: event.status as BridgeEventStatus | undefined,
        rawOutput: event.output,
      }
    case 'permission_request':
      return {
        ...bridgeBaseEvent(event, 'permission.requested'),
        title: event.action_summary,
        toolCallId: event.request_id,
        status: 'requested',
        permissionOptions: event.options.map((option) => ({
          optionId: option.option_id,
          name: option.name,
          kind: option.kind,
        })),
      }
    case 'permission_outcome':
      return {
        ...bridgeBaseEvent(event, 'permission.resolved'),
        toolCallId: event.request_id,
        status: 'resolved',
        permissionSelection: event.granted ? 'granted' : 'rejected',
      }
    case 'usage_update':
      return {
        ...bridgeBaseEvent(event, 'usage'),
        usage: event.raw as BridgeEvent['usage'],
      }
    case 'content_blocks':
      return {
        ...bridgeBaseEvent(event, 'content'),
        raw: { blocks: event.blocks },
      }
    case 'session_update':
      return {
        ...bridgeBaseEvent(event, 'status'),
        status: event.state as BridgeEventStatus,
      }
    case 'provider_info': {
      const rawType = providerRawType(event.raw)
      switch (rawType) {
        case 'stderr':
        case 'debug':
          return {
            ...bridgeBaseEvent(event, 'debug', event.provider),
            title: isRecord(event.raw) && typeof event.raw.title === 'string' ? event.raw.title : undefined,
            text: isRecord(event.raw) && typeof event.raw.text === 'string' ? event.raw.text : undefined,
            raw: event.raw,
          }
        case 'plan':
          return {
            ...bridgeBaseEvent(event, 'plan', event.provider),
            title: isRecord(event.raw) && typeof event.raw.title === 'string' ? event.raw.title : undefined,
            plan:
              isRecord(event.raw) && Array.isArray(event.raw.entries)
                ? (event.raw.entries as BridgeEvent['plan'])
                : undefined,
            raw: event.raw,
          }
        case 'commands':
          return {
            ...bridgeBaseEvent(event, 'commands', event.provider),
            title: isRecord(event.raw) && typeof event.raw.title === 'string' ? event.raw.title : undefined,
            commands:
              isRecord(event.raw) && Array.isArray(event.raw.commands)
                ? (event.raw.commands as BridgeEvent['commands'])
                : undefined,
            raw: event.raw,
          }
        case 'current_mode':
          return {
            ...bridgeBaseEvent(event, 'mode', event.provider),
            title: isRecord(event.raw) && typeof event.raw.title === 'string' ? event.raw.title : undefined,
            currentMode: isRecord(event.raw) ? (event.raw.current_mode as BridgeEvent['currentMode']) : undefined,
            raw: event.raw,
          }
        case 'config_update':
          return {
            ...bridgeBaseEvent(event, 'config', event.provider),
            title: isRecord(event.raw) && typeof event.raw.title === 'string' ? event.raw.title : undefined,
            configUpdate: isRecord(event.raw) ? (event.raw.config_update as BridgeEvent['configUpdate']) : undefined,
            raw: event.raw,
          }
        case 'session_info':
          return {
            ...bridgeBaseEvent(event, 'session.info', event.provider),
            title:
              isRecord(event.raw) && isRecord(event.raw.session_info) && typeof event.raw.session_info.title === 'string'
                ? event.raw.session_info.title
                : isRecord(event.raw) && typeof event.raw.title === 'string'
                  ? event.raw.title
                  : undefined,
            sessionInfo: isRecord(event.raw) ? (event.raw.session_info as BridgeEvent['sessionInfo']) : undefined,
            raw: event.raw,
          }
        case 'stop_reason':
          return {
            ...bridgeBaseEvent(event, 'status', event.provider),
            title: isRecord(event.raw) && typeof event.raw.title === 'string' ? event.raw.title : undefined,
            status: isRecord(event.raw) && typeof event.raw.status === 'string' ? event.raw.status as BridgeEventStatus : undefined,
            promptStopReason:
              isRecord(event.raw) && typeof event.raw.stop_reason === 'string'
                ? (event.raw.stop_reason as BridgeEvent['promptStopReason'])
                : undefined,
            raw: event.raw,
          }
        case 'idle_completion':
          return {
            ...bridgeBaseEvent(event, 'idle_completion', event.provider),
            title:
              isRecord(event.raw) && typeof event.raw.title === 'string'
                ? event.raw.title
                : 'Session paused – provider idle timeout',
            status: 'completed',
            raw: event.raw,
          }
        case 'subscriber_backpressure':
          return {
            ...bridgeBaseEvent(event, 'reconnect', event.provider),
            title: 'Subscriber backpressure',
            status: 'reconnect' as BridgeEventStatus,
            raw: event.raw,
          }
        default:
          return {
            ...bridgeBaseEvent(event, rawType ?? 'debug', event.provider),
            raw: event.raw,
          }
      }
    }
    case 'unknown':
      return {
        ...bridgeBaseEvent(event, event.event_kind),
        raw: event.raw,
      }
  }
}

function toolPatchFromEvent(event: BridgeEvent): ToolCallPatch | null {
  if (event.kind === 'tool.call') {
    if (!event.toolCallId) {
      return null
    }

    return {
      id: event.toolCallId,
      title: event.title,
      status: isToolCallStatus(event.status),
      kind: event.toolKind ?? undefined,
      input: event.rawInput,
      output: event.rawOutput,
      content: (event.toolContent as unknown[]) ?? undefined,
      locations: event.locations,
    }
  }

  if (event.kind === 'tool.update') {
    if (!event.toolCallId) {
      return null
    }

    const outputMetadata = readToolMetadata(event.rawOutput)
    const terminalPatch = readTerminalPatch(event.rawOutput)
    return {
      id: event.toolCallId,
      title: outputMetadata?.title,
      status: isToolCallStatus(event.status) ?? outputMetadata?.status,
      kind: outputMetadata?.kind,
      output: event.rawOutput,
      content: outputMetadata?.content,
      locations: outputMetadata?.locations,
      terminalPatch,
    }
  }

  if (event.kind === 'tool_call_metadata') {
    const metadata = readToolMetadata(event.raw)
    const toolCallId =
      event.toolCallId ??
      (isRecord(event.raw) && typeof event.raw.tool_call_id === 'string' ? event.raw.tool_call_id : null)

    if (!toolCallId || !metadata) {
      return null
    }

    return {
      id: toolCallId,
      title: metadata.title ?? event.title,
      status: metadata.status,
      kind: metadata.kind,
      content: metadata.content,
      locations: metadata.locations,
      output: metadata.output,
    }
  }

  return null
}

export function appendSessionEvent(
  current: BridgeEvent[],
  incoming: BridgeEvent,
  maxEvents = MAX_SESSION_EVENTS,
) {
  const lastEvent = current.at(-1)
  if (lastEvent && incoming.seq <= lastEvent.seq) {
    return current
  }

  const next = [...current, incoming]
  if (next.length <= maxEvents) {
    return next
  }
  return next.slice(next.length - maxEvents)
}

export function resolveLastSessionEventSeq(events: BridgeEvent[], cachedLastSeq = 0) {
  return Math.max(cachedLastSeq, events.at(-1)?.seq ?? 0)
}

export function resolveSessionStatusFromEvents(
  events: BridgeEvent[],
  fallback: BridgeSessionStatus = 'idle',
) {
  // Reverse scan — stop at the first (most-recent) status event: O(1) in the common case.
  for (let i = events.length - 1; i >= 0; i--) {
    const event = events[i]
    if (event && event.kind === 'status' && isBridgeSessionStatus(event.status)) {
      return event.status
    }
  }
  return fallback
}

/**
 * Upsert a tool call patch into the tool calls array.
 *
 * Returns [updatedToolCalls, changed] where changed=true when any field
 * was actually mutated (used to decide whether to bump message version).
 *
 * Terminal patch handling:
 * - terminal_info: creates terminal entry on this tool call
 * - terminal_output: orphan output (no prior terminal entry on this toolCall)
 *   is logged at warn and dropped per R8 ordering invariant
 * - terminal_exit: triggers compact-and-freeze
 * - second terminal_info for same toolCallId: overwrites (C5)
 */
function upsertToolCall(
  toolCalls: TranscriptToolCall[],
  patch: ToolCallPatch,
): [TranscriptToolCall[], boolean] {
  const next = [...toolCalls]
  const index = next.findIndex((toolCall) => toolCall.id === patch.id)
  const previous = index >= 0 ? next[index] : null

  // Resolve terminal state.
  let terminal = previous?.terminal ?? null
  if (patch.terminalPatch) {
    if (patch.terminalPatch.kind === 'output' && terminal == null) {
      // R8: terminal_output before terminal_info — drop with warn.
      console.warn(
        '[acp] terminal_output received before terminal_info for toolCallId',
        patch.id,
        '— dropping orphan chunk',
      )
      // Still apply rest of patch; just skip terminal update.
    } else {
      terminal = applyTerminalPatch(terminal, patch.terminalPatch)
    }
  }

  const value: TranscriptToolCall = {
    id: patch.id,
    title: patch.title ?? previous?.title ?? patch.id,
    status: patch.status ?? previous?.status,
    kind: patch.kind ?? previous?.kind ?? null,
    input: patch.input ?? previous?.input,
    output: patch.output ?? previous?.output,
    content: patch.content ?? previous?.content ?? null,
    locations: patch.locations ?? previous?.locations ?? [],
    permissionOptions: patch.permissionOptions ?? previous?.permissionOptions,
    permissionSelection: patch.permissionSelection !== undefined ? patch.permissionSelection : previous?.permissionSelection,
    terminal,
  }

  // Changed detection: O(1) field-level check, no JSON.stringify (perf).
  const changed =
    !previous ||
    previous.title !== value.title ||
    previous.status !== value.status ||
    previous.kind !== value.kind ||
    previous.output !== value.output ||
    previous.content !== value.content ||
    previous.terminal !== value.terminal

  if (index >= 0) {
    next[index] = value
  } else {
    next.push(value)
  }

  return [next, changed]
}

function ensureAssistantMessage(
  sessionId: string,
  createdAt: string,
  messages: Map<string, ACPMessage>,
  orderedMessageIds: string[],
  preferredId: string | null,
) {
  const key = preferredId ?? `assistant-${sessionId}-${orderedMessageIds.length + 1}`
  let message = messages.get(key)

  if (!message) {
    message = {
      id: key,
      runId: sessionId,
      role: 'assistant',
      text: '',
      createdAt: toDate(createdAt),
      isStreaming: true,
      thoughts: [],
      toolCalls: [],
      version: 0,
    }
    messages.set(key, message)
    orderedMessageIds.push(key)
  }

  return { key, message }
}

export function deriveTranscriptAndActivity(events: BridgeEvent[]): {
  messages: ACPMessage[]
  activity: ActivityItem[]
} {
  const messages = new Map<string, ACPMessage>()
  const orderedMessageIds: string[] = []
  const activity: ActivityItem[] = []
  let lastAssistantMessageId: string | null = null
  let activeUserMessageId: string | null = null
  let activeAssistantMessageId: string | null = null

  for (const event of events) {
    if (event.kind === 'message.chunk') {
      if (event.role === 'thinking') {
        activity.push(event)
        const { key, message } = ensureAssistantMessage(
          event.sessionId,
          event.createdAt,
          messages,
          orderedMessageIds,
          activeAssistantMessageId ?? lastAssistantMessageId,
        )
        if (event.text) {
          if (message.thoughts.length === 0) {
            message.thoughts.push(event.text)
          } else {
            message.thoughts[message.thoughts.length - 1] += event.text
          }
          message.createdAt = toDate(event.createdAt)
        }
        activeAssistantMessageId = key
        lastAssistantMessageId = key
        continue
      }

      const role = event.role === 'user' ? 'user' : 'assistant'
      const activeMessageId: string | null = role === 'user' ? activeUserMessageId : activeAssistantMessageId
      const key: string =
        activeMessageId ??
        event.messageId ??
        `${role}-${event.sessionId}-${orderedMessageIds.length + 1}`
      const existing = messages.get(key)

      if (!existing) {
        const message: ACPMessage = {
          id: key,
          runId: event.sessionId,
          role,
          text: event.text ?? '',
          createdAt: toDate(event.createdAt),
          isStreaming: role === 'assistant',
          thoughts: [],
          toolCalls: [],
          version: 0,
        }
        messages.set(key, message)
        orderedMessageIds.push(key)
      } else {
        existing.text += event.text ?? ''
        existing.createdAt = toDate(event.createdAt)
      }

      if (role === 'user') {
        activeUserMessageId = key
        activeAssistantMessageId = null
      }

      if (role === 'assistant') {
        activeUserMessageId = null
        activeAssistantMessageId = key
        lastAssistantMessageId = key
      }
      continue
    }

    const toolPatch = toolPatchFromEvent(event)
    if (toolPatch) {
      if (!INTERNAL_EVENT_KINDS.has(event.kind)) {
        activity.push(event)
      }
      const { key, message } = ensureAssistantMessage(
        event.sessionId,
        event.createdAt,
        messages,
        orderedMessageIds,
        activeAssistantMessageId ?? lastAssistantMessageId,
      )
      const [updatedToolCalls, changed] = upsertToolCall(message.toolCalls, toolPatch)
      message.toolCalls = updatedToolCalls
      // C3: bump version on any tool call change for per-message memoization.
      if (changed) {
        message.version = (message.version ?? 0) + 1
      }
      activeAssistantMessageId = key
      lastAssistantMessageId = key
      if (
        toolPatch.status === 'completed' ||
        toolPatch.status === 'failed' ||
        toolPatch.status === 'cancelled'
      ) {
        message.isStreaming = false
      }
      continue
    }

    if (event.kind === 'status') {
      const latest = lastAssistantMessageId ? messages.get(lastAssistantMessageId) : null
      if (latest) {
        latest.isStreaming = event.status === 'running'
      }
      if (event.status !== 'running') {
        activeAssistantMessageId = null
      }
    }

    activity.push(event)
  }

  return {
    messages: orderedMessageIds.map((id) => messages.get(id)!).filter(Boolean),
    activity,
  }
}

export function toProjects(sessions: BridgeSessionSummary[]) {
  if (sessions.length === 0) {
    return [{ id: 'workspace', name: 'workspace', agentId: 'codex' }]
  }

  const projectName = sessions[0]?.cwd.split('/').filter(Boolean).at(-1) ?? 'workspace'
  return [{ id: 'workspace', name: projectName, agentId: sessions[0]?.provider ?? 'codex' }]
}
