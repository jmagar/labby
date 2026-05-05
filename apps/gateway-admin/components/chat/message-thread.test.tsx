import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { act } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { MessageThread, reduceSelectedMessageId, shouldShowWorkingAssistantBubble } from './message-thread'
import { installChatTestDom, renderClient } from './test-utils'
import type { ACPMessage, ACPRun } from './types'

installChatTestDom()

const RUN_TIMESTAMP = new Date('2026-05-05T00:00:00Z')
const MESSAGE_TIMESTAMP = new Date('2026-05-05T00:00:01Z')

function run(status: ACPRun['status'] = 'running', overrides: Partial<ACPRun> = {}): ACPRun {
  return {
    id: 'run-1',
    projectId: 'workspace',
    agentId: 'codex',
    provider: 'codex-acp',
    title: 'Run',
    createdAt: RUN_TIMESTAMP,
    updatedAt: RUN_TIMESTAMP,
    status,
    providerSessionId: 'provider-run-1',
    cwd: '/home/jmagar/workspace/lab',
    ...overrides,
  }
}

function runWithId(id: string): ACPRun {
  return run('running', { id, providerSessionId: `provider-${id}` })
}

function message(overrides: Partial<ACPMessage> = {}): ACPMessage {
  return {
    id: 'message-1',
    runId: 'run-1',
    role: 'user',
    text: 'Please continue.',
    createdAt: MESSAGE_TIMESTAMP,
    isStreaming: false,
    thoughts: [],
    toolCalls: [],
    version: 1,
    ...overrides,
  }
}

test('message thread renders timestamp-ready bubbles with stable message ids', () => {
  const markup = renderToStaticMarkup(
    <MessageThread
      run={run('idle')}
      messages={[
        message({ id: 'm1', text: 'First', createdAt: new Date('2026-05-04T12:10:00Z') }),
        message({ id: 'm2', text: 'Second', createdAt: new Date('2026-05-04T12:20:00Z') }),
      ]}
    />,
  )

  assert.match(markup, /data-message-id="m1"/)
  assert.match(markup, /data-message-id="m2"/)
  assert.match(markup, /12:10 PM UTC/)
  assert.match(markup, /12:20 PM UTC/)
  assert.match(markup, /opacity-0 group-hover\/bubble:opacity-100 group-focus-within\/bubble:opacity-100/)
})

test('working assistant bubble logic remains unchanged for running sessions', () => {
  assert.equal(shouldShowWorkingAssistantBubble(null, [], 'open'), false)
  assert.equal(shouldShowWorkingAssistantBubble(run('idle'), [], 'open'), false)
  assert.equal(shouldShowWorkingAssistantBubble(run('waiting_for_permission'), [], 'open'), false)
  assert.equal(shouldShowWorkingAssistantBubble(run('running'), [], 'connecting'), true)
  assert.equal(shouldShowWorkingAssistantBubble(run('running'), [], 'open'), true)
  assert.equal(shouldShowWorkingAssistantBubble(run('running'), [], 'error'), false)
  assert.equal(
    shouldShowWorkingAssistantBubble(
      run('running'),
      [message({ id: 'assistant-1', role: 'assistant', text: 'Streaming', isStreaming: true })],
      'open',
    ),
    false,
  )
})

test('touch selection shows actions and timestamp for one message and selecting another moves the row', async () => {
  const view = await renderClient(
    <MessageThread
      run={run()}
      messages={[
        message({ id: 'm1', text: 'first' }),
        message({ id: 'm2', text: 'second' }),
      ]}
      canRetryMessages
      canEditMessages
    />,
  )

  const bubbles = view.container.querySelectorAll('[data-message-id]')
  await act(async () => {
    bubbles[0]!.click()
  })
  assert.equal(
    view.container.querySelector('[data-message-id="m1"] [aria-label="Message actions"]')?.getAttribute('data-selected'),
    'true',
  )
  assert.match(
    view.container.querySelector('[data-message-id="m1"] [data-message-timestamp]')?.getAttribute('class') ?? '',
    /(?:^|\s)opacity-100(?:\s|$)/,
  )

  await act(async () => {
    bubbles[1]!.click()
  })
  assert.equal(
    view.container.querySelector('[data-message-id="m1"] [aria-label="Message actions"]')?.getAttribute('data-selected'),
    'false',
  )
  assert.equal(
    view.container.querySelector('[data-message-id="m2"] [aria-label="Message actions"]')?.getAttribute('data-selected'),
    'true',
  )

  await view.unmount()
})

test('escape dismisses selected mobile message actions and timestamps', async () => {
  const view = await renderClient(
    <MessageThread run={run()} messages={[message({ id: 'm1', text: 'first' })]} canRetryMessages canEditMessages />,
  )

  const bubble = view.container.querySelector('[data-message-id="m1"]')!
  await act(async () => {
    bubble.dispatchEvent(new MouseEvent('click', { bubbles: true }))
  })
  assert.equal(view.container.querySelector('[aria-label="Message actions"]')?.getAttribute('data-selected'), 'true')

  await act(async () => {
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
  })
  assert.equal(view.container.querySelector('[aria-label="Message actions"]')?.getAttribute('data-selected'), 'false')

  await view.unmount()
})

test('outside pointer and run changes clear selected timestamps', async () => {
  const view = await renderClient(
    <MessageThread run={runWithId('run-1')} messages={[message({ id: 'shared-id', text: 'First' })]} connectionState="open" />,
  )
  const bubble = view.container.querySelector('[data-message-id="shared-id"]')!

  await act(async () => {
    bubble.dispatchEvent(new MouseEvent('click', { bubbles: true }))
  })
  assert.equal(view.container.querySelector('[aria-label="Message actions"]')?.getAttribute('data-selected'), 'true')

  await act(async () => {
    document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }))
  })
  assert.equal(view.container.querySelector('[aria-label="Message actions"]')?.getAttribute('data-selected'), 'false')

  await act(async () => {
    bubble.dispatchEvent(new MouseEvent('click', { bubbles: true }))
  })
  await view.rerender(
    <MessageThread
      run={runWithId('run-2')}
      messages={[message({ id: 'shared-id', text: 'Second run', runId: 'run-2' })]}
      connectionState="open"
    />,
  )
  assert.equal(view.container.querySelector('[aria-label="Message actions"]')?.getAttribute('data-selected'), 'false')

  await view.unmount()
})

test('timestamp selection state handles tap selection and dismiss paths', () => {
  assert.equal(reduceSelectedMessageId(null, { type: 'select', messageId: 'm1' }), 'm1')
  assert.equal(reduceSelectedMessageId('m1', { type: 'select', messageId: 'm2' }), 'm2')
  assert.equal(reduceSelectedMessageId('m1', { type: 'dismiss' }), null)
  assert.equal(reduceSelectedMessageId('m1', { type: 'run-change', runId: runWithId('run-2').id }), null)

  const selectedMarkup = renderToStaticMarkup(
    <MessageThread
      run={runWithId('run-2')}
      messages={[message({ id: 'shared-id', text: 'Second run', runId: 'run-2' })]}
      connectionState="open"
    />,
  )
  assert.doesNotMatch(selectedMarkup, /\sopacity-100"/)
})
