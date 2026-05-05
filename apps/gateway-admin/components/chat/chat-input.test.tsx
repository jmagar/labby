import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import {
  CHAT_INPUT_MAX_HEIGHT_PX,
  ChatInput,
  resizeChatPromptTextarea,
} from './chat-input'

function textarea(scrollHeight: number) {
  const style: Record<string, string> = {}
  return {
    scrollHeight,
    scrollTop: 12,
    style,
  } as unknown as HTMLTextAreaElement
}

const selectedAgent = {
  id: 'codex-acp',
  name: 'Codex ACP',
  description: 'codex-acp over local ACP bridge',
  version: 'live',
  capabilities: [],
}

test('resizeChatPromptTextarea grows until max height then enables internal scroll', () => {
  const el = textarea(CHAT_INPUT_MAX_HEIGHT_PX + 80)

  resizeChatPromptTextarea(el)

  assert.equal(el.style.height, `${CHAT_INPUT_MAX_HEIGHT_PX}px`)
  assert.equal(el.style.overflowY, 'auto')
})

test('resizeChatPromptTextarea hides vertical overflow while content fits', () => {
  const el = textarea(88)

  resizeChatPromptTextarea(el)

  assert.equal(el.style.height, '88px')
  assert.equal(el.style.overflowY, 'hidden')
})

test('chat input textarea renders with max height and without whole-composer overflow behavior', () => {
  const markup = renderToStaticMarkup(
    <ChatInput
      onSend={() => {}}
      selectedAgent={selectedAgent}
      agents={[selectedAgent]}
      onSelectAgent={() => {}}
    />,
  )

  assert.match(markup, /aria-label="Message"/)
  assert.match(markup, new RegExp(`max-height:${CHAT_INPUT_MAX_HEIGHT_PX}px`))
  assert.doesNotMatch(markup, /overflow-y:hidden/)
  assert.doesNotMatch(markup, /overflow-hidden bg-transparent/)
})
