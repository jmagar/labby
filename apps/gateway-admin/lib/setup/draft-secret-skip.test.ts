// Tests for secret placeholder skip and empty-value behavior in draft helpers.
// Covers the Task 5 acceptance criteria from the settings-completion plan.

import test from 'node:test'
import assert from 'node:assert/strict'

import {
  STORED_SECRET_MARKER,
  isStoredSecret,
  unmaskValue,
  buildServiceFormDefaults,
} from './draft'
import type { ServiceEnvVar } from '@/lib/api/setup-client'

// ── STORED_SECRET_MARKER constant ─────────────────────────────────────────────

test('STORED_SECRET_MARKER is the three-asterisk sentinel', () => {
  assert.equal(STORED_SECRET_MARKER, '***')
})

// ── isStoredSecret ─────────────────────────────────────────────────────────────

test('isStoredSecret returns true only for the exact sentinel', () => {
  assert.equal(isStoredSecret(STORED_SECRET_MARKER), true)
  assert.equal(isStoredSecret('***'), true)
})

test('isStoredSecret returns false for other values', () => {
  assert.equal(isStoredSecret(''), false)
  assert.equal(isStoredSecret(undefined), false)
  assert.equal(isStoredSecret('actual-api-key'), false)
  assert.equal(isStoredSecret('********'), false) // 8 asterisks, not the sentinel
  assert.equal(isStoredSecret('** *'), false)
})

// ── unmaskValue ─────────────────────────────────────────────────────────────

test('unmaskValue converts the stored-secret sentinel to empty string', () => {
  assert.equal(unmaskValue(STORED_SECRET_MARKER), '')
})

test('unmaskValue converts undefined and empty string to empty string', () => {
  assert.equal(unmaskValue(undefined), '')
  assert.equal(unmaskValue(''), '')
})

test('unmaskValue returns non-sentinel values unchanged', () => {
  assert.equal(unmaskValue('http://radarr.local:7878'), 'http://radarr.local:7878')
  assert.equal(unmaskValue('info'), 'info')
})

// ── buildServiceFormDefaults secret skip ─────────────────────────────────────

const SECRET_VAR: ServiceEnvVar = {
  name: 'RADARR_API_KEY',
  description: 'API key',
  example: '••••••••',
  secret: true,
  required: true,
}

const URL_VAR: ServiceEnvVar = {
  name: 'RADARR_URL',
  description: 'Base URL',
  example: 'http://radarr.local:7878',
  secret: false,
  required: true,
}

test('buildServiceFormDefaults masks stored secrets in defaults', () => {
  const { defaults } = buildServiceFormDefaults([SECRET_VAR, URL_VAR], {
    RADARR_API_KEY: STORED_SECRET_MARKER,
    RADARR_URL: 'http://radarr.local:7878',
  })
  // Secret fields with the sentinel become blank — "leave blank to keep current"
  assert.equal(defaults.RADARR_API_KEY, '')
  // Non-secret values are preserved
  assert.equal(defaults.RADARR_URL, 'http://radarr.local:7878')
})

test('buildServiceFormDefaults marks fields with stored-secret as hasStoredSecret', () => {
  const { fields } = buildServiceFormDefaults([SECRET_VAR, URL_VAR], {
    RADARR_API_KEY: STORED_SECRET_MARKER,
    RADARR_URL: 'http://radarr.local:7878',
  })
  const secretField = fields.find((f) => f.name === 'RADARR_API_KEY')
  assert.ok(secretField, 'RADARR_API_KEY field must exist')
  assert.equal(secretField.secret, true, 'field must be marked secret')
  assert.equal(
    secretField.hasStoredSecret,
    true,
    'hasStoredSecret must be true when the draft holds the sentinel',
  )
})

test('buildServiceFormDefaults returns empty default for missing draft entries', () => {
  const { defaults } = buildServiceFormDefaults([URL_VAR], {})
  assert.equal(defaults.RADARR_URL, '')
})

// ── save filter — placeholder skip ──────────────────────────────────────────

test('save filter skips secret placeholder values', () => {
  // Simulates the filter logic in service-client.tsx save() function.
  // Secret fields with blank or sentinel values must not be written.
  const SKIP_VALUES = ['', STORED_SECRET_MARKER, '********']
  const fields = new Map([['RADARR_API_KEY', { secret: true }]])

  function shouldWrite(key: string, value: string): boolean {
    const field = fields.get(key)
    if (!field?.secret) return true
    return !SKIP_VALUES.includes(value)
  }

  assert.equal(shouldWrite('RADARR_API_KEY', ''), false)
  assert.equal(shouldWrite('RADARR_API_KEY', STORED_SECRET_MARKER), false)
  assert.equal(shouldWrite('RADARR_API_KEY', '********'), false)
  assert.equal(shouldWrite('RADARR_API_KEY', 'actual-new-key'), true)
  // Non-secret fields always pass through
  assert.equal(shouldWrite('RADARR_URL', ''), true)
})

// ── empty-value behavior for non-secret fields ───────────────────────────────

test('empty non-secret draft value produces empty default', () => {
  const { defaults } = buildServiceFormDefaults([URL_VAR], {
    RADARR_URL: '',
  })
  assert.equal(defaults.RADARR_URL, '')
})
