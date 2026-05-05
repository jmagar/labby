import type { BridgeEvent, BridgeSessionStatus } from '@/lib/acp/types'
import type { AttachmentRef } from '@/lib/fs/types'

export type { AttachmentRef }

export interface ACPModelOption {
  id: string
  name: string
  description?: string | null
  fixed?: boolean
}

export interface ACPAgent {
  id: string
  name: string
  description: string
  version: string
  capabilities: string[]
  models?: ACPModelOption[]
  defaultModelId?: string | null
  currentModelId?: string | null
}

export interface ACPProject {
  id: string
  name: string
  agentId: string
  collapsed?: boolean
}

export type ACPRunStatus = BridgeSessionStatus

export interface ACPRun {
  id: string
  projectId: string
  agentId: string
  provider: string
  title: string
  createdAt: Date
  updatedAt: Date
  status: ACPRunStatus
  providerSessionId: string
  cwd: string
  modelId?: string | null
  modelName?: string | null
}

/**
 * Chunked terminal output state for a tool call.
 *
 * rawChunks stores individual output chunks as received from ACP _meta.
 * All renderers MUST go through getDisplayText(rawChunks) — direct consumption
 * of rawChunks outside the helper is a violation of the sanitization boundary.
 */
export interface TranscriptTerminal {
  /** Raw output chunks — use getDisplayText() before rendering. */
  rawChunks: string[]
  /**
   * Running count maintained in O(1) on push and eviction.
   *
   * Measured in UTF-16 code units (String.prototype.length), not actual bytes.
   * For ASCII/BMP terminal output (the common case) this equals the byte count.
   * The caps MAX_TOTAL_BYTES, MAX_CHUNK_BYTES, and TERMINAL_RENDER_TAIL_BYTES
   * are defined in the same units so budget comparisons remain correct.
   */
  totalBytes: number
  /** True when chunks have been evicted due to MAX_TOTAL_BYTES cap. */
  truncated: boolean
  /** Exit code from terminal_exit meta, null if not yet exited. */
  exitCode?: number | null
}

export interface TranscriptToolCall {
  id: string
  title: string
  status?: 'pending' | 'in_progress' | 'completed' | 'failed' | 'idle' | 'running' | 'cancelled'
  kind?: string | null
  input?: unknown
  output?: unknown
  content?: unknown[] | null
  locations: string[]
  permissionOptions?: Array<{ optionId: string; name: string; kind: string }>
  permissionSelection?: string | null
  /** ACP terminal display metadata — present when agent streams terminal output. */
  terminal?: TranscriptTerminal | null
}

export interface ACPMessage {
  id: string
  runId: string
  role: 'user' | 'assistant' | 'system'
  text: string
  createdAt: Date
  isStreaming?: boolean
  thoughts: string[]
  toolCalls: TranscriptToolCall[]
  /**
   * Monotonic version incremented whenever this message's toolCalls change.
   * Used as a memoization key so unchanged messages are not re-derived.
   * C3: without this, virtualization is cosmetic — INP > 200ms at 10x volume.
   */
  version: number
}

export type ActivityItem = BridgeEvent
