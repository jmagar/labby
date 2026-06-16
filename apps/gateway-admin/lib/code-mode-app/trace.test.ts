import assert from 'node:assert/strict'
import { test } from 'node:test'

import {
  describeResultShape,
  flattenTraceRows,
  parseCodeModeTrace,
  stringifyRedactedParams,
} from './trace'

test('parses execute traces with redacted params', () => {
  const trace = parseCodeModeTrace({
    kind: 'code_mode_execute_trace',
    call_count: 1,
    calls: [
      {
        id: 'github::search_issues',
        upstream: 'github',
        tool: 'search_issues',
        ok: true,
        elapsed_ms: 12,
        params: { query: 'bug', token: '[redacted]' },
      },
    ],
    result_shape: { type: 'object', key_count: 2 },
  })

  assert.equal(trace?.kind, 'code_mode_execute_trace')
  const rows = flattenTraceRows(trace)
  assert.equal(rows.calls.length, 1)
  assert.equal(stringifyRedactedParams(rows.calls[0].params).includes('[redacted]'), true)
})

test('parses search traces with matched tools', () => {
  const trace = parseCodeModeTrace({
    kind: 'code_mode_search_trace',
    query_kind: 'catalog_filter',
    match_count: 1,
    matches: [
      {
        id: 'axon::ask',
        upstream: 'axon',
        tool: 'ask',
        description: 'Ask indexed docs',
        has_schema: true,
        has_output_schema: false,
      },
    ],
  })

  assert.equal(trace?.kind, 'code_mode_search_trace')
  assert.equal(flattenTraceRows(trace).matches[0].tool, 'ask')
})

test('parses search traces that carry a reduced result value', () => {
  const trace = parseCodeModeTrace({
    kind: 'code_mode_search_trace',
    query_kind: 'catalog_filter',
    match_count: 0,
    matches: [],
    result_shape: { type: 'object', key_count: 2, keys: ['total', 'upstreams'] },
    result: { total: 398, upstreams: 42 },
  })

  assert.equal(trace?.kind, 'code_mode_search_trace')
  assert.equal(flattenTraceRows(trace).matches.length, 0)
  assert.deepEqual(
    trace?.kind === 'code_mode_search_trace' ? trace.result : undefined,
    { total: 398, upstreams: 42 },
  )
})

test('describes result shapes for the no-match search view', () => {
  assert.equal(
    describeResultShape({ type: 'object', key_count: 2, keys: ['total', 'upstreams'] }),
    'object · 2 keys — keys: total, upstreams',
  )
  assert.equal(
    describeResultShape({ type: 'array', length: 3, item_types: ['string'] }),
    'array · 3 items — items: string',
  )
  assert.equal(describeResultShape(undefined), '')
})

test('parses history traces and flattens nested execute calls', () => {
  const trace = parseCodeModeTrace({
    kind: 'code_mode_history',
    entries: [
      {
        seq: 7,
        kind: 'execute',
        ok: true,
        elapsed_ms: 24,
        calls: [
          {
            id: 'github::search_issues',
            upstream: 'github',
            tool: 'search_issues',
            ok: true,
            elapsed_ms: 12,
          },
        ],
      },
      {
        seq: 8,
        kind: 'search',
        ok: false,
        elapsed_ms: 4,
        error_kind: 'invalid_query',
        match_count: 0,
      },
    ],
  })

  assert.equal(trace?.kind, 'code_mode_history')
  const rows = flattenTraceRows(trace)
  assert.equal(rows.history.length, 2)
  assert.equal(rows.calls.length, 1)
  assert.equal(rows.calls[0].tool, 'search_issues')
})

test('reports dropped malformed history rows', () => {
  const trace = parseCodeModeTrace({
    kind: 'code_mode_history',
    entries: [
      { seq: 1, kind: 'search', ok: true, elapsed_ms: 3 },
      { seq: 2, kind: 'unknown', ok: true, elapsed_ms: 3 },
    ],
  })

  assert.equal(trace?.kind, 'code_mode_history')
  assert.equal(trace?.entries.length, 1)
  assert.deepEqual(trace?.warnings, [
    { kind: 'dropped_rows', message: 'Dropped 1 malformed history entry.' },
  ])
})

test('accepts only literal booleans for status fields', () => {
  const trace = parseCodeModeTrace({
    kind: 'code_mode_execute_trace',
    call_count: 1,
    calls: [
      {
        id: 'github::search_issues',
        upstream: 'github',
        tool: 'search_issues',
        ok: 'false',
        elapsed_ms: 12,
      },
    ],
  })

  assert.equal(trace?.kind, 'code_mode_execute_trace')
  assert.equal(flattenTraceRows(trace).calls[0].ok, false)
})

test('stringifies unsupported params without throwing', () => {
  const cyclic: { child?: unknown } = {}
  cyclic.child = cyclic

  const params = stringifyRedactedParams(cyclic)

  assert.match(params, /^\[unsupported params:/)
  assert.ok(params.length < 160)
})

test('parses search truncation metadata', () => {
  const trace = parseCodeModeTrace({
    kind: 'code_mode_search_trace',
    query_kind: 'catalog_filter',
    displayed_count: 50,
    truncated: true,
    match_count: 200,
    matches: [],
  })

  assert.equal(trace?.kind, 'code_mode_search_trace')
  assert.equal(trace?.displayed_count, 50)
  assert.equal(trace?.truncated, true)
  assert.equal(trace?.match_count, 200)
})

test('rejects unknown trace shapes', () => {
  assert.equal(parseCodeModeTrace({ kind: 'tool_explorer' }), null)
})
