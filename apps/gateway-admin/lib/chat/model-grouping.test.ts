import test from 'node:test'
import assert from 'node:assert/strict'

import { groupModels, parseModelId } from './model-grouping'
import type { ACPModelOption } from '@/components/chat/types'

function option(id: string, name?: string): ACPModelOption {
  return { id, name: name ?? id }
}

test('parseModelId handles slash separator', () => {
  assert.deepEqual(parseModelId('gpt-5.5/medium'), { base: 'gpt-5.5', effort: 'medium' })
})

test('parseModelId handles paren separator after normalization', () => {
  assert.deepEqual(parseModelId('GPT-5.5 (medium)'), { base: 'GPT-5.5', effort: 'medium' })
})

test('parseModelId returns null for non-effort suffixes', () => {
  assert.equal(parseModelId('gpt-5.4-mini'), null)
  assert.equal(parseModelId('gpt-5.3-codex-spark'), null)
  assert.equal(parseModelId('Default (recommended)'), null)
  assert.equal(parseModelId('Auto (Gemini 3)'), null)
})

test('parseModelId rejects names containing slash that are not effort suffixes', () => {
  assert.equal(parseModelId('OpenCode Zen/Big Pickle'), null)
})

test('groupModels returns grouped result for codex-style list', () => {
  const opts = [
    option('gpt-5.5/low', 'GPT-5.5 (low)'),
    option('gpt-5.5/medium', 'GPT-5.5 (medium)'),
    option('gpt-5.5/high', 'GPT-5.5 (high)'),
    option('gpt-5.5/xhigh', 'GPT-5.5 (xhigh)'),
    option('gpt-5.4/low', 'GPT-5.4 (low)'),
  ]
  const result = groupModels(opts)
  assert.equal(result.kind, 'grouped')
  if (result.kind !== 'grouped') return
  assert.equal(result.groups.length, 2)
  assert.equal(result.groups[0].base, 'gpt-5.5')
  assert.equal(result.groups[0].variants.length, 4)
  assert.deepEqual(
    result.groups[0].variants.map((v) => v.effort),
    ['low', 'medium', 'high', 'xhigh'],
  )
})

test('groupModels returns flat for the claude default option', () => {
  assert.equal(groupModels([option('default', 'Default (recommended)')]).kind, 'flat')
})

test('groupModels returns flat if any option fails to parse', () => {
  const opts = [option('gpt-5.5/medium', 'GPT-5.5 (medium)'), option('gpt-5.4-mini', 'GPT-5.4-Mini')]
  assert.equal(groupModels(opts).kind, 'flat')
})

test('groupModels returns flat for empty and single-option lists', () => {
  assert.equal(groupModels([]).kind, 'flat')
  assert.equal(groupModels([option('x/medium', 'X')]).kind, 'flat')
})
