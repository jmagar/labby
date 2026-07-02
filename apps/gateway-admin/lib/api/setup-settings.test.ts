// Tests for the schema-backed settings client contract.

import test from 'node:test'
import assert from 'node:assert/strict'

import {
  MOCK_ENV_SCHEMA,
  MOCK_SETTINGS_SCHEMA,
  setupApi,
  type SettingsState,
  type SettingsUpdateEntry,
} from './setup-client'

function isSettingsState(v: unknown): v is SettingsState {
  if (typeof v !== 'object' || v === null) return false
  const obj = v as Record<string, unknown>
  return (
    typeof obj.schema_version === 'number' &&
    typeof obj.config_path === 'string' &&
    typeof obj.env_path === 'string' &&
    typeof obj.section === 'string' &&
    typeof obj.values === 'object' &&
    obj.values !== null &&
    typeof obj.sources === 'object' &&
    obj.sources !== null
  )
}

test('settings schema carries risk source and write policy metadata', () => {
  const field = MOCK_SETTINGS_SCHEMA.fields.find((item) => item.key === 'services.built_in_upstream_apis_enabled')
  assert.equal(field?.write_policy, 'editable')
  assert.equal(field?.apply_mode, 'immediate')
  assert.equal(field?.backend, 'config_toml')
})

test('env schema marks token secret and not editable', () => {
  const token = MOCK_ENV_SCHEMA.find((item) => item.key === 'LAB_MCP_HTTP_TOKEN')
  assert.equal(token?.secret, true)
  assert.equal(token?.editable, false)
})

test('settings state contract is section scoped', () => {
  const state: SettingsState = {
    schema_version: 1,
    config_path: '~/.config/labby/config.toml',
    env_path: '~/.labby/.env',
    section: 'core',
    values: { LAB_LOG: 'labby=info' },
    sources: { LAB_LOG: { source: 'env', overridden_by_env: null } },
  }
  assert.equal(isSettingsState(state), true)
  assert.equal(state.section, 'core')
})

test('settings update entries support explicit unset', () => {
  const entry: SettingsUpdateEntry = { key: 'mcp.port', value: null, previous: 8765, unset: true }
  assert.equal(entry.unset, true)
})

test('settingsConfigUpdate sends confirm and entries through setup action transport', async () => {
  const originalFetch = globalThis.fetch
  let requestBody: unknown
  globalThis.fetch = (async (_input, init) => {
    requestBody = JSON.parse(String(init?.body))
    return new Response(JSON.stringify({
      state: {
        schema_version: 1,
        config_path: '/tmp/config.toml',
        env_path: '/tmp/.env',
        section: 'surfaces',
        values: {},
        sources: {},
      },
      backup_path: null,
    }), { status: 200 })
  }) as typeof fetch

  try {
    const entries: SettingsUpdateEntry[] = [{ key: 'mcp.port', value: 8766, previous: 8765 }]
    await setupApi.settingsConfigUpdate('surfaces', entries, true)
    assert.deepEqual(requestBody, {
      action: 'settings.config.update',
      params: { section: 'surfaces', entries, confirm: true },
    })
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('settingsEnvUpdate sends confirm and entries through setup action transport', async () => {
  const originalFetch = globalThis.fetch
  let requestBody: unknown
  globalThis.fetch = (async (_input, init) => {
    requestBody = JSON.parse(String(init?.body))
    return new Response(JSON.stringify({
      schema_version: 1,
      config_path: '/tmp/config.toml',
      env_path: '/tmp/.env',
      section: 'core',
      values: {},
      sources: {},
    }), { status: 200 })
  }) as typeof fetch

  try {
    const entries: SettingsUpdateEntry[] = [{ key: 'LAB_LOG', value: 'labby=debug', previous: 'labby=info' }]
    await setupApi.settingsEnvUpdate('core', entries, true)
    assert.deepEqual(requestBody, {
      action: 'settings.env.update',
      params: { section: 'core', entries, confirm: true },
    })
  } finally {
    globalThis.fetch = originalFetch
  }
})
