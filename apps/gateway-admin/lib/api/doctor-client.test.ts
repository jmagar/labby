import test from 'node:test'
import assert from 'node:assert/strict'

import { __setBrowserSessionStateForTests } from '../auth/session-store.ts'
import { doctorApi, isBlockingDoctorSeverity } from './doctor-client.ts'

test('doctorApi.proxyCheck sends proxy.check payload', async () => {
  const originalFetch = globalThis.fetch
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  let recorded: { action?: string; params?: Record<string, unknown> } | undefined

  globalThis.fetch = (async (_input, init) => {
    recorded = JSON.parse(String(init?.body ?? '{}'))
    return new Response(JSON.stringify({ findings: [] }), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    })
  }) as typeof fetch

  try {
    await doctorApi.proxyCheck({
      app_url: 'https://lab.example.test',
      mcp_url: 'https://mcp.example.test',
      route: '/syslog',
    })
  } finally {
    globalThis.fetch = originalFetch
  }

  assert.deepEqual(recorded, {
    action: 'proxy.check',
    params: {
      app_url: 'https://lab.example.test',
      mcp_url: 'https://mcp.example.test',
      route: '/syslog',
    },
  })
})

test('fail and error doctor severities block setup progression', () => {
  assert.equal(isBlockingDoctorSeverity('ok'), false)
  assert.equal(isBlockingDoctorSeverity('warn'), false)
  assert.equal(isBlockingDoctorSeverity('unknown'), false)
  assert.equal(isBlockingDoctorSeverity('fail'), true)
  assert.equal(isBlockingDoctorSeverity('error'), true)
})
