import test from 'node:test'
import assert from 'node:assert/strict'
import { once } from 'node:events'
import http from 'node:http'
import { spawn, type ChildProcess } from 'node:child_process'

import { chromium } from 'playwright'

const PORT = 3103
const BASE_URL = `http://127.0.0.1:${PORT}`
const APP_DIR = new URL('../../', import.meta.url)
let previewServer: ChildProcess | null = null
let previewServerReady: Promise<void> | null = null

type BrowserSession = {
  id: string
  providerSessionId: string
  provider: string
  title: string
  cwd: string
  createdAt: string
  updatedAt: string
  status: 'idle'
  agentName: string
  agentVersion: string
  resumable: boolean
}

type BrowserEvent = {
  id: string
  seq: number
  sessionId: string
  provider: string
  kind: string
  createdAt: string
  role?: 'user' | 'assistant' | 'thinking'
  text?: string
  messageId?: string
}

function session(id: string, title: string, provider = 'codex'): BrowserSession {
  return {
    id,
    providerSessionId: `provider-${id}`,
    provider,
    title,
    cwd: '/home/jmagar/workspace/lab',
    createdAt: '2026-04-23T00:00:00Z',
    updatedAt: '2026-04-23T00:00:00Z',
    status: 'idle',
    agentName: provider === 'codex' || provider === 'codex-acp' ? 'Codex ACP' : provider,
    agentVersion: 'live',
    resumable: true,
  }
}

function bridgeEvent(
  sessionId: string,
  seq: number,
  overrides: Partial<BrowserEvent> = {},
): BrowserEvent {
  return {
    id: `evt-${sessionId}-${seq}`,
    seq,
    sessionId,
    provider: 'codex',
    kind: 'message.chunk',
    createdAt: '2026-04-23T00:00:00Z',
    role: 'assistant',
    text: `chunk-${seq}`,
    ...overrides,
  }
}

function sseFrame(event: BrowserEvent) {
  return `data: ${JSON.stringify(event)}\n\n`
}

async function waitForServer(url: string) {
  const deadline = Date.now() + 60_000

  while (Date.now() < deadline) {
    try {
      const status = await new Promise<number>((resolve, reject) => {
        const request = http.get(url, (response) => {
          resolve(response.statusCode ?? 0)
          response.resume()
        })
        request.on('error', reject)
      })

      if (status >= 200 && status < 500) {
        return
      }
    } catch {
      // Retry until deadline.
    }

    await new Promise((resolve) => setTimeout(resolve, 200))
  }

  throw new Error(`Timed out waiting for preview server at ${url}`)
}

async function startPreviewServer() {
  if (previewServerReady) {
    await previewServerReady
    return
  }

  previewServer = spawn(
    '/usr/bin/zsh',
    ['-lc', `LAB_ALLOWED_DEV_ORIGINS=127.0.0.1 NEXT_PUBLIC_MOCK_DATA=false pnpm exec next build && python3 -m http.server ${PORT} --directory out --bind 127.0.0.1`],
    {
      cwd: APP_DIR,
      stdio: 'ignore',
      env: process.env,
    },
  )

  previewServerReady = waitForServer(`${BASE_URL}/chat/`)
  await previewServerReady
}

async function mockAuthenticatedSession(page: import('playwright').Page) {
  await page.route('**/auth/session', async (route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        authenticated: true,
        user: { sub: 'browser-user', email: 'browser@example.com' },
        expires_at: Date.now() + 60_000,
        csrf_token: 'csrf-token',
        is_admin: true,
      }),
    })
  })
}

async function waitForCondition(predicate: () => boolean, timeoutMs = 5_000) {
  const deadline = Date.now() + timeoutMs

  while (Date.now() < deadline) {
    if (predicate()) {
      return
    }
    await new Promise((resolve) => setTimeout(resolve, 50))
  }

  throw new Error('Timed out waiting for condition')
}

test.after(async () => {
  if (!previewServer) {
    return
  }

  previewServer.kill('SIGTERM')
  await Promise.race([
    once(previewServer, 'exit').catch(() => undefined),
    new Promise((resolve) => setTimeout(resolve, 2_000)),
  ])

  if (previewServer.exitCode === null && !previewServer.killed) {
    previewServer.kill('SIGKILL')
    await once(previewServer, 'exit').catch(() => undefined)
  }
})

test('chat page keeps the header visible while the mobile message thread scrolls', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => {
    await browser.close()
  })

  const page = await browser.newPage({ viewport: { width: 390, height: 844 }, isMobile: true })
  const sessions: BrowserSession[] = [session('session-mobile', 'Mobile sticky header')]

  await mockAuthenticatedSession(page)
  await page.route('**/v1/acp/**', async (route) => {
    const request = route.request()
    const url = new URL(request.url())

    if (url.pathname === '/v1/acp/provider') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          provider: {
            provider: 'codex',
            ready: true,
            command: 'npx',
            args: ['@zed-industries/codex-acp'],
            message: 'ready',
          },
        }),
      })
      return
    }

    if (url.pathname === '/v1/acp/sessions' && request.method() === 'GET') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ sessions }),
      })
      return
    }

    const ticketMatch = url.pathname.match(/^\/v1\/acp\/sessions\/([^/]+)\/subscribe_ticket$/)
    if (ticketMatch && request.method() === 'POST') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ ticket: `ticket-${decodeURIComponent(ticketMatch[1]!)}` }),
      })
      return
    }

    const eventMatch = url.pathname.match(/^\/v1\/acp\/sessions\/([^/]+)\/events$/)
    if (eventMatch && request.method() === 'GET') {
      const sessionId = decodeURIComponent(eventMatch[1]!)
      const body = Array.from({ length: 40 }, (_, index) =>
        sseFrame(bridgeEvent(sessionId, index + 1, {
          messageId: `mobile-message-${index + 1}`,
          role: index % 2 === 0 ? 'user' : 'assistant',
          text: `Mobile message ${index + 1}`,
        })),
      ).join('')

      await route.fulfill({
        status: 200,
        contentType: 'text/event-stream',
        body,
      })
      return
    }

    await route.fulfill({
      status: 404,
      contentType: 'application/json',
      body: JSON.stringify({ message: `Unhandled ACP request: ${url.pathname}` }),
    })
  })

  await page.goto(`${BASE_URL}/chat/`, { waitUntil: 'networkidle' })
  await page.getByText('Mobile sticky header').first().waitFor()
  await page.getByText('Mobile message 40').waitFor()

  const header = page.getByRole('banner').first()
  const input = page.getByRole('textbox', { name: 'Message' })
  const scrollViewport = page.locator('[data-slot="scroll-area-viewport"]').last()

  const before = await header.boundingBox()
  assert.ok(before, 'header should be measurable before scroll')

  const scrollMetrics = await scrollViewport.evaluate((node) => {
    node.scrollTop = 0
    const before = node.scrollTop
    node.scrollTop = Math.max(1, node.scrollHeight / 2)
    node.dispatchEvent(new Event('scroll', { bubbles: true }))
    return {
      before,
      after: node.scrollTop,
      scrollHeight: node.scrollHeight,
      clientHeight: node.clientHeight,
    }
  })
  assert.ok(
    scrollMetrics.scrollHeight > scrollMetrics.clientHeight,
    'message viewport should have scrollable overflow for sticky-header coverage',
  )
  assert.ok(
    scrollMetrics.after > scrollMetrics.before,
    `message viewport should scroll before measuring sticky header, got ${scrollMetrics.before} -> ${scrollMetrics.after}`,
  )

  const after = await header.boundingBox()
  const inputBox = await input.boundingBox()
  assert.ok(after, 'header should be measurable after scroll')
  assert.ok(inputBox, 'input should be measurable after scroll')
  assert.ok(after.y >= 0, `header should stay within the viewport, got y=${after.y}`)
  assert.ok(
    after.y + after.height <= inputBox.y,
    `header must not overlap the prompt input, header bottom=${after.y + after.height}, input top=${inputBox.y}`,
  )
  assert.equal(Math.round(after.height), Math.round(before.height))
})

test('chat shell sends prompts without bearer auth and resumes session streams from the last sequence on reselection', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => {
    await browser.close()
  })

  const page = await browser.newPage({ viewport: { width: 1360, height: 960 } })
  const sessions: BrowserSession[] = []
  const streamSince = new Map<string, string[]>()
  const promptRequests: Array<{ sessionId: string; prompt: string; authorization: string | null }> = []
  const observedAuthorizations: Array<string | null> = []

  await mockAuthenticatedSession(page)

  await page.route('**/v1/acp/**', async (route) => {
    const request = route.request()
    const url = new URL(request.url())
    const authorization = request.headers()['authorization'] ?? null
    observedAuthorizations.push(authorization)

    if (url.pathname === '/v1/acp/provider') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          provider: {
            provider: 'codex',
            ready: true,
            command: 'npx',
            args: ['@zed-industries/codex-acp'],
            message: 'ready',
          },
        }),
      })
      return
    }

    if (url.pathname === '/v1/acp/sessions' && request.method() === 'GET') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ sessions }),
      })
      return
    }

    if (url.pathname === '/v1/acp/sessions' && request.method() === 'POST') {
      const created =
        sessions.length === 0
          ? session('session-1', 'Bootstrap session')
          : session('session-2', 'Second session')
      sessions.unshift(created)

      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ session: created }),
      })
      return
    }

    const promptMatch = url.pathname.match(/^\/v1\/acp\/sessions\/([^/]+)\/prompt$/)
    if (promptMatch && request.method() === 'POST') {
      const payload = JSON.parse(request.postData() ?? '{}') as { prompt?: string }
      promptRequests.push({
        sessionId: decodeURIComponent(promptMatch[1]!),
        prompt: payload.prompt ?? '',
        authorization,
      })

      await route.fulfill({
        status: 202,
        contentType: 'application/json',
        body: JSON.stringify({ accepted: true }),
      })
      return
    }

    const ticketMatch = url.pathname.match(/^\/v1\/acp\/sessions\/([^/]+)\/subscribe_ticket$/)
    if (ticketMatch && request.method() === 'POST') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ ticket: `ticket-${decodeURIComponent(ticketMatch[1]!)}` }),
      })
      return
    }

    const eventMatch = url.pathname.match(/^\/v1\/acp\/sessions\/([^/]+)\/events$/)
    if (eventMatch && request.method() === 'GET') {
      const sessionId = decodeURIComponent(eventMatch[1]!)
      const since = url.searchParams.get('since') ?? '0'
      streamSince.set(sessionId, [...(streamSince.get(sessionId) ?? []), since])

      let body = ''
      if (sessionId === 'session-1' && since === '0') {
        body = sseFrame(bridgeEvent(sessionId, 1, { text: 'Hello session 1' }))
      } else if (sessionId === 'session-1' && since === '1') {
        body = sseFrame(bridgeEvent(sessionId, 2, { text: 'Resumed session 1' }))
      } else if (sessionId === 'session-2' && since === '0') {
        body = sseFrame(bridgeEvent(sessionId, 1, { text: 'Hello session 2' }))
      }

      await route.fulfill({
        status: 200,
        contentType: 'text/event-stream',
        body,
      })
      return
    }

    await route.fulfill({
      status: 404,
      contentType: 'application/json',
      body: JSON.stringify({ message: `Unhandled ACP request: ${url.pathname}` }),
    })
  })

  await page.goto(`${BASE_URL}/chat/`, { waitUntil: 'networkidle' })
  assert.equal(sessions.length, 0, 'opening /chat must not create an empty backend session')

  await page.getByRole('textbox', { name: 'Message' }).fill('Summarize Stage 3 browser coverage')
  await page.getByRole('button', { name: 'Send message' }).click()

  await waitForCondition(() => promptRequests.length === 1)
  await assert.doesNotReject(() => page.getByText('Bootstrap session').first().waitFor())
  await assert.doesNotReject(() => page.getByText('Hello session 1').waitFor())
  assert.deepEqual(promptRequests, [
    {
      sessionId: 'session-1',
      prompt: 'Summarize Stage 3 browser coverage',
      authorization: null,
    },
  ])

  await page.getByRole('button', { name: 'Start new session' }).click()

  await assert.doesNotReject(() => page.getByText('Second session').first().waitFor())
  await assert.doesNotReject(() => page.getByText('Hello session 2').waitFor())

  await page.locator('button').filter({ hasText: 'Bootstrap session' }).first().click()

  await assert.doesNotReject(() => page.getByText('Resumed session 1').waitFor())
  assert.deepEqual(streamSince.get('session-1'), ['0', '1'])
  assert.deepEqual(streamSince.get('session-2'), ['0'])
  assert.ok(observedAuthorizations.every((value) => value === null))
})

test('chat shell recovers from a failed session stream after switching sessions and reselecting the failed run', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => {
    await browser.close()
  })

  const page = await browser.newPage({ viewport: { width: 1360, height: 960 } })
  const sessions: BrowserSession[] = [session('session-1', 'Flaky session')]
  const streamSince = new Map<string, string[]>()
  let sessionOneAttempts = 0

  await mockAuthenticatedSession(page)

  await page.route('**/v1/acp/**', async (route) => {
    const request = route.request()
    const url = new URL(request.url())

    if (url.pathname === '/v1/acp/provider') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          provider: {
            provider: 'codex',
            ready: true,
            command: 'npx',
            args: ['@zed-industries/codex-acp'],
            message: 'ready',
          },
        }),
      })
      return
    }

    if (url.pathname === '/v1/acp/sessions' && request.method() === 'GET') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ sessions }),
      })
      return
    }

    if (url.pathname === '/v1/acp/sessions' && request.method() === 'POST') {
      const created = session('session-2', 'Healthy session')
      sessions.unshift(created)

      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ session: created }),
      })
      return
    }

    const ticketMatch = url.pathname.match(/^\/v1\/acp\/sessions\/([^/]+)\/subscribe_ticket$/)
    if (ticketMatch && request.method() === 'POST') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ ticket: `ticket-${decodeURIComponent(ticketMatch[1]!)}` }),
      })
      return
    }

    const eventMatch = url.pathname.match(/^\/v1\/acp\/sessions\/([^/]+)\/events$/)
    if (eventMatch && request.method() === 'GET') {
      const sessionId = decodeURIComponent(eventMatch[1]!)
      const since = url.searchParams.get('since') ?? '0'
      streamSince.set(sessionId, [...(streamSince.get(sessionId) ?? []), since])

      if (sessionId === 'session-1') {
        sessionOneAttempts += 1
        if (sessionOneAttempts === 1) {
          await route.fulfill({
            status: 500,
            contentType: 'application/json',
            body: JSON.stringify({ message: 'stream failed' }),
          })
          return
        }

        await route.fulfill({
          status: 200,
          contentType: 'text/event-stream',
          body: sseFrame(bridgeEvent(sessionId, 1, { text: 'Recovered session 1' })),
        })
        return
      }

      if (sessionId === 'session-2') {
        await route.fulfill({
          status: 200,
          contentType: 'text/event-stream',
          body: sseFrame(bridgeEvent(sessionId, 1, { text: 'Healthy session 2' })),
        })
        return
      }
    }

    await route.fulfill({
      status: 404,
      contentType: 'application/json',
      body: JSON.stringify({ message: `Unhandled ACP request: ${url.pathname}` }),
    })
  })

  await page.goto(`${BASE_URL}/chat/`, { waitUntil: 'networkidle' })

  await assert.doesNotReject(() => page.getByText('Flaky session').first().waitFor())

  await page.getByRole('button', { name: 'Start new session' }).click()

  await assert.doesNotReject(() => page.getByText('Healthy session').first().waitFor())
  await assert.doesNotReject(() => page.getByText('Healthy session 2').waitFor())

  await page.locator('button').filter({ hasText: 'Flaky session' }).first().click()

  await assert.doesNotReject(() => page.getByText('Recovered session 1').waitFor())
  assert.deepEqual(streamSince.get('session-1'), ['0', '0'])
  assert.deepEqual(streamSince.get('session-2'), ['0'])
})

test('chat shell agent picker switches provider and sends through a provider-matched session', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => {
    await browser.close()
  })

  const page = await browser.newPage({ viewport: { width: 1360, height: 960 } })
  const sessions: BrowserSession[] = []
  const createRequests: Array<{ provider: string }> = []
  const promptRequests: Array<{ sessionId: string; prompt: string }> = []

  await mockAuthenticatedSession(page)

  await page.route('**/v1/acp/**', async (route) => {
    const request = route.request()
    const url = new URL(request.url())

    if (url.pathname === '/v1/acp/provider') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          providers: [
            {
              name: 'codex-acp',
              available: true,
              command: 'npx',
              args: ['@zed-industries/codex-acp'],
            },
            {
              name: 'claude-acp',
              available: true,
              command: 'claude-acp',
              args: [],
            },
            {
              name: 'gemini',
              available: true,
              command: 'gemini',
              args: [],
            },
          ],
        }),
      })
      return
    }

    if (url.pathname === '/v1/acp/sessions' && request.method() === 'GET') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ sessions }),
      })
      return
    }

    if (url.pathname === '/v1/acp/sessions' && request.method() === 'POST') {
      const payload = JSON.parse(request.postData() ?? '{}') as { provider?: string }
      const provider = payload.provider ?? 'codex-acp'
      createRequests.push({ provider })
      const created = session(`session-${sessions.length + 1}`, `${provider} session`, provider)
      sessions.unshift(created)

      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ session: created }),
      })
      return
    }

    const promptMatch = url.pathname.match(/^\/v1\/acp\/sessions\/([^/]+)\/prompt$/)
    if (promptMatch && request.method() === 'POST') {
      const payload = JSON.parse(request.postData() ?? '{}') as { prompt?: string }
      promptRequests.push({
        sessionId: decodeURIComponent(promptMatch[1]!),
        prompt: payload.prompt ?? '',
      })

      await route.fulfill({
        status: 202,
        contentType: 'application/json',
        body: JSON.stringify({ accepted: true }),
      })
      return
    }

    const ticketMatch = url.pathname.match(/^\/v1\/acp\/sessions\/([^/]+)\/subscribe_ticket$/)
    if (ticketMatch && request.method() === 'POST') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ ticket: `ticket-${decodeURIComponent(ticketMatch[1]!)}` }),
      })
      return
    }

    const eventMatch = url.pathname.match(/^\/v1\/acp\/sessions\/([^/]+)\/events$/)
    if (eventMatch && request.method() === 'GET') {
      const sessionId = decodeURIComponent(eventMatch[1]!)
      await route.fulfill({
        status: 200,
        contentType: 'text/event-stream',
        body: sseFrame(bridgeEvent(sessionId, 1, { text: `Hello ${sessionId}` })),
      })
      return
    }

    await route.fulfill({
      status: 404,
      contentType: 'application/json',
      body: JSON.stringify({ message: `Unhandled ACP request: ${url.pathname}` }),
    })
  })

  await page.goto(`${BASE_URL}/chat/`, { waitUntil: 'networkidle' })

  assert.deepEqual(createRequests, [], 'opening /chat must not create a provider session')
  await page.getByRole('button', { name: 'Selected agent: Codex ACP' }).click()
  await page.getByRole('option', { name: /Claude ACP/ }).click()
  await assert.doesNotReject(() =>
    page.getByRole('button', { name: 'Selected agent: Claude ACP' }).waitFor(),
  )

  await page.getByRole('textbox', { name: 'Message' }).fill('Use Claude for this')
  await page.getByRole('button', { name: 'Send message' }).click()

  await waitForCondition(() => promptRequests.length === 1)
  assert.deepEqual(createRequests, [
    { provider: 'claude-acp' },
  ])
  assert.deepEqual(promptRequests, [
    {
      sessionId: 'session-1',
      prompt: 'Use Claude for this',
    },
  ])
})

test('chat shell attaches and removes local files before sending prompt', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => {
    await browser.close()
  })

  const page = await browser.newPage({ viewport: { width: 390, height: 844 } })
  const sessions: BrowserSession[] = []
  const promptRequests: Array<{ prompt: string; attachments?: unknown[] }> = []

  await mockAuthenticatedSession(page)
  await page.route('**/v1/acp/**', async (route) => {
    const request = route.request()
    const url = new URL(request.url())

    if (url.pathname === '/v1/acp/provider') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ provider: { provider: 'codex', ready: true, command: 'npx', args: [], message: 'ready' } }),
      })
      return
    }

    if (url.pathname === '/v1/acp/sessions' && request.method() === 'GET') {
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ sessions }) })
      return
    }

    if (url.pathname === '/v1/acp/sessions' && request.method() === 'POST') {
      const created = session('session-attach', 'Attachment session')
      sessions.unshift(created)
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ session: created }) })
      return
    }

    const promptMatch = url.pathname.match(/^\/v1\/acp\/sessions\/([^/]+)\/prompt$/)
    if (promptMatch && request.method() === 'POST') {
      const payload = JSON.parse(request.postData() ?? '{}') as { prompt?: string; attachments?: unknown[] }
      promptRequests.push({ prompt: payload.prompt ?? '', attachments: payload.attachments })
      await route.fulfill({ status: 202, contentType: 'application/json', body: JSON.stringify({ accepted: true }) })
      return
    }

    const ticketMatch = url.pathname.match(/^\/v1\/acp\/sessions\/([^/]+)\/subscribe_ticket$/)
    if (ticketMatch && request.method() === 'POST') {
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ ticket: 'ticket-attach' }) })
      return
    }

    const eventMatch = url.pathname.match(/^\/v1\/acp\/sessions\/([^/]+)\/events$/)
    if (eventMatch && request.method() === 'GET') {
      await route.fulfill({ status: 200, contentType: 'text/event-stream', body: '' })
      return
    }

    await route.fulfill({ status: 404, contentType: 'application/json', body: JSON.stringify({ message: `Unhandled ACP request: ${url.pathname}` }) })
  })

  await page.goto(`${BASE_URL}/chat/`, { waitUntil: 'networkidle' })
  await page.getByRole('textbox', { name: 'Message' }).fill('Use these notes')

  const chooserPromise = page.waitForEvent('filechooser')
  await page.getByRole('button', { name: 'Attach local file' }).click()
  const chooser = await chooserPromise
  await chooser.setFiles([
    {
      name: 'notes.txt',
      mimeType: 'text/plain',
      buffer: Buffer.from('local browser notes'),
    },
  ])

  await assert.doesNotReject(() => page.getByText('notes.txt').waitFor())
  await page.getByRole('button', { name: 'Remove notes.txt' }).click()
  await assert.doesNotReject(() => page.getByRole('button', { name: 'Send message' }).waitFor())

  const chooserPromise2 = page.waitForEvent('filechooser')
  await page.getByRole('button', { name: 'Attach local file' }).click()
  const chooser2 = await chooserPromise2
  await chooser2.setFiles([
    {
      name: 'notes.txt',
      mimeType: 'text/plain',
      buffer: Buffer.from('local browser notes'),
    },
  ])

  await page.getByRole('button', { name: 'Send message' }).click()
  await waitForCondition(() => promptRequests.length === 1)

  assert.equal(promptRequests[0]?.prompt, 'Use these notes')
  const attachments = promptRequests[0]?.attachments as Array<Record<string, unknown>> | undefined
  assert.equal(attachments?.length, 1)
  assert.deepEqual(
    {
      ...attachments?.[0],
      id: typeof attachments?.[0]?.id === 'string' && attachments[0].id.startsWith('local-notes.txt-19-')
        ? 'local-notes.txt-19-*'
        : attachments?.[0]?.id,
    },
    {
      kind: 'local',
      id: 'local-notes.txt-19-*',
      name: 'notes.txt',
      mimeType: 'text/plain',
      size: 19,
      contentKind: 'text',
      text: 'local browser notes',
    },
  )
})
