import { spawn } from 'node:child_process'
import { readFile, writeFile } from 'node:fs/promises'
import { Readable, Writable } from 'node:stream'
import {
  ClientSideConnection,
  PROTOCOL_VERSION,
  ndJsonStream,
  type Client,
  type PromptResponse,
  type RequestPermissionRequest,
  type RequestPermissionResponse,
  type SessionNotification,
} from '@agentclientprotocol/sdk'
import type { AcpProvider, AcpProviderHandle, ProviderEventHandler } from '../provider'
import { getCodexProviderHealth, resolveCodexLaunch } from '../health'
import type { ProviderHealth, StartSessionInput, StartSessionResult } from '../types'

type CodexHandle = AcpProviderHandle & {
  process: ReturnType<typeof spawn>
  connection: ClientSideConnection
}

type EventfulConnection = ClientSideConnection & {
  __onEvent?: ProviderEventHandler
}

function setEventHandler(connection: ClientSideConnection, onEvent: ProviderEventHandler) {
  ;(connection as EventfulConnection).__onEvent = onEvent
}

function emitConnectionEvent(connection: ClientSideConnection, event: Parameters<ProviderEventHandler>[0]) {
  ;(connection as EventfulConnection).__onEvent?.(event)
}

class CodexBridgeClient implements Client {
  constructor(
    private readonly onEvent: ProviderEventHandler,
  ) {}

  async requestPermission(params: RequestPermissionRequest): Promise<RequestPermissionResponse> {
    this.onEvent({ type: 'permission_request', request: params })

    // Do NOT auto-approve. Surface the permission request to the UI and wait
    // for the user to respond. For now, always cancel — callers must wire a
    // proper user-mediated resolution path before enabling auto-accept here.
    // See lab-qq8y.6 for the full hardening spec.
    this.onEvent({
      type: 'permission_resolved',
      request: params,
      selectedOptionId: null,
    })

    return {
      outcome: { outcome: 'cancelled' },
    }
  }

  async sessionUpdate(params: SessionNotification): Promise<void> {
    this.onEvent({ type: 'session_notification', notification: params })
  }

  async readTextFile(params: { path: string }): Promise<{ content: string }> {
    const content = await readFile(params.path, 'utf8')
    return { content }
  }

  async writeTextFile(params: { path: string; content: string }): Promise<Record<string, never>> {
    await writeFile(params.path, params.content, 'utf8')
    return {}
  }
}

export class CodexAcpProvider implements AcpProvider {
  readonly kind = 'codex' as const
  private readonly handles = new Map<string, CodexHandle>()

  async health(): Promise<ProviderHealth> {
    return getCodexProviderHealth()
  }

  async startSession(input: StartSessionInput, onEvent: ProviderEventHandler): Promise<{
    handle: AcpProviderHandle
    result: StartSessionResult
  }> {
    const launch = resolveCodexLaunch()

    // Use an explicit env allowlist rather than forwarding all of process.env.
    // This prevents the child process from inheriting secrets (tokens, API keys,
    // session cookies) that are present in the Node.js server environment.
    const ALLOWED_ENV_KEYS = [
      'HOME', 'USER', 'LOGNAME', 'SHELL', 'TERM', 'LANG', 'LC_ALL', 'LC_CTYPE',
      'PATH', 'TMPDIR', 'TMP', 'TEMP',
      // Codex-specific variables callers may need to set.
      'CODEX_SANDBOX_NETWORK', 'CODEX_DISABLE_SANDBOX',
    ] as const
    const filteredEnv: NodeJS.ProcessEnv = {
      // NODE_ENV is always forwarded — it is not a secret and is required by ProcessEnv.
      NODE_ENV: process.env.NODE_ENV,
      ...Object.fromEntries(
        ALLOWED_ENV_KEYS
          .filter((key) => key in process.env)
          .map((key) => [key, process.env[key]]),
      ),
    }

    const child = spawn(launch.command, launch.args, {
      cwd: input.cwd,
      env: filteredEnv,
      stdio: ['pipe', 'pipe', 'pipe'],
    })

    if (!child.stdin || !child.stdout) {
      throw new Error('Unable to open stdio streams for codex-acp')
    }

    child.stderr?.on('data', (chunk) => {
      const text = chunk.toString().trim()
      if (text) {
        onEvent({ type: 'stderr', text })
      }
    })

    child.on('exit', (code, signal) => {
      onEvent({ type: 'process_exit', code, signal })
    })

    const inputStream = Writable.toWeb(child.stdin) as WritableStream<Uint8Array<ArrayBufferLike>>
    const outputStream = Readable.toWeb(child.stdout) as ReadableStream<Uint8Array<ArrayBufferLike>>
    const stream = ndJsonStream(inputStream, outputStream)
    const client = new CodexBridgeClient(onEvent)
    const connection = new ClientSideConnection(() => client, stream)

    const init = await connection.initialize({
      protocolVersion: PROTOCOL_VERSION,
      clientInfo: {
        name: 'gateway-admin',
        title: 'Gateway Admin',
        version: '0.2.3',
      },
      // Filesystem capabilities default OFF — callers must explicitly opt in
      // by providing clientCapabilities in StartSessionInput. This prevents
      // unintended filesystem exposure when the provider is used without a
      // customized input. See lab-qq8y.6.
      clientCapabilities: input.clientCapabilities ?? {},
    })

    const created = await connection.newSession({
      cwd: input.cwd,
      mcpServers: [],
    })

    const handle: CodexHandle = {
      providerSessionId: created.sessionId,
      process: child,
      connection,
    }

    setEventHandler(connection, onEvent)

    this.handles.set(created.sessionId, handle)

    return {
      handle,
      result: {
        providerSessionId: created.sessionId,
        agentName: init.agentInfo?.title ?? init.agentInfo?.name ?? 'Codex ACP',
        agentVersion: init.agentInfo?.version ?? 'unknown',
        capabilities: init.agentCapabilities,
      },
    }
  }

  async promptSession(handle: AcpProviderHandle, prompt: string): Promise<void> {
    const codexHandle = this.handles.get(handle.providerSessionId)
    if (!codexHandle) {
      throw new Error(`Unknown ACP session: ${handle.providerSessionId}`)
    }

    void codexHandle.connection
      .prompt({
        sessionId: codexHandle.providerSessionId,
        messageId: crypto.randomUUID(),
        prompt: [{ type: 'text', text: prompt }],
      })
      .then((response: PromptResponse) => {
        emitConnectionEvent(codexHandle.connection, {
          type: 'prompt_response',
          response,
        })
      })
      .catch((error: unknown) => {
        emitConnectionEvent(codexHandle.connection, {
          type: 'error',
          message: error instanceof Error ? error.message : 'Prompt failed',
          raw: error,
        })
      })

    emitConnectionEvent(codexHandle.connection, {
      type: 'prompt_started',
      prompt,
    })
  }

  async cancelSession(handle: AcpProviderHandle): Promise<void> {
    const codexHandle = this.handles.get(handle.providerSessionId)
    if (!codexHandle) {
      return
    }
    await codexHandle.connection.cancel({ sessionId: codexHandle.providerSessionId })
  }

  async shutdownSession(handle: AcpProviderHandle): Promise<void> {
    const codexHandle = this.handles.get(handle.providerSessionId)
    if (!codexHandle) {
      return
    }

    try {
      await codexHandle.connection.cancel({ sessionId: codexHandle.providerSessionId })
    } catch {
        // Best-effort cleanup; process exit may already have closed the handle.
      }

    codexHandle.process.kill()
    this.handles.delete(handle.providerSessionId)
  }

  async listSessions(): Promise<Array<{ providerSessionId: string }>> {
    return Array.from(this.handles.keys()).map((providerSessionId) => ({ providerSessionId }))
  }
}

export function attachProviderEventHandler(
  handle: AcpProviderHandle,
  onEvent: ProviderEventHandler,
): AcpProviderHandle {
  const codexHandle = handle as CodexHandle
  if (codexHandle.connection) {
    setEventHandler(codexHandle.connection, onEvent)
  }
  return handle
}
