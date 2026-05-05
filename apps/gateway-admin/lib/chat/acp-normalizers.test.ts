import test from 'node:test'
import assert from 'node:assert/strict'

import { sameProviderList } from './acp-normalizers'
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
