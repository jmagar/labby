export type CodeModeTrace =
  | CodeModeExecuteTrace
  | CodeModeSearchTrace
  | CodeModeHistoryTrace

export interface CodeModeExecuteTrace {
  kind: 'code_mode_execute_trace'
  call_count: number
  calls: CodeModeCallTrace[]
  result_shape?: ResultShape
  logs_count?: number
  warnings?: CodeModeTraceWarning[]
}

export interface CodeModeSearchTrace {
  kind: 'code_mode_search_trace'
  query_kind: string
  match_count: number
  displayed_count?: number
  truncated?: boolean
  matches: CodeModeSearchMatch[]
  result_shape?: ResultShape
  warnings?: CodeModeTraceWarning[]
}

export interface CodeModeHistoryTrace {
  kind: 'code_mode_history'
  entries: CodeModeHistoryEntry[]
  warnings?: CodeModeTraceWarning[]
}

export interface CodeModeTraceWarning {
  kind: 'dropped_rows'
  message: string
}

export interface CodeModeHistoryEntry {
  seq: number
  kind: 'search' | 'execute'
  ok: boolean
  elapsed_ms: number
  error_kind?: string
  calls?: CodeModeCallTrace[]
  match_count?: number
}

export interface CodeModeCallTrace {
  id: string
  upstream: string
  tool: string
  ok: boolean
  elapsed_ms: number
  params?: unknown
  error_kind?: string
}

export interface CodeModeSearchMatch {
  id: string
  upstream: string
  tool: string
  description: string
  has_schema: boolean
  has_output_schema: boolean
}

export interface ResultShape {
  type: string
  size_bytes?: number
  length?: number
  key_count?: number
  keys?: string[]
  item_types?: string[]
  truncated?: boolean
  content_block_kinds?: string[]
}

export function parseCodeModeTrace(value: unknown): CodeModeTrace | null {
  if (!isRecord(value)) return null
  if (value.kind === 'code_mode_execute_trace') return parseExecuteTrace(value)
  if (value.kind === 'code_mode_search_trace') return parseSearchTrace(value)
  if (value.kind === 'code_mode_history') return parseHistoryTrace(value)
  return null
}

export function stringifyRedactedParams(value: unknown): string {
  if (value === undefined || value === null) return ''
  try {
    return JSON.stringify(value, null, 2)
  } catch (error) {
    const reason = error instanceof Error && error.message ? error.message : 'unsupported value'
    return `[unsupported params: ${truncateText(reason, 96)}]`
  }
}

export function flattenTraceRows(trace: CodeModeTrace | null) {
  if (!trace) return { calls: [], matches: [], history: [] }
  if (trace.kind === 'code_mode_execute_trace') {
    return { calls: trace.calls, matches: [], history: [] }
  }
  if (trace.kind === 'code_mode_search_trace') {
    return { calls: [], matches: trace.matches, history: [] }
  }
  return {
    calls: trace.entries.flatMap((entry) => entry.calls ?? []),
    matches: [],
    history: trace.entries,
  }
}

function parseExecuteTrace(value: Record<string, unknown>): CodeModeExecuteTrace | null {
  const calls = arrayOfWithDropped(value.calls, parseCallTrace)
  if (!calls) return null
  return {
    kind: 'code_mode_execute_trace',
    call_count: numberValue(value.call_count, calls.items.length),
    calls: calls.items,
    result_shape: parseResultShape(value.result_shape),
    logs_count: optionalNumber(value.logs_count),
    warnings: droppedWarning(calls.dropped, 'execute call'),
  }
}

function parseSearchTrace(value: Record<string, unknown>): CodeModeSearchTrace | null {
  const matches = arrayOfWithDropped(value.matches, parseSearchMatch)
  if (!matches) return null
  return {
    kind: 'code_mode_search_trace',
    query_kind: stringValue(value.query_kind, 'catalog_filter'),
    match_count: numberValue(value.match_count, matches.items.length),
    displayed_count: optionalNumber(value.displayed_count),
    truncated: booleanOptional(value.truncated),
    matches: matches.items,
    result_shape: parseResultShape(value.result_shape),
    warnings: droppedWarning(matches.dropped, 'search match'),
  }
}

function parseHistoryTrace(value: Record<string, unknown>): CodeModeHistoryTrace | null {
  const entries = arrayOfWithDropped(value.entries, parseHistoryEntry)
  if (!entries) return null
  return {
    kind: 'code_mode_history',
    entries: entries.items,
    warnings: droppedWarning(entries.dropped, 'history entry'),
  }
}

function parseHistoryEntry(value: unknown): CodeModeHistoryEntry | null {
  if (!isRecord(value)) return null
  let kind: CodeModeHistoryEntry['kind']
  switch (value.kind) {
    case 'execute':
      kind = 'execute'
      break
    case 'search':
      kind = 'search'
      break
    default:
      return null
  }
  return {
    seq: numberValue(value.seq, 0),
    kind,
    ok: booleanValue(value.ok),
    elapsed_ms: numberValue(value.elapsed_ms, 0),
    error_kind: optionalString(value.error_kind),
    calls: arrayOf(value.calls, parseCallTrace) ?? [],
    match_count: optionalNumber(value.match_count),
  }
}

function parseCallTrace(value: unknown): CodeModeCallTrace | null {
  if (!isRecord(value)) return null
  return {
    id: stringValue(value.id, ''),
    upstream: stringValue(value.upstream, ''),
    tool: stringValue(value.tool, ''),
    ok: booleanValue(value.ok),
    elapsed_ms: numberValue(value.elapsed_ms, 0),
    params: value.params,
    error_kind: optionalString(value.error_kind),
  }
}

function parseSearchMatch(value: unknown): CodeModeSearchMatch | null {
  if (!isRecord(value)) return null
  return {
    id: stringValue(value.id, ''),
    upstream: stringValue(value.upstream, ''),
    tool: stringValue(value.tool, ''),
    description: stringValue(value.description, ''),
    has_schema: booleanValue(value.has_schema),
    has_output_schema: booleanValue(value.has_output_schema),
  }
}

function parseResultShape(value: unknown): ResultShape | undefined {
  if (!isRecord(value)) return undefined
  return {
    type: stringValue(value.type, 'unknown'),
    size_bytes: optionalNumber(value.size_bytes),
    length: optionalNumber(value.length),
    key_count: optionalNumber(value.key_count),
    keys: stringArray(value.keys),
    item_types: stringArray(value.item_types),
    truncated: booleanOptional(value.truncated),
    content_block_kinds: stringArray(value.content_block_kinds),
  }
}

function arrayOf<T>(value: unknown, parse: (item: unknown) => T | null): T[] | null {
  const result = arrayOfWithDropped(value, parse)
  return result?.items ?? null
}

function arrayOfWithDropped<T>(
  value: unknown,
  parse: (item: unknown) => T | null,
): { items: T[]; dropped: number } | null {
  if (!Array.isArray(value)) return null
  const items: T[] = []
  let dropped = 0
  for (const item of value) {
    const parsed = parse(item)
    if (parsed) {
      items.push(parsed)
    } else {
      dropped += 1
    }
  }
  return { items, dropped }
}

function stringArray(value: unknown): string[] | undefined {
  if (!Array.isArray(value)) return undefined
  return value.filter((item): item is string => typeof item === 'string')
}

function stringValue(value: unknown, fallback: string): string {
  return typeof value === 'string' ? value : fallback
}

function optionalString(value: unknown): string | undefined {
  return typeof value === 'string' ? value : undefined
}

function numberValue(value: unknown, fallback: number): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback
}

function optionalNumber(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined
}

function booleanValue(value: unknown): boolean {
  return value === true
}

function booleanOptional(value: unknown): boolean | undefined {
  return typeof value === 'boolean' ? value : undefined
}

function droppedWarning(count: number, label: string): CodeModeTraceWarning[] | undefined {
  if (count <= 0) return undefined
  return [
    {
      kind: 'dropped_rows',
      message: `Dropped ${count} malformed ${label}${count === 1 ? '' : 's'}.`,
    },
  ]
}

function truncateText(value: string, maxLength: number): string {
  return value.length <= maxLength ? value : `${value.slice(0, maxLength - 3)}...`
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}
