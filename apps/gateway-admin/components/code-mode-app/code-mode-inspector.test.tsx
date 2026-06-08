import assert from 'node:assert/strict'
import { test } from 'node:test'

import { act } from 'react'
import React from 'react'

import { CodeModeInspector } from './code-mode-inspector'
import { installChatTestDom, renderClient } from '@/components/chat/test-utils'

test('renders execute call rows with redacted params', async () => {
  installChatTestDom()
  const { container, unmount } = await renderClient(
    <CodeModeInspector
      initialTrace={{
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
      }}
    />,
  )

  assert.match(container.textContent ?? '', /Broker-observed execute calls/)
  assert.match(container.textContent ?? '', /github \/ search_issues/)
  assert.match(container.textContent ?? '', /12ms/)
  assert.match(container.textContent ?? '', /\[redacted\]/)
  assert.doesNotMatch(container.textContent ?? '', /raw-secret-token/)
  await unmount()
})

test('renders search match rows', async () => {
  installChatTestDom()
  const { container, unmount } = await renderClient(
    <CodeModeInspector
      initialTrace={{
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
      }}
    />,
  )

  assert.match(container.textContent ?? '', /Catalog-inferred search matches/)
  assert.match(container.textContent ?? '', /axon \/ ask/)
  assert.match(container.textContent ?? '', /schema/)
  await unmount()
})

test('renders search truncation count metadata', async () => {
  installChatTestDom()
  const { container, unmount } = await renderClient(
    <CodeModeInspector
      initialTrace={{
        kind: 'code_mode_search_trace',
        query_kind: 'catalog_filter',
        displayed_count: 50,
        truncated: true,
        match_count: 200,
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
      }}
    />,
  )

  assert.match(container.textContent ?? '', /50 of 200/)
  assert.match(container.textContent ?? '', /truncated/)
  await unmount()
})

test('renders history rows and flattened nested calls', async () => {
  installChatTestDom()
  const { container, unmount } = await renderClient(
    <CodeModeInspector
      initialTrace={{
        kind: 'code_mode_history',
        entries: [
          {
            seq: 7,
            kind: 'execute',
            ok: 'false',
            elapsed_ms: 24,
            error_kind: 'tool_failed',
            calls: [
              {
                id: 'github::search_issues',
                upstream: 'github',
                tool: 'search_issues',
                ok: 'false',
                elapsed_ms: 12,
                error_kind: 'tool_failed',
              },
            ],
          },
        ],
      }}
    />,
  )

  assert.match(container.textContent ?? '', /Recent history/)
  assert.match(container.textContent ?? '', /#7 execute/)
  assert.match(container.textContent ?? '', /tool_failed/)
  assert.match(container.textContent ?? '', /github \/ search_issues/)
  assert.doesNotMatch(container.textContent ?? '', /\bok\b/)
  await unmount()
})

test('renders parser warnings for dropped history rows', async () => {
  installChatTestDom()
  const { container, unmount } = await renderClient(
    <CodeModeInspector
      initialTrace={{
        kind: 'code_mode_history',
        entries: [
          { seq: 1, kind: 'search', ok: true, elapsed_ms: 3 },
          { seq: 2, kind: 'unknown', ok: true, elapsed_ms: 3 },
        ],
      }}
    />,
  )

  assert.match(container.textContent ?? '', /Dropped 1 malformed history entry/)
  await unmount()
})

test('updates from bridge tool results using both structured content field names', async () => {
  installChatTestDom()
  let instance:
    | {
        ontoolresult?: (result: { structuredContent?: unknown; structured_content?: unknown }) => void
        connect: () => Promise<unknown>
      }
    | undefined

  globalThis.window.ExtApps = {
    App: class {
      ontoolresult?: (result: { structuredContent?: unknown; structured_content?: unknown }) => void
      connect = async () => ({})
      constructor() {
        instance = this
      }
    },
  }

  const { container, unmount } = await renderClient(<CodeModeInspector />)

  await act(async () => {
    instance?.ontoolresult?.({
      structuredContent: {
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
      },
    })
  })

  assert.match(container.textContent ?? '', /axon \/ ask/)

  await act(async () => {
    instance?.ontoolresult?.({
      structured_content: {
        kind: 'code_mode_execute_trace',
        call_count: 1,
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
    })
  })

  assert.match(container.textContent ?? '', /github \/ search_issues/)
  await unmount()
})

test('renders a warning for malformed bridge payloads', async () => {
  installChatTestDom()
  let instance:
    | {
        ontoolresult?: (result: { structuredContent?: unknown; structured_content?: unknown }) => void
        connect: () => Promise<unknown>
      }
    | undefined

  globalThis.window.ExtApps = {
    App: class {
      ontoolresult?: (result: { structuredContent?: unknown; structured_content?: unknown }) => void
      connect = async () => ({})
      constructor() {
        instance = this
      }
    },
  }

  const { container, unmount } = await renderClient(<CodeModeInspector />)

  await act(async () => {
    instance?.ontoolresult?.({ structuredContent: { kind: 'tool_explorer' } })
  })

  assert.match(container.textContent ?? '', /Ignored malformed bridge payload/)
  await unmount()
})

test('renders empty state without bridge data', async () => {
  installChatTestDom()
  const { container, unmount } = await renderClient(<CodeModeInspector />)

  assert.match(container.textContent ?? '', /Waiting for an MCP Apps tool result/)
  await unmount()
})
