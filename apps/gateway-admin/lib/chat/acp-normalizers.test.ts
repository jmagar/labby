import test from 'node:test'
import assert from 'node:assert/strict'

import { deriveSessionTitle, sameProviderList, toRun, type RawSessionSummary } from './acp-normalizers'
import type { ProviderHealth } from '@/lib/acp/types'

function provider(modelName: string): ProviderHealth {
  return {
    provider: 'codex-acp',
    ready: true,
    command: 'codex-acp',
    args: [],
    message: '',
    models: [
      {
        id: 'gpt-5.4',
        name: modelName,
        description: 'balanced model',
        fixed: false,
      },
    ],
    defaultModelId: 'gpt-5.4',
    currentModelId: 'gpt-5.4',
  }
}

test('sameProviderList detects model metadata changes for stable model ids', () => {
  assert.equal(sameProviderList([provider('GPT 5.4')], [provider('GPT 5.4')]), true)
  assert.equal(sameProviderList([provider('GPT 5.4')], [provider('GPT 5.4 refreshed')]), false)
})

function rawSession(overrides: Partial<RawSessionSummary> = {}): RawSessionSummary {
  return {
    id: 'sess-1',
    provider: 'codex-acp',
    title: 'Real conversation title',
    cwd: '/tmp',
    createdAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
    modelId: 'gpt-5.5/medium',
    ...overrides,
  }
}

test('deriveSessionTitle keeps real titles unchanged', () => {
  assert.equal(deriveSessionTitle(rawSession()), 'Real conversation title')
})

test('deriveSessionTitle replaces "New session" placeholder with provider/model/time label', () => {
  const derived = deriveSessionTitle(rawSession({ title: 'New session' }))
  assert.notEqual(derived, 'New session')
  // Contains provider name, short model id, and a time-ago marker.
  assert.match(derived, /codex-acp/)
  assert.match(derived, /gpt-5\.5/)
  assert.match(derived, /now|ago/)
})

test('deriveSessionTitle replaces "action route session" placeholder', () => {
  const derived = deriveSessionTitle(rawSession({ title: 'action route session', modelId: null }))
  assert.notEqual(derived, 'action route session')
  assert.match(derived, /codex-acp/)
})

test('deriveSessionTitle replaces empty title', () => {
  const derived = deriveSessionTitle(rawSession({ title: '' }))
  assert.notEqual(derived, '')
})

test('deriveSessionTitle drops short model id when modelId missing', () => {
  const derived = deriveSessionTitle(rawSession({ title: 'New session', modelId: null, model_id: null }))
  assert.doesNotMatch(derived, /\/$/)
  assert.match(derived, /codex-acp/)
})

test('toRun applies derived title for placeholder sessions', () => {
  const run = toRun(rawSession({ title: 'New session' }))
  assert.notEqual(run.title, 'New session')
  assert.match(run.title, /codex-acp/)
})
