import test from 'node:test'
import assert from 'node:assert/strict'
import { once } from 'node:events'
import http from 'node:http'
import { spawn, type ChildProcess } from 'node:child_process'

import { chromium } from 'playwright'

const PORT = 3101
const BASE_URL = `http://127.0.0.1:${PORT}`
const APP_DIR = new URL('../../', import.meta.url)
let previewServer: ChildProcess | null = null
let previewServerReady: Promise<void> | null = null

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
    ['-lc', `LAB_ALLOWED_DEV_ORIGINS=127.0.0.1 NEXT_PUBLIC_MOCK_DATA=true NEXT_PUBLIC_API_TOKEN=dev-token pnpm exec next build && python3 -m http.server ${PORT} --directory out --bind 127.0.0.1`],
    {
      cwd: APP_DIR,
      stdio: 'ignore',
      env: process.env,
    },
  )

  previewServerReady = waitForServer(`${BASE_URL}/gateway/?id=gw-2`)
  await previewServerReady
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

test('gateway manage tools flow persists after a full reload in mock preview', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => {
    await browser.close()
  })

  const page = await browser.newPage()
  await page.goto(`${BASE_URL}/gateway/?id=gw-2`, { waitUntil: 'networkidle' })
  await page.evaluate(() => {
    window.localStorage.clear()
  })
  await page.reload({ waitUntil: 'networkidle' })

  await page.getByRole('button', { name: 'Manage Tools' }).click()
  await page.locator('#select-all-visible').click()
  await page.getByRole('button', { name: 'Disable selected' }).click()
  await page.getByRole('button', { name: 'Save changes' }).click()

  await page.getByText('Tool exposure updated successfully').waitFor()
  await assert.doesNotReject(() =>
    page.locator('p, div').filter({ hasText: /^0\/12$/ }).first().waitFor(),
  )

  await page.reload({ waitUntil: 'networkidle' })

  await assert.doesNotReject(() => page.getByRole('button', { name: 'Manage Tools' }).waitFor())
  await assert.doesNotReject(() =>
    page.locator('p, div').filter({ hasText: /^0\/12$/ }).first().waitFor(),
  )
  await assert.doesNotReject(() => page.getByText('12 hidden').waitFor())
})

test('gateway detail uses a compact summary and endpoint block in mock preview', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => {
    await browser.close()
  })

  const page = await browser.newPage({ viewport: { width: 1360, height: 960 } })
  await page.goto(`${BASE_URL}/gateway/?id=gw-2`, { waitUntil: 'networkidle' })
  await page.evaluate(() => {
    window.localStorage.clear()
  })
  await page.reload({ waitUntil: 'networkidle' })

  await assert.doesNotReject(() => page.getByText('12/12').first().waitFor())
  await assert.doesNotReject(() => page.getByText('Resources').first().waitFor())
  await assert.doesNotReject(() => page.getByText('Prompts').first().waitFor())
  await assert.doesNotReject(() => page.getByText('http://localhost:3001/mcp').waitFor())
  await assert.doesNotReject(() => page.getByRole('button', { name: 'Manage Tools' }).waitFor())

  assert.equal(await page.getByText('TOOL SURFACE').count(), 0)
  assert.equal(await page.getByText('BEARER ENV').count(), 0)
  assert.equal(await page.getByText('LAB CONTROLS').count(), 0)

  const hasHorizontalOverflow = await page.evaluate(() => {
    const root = document.documentElement
    return root.scrollWidth > root.clientWidth
  })

  assert.equal(hasHorizontalOverflow, false)
})

test('gateway list stays compact without horizontal overflow in mock preview', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => {
    await browser.close()
  })

  const page = await browser.newPage({ viewport: { width: 1360, height: 960 } })
  await page.goto(`${BASE_URL}/gateways/`, { waitUntil: 'networkidle' })
  await page.evaluate(() => {
    window.localStorage.clear()
  })
  await page.reload({ waitUntil: 'networkidle' })

  await assert.doesNotReject(() => page.getByText('CONFIGURED').first().waitFor())
  await assert.doesNotReject(() => page.getByText('5').first().waitFor())
  await assert.doesNotReject(() => page.getByText('DISCOVERED TOOLS').first().waitFor())
  await assert.doesNotReject(() => page.getByText('39').first().waitFor())
  assert.match(await page.locator('body').innerText(), /github-server[\s\S]*12\/12/)

  const hasHorizontalOverflow = await page.evaluate(() => {
    const root = document.documentElement
    return root.scrollWidth > root.clientWidth
  })

  assert.equal(hasHorizontalOverflow, false)
})

test('gateway detail disable flow shows confirmation, persists disabled state, and can be re-enabled', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => {
    await browser.close()
  })

  const page = await browser.newPage({ viewport: { width: 1360, height: 960 } })
  await page.goto(`${BASE_URL}/gateway/?id=gw-2`, { waitUntil: 'networkidle' })
  await page.evaluate(() => {
    window.localStorage.clear()
  })
  await page.reload({ waitUntil: 'networkidle' })

  const enabledSwitch = page.getByRole('switch', { name: 'Server enabled' })
  await assert.doesNotReject(() => enabledSwitch.waitFor())
  assert.equal(await enabledSwitch.getAttribute('aria-checked'), 'true')

  await enabledSwitch.focus()
  await page.keyboard.press('Space')
  await assert.doesNotReject(() => page.getByText('Disable server?').waitFor())
  await assert.doesNotReject(() =>
    page.getByText('Connected clients should no longer have access').waitFor(),
  )

  await page.getByRole('button', { name: 'Disable server' }).click()
  await assert.doesNotReject(() =>
    page.getByText('Server disabled. Catalog change sent and runtime cleanup requested.').waitFor(),
  )
  await assert.doesNotReject(() =>
    page
      .getByText('This server is excluded from the active catalog. Clients should no longer see its tools, resources, or prompts until you re-enable it.')
      .waitFor(),
  )
  assert.equal(await enabledSwitch.getAttribute('aria-checked'), 'false')
  assert.equal(await page.getByRole('button', { name: 'Test server' }).isDisabled(), true)
  assert.equal(await page.getByRole('button', { name: 'Reload server' }).isDisabled(), true)

  await enabledSwitch.focus()
  await page.keyboard.press('Space')
  await assert.doesNotReject(() =>
    page.getByText('Server enabled. Catalog change sent to clients.').waitFor(),
  )
  assert.equal(await enabledSwitch.getAttribute('aria-checked'), 'true')
  assert.equal(
    await page
      .getByText('This server is excluded from the active catalog. Clients should no longer see its tools, resources, or prompts until you re-enable it.')
      .count(),
    0,
  )
  assert.equal(await page.getByRole('button', { name: 'Test server' }).isDisabled(), false)
  assert.equal(await page.getByRole('button', { name: 'Reload server' }).isDisabled(), false)
})

test('gateway list row action disable flow opens and completes successfully', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => {
    await browser.close()
  })

  const page = await browser.newPage({ viewport: { width: 1360, height: 960 } })
  await page.goto(`${BASE_URL}/gateways/`, { waitUntil: 'networkidle' })
  await page.evaluate(() => {
    window.localStorage.clear()
  })
  await page.reload({ waitUntil: 'networkidle' })

  const githubRow = page.locator('tr').filter({ has: page.getByText('github-server') }).first()
  const disableButton = githubRow.getByRole('button', { name: 'Disable server' })
  await assert.doesNotReject(() => disableButton.waitFor())

  await disableButton.click()
  await assert.doesNotReject(() => page.getByText('Disable server?').waitFor())
  await page.getByRole('button', { name: 'Disable server' }).click()

  await assert.doesNotReject(() =>
    page.getByText('Server disabled. Catalog change sent and runtime cleanup requested.').waitFor(),
  )
})
