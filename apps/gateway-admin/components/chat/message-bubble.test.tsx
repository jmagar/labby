import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { act } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import {
  MessageBubble,
  WorkingAssistantBubble,
  areMessageBubblePropsEqual,
  getMessageActionAvailability,
  getMessageCopyText,
  getMessageTimestampLabels,
  shouldRenderMessageTimestamp,
} from './message-bubble'
import { installChatTestDom, renderClient } from './test-utils'
import type { ACPMessage } from './types'

installChatTestDom()

const MESSAGE_TIMESTAMP = new Date('2026-05-04T12:00:00Z')

function baseMessage(overrides: Partial<ACPMessage>): ACPMessage {
  return {
    id: 'message-1',
    runId: 'run-1',
    role: 'assistant',
    text: 'Done.',
    createdAt: MESSAGE_TIMESTAMP,
    isStreaming: false,
    thoughts: [],
    toolCalls: [],
    version: 1,
    ...overrides,
  }
}

function assistantMessage(overrides: Partial<ACPMessage> = {}): ACPMessage {
  return baseMessage({
    role: 'assistant',
    text: 'Done.',
    isStreaming: true,
    thoughts: ['Checked the current chat surface.'],
    toolCalls: [
      {
        id: 'tool-1',
        title: 'Read CLAUDE.md',
        status: 'completed',
        kind: 'read',
        input: { path: 'CLAUDE.md' },
        locations: ['CLAUDE.md'],
      },
      {
        id: 'tool-2',
        title: 'Read README.md',
        status: 'completed',
        kind: 'read',
        input: { path: 'README.md' },
        locations: ['README.md'],
      },
    ],
    ...overrides,
  })
}

function userMessage(overrides: Partial<ACPMessage> = {}): ACPMessage {
  return baseMessage({
    id: 'message-user-1',
    role: 'user',
    text: 'Hello.',
    ...overrides,
  })
}

test('renders reasoning summary separately from agent actions', () => {
  const markup = renderToStaticMarkup(<MessageBubble message={assistantMessage()} />)

  assert.match(markup, /Reasoning Summary/)
  assert.match(markup, /Reasoning/)
  assert.match(markup, /Checked the current chat surface/)
  assert.match(markup, /Agent Actions/)
  assert.match(markup, /Read 2 files/)
  assert.doesNotMatch(markup, /Chain of Thought/)

  assert.ok(
    markup.indexOf('Reasoning Summary') < markup.indexOf('Agent Actions'),
    'reasoning panel should render before the separate actions panel',
  )
})

test('renders assistant markdown as structured content', () => {
  const markup = renderToStaticMarkup(
    <MessageBubble
      message={assistantMessage({
        isStreaming: false,
        thoughts: [],
        toolCalls: [],
        text: [
          '## Heading',
          '',
          '- item',
          '',
          'Use `labby serve`.',
          '',
          '```ts',
          'const value = 1',
          '```',
          '',
          '[Docs](https://example.com/docs)',
        ].join('\n'),
      })}
    />,
  )

  assert.match(markup, /<h2/)
  assert.match(markup, /<li/)
  assert.match(markup, /<code/)
  assert.match(markup, /<pre/)
  assert.match(markup, /href="https:\/\/example.com\/docs"/)
  assert.doesNotMatch(markup, /## Heading/)
  assert.doesNotMatch(markup, /```ts/)
})

test('keeps user markdown and html as literal escaped text', () => {
  const markup = renderToStaticMarkup(
    <MessageBubble
      message={userMessage({
        text: '**bold**\n# heading\n<b>not html</b>',
      })}
    />,
  )

  assert.match(markup, /\*\*bold\*\*/)
  assert.match(markup, /# heading/)
  assert.match(markup, /&lt;b&gt;not html&lt;\/b&gt;/)
  assert.doesNotMatch(markup, /<strong/)
  assert.doesNotMatch(markup, /<h1/)
  assert.doesNotMatch(markup, /<b>not html<\/b>/)
})

test('drops assistant raw html instead of emitting live elements or attributes', () => {
  const markup = renderToStaticMarkup(
    <MessageBubble
      message={assistantMessage({
        isStreaming: false,
        thoughts: [],
        toolCalls: [],
        text: '<script>alert(1)</script><img src=x onerror=alert(1)><iframe src="https://evil.example"></iframe><form action="/x"></form><b>bold html</b>',
      })}
    />,
  )

  assert.doesNotMatch(markup, /<script/i)
  assert.doesNotMatch(markup, /<img[^>]*onerror/i)
  assert.doesNotMatch(markup, /<img/i)
  assert.doesNotMatch(markup, /<iframe/i)
  assert.doesNotMatch(markup, /<form/i)
  assert.doesNotMatch(markup, /<b>/i)
  assert.match(markup, /&lt;script&gt;alert\(1\)&lt;\/script&gt;/)
})

test('drops markdown images so assistant content cannot trigger image loads', () => {
  const markup = renderToStaticMarkup(
    <MessageBubble
      message={assistantMessage({
        isStreaming: false,
        thoughts: [],
        toolCalls: [],
        text: [
          '![remote](https://attacker.example/pixel.png)',
          '![data](data:image/svg+xml,<svg></svg>)',
          '![loopback](http://127.0.0.1/admin)',
          '![file](file:///etc/passwd)',
        ].join('\n'),
      })}
    />,
  )

  assert.doesNotMatch(markup, /<img/i)
  assert.doesNotMatch(markup, /attacker\.example/)
  assert.doesNotMatch(markup, /data:image/)
  assert.doesNotMatch(markup, /127\.0\.0\.1/)
  assert.doesNotMatch(markup, /file:\/\//)
})

test('neutralizes unsafe assistant links while preserving normal https links', () => {
  const markup = renderToStaticMarkup(
    <MessageBubble
      message={assistantMessage({
        isStreaming: false,
        thoughts: [],
        toolCalls: [],
        text: [
          '[ok](https://example.com)',
          '[script](javascript:alert(1))',
          '[data](data:text/html;base64,PGgxPkJvb208L2gxPg==)',
          '[protocol](//evil.example/path)',
        ].join('\n'),
      })}
    />,
  )

  assert.match(markup, /href="https:\/\/example.com"/)
  assert.doesNotMatch(markup, /href="javascript:/i)
  assert.doesNotMatch(markup, /href="data:/i)
  assert.doesNotMatch(markup, /href="\/\/evil\.example/i)
})

test('copies raw message text rather than rendered markdown text', () => {
  const message = assistantMessage({
    text: '[Docs](https://example.com)\n\n```sh\nlabby serve\n```',
  })

  assert.equal(getMessageCopyText(message), message.text)
})

test('derives message actions from role, text, and callback availability', () => {
  assert.deepEqual(
    getMessageActionAvailability(userMessage({ text: 'Retry me.' }), {
      canRetry: true,
      canEdit: true,
    }),
    { copy: true, retry: true, edit: true },
  )

  assert.deepEqual(
    getMessageActionAvailability(assistantMessage({ text: 'Assistant.' }), {
      canRetry: true,
      canEdit: true,
    }),
    { copy: true, retry: false, edit: false },
  )

  assert.deepEqual(
    getMessageActionAvailability(userMessage({ text: '   ' }), {
      canRetry: true,
      canEdit: true,
    }),
    { copy: false, retry: false, edit: false },
  )
})

test('renders message actions under the bubble and right aligned', () => {
  const markup = renderToStaticMarkup(
    <MessageBubble
      message={userMessage({ text: 'Can you retry this?' })}
      actionState={{ selected: false, canRetry: true, canEdit: true }}
    />,
  )

  assert.match(markup, /aria-label="Message actions"/)
  assert.match(markup, /role="group"/)
  assert.match(markup, /Copy message/)
  assert.match(markup, /Retry message/)
  assert.match(markup, /Edit message/)
  assert.match(markup, /justify-end/)
  assert.ok(
    markup.indexOf('Can you retry this?') < markup.indexOf('aria-label="Message actions"'),
    'actions should render after the message content',
  )
})

test('omits retry and edit for assistant messages', () => {
  const markup = renderToStaticMarkup(
    <MessageBubble
      message={assistantMessage({ isStreaming: false, thoughts: [], toolCalls: [] })}
      actionState={{ selected: false, canRetry: true, canEdit: true }}
    />,
  )

  assert.match(markup, /Copy message/)
  assert.doesNotMatch(markup, /Retry message/)
  assert.doesNotMatch(markup, /Edit message/)
})

test('copy action writes raw message text and shows copied state', async () => {
  const writes: string[] = []
  Object.defineProperty(navigator, 'clipboard', {
    value: {
      writeText: async (value: string) => {
        writes.push(value)
      },
    },
    configurable: true,
  })

  const view = await renderClient(
    <MessageBubble
      message={assistantMessage({ text: '**raw** markdown', isStreaming: false, thoughts: [], toolCalls: [] })}
      actionState={{ selected: true }}
    />,
  )

  const button = view.container.querySelector('button[aria-label="Copy message"]') as HTMLButtonElement
  await act(async () => {
    button.click()
  })

  assert.deepEqual(writes, ['**raw** markdown'])
  assert.match(view.container.textContent ?? '', /Copied/)
  await view.unmount()
})

test('copy action exposes failure state when clipboard write is denied', async () => {
  Object.defineProperty(navigator, 'clipboard', {
    value: {
      writeText: async () => {
        throw new DOMException('Denied', 'NotAllowedError')
      },
    },
    configurable: true,
  })

  const view = await renderClient(
    <MessageBubble
      message={assistantMessage({ text: 'copy me', isStreaming: false, thoughts: [], toolCalls: [] })}
      actionState={{ selected: true }}
    />,
  )

  const button = view.container.querySelector('button[aria-label="Copy message"]') as HTMLButtonElement
  await act(async () => {
    button.click()
  })

  assert.match(button.getAttribute('aria-label') ?? '', /Copy failed/)
  await view.unmount()
})

test('formats message timestamp labels from createdAt metadata', () => {
  const labels = getMessageTimestampLabels(userMessage({
    createdAt: new Date('2026-05-04T12:34:00Z'),
  }))

  assert.equal(labels.visible, '12:34 PM UTC')
  assert.equal(labels.detail, 'May 4, 2026, 12:34 PM UTC')
})

test('omits timestamp presentation when createdAt is invalid', () => {
  const message = userMessage({ createdAt: new Date(Number.NaN) })

  assert.equal(shouldRenderMessageTimestamp(message), false)
})

test('renders timestamp row after message content without overlapping copy action', () => {
  const markup = renderToStaticMarkup(
    <MessageBubble message={userMessage({ createdAt: new Date('2026-05-04T12:34:00Z') })} />,
  )

  assert.match(markup, /aria-label="Message sent at May 4, 2026, 12:34 PM UTC"/)
  assert.match(markup, /12:34 PM UTC/)
  assert.match(markup, /data-message-timestamp/)
  assert.match(markup, /group-hover\/bubble:opacity-100/)
  assert.match(markup, /group-focus-within\/bubble:opacity-100/)
  assert.ok(
    markup.indexOf('Hello.') < markup.indexOf('data-message-timestamp'),
    'timestamp should render after the message text',
  )
  assert.match(markup, /aria-label="Message actions"/)
})

test('renders timestamp for assistant messages with only tool activity', () => {
  const markup = renderToStaticMarkup(
    <MessageBubble
      message={assistantMessage({
        text: '',
        isStreaming: false,
        thoughts: [],
        toolCalls: [
          {
            id: 'tool-only',
            title: 'Read file',
            status: 'completed',
            locations: ['README.md'],
          },
        ],
        createdAt: new Date('2026-05-04T12:45:00Z'),
      })}
    />,
  )

  assert.match(markup, /data-message-timestamp/)
  assert.match(markup, /12:45 PM UTC/)
})

test('selected message renders timestamp as visible for touch interaction', () => {
  const markup = renderToStaticMarkup(<MessageBubble message={userMessage()} actionState={{ selected: true }} />)

  assert.match(markup, /data-message-id="message-user-1"/)
  assert.match(markup, /opacity-100/)
})

test('message bubble memo comparison includes timestamp and selected state', () => {
  const base = userMessage({ createdAt: new Date('2026-05-04T12:00:00Z') })
  const changedTimestamp = { ...base, createdAt: new Date('2026-05-04T12:01:00Z') }

  assert.equal(
    areMessageBubblePropsEqual({ message: base }, { message: changedTimestamp }),
    false,
  )
  assert.equal(
    areMessageBubblePropsEqual({ message: base, actionState: { selected: false } }, { message: base, actionState: { selected: true } }),
    false,
  )
})

test('message bubble memo comparison treats invalid timestamps as stable', () => {
  const invalid = userMessage({ createdAt: new Date(Number.NaN) })
  const nextInvalid = { ...invalid, createdAt: new Date(Number.NaN) }

  assert.equal(
    areMessageBubblePropsEqual({ message: invalid }, { message: nextInvalid }),
    true,
  )
})

test('keeps the streaming cursor adjacent to assistant markdown content', () => {
  const markup = renderToStaticMarkup(
    <MessageBubble
      message={assistantMessage({
        text: '## Streaming\n\n- first',
        thoughts: [],
        toolCalls: [],
        isStreaming: true,
      })}
    />,
  )

  assert.match(markup, /<h2/)
  assert.match(markup, /animate-pulse/)
  assert.match(markup, /bg-aurora-accent-primary/)
})

test('keeps large fenced code blocks in an overflow-safe markdown container', () => {
  const longCommand = `labby ${'x'.repeat(240)}`
  const markup = renderToStaticMarkup(
    <MessageBubble
      message={assistantMessage({
        isStreaming: false,
        thoughts: [],
        toolCalls: [],
        text: ['```sh', longCommand, '```'].join('\n'),
      })}
    />,
  )

  assert.match(markup, /<pre/)
  assert.match(markup, /overflow-x-auto/)
  assert.match(markup, new RegExp(longCommand))
  assert.doesNotMatch(markup, /```sh/)
})

test('renders assistant working placeholder as a stable assistant bubble', () => {
  const markup = renderToStaticMarkup(<WorkingAssistantBubble label="Codex is working" />)

  assert.match(markup, /Codex is working/)
  assert.match(markup, /aria-label="Codex is working"/)
  assert.match(markup, /role="status"/)
  assert.match(markup, /group\/bubble flex min-w-0 gap-3/)
  assert.match(markup, /border-aurora-accent-primary\/30/)
  assert.match(markup, /animate-pulse/)
  assert.match(markup, /motion-reduce:animate-none/)
  assert.match(markup, /h-2 rounded-full/)
  assert.doesNotMatch(markup, /Copy message/)
})
