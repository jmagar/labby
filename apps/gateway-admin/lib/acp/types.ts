import type {
  AgentCapabilities,
  AvailableCommand,
  ClientCapabilities,
  ConfigOptionUpdate,
  CurrentModeUpdate,
  PlanEntry,
  PromptResponse,
  RequestPermissionRequest,
  SessionInfoUpdate,
  SessionNotification,
  ToolCallContent,
  ToolCallStatus,
  ToolKind,
  UsageUpdate,
} from '@agentclientprotocol/sdk'

export type AcpProviderKind = string
export type ACPModelOption = {
  id: string
  name: string
  description?: string | null
  fixed?: boolean
}

export type BridgeSessionStatus =
  | 'idle'
  | 'running'
  | 'waiting_for_permission'
  | 'completed'
  | 'failed'
  | 'cancelled'
  | 'closed'

export type ProviderHealth = {
  provider: AcpProviderKind
  ready: boolean
  command: string
  args: string[]
  message: string
  models?: ACPModelOption[]
  defaultModelId?: string | null
  currentModelId?: string | null
}

export type BridgeSessionSummary = {
  id: string
  providerSessionId: string
  provider: AcpProviderKind
  title: string
  cwd: string
  createdAt: string
  updatedAt: string
  status: BridgeSessionStatus
  agentName: string
  agentVersion: string
  resumable?: boolean
  modelId?: string | null
  modelName?: string | null
}

export type BridgePermissionOption = {
  optionId: string
  name: string
  kind: string
}

export type AcpPermissionOption = {
  option_id: string
  name: string
  kind: string
}

export type AcpSessionState =
  | 'creating'
  | 'idle'
  | 'running'
  | 'waiting_for_permission'
  | 'completed'
  | 'cancelled'
  | 'failed'
  | 'closed'

export type AcpContentBlock =
  | { type: 'text'; text: string }
  | { type: 'reasoning'; text: string }
  | {
      type: 'tool_call'
      tool_call_id: string
      name: string
      input: unknown
      output?: unknown
      status?: string
    }
  | { type: 'code'; language?: string; code: string }
  | { type: 'unknown'; type_tag: string; raw: unknown }

type AcpEventEnvelope = {
  id: string
  seq: number
  session_id: string
  created_at: string
}

export type AcpEvent =
  | (AcpEventEnvelope & {
      kind: 'message_chunk'
      role: 'user' | 'assistant'
      text: string
      message_id: string
    })
  | (AcpEventEnvelope & {
      kind: 'reasoning_chunk'
      text: string
    })
  | (AcpEventEnvelope & {
      kind: 'tool_call_start'
      tool_call_id: string
      name: string
      input: unknown
    })
  | (AcpEventEnvelope & {
      kind: 'tool_call_update'
      tool_call_id: string
      output: unknown
      status: string
    })
  | (AcpEventEnvelope & {
      kind: 'permission_request'
      request_id: string
      action_summary: string
      options: AcpPermissionOption[]
    })
  | (AcpEventEnvelope & {
      kind: 'permission_outcome'
      request_id: string
      granted: boolean
    })
  | (AcpEventEnvelope & {
      kind: 'usage_update'
      raw: unknown
    })
  | (AcpEventEnvelope & {
      kind: 'content_blocks'
      blocks: AcpContentBlock[]
    })
  | (AcpEventEnvelope & {
      kind: 'session_update'
      state: AcpSessionState
    })
  | (AcpEventEnvelope & {
      kind: 'provider_info'
      provider: string
      raw: unknown
    })
  | (AcpEventEnvelope & {
      kind: 'unknown'
      event_kind: string
      raw: unknown
    })

export type BridgeEventStatus =
  | BridgeSessionStatus
  | ToolCallStatus
  | 'requested'
  | 'resolved'
  | 'updated'

export type KnownBridgeEventKind =
  | 'message.chunk'
  | 'tool.call'
  | 'tool.update'
  | 'plan'
  | 'permission.requested'
  | 'permission.resolved'
  | 'session.info'
  | 'usage'
  | 'commands'
  | 'mode'
  | 'config'
  | 'status'
  | 'error'
  | 'debug'

export type BridgeEventKind = KnownBridgeEventKind | (string & {})

export type BridgeEvent = {
  id: string
  seq: number
  sessionId: string
  provider: AcpProviderKind
  kind: BridgeEventKind
  createdAt: string
  role?: 'user' | 'assistant' | 'thinking'
  messageId?: string | null
  text?: string
  title?: string
  status?: BridgeEventStatus
  toolCallId?: string
  toolKind?: ToolKind | null
  rawInput?: unknown
  rawOutput?: unknown
  toolContent?: ToolCallContent[] | null
  locations?: string[]
  plan?: PlanEntry[]
  permissionOptions?: BridgePermissionOption[]
  permissionSelection?: string | null
  sessionInfo?: SessionInfoUpdate
  usage?: UsageUpdate
  commands?: AvailableCommand[]
  currentMode?: CurrentModeUpdate
  configUpdate?: ConfigOptionUpdate
  promptStopReason?: PromptResponse['stopReason']
  raw?: unknown
}

export type ProviderRuntimeEvent =
  | { type: 'session_notification'; notification: SessionNotification }
  | { type: 'permission_request'; request: RequestPermissionRequest }
  | {
      type: 'permission_resolved'
      request: RequestPermissionRequest
      selectedOptionId: string | null
    }
  | { type: 'prompt_started'; prompt: string }
  | { type: 'prompt_response'; response: PromptResponse }
  | { type: 'stderr'; text: string }
  | { type: 'error'; message: string; raw?: unknown }
  | { type: 'process_exit'; code: number | null; signal: string | null }

export type StartSessionInput = {
  cwd: string
  title?: string
  clientCapabilities?: ClientCapabilities
}

export type StartSessionResult = {
  providerSessionId: string
  agentName: string
  agentVersion: string
  capabilities?: AgentCapabilities
}
