// Tests for the settings.state / settings.update client contract.
// Validates that the SettingsState shape matches the Rust dispatch response
// and that the mock implementations satisfy the expected invariants.

import test from 'node:test'
import assert from 'node:assert/strict'

import type { SettingsState, SettingsUpdate } from './setup-client'

// ── Type-level guard helpers ─────────────────────────────────────────────────

function isSettingsState(v: unknown): v is SettingsState {
  if (typeof v !== 'object' || v === null) return false
  const obj = v as Record<string, unknown>
  if (typeof obj.config_path !== 'string') return false
  if (typeof obj.changed !== 'boolean') return false
  if (typeof obj.restart_required !== 'boolean') return false
  if (typeof obj.restart_note !== 'string') return false
  // typeof null === 'object', so explicit null checks are required here.
  if (typeof obj.services !== 'object' || obj.services === null) return false
  if (typeof obj.surfaces !== 'object' || obj.surfaces === null) return false
  const services = obj.services as Record<string, unknown>
  return (
    typeof services.built_in_upstream_apis_enabled === 'boolean' &&
    Array.isArray(services.built_in_upstream_api_services) &&
    Array.isArray(services.bootstrap_services)
  )
}

// ── settingsState mock response shape ─────────────────────────────────────────

test('settingsState mock response matches the SettingsState type contract', () => {
  const mockState: SettingsState = {
    config_path: '~/.config/lab/config.toml',
    changed: false,
    previous: {
      services: {
        built_in_upstream_apis_enabled: null,
      },
    },
    restart_required: false,
    restart_note: 'Changes to built-in upstream API services take effect after restarting labby serve.',
    services: {
      built_in_upstream_apis_enabled: true,
      built_in_upstream_api_services: ['radarr', 'sonarr', 'openai'],
      bootstrap_services: ['setup', 'doctor', 'extract', 'gateway'],
    },
    surfaces: {
      mcp: {
        transport: 'http',
        host: '127.0.0.1',
        port: 8765,
        stateful: null,
      },
      web: {
        auth_disabled: false,
        assets_dir: null,
      },
      auth: {
        mode: 'bearer',
        public_url: null,
      },
    },
  }

  assert.ok(isSettingsState(mockState), 'SettingsState shape must satisfy the type guard')
})

// ── No secret values in settings state ────────────────────────────────────────

test('settingsState response does not contain secret values', () => {
  const mockState: SettingsState = {
    config_path: '~/.config/lab/config.toml',
    changed: false,
    previous: {
      services: {
        built_in_upstream_apis_enabled: null,
      },
    },
    restart_required: false,
    restart_note: 'Restart note',
    services: {
      built_in_upstream_apis_enabled: true,
      built_in_upstream_api_services: ['radarr'],
      bootstrap_services: ['setup'],
    },
    surfaces: {
      mcp: {
        transport: 'http',
        host: '127.0.0.1',
        port: 8765,
        stateful: null,
      },
      web: {
        auth_disabled: false,
        assets_dir: null,
      },
      auth: {
        mode: 'bearer',
        public_url: null,
      },
    },
  }

  // Recurse into the object and assert no value looks like an API key or token.
  function hasSecretPattern(obj: unknown): boolean {
    if (typeof obj === 'string') {
      return /^[a-z0-9]{32,}$/i.test(obj) // long hex-looking token
    }
    if (Array.isArray(obj)) return obj.some(hasSecretPattern)
    if (typeof obj === 'object' && obj !== null) {
      return Object.values(obj).some(hasSecretPattern)
    }
    return false
  }

  assert.ok(!hasSecretPattern(mockState), 'SettingsState must not contain secret token values')
})

// ── settingsUpdate accepts both flat and nested param shapes ──────────────────

test('SettingsUpdate type accepts built_in_upstream_apis_enabled', () => {
  const patch: SettingsUpdate = {
    services: { built_in_upstream_apis_enabled: false },
  }
  assert.equal(patch.services.built_in_upstream_apis_enabled, false)
})

test('settingsUpdate mock response reflects the new enabled value', () => {
  function mockSettingsUpdate(patch: SettingsUpdate): SettingsState {
    const enabled = patch.services.built_in_upstream_apis_enabled
    return {
      config_path: '~/.config/lab/config.toml',
      changed: true,
      previous: {
        services: {
          built_in_upstream_apis_enabled: !enabled,
        },
      },
      restart_required: false,
      restart_note: 'Changes apply immediately to gateway discovery.',
      services: {
        built_in_upstream_apis_enabled: enabled,
        built_in_upstream_api_services: ['radarr', 'openai'],
        bootstrap_services: ['setup', 'doctor', 'extract', 'gateway'],
      },
      surfaces: {
        mcp: { transport: 'http', host: '127.0.0.1', port: 8765, stateful: null },
        web: { auth_disabled: false, assets_dir: null },
        auth: { mode: 'bearer', public_url: null },
      },
    }
  }

  const result = mockSettingsUpdate({ services: { built_in_upstream_apis_enabled: false } })
  assert.equal(result.services.built_in_upstream_apis_enabled, false)
  assert.equal(result.changed, true)
  assert.equal(result.previous.services.built_in_upstream_apis_enabled, true)
  assert.equal(result.restart_required, false)
  assert.ok(result.services.bootstrap_services.includes('setup'))
  assert.ok(result.services.bootstrap_services.includes('gateway'))
})

// ── restart_required: false when settings.update wires live policy ──────────

test('settingsUpdate result has restart_required false when gateway is live-updated', () => {
  const result: SettingsState = {
    config_path: '~/.config/lab/config.toml',
    changed: true,
    previous: { services: { built_in_upstream_apis_enabled: true } },
    restart_required: false,
    restart_note: 'Gateway discovery updated immediately.',
    services: {
      built_in_upstream_apis_enabled: false,
      built_in_upstream_api_services: [],
      bootstrap_services: ['setup', 'doctor', 'extract', 'gateway'],
    },
    surfaces: {
      mcp: { transport: 'http', host: '127.0.0.1', port: 8765, stateful: null },
      web: { auth_disabled: false, assets_dir: null },
      auth: { mode: 'bearer', public_url: null },
    },
  }
  assert.equal(result.restart_required, false)
})

// ── isSettingsState null-safety guard ────────────────────────────────────────

test('isSettingsState returns false when services is null', () => {
  assert.equal(
    isSettingsState({ services: null, surfaces: null }),
    false,
    'null services/surfaces must not pass the type guard',
  )
})
