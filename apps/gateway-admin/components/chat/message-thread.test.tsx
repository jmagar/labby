import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { act } from 'react'
import { createRoot } from 'react-dom/client'
import { Window } from 'happy-dom'

import { MessageThread } from './message-thread'
import type { ACPMessage, ACPRun } from './types'

const window = new Window()
Object.defineProperty(globalThis, 'window', { value: window, configurable: true })
Object.defineProperty(globalThis, 'document', { value: window.document, configurable: true })
Object.defineProperty(globalThis, 'navigator', { value: window.navigator, configurable: true })
Object.defineProperty(globalThis, 'Node', { value: window.Node, configurable: true })
Object.defineProperty(globalThis, 'PointerEvent', { value: window.PointerEvent, configurable: true })
Object.defineProperty(globalThis, 'KeyboardEvent', { value: window.KeyboardEvent, configurable: true })
Object.defineProperty(globalThis, 'IS_REACT_ACT_ENVIRONMENT', { value: true, configurable: true })

function run(): ACPRun {
  return {
    id: 'run-1',
    projectId: 'workspace',
    agentId: 'codex',
    provider: 'codex-acp',
    title: 'Run',
    createdAt: new Date('2026-05-05T00:00:00Z'),
    updatedAt: new Date('2026-05-05T00:00:00Z'),
    status: 'idle',
    providerSessionId: 'provider-run-1',
    cwd: '/home/jmagar/workspace/lab',
  }
}

function message(id: string, text: string): ACPMessage {
  return {
    id,
    runId: 'run-1',
    role: 'user',
    text,
    createdAt: new Date('2026-05-05T00:00:00Z'),
    isStreaming: false,
    thoughts: [],
    toolCalls: [],
    version: 1,
  }
}

async function renderClient(element: React.ReactElement) {
  const container = document.createElement('div')
  document.body.appendChild(container)
  const root = createRoot(container)
  await act(async () => root.render(element))
  return {
    container,
    unmount: async () => {
      await act(async () => root.unmount())
      container.remove()
    },
  }
}

test('touch selection shows actions for one message and selecting another moves the row', async () => {
  const view = await renderClient(
    <MessageThread
      run={run()}
      messages={[message('m1', 'first'), message('m2', 'second')]}
      canRetryMessages
      canEditMessages
    />,
  )

  const bubbles = view.container.querySelectorAll('[data-message-id]')
  await act(async () => {
    bubbles[0]!.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch' }))
  })
  assert.equal(
    view.container.querySelector('[data-message-id="m1"] [aria-label="Message actions"]')?.getAttribute('data-selected'),
    'true',
  )

  await act(async () => {
    bubbles[1]!.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch' }))
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

test('escape dismisses selected mobile message actions', async () => {
  const view = await renderClient(
    <MessageThread run={run()} messages={[message('m1', 'first')]} canRetryMessages canEditMessages />,
  )

  const bubble = view.container.querySelector('[data-message-id="m1"]')!
  await act(async () => {
    bubble.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch' }))
  })
  assert.equal(view.container.querySelector('[aria-label="Message actions"]')?.getAttribute('data-selected'), 'true')

  await act(async () => {
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
  })
  assert.equal(view.container.querySelector('[aria-label="Message actions"]')?.getAttribute('data-selected'), 'false')

  await view.unmount()
})

test('outside pointer dismisses selected mobile message actions', async () => {
  const view = await renderClient(
    <MessageThread run={run()} messages={[message('m1', 'first')]} canRetryMessages canEditMessages />,
  )

  const bubble = view.container.querySelector('[data-message-id="m1"]')!
  await act(async () => {
    bubble.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch' }))
  })
  assert.equal(view.container.querySelector('[aria-label="Message actions"]')?.getAttribute('data-selected'), 'true')

  await act(async () => {
    document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch' }))
  })
  assert.equal(view.container.querySelector('[aria-label="Message actions"]')?.getAttribute('data-selected'), 'false')

  await view.unmount()
})
