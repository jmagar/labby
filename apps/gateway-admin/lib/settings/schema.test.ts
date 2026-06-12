import test from 'node:test'
import assert from 'node:assert/strict'

import { buildDirtyEntries, parseFieldInput, valueAsInputString } from './schema'
import type { SettingsFieldSpec } from '@/lib/api/setup-client'

const numberField: SettingsFieldSpec = {
  key: 'mcp.port',
  label: 'Port',
  description: '',
  section: 'surfaces',
  backend: 'config_toml',
  control: 'number',
  risk: 'restart',
  write_policy: 'editable',
  apply_mode: 'restart',
  secret: false,
  required: false,
  env_override: 'LAB_MCP_HTTP_PORT',
  min: 1,
  max: 65535,
  options: [],
  example: '8765',
}

test('settings schema helpers parse scalar controls', () => {
  assert.equal(parseFieldInput(numberField, '8765'), 8765)
  assert.equal(parseFieldInput(numberField, '1.5'), null)
  assert.equal(parseFieldInput(numberField, '70000'), null)
  assert.equal(parseFieldInput({ ...numberField, control: 'bool' }, true), true)
  assert.deepEqual(parseFieldInput({ ...numberField, control: 'string_list' }, 'a,b\nc'), ['a', 'b', 'c'])
})

test('settings schema helpers build dirty entries only for changed keys', () => {
  assert.deepEqual(buildDirtyEntries([numberField], new Set(['mcp.port']), { 'mcp.port': 8766 }, { 'mcp.port': 8765 }), [
    { key: 'mcp.port', value: 8766, previous: 8765 },
  ])
})

test('settings schema helpers do not stringify arrays as objects', () => {
  assert.equal(valueAsInputString(['a', 'b']), 'a\nb')
})
