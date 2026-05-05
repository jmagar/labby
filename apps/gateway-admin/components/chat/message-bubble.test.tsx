import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { act } from 'react'
import { createRoot } from 'react-dom/client'
import { renderToStaticMarkup } from 'react-dom/server'
import { Window } from 'happy-dom'

import { MessageBubble, getMessageActionAvailability, getMessageCopyText } from './message-bubble'
import type { ACPMessage } from './types'

const window = new Window()
Object.defineProperty(globalThis, 'window', { value: window, configurable: true })
Object.defineProperty(globalThis, 'document', { value: window.document, configurable: true })
Object.defineProperty(globalThis, 'navigator', { value: window.navigator, configurable: true })
Object.defineProperty(globalThis, 'DOMException', { value: window.DOMException, configurable: true })
Object.defineProperty(globalThis, 'IS_REACT_ACT_ENVIRONMENT', { value: true, configurable: true })

async function renderClient(element: React.ReactElement) {
  const container = document.createElement('div')
  document.body.appendChild(container)
  const root = createRoot(container)

  await act(async () => {
    root.render(element)
  })

  return {
    container,
    unmount: async () => {
      await act(async () => root.unmount())
      container.remove()
    },
  }
}

function assistantMessage(overrides: Partial<ACPMessage> = {}): ACPMessage {
  return {
    id: 'message-1',
    runId: 'run-1',
    role: 'assistant',
    text: 'Done.',
    createdAt: new Date('2026-05-04T12:00:00Z'),
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
    version: 1,
    ...overrides,
  }
}

function userMessage(overrides: Partial<ACPMessage> = {}): ACPMessage {
  return {
    id: 'message-user-1',
    runId: 'run-1',
    role: 'user',
    text: 'Hello.',
    createdAt: new Date('2026-05-04T12:00:00Z'),
    isStreaming: false,
    thoughts: [],
    toolCalls: [],
    version: 1,
    ...overrides,
  }
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
