import test from 'node:test'
import assert from 'node:assert/strict'
import { renderToStaticMarkup } from 'react-dom/server'

import { SettingsScalarField } from './SettingsScalarField'
import type { SettingsFieldSpec, SettingsState } from '@/lib/api/setup-client'

const field: SettingsFieldSpec = {
  key: 'LAB_LOG',
  label: 'Log filter',
  description: 'Tracing filter directive.',
  section: 'core',
  backend: 'env',
  control: 'text',
  risk: 'restart',
  write_policy: 'editable',
  apply_mode: 'restart',
  secret: false,
  required: false,
  env_override: null,
  min: null,
  max: null,
  options: [],
  example: 'labby=info',
}

const state: SettingsState = {
  schema_version: 1,
  config_path: '/tmp/config.toml',
  env_path: '/tmp/.env',
  section: 'core',
  values: { LAB_LOG: 'labby=info' },
  sources: { LAB_LOG: { source: 'env', overridden_by_env: null } },
}

test('SettingsScalarField renders scalar metadata and value', () => {
  const html = renderToStaticMarkup(
    <SettingsScalarField field={field} value="labby=info" state={state} onChange={() => undefined} />,
  )
  assert.match(html, /Log filter/)
  assert.match(html, /LAB_LOG/)
  assert.match(html, /labby=info/)
})

test('SettingsScalarField disables config values shadowed by env overrides', () => {
  const configField: SettingsFieldSpec = {
    ...field,
    key: 'mcp.port',
    backend: 'config_toml',
    control: 'number',
    env_override: 'LAB_MCP_HTTP_PORT',
    min: 1,
    max: 65535,
  }
  const configState: SettingsState = {
    ...state,
    values: { 'mcp.port': 9000 },
    sources: { 'mcp.port': { source: 'env', overridden_by_env: 'LAB_MCP_HTTP_PORT' } },
  }
  const html = renderToStaticMarkup(
    <SettingsScalarField field={configField} value={9000} state={configState} onChange={() => undefined} />,
  )
  assert.match(html, /disabled/)
  assert.match(html, /Edit the env var or remove the override first/)
})
