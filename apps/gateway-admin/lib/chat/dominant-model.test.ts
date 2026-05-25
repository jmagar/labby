import test from 'node:test'
import assert from 'node:assert/strict'

import { dominantModelId } from './dominant-model'
import type { ACPRun } from '@/components/chat/types'

function run(modelId: string | null): ACPRun {
  return { modelId } as ACPRun
}

test('empty list returns null', () => {
  assert.equal(dominantModelId([]), null)
})

test('single run returns its modelId', () => {
  assert.equal(dominantModelId([run('gpt-5.5')]), 'gpt-5.5')
})

test('strict majority returns the majority id', () => {
  assert.equal(
    dominantModelId([run('gpt-5.5'), run('gpt-5.5'), run('gpt-5.5'), run('claude')]),
    'gpt-5.5',
  )
})

test('exact 50/50 returns null', () => {
  assert.equal(dominantModelId([run('a'), run('a'), run('b'), run('b')]), null)
})

test('all distinct returns null', () => {
  assert.equal(dominantModelId([run('a'), run('b'), run('c')]), null)
})

test('null modelIds count in the denominator', () => {
  // 2/3 is strict majority for 'gpt-5.5' since floor(3/2)+1 = 2.
  assert.equal(dominantModelId([run('gpt-5.5'), run('gpt-5.5'), run(null)]), 'gpt-5.5')
})

test('null dominant returns null (no badge string)', () => {
  // null is the majority — treat as "no dominant" so badges still render.
  assert.equal(dominantModelId([run(null), run(null), run(null), run('gpt-5.5')]), null)
})
