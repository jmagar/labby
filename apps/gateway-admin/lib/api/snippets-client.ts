import { snippetsActionUrl } from './gateway-config'
import { performServiceAction, type ServiceActionError } from './service-action-client'
import type {
  CodeModeExecutionResponse,
  ResolvedSnippet,
  SnippetInfo,
  SnippetListResponse,
  SnippetTestResult,
  SnippetValidation,
} from '@/lib/types/snippets'

export class SnippetsApiError extends Error implements ServiceActionError {
  status: number
  code?: string
  param?: string

  constructor(message: string, status: number, code?: string, param?: string) {
    super(message)
    this.name = 'SnippetsApiError'
    this.status = status
    this.code = code
    this.param = param
  }
}

async function snippetsAction<T>(action: string, params: object, signal?: AbortSignal): Promise<T> {
  return performServiceAction<T, SnippetsApiError>({
    action,
    params,
    signal,
    serviceLabel: 'Snippets',
    url: snippetsActionUrl(),
    createError: (message, status, code, param) => new SnippetsApiError(message, status, code, param),
  })
}

export const snippetsApi = {
  async list(signal?: AbortSignal): Promise<SnippetInfo[]> {
    const response = await snippetsAction<SnippetListResponse>('snippets.list', {}, signal)
    return response.snippets
  },

  get(name: string, signal?: AbortSignal): Promise<ResolvedSnippet> {
    return snippetsAction<ResolvedSnippet>('snippets.get', { name }, signal)
  },

  validate(name: string, body?: string, signal?: AbortSignal): Promise<SnippetValidation> {
    return snippetsAction<SnippetValidation>(
      'snippets.validate',
      body === undefined ? { name } : { name, body },
      signal,
    )
  },

  test(name: string, params: Record<string, unknown> = {}, signal?: AbortSignal): Promise<SnippetTestResult> {
    return snippetsAction<SnippetTestResult>('snippets.test', { name, params }, signal)
  },

  testAll(signal?: AbortSignal): Promise<SnippetTestResult> {
    return snippetsAction<SnippetTestResult>('snippets.test', { all: true }, signal)
  },

  exec(
    name: string,
    params: Record<string, unknown> = {},
    signal?: AbortSignal,
  ): Promise<CodeModeExecutionResponse> {
    return snippetsAction<CodeModeExecutionResponse>('snippets.exec', { name, params }, signal)
  },
}
