import test from 'node:test'
import assert from 'node:assert/strict'

import {
  buildDirtyEntries,
  buildDirtyEntriesByBackend,
  collectFieldInputErrors,
  isInvalidFieldInput,
  parseFieldInput,
  valueAsInputString,
} from './schema'
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
  assert.equal(parseFieldInput(numberField, ''), null)
  assert.equal(isInvalidFieldInput(parseFieldInput(numberField, '1.5')), true)
  assert.equal(isInvalidFieldInput(parseFieldInput(numberField, '70000')), true)
  assert.equal(parseFieldInput({ ...numberField, control: 'bool' }, true), true)
  assert.deepEqual(parseFieldInput({ ...numberField, control: 'string_list' }, 'a,b\nc'), ['a', 'b', 'c'])
})

test('settings schema helpers surface invalid numeric errors without losing raw input', () => {
  const invalid = parseFieldInput(numberField, '70000')
  assert.equal(isInvalidFieldInput(invalid), true)
  assert.equal(valueAsInputString(invalid), '70000')
  assert.deepEqual(
    collectFieldInputErrors([numberField], new Set(['mcp.port']), { 'mcp.port': invalid }),
    { 'mcp.port': 'Must be at most 65535.' },
  )
})

test('settings schema helpers build dirty entries only for changed keys', () => {
  assert.deepEqual(buildDirtyEntries([numberField], new Set(['mcp.port']), { 'mcp.port': 8766 }, { 'mcp.port': 8765 }), [
    { key: 'mcp.port', value: 8766, previous: 8765 },
  ])
})

test('settings schema helpers emit unset for blank optional config fields', () => {
  const pathField: SettingsFieldSpec = {
    ...numberField,
    key: 'workspace.root',
    control: 'text',
    env_override: null,
    min: null,
    max: null,
  }
  assert.deepEqual(buildDirtyEntries([pathField], new Set(['workspace.root']), { 'workspace.root': '' }, { 'workspace.root': '/srv/lab' }), [
    { key: 'workspace.root', value: null, previous: '/srv/lab', unset: true },
  ])
})

test('settings schema helpers partition dirty entries by backend', () => {
  const envField: SettingsFieldSpec = { ...numberField, key: 'LAB_MCP_HTTP_PORT', backend: 'env' }
  const partitioned = buildDirtyEntriesByBackend(
    [numberField, envField],
    new Set(['mcp.port', 'LAB_MCP_HTTP_PORT']),
    { 'mcp.port': 8766, LAB_MCP_HTTP_PORT: 8767 },
    { 'mcp.port': 8765, LAB_MCP_HTTP_PORT: 8765 },
  )
  assert.deepEqual(partitioned.configEntries, [{ key: 'mcp.port', value: 8766, previous: 8765 }])
  assert.deepEqual(partitioned.envEntries, [{ key: 'LAB_MCP_HTTP_PORT', value: 8767, previous: 8765 }])
})

test('settings schema helpers exclude config fields shadowed by env overrides', () => {
  const partitioned = buildDirtyEntriesByBackend(
    [numberField],
    new Set(['mcp.port']),
    { 'mcp.port': 8766 },
    { 'mcp.port': 8765 },
    { 'mcp.port': { source: 'env', overridden_by_env: 'LAB_MCP_HTTP_PORT' } },
  )
  assert.deepEqual(partitioned.configEntries, [])
  assert.deepEqual(partitioned.envEntries, [])
})

test('settings schema helpers do not stringify arrays as objects', () => {
  assert.equal(valueAsInputString(['a', 'b']), 'a\nb')
})
