export type SnippetSource = 'builtin' | 'user'

export type SnippetInputType =
  | 'string'
  | 'integer'
  | 'number'
  | 'boolean'
  | 'object'
  | 'array'
  | 'json'

export interface SnippetInputSpec {
  ty: SnippetInputType
  required?: boolean
  default?: unknown
  description?: string
}

export interface SnippetInfo {
  name: string
  description?: string | null
  tags: string[]
  inputs?: Record<string, SnippetInputSpec>
  source: SnippetSource
  path: string
  shadowed: boolean
}

export interface ResolvedSnippet extends SnippetInfo {
  body: string
}

export interface SnippetListResponse {
  snippets: SnippetInfo[]
}

export interface SnippetValidation {
  valid: boolean
  name: string
  mode: 'body' | 'existing'
  source?: SnippetSource
  path?: string
}

export interface SnippetTestResult {
  name?: string
  passed: boolean
  response?: unknown
  results?: Array<{
    name: string
    passed: boolean
    response?: unknown
    error?: unknown
  }>
}

export interface CodeModeExecutionResponse {
  result?: unknown
  calls?: unknown[]
  logs?: unknown[]
}
