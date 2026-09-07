import assert from 'node:assert/strict'
import { appendFileSync } from 'node:fs'
import { access, readFile, writeFile } from 'node:fs/promises'
import test from 'node:test'

import { chromium, type Page } from 'playwright'

import {
  assertCanaryFree,
  captureFailureEvidence,
  observeLivePage,
  ownedBrowserLaunchOptions,
  readPrivateCsrf,
  readLiveDescriptor,
  runBrowserCleanupIfActive,
  useBrowserWithAbort,
  withAbsoluteDeadline,
} from './live-backend-harness.ts'

const liveEnabled = Boolean(process.env.LABBY_LIVE_BROWSER_DESCRIPTOR)
const nightlyEnabled = process.env.LABBY_LIVE_BROWSER_NIGHTLY === 'true'
const progressPath = process.env.LABBY_LIVE_BROWSER_PROGRESS
let progressBytes = 0
function progress(message: string) {
  const rendered = `${message}\n`
  progressBytes += Buffer.byteLength(rendered)
  if (progressPath && progressBytes <= 16 * 1024) appendFileSync(progressPath, rendered, { mode: 0o600 })
}

async function action(page: Page, csrfToken: string, service: string, name: string, params: object) {
  return page.evaluate(async ({ csrfToken, service, name, params }) => {
    const session = await fetch('/auth/session', { credentials: 'include', cache: 'no-store' })
    const sessionBody = await session.json()
    if (!session.ok || !sessionBody.authenticated) return { status: session.status, body: sessionBody }
    const response = await fetch(`/v1/${service}`, {
      method: 'POST', credentials: 'include', cache: 'no-store',
      headers: {
        'content-type': 'application/json',
        'x-csrf-token': csrfToken,
        ...(typeof sessionBody.project_id === 'string' ? { 'x-labby-project-id': sessionBody.project_id } : {}),
      },
      body: JSON.stringify({ action: name, params }),
    })
    return {
      status: response.status,
      body: await response.json(),
      sessionAuthenticated: sessionBody.authenticated === true,
      csrfLength: typeof sessionBody.csrf_token === 'string' ? sessionBody.csrf_token.length : 0,
      sessionProjectId: typeof sessionBody.project_id === 'string' ? sessionBody.project_id : null,
    }
  }, { csrfToken, service, name, params })
}

async function addGatewayThroughUi(page: Page, name: string) {
  const observedPosts: string[] = []
  const observePost = (request: import('playwright').Request) => {
    if (request.method() === 'POST') observedPosts.push(`${new URL(request.url()).pathname}:${request.postData() ?? ''}`)
  }
  page.on('request', observePost)
  await page.getByRole('button', { name: 'Add server', exact: true }).last().click()
  const dialog = page.getByRole('dialog', { name: 'Add server' })
  await dialog.getByLabel('Name').fill(name)
  await dialog.getByLabel('URL').fill('http://127.0.0.1:9/mcp')
  const mutation = page.waitForResponse((response) =>
    response.request().method() === 'POST'
      && response.request().postData()?.includes('gateway.add') === true,
  { timeout: 30_000 })
  await dialog.getByRole('button', { name: 'Add server', exact: true }).click()
  const response = await mutation.catch(async (error) => {
    throw new Error(`add-server request was not sent; posts=${JSON.stringify(observedPosts)}; page=${await page.locator('body').innerText()}`, { cause: error })
  }).finally(() => page.off('request', observePost))
  assert.equal(response.status(), 200, `UI gateway.add returned ${response.status()}: ${await response.text()}`)
  await dialog.waitFor({ state: 'hidden', timeout: 10_000 }).catch(async (error) => {
    throw new Error(`add-server dialog remained open: ${await dialog.innerText()}`, { cause: error })
  })
  const serverLink = page.locator('a:visible').filter({ hasText: name }).first()
  await assert.doesNotReject(serverLink.waitFor({ state: 'visible', timeout: 10_000 }))
  const href = await serverLink.getAttribute('href')
  const id = new URL(href ?? '', 'http://loopback.invalid').searchParams.get('id')
  assert.ok(id, 'new server link must expose its stable identifier')
  return id
}

async function stageProtectedRouteThroughUi(page: Page, name: string) {
  await page.goto('/settings/surfaces/', { waitUntil: 'domcontentloaded', timeout: 15_000 })
  const panel = page.locator('[data-protected-routes-panel]')
  await panel.getByLabel('Name').fill(name)
  await panel.getByLabel('Public host').fill('browser.invalid')
  await panel.getByLabel('Public path').fill('/mcp')
  await panel.getByLabel('Loadout').click()
  await page.getByRole('option', { name: 'production', exact: true }).click()
  await panel.getByRole('button', { name: 'Add route', exact: true }).click()
  await assert.doesNotReject(panel.getByText(name, { exact: true }).first().waitFor({ state: 'visible', timeout: 10_000 }))
  await assert.doesNotReject(panel.getByText(/saved for restart/i).waitFor({ state: 'visible', timeout: 10_000 }))
}

async function removeProtectedRouteThroughUi(page: Page, name: string) {
  const panel = page.locator('[data-protected-routes-panel]')
  const removeButton = panel.getByRole('button', { name: `Remove protected route ${name}` })
  await removeButton.click()
  // Removing a just-staged add cancels it outright; removing an already
  // mounted route instead leaves a restart-required tombstone.
  await assert.doesNotReject(removeButton.waitFor({ state: 'hidden', timeout: 10_000 }))
}

async function exerciseArtifactLibraryThroughUi(page: Page, csrfToken: string, name: string) {
  await page.goto('/skills/', { waitUntil: 'domcontentloaded', timeout: 15_000 })
  await page.getByRole('button', { name: 'Create skill', exact: true }).click()
  await page.getByLabel('Skill name').fill(name)
  await page.getByLabel('Contents').fill(`---\nname: ${name}\ndescription: Live browser Artifact lifecycle\n---\n\n# First revision\n`)
  await page.getByRole('button', { name: 'Validate', exact: true }).click()
  await assert.doesNotReject(page.getByText('Skill is valid').waitFor({ state: 'visible', timeout: 10_000 }))
  let mutation = page.waitForResponse(response => response.request().method() === 'POST'
    && new URL(response.url()).pathname === '/v1/artifacts'
    && response.request().postData()?.includes('artifacts.create') === true)
  await page.getByRole('button', { name: 'Save immutable revision', exact: true }).click()
  let response = await mutation
  assert.equal(response.status(), 200, `Artifact create returned ${response.status()}: ${await response.text()}`)
  await assert.doesNotReject(page.getByRole('button', { name: new RegExp(`^${name}`) }).waitFor({ state: 'visible', timeout: 10_000 }))

  await page.getByRole('button', { name: 'Activate latest revision', exact: true }).click()
  await assert.doesNotReject(page.getByText('Published', { exact: true }).last().waitFor({ state: 'visible', timeout: 10_000 }))

  await page.getByRole('button', { name: 'Edit latest', exact: true }).click()
  await page.getByLabel('Contents').fill(`---\nname: ${name}\ndescription: Live browser Artifact lifecycle\n---\n\n# Second revision\n`)
  mutation = page.waitForResponse(response => response.request().method() === 'POST'
    && new URL(response.url()).pathname === '/v1/artifacts'
    && response.request().postData()?.includes('artifacts.save') === true)
  await page.getByRole('button', { name: 'Save immutable revision', exact: true }).click()
  response = await mutation
  assert.equal(response.status(), 200, `Artifact save returned ${response.status()}: ${await response.text()}`)
  const afterSave = await action(page, csrfToken, 'artifacts', 'artifacts.list', { limit: 10 })
  progress(`artifact-after-save:${afterSave.status}:${JSON.stringify(afterSave.body).slice(0, 1000)}`)
  const activate = page.getByRole('button', { name: 'Activate latest revision', exact: true })
  await activate.waitFor({ state: 'visible', timeout: 10_000 })
  await activate.click({ timeout: 10_000 })

  page.once('dialog', dialog => dialog.accept())
  mutation = page.waitForResponse(response => response.request().method() === 'POST'
    && new URL(response.url()).pathname === '/v1/artifacts'
    && response.request().postData()?.includes('artifacts.archive') === true)
  await page.getByRole('button', { name: 'Archive', exact: true }).click()
  response = await mutation
  assert.equal(response.status(), 200, `Artifact archive returned ${response.status()}: ${await response.text()}`)
  const afterArchive = await action(page, csrfToken, 'artifacts', 'artifacts.list', { limit: 10 })
  assert.equal(afterArchive.status, 200)
  assert.equal(afterArchive.body.items.find((item: { name: string }) => item.name === name)?.archived, true)
}

async function exerciseActionableImportFailure(page: Page) {
  await page.goto('/skills/', { waitUntil: 'domcontentloaded', timeout: 15_000 })
  await page.getByRole('button', { name: 'Import', exact: true }).click()
  await page.getByLabel('Import connection').fill('unconfigured-repository')
  await page.getByLabel('Import artifact ID').fill('missing-artifact')
  await page.getByLabel('Import revision ID').fill(`sha256:${'a'.repeat(64)}`)
  const mutation = page.waitForResponse(response => response.request().method() === 'POST'
    && new URL(response.url()).pathname === '/v1/artifacts'
    && response.request().postData()?.includes('artifacts.import') === true)
  await page.getByRole('button', { name: 'Import exact revision', exact: true }).click()
  const response = await mutation
  assert.equal(response.status(), 503)
  const body = await response.json()
  assert.equal(body.kind, 'source_unavailable')
  await assert.doesNotReject(page.getByText(/import sources are not configured|source is not configured/i).waitFor({ state: 'visible', timeout: 10_000 }))
}

async function waitForFile(path: string, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    if (await access(path).then(() => true, () => false)) return
    await new Promise(resolve => setTimeout(resolve, 100))
  }
  throw new Error(`timed out waiting for ${path}`)
}

async function exerciseStashThroughUi(page: Page, descriptor: Awaited<ReturnType<typeof readLiveDescriptor>>) {
  assert.ok(descriptor)
  const name = `browser-stash-${descriptor.run_id}.txt`
  const contents = `live browser stash ${descriptor.run_id}\n`
  await page.goto('/stash/', { waitUntil: 'domcontentloaded', timeout: 15_000 })
  const dropTarget = page.getByRole('button', { name: /drop files here or browse/i })
  await dropTarget.waitFor({ state: 'visible', timeout: 10_000 })
  await dropTarget.focus()
  assert.equal(await dropTarget.evaluate(element => element === document.activeElement), true)
  const fileChooser = page.waitForEvent('filechooser')
  await dropTarget.press('Enter')
  await (await fileChooser).setFiles({ name, mimeType: 'text/plain', buffer: Buffer.from(contents) })
  await page.getByRole('status').filter({ hasText: `${name} uploaded.` }).waitFor({ state: 'attached', timeout: 15_000 })
  const row = page.getByRole('article').filter({ hasText: name })
  await row.waitFor({ state: 'visible', timeout: 10_000 })
  const uri = await row.getByTitle('Copy canonical URI').locator('code').innerText()
  assert.match(uri, /^stash:\/\/me\/files\/[A-Z0-9]+$/)
  await assert.doesNotReject(page.getByText('1', { exact: true }).first().waitFor({ state: 'visible' }))
  await assert.doesNotReject(page.getByText(`${Buffer.byteLength(contents)} B`, { exact: true }).first().waitFor({ state: 'visible' }))

  const download = page.waitForEvent('download')
  await row.getByRole('link', { name: `Download ${name}` }).click()
  assert.equal(await readFile(await (await download).path()!, 'utf8'), contents)

  await row.getByRole('button', { name: `Rename or share ${name}` }).click()
  const dialog = page.getByRole('dialog', { name: `Manage ${name}` })
  await dialog.getByLabel('Filename').waitFor({ state: 'visible' })
  assert.equal(await dialog.getByLabel('Filename').evaluate(element => element === document.activeElement), true)
  await dialog.getByLabel('Find a recipient').fill('Browser Stash')
  await dialog.getByText('Browser Stash Recipient', { exact: true }).waitFor({ state: 'visible', timeout: 10_000 })
  await dialog.getByRole('button', { name: 'Grant access' }).click()
  await page.getByRole('status').filter({ hasText: 'Access granted' }).waitFor({ state: 'attached', timeout: 10_000 })

  await writeFile(descriptor.restart_request_path, 'restart\n', { mode: 0o600, flag: 'wx' })
  await waitForFile(descriptor.restart_complete_path)
  await page.reload({ waitUntil: 'domcontentloaded', timeout: 15_000 })
  const persisted = page.getByRole('article').filter({ hasText: name })
  await persisted.waitFor({ state: 'visible', timeout: 10_000 })

  await persisted.getByRole('button', { name: `Rename or share ${name}` }).click()
  const persistedDialog = page.getByRole('dialog', { name: `Manage ${name}` })
  const revoke = persistedDialog.getByRole('button', { name: 'Revoke' })
  await revoke.waitFor({ state: 'visible', timeout: 10_000 })
  await revoke.click()
  await page.getByRole('status').filter({ hasText: 'Access revoked' }).waitFor({ state: 'attached', timeout: 10_000 })

  const afterRevoke = page.getByRole('article').filter({ hasText: name })
  await afterRevoke.getByRole('button', { name: `Delete ${name}` }).click()
  const confirmation = page.getByRole('alertdialog', { name: 'Delete this file?' })
  await confirmation.getByRole('button', { name: 'Delete file' }).click()
  await page.getByRole('status').filter({ hasText: `${name} deleted.` }).waitFor({ state: 'attached', timeout: 10_000 })
  await assert.doesNotReject(afterRevoke.waitFor({ state: 'detached', timeout: 10_000 }))
}

test('embedded Gateway Admin completes a real backend journey', {
  concurrency: false,
  skip: liveEnabled ? false : 'outer supervisor did not supply LABBY_LIVE_BROWSER_DESCRIPTOR',
}, async () => {
  progress('test-start')
  const descriptor = await readLiveDescriptor()
  assert.ok(descriptor)
  const csrfToken = await readPrivateCsrf(descriptor)
  progress('descriptor-read')
  await withAbsoluteDeadline(async (signal) => {
    progress('chromium-launch-start')
    await useBrowserWithAbort(signal, async () => chromium.launch(await ownedBrowserLaunchOptions(chromium.executablePath())), async (browser) => {
    progress('chromium-launched')
    const context = await browser.newContext({
        baseURL: descriptor.base_url,
        storageState: descriptor.storage_state_path,
        viewport: { width: 1360, height: 900 },
        reducedMotion: 'reduce',
      })
    await context.tracing.start({ screenshots: false, snapshots: false, sources: false })
    const page = await context.newPage()
    // The outer supervisor owns the authenticated browser fixture and its
    // CSRF material. Attach that material at the transport boundary so UI
    // controls exercise their real action mapping without an out-of-band
    // /auth/session probe rotating the fixture token.
    await page.route('**/v1/**', async (route) => {
      if (route.request().method() !== 'POST') return route.continue()
      return route.continue({
        headers: { ...route.request().headers(), 'x-csrf-token': csrfToken },
      })
    })
    const evidence = observeLivePage(page, descriptor.base_url)
    let failure: unknown
    const ownedName = `browser-${descriptor.run_id.toLowerCase()}`
    let ownedGatewayId = ownedName
    const ownedRoute = `${ownedName}-route`
    let ownedRouteRemoved = false
    const cleanupFailures: string[] = []
    try {
      progress('health-session-catalog')
      // Do not probe /auth/session out of band: that endpoint rotates CSRF
      // material, and the UI bootstrap must remain the sole browser-session
      // reader for this context.
      for (const route of ['/health', '/v1/catalog']) {
        const response = await page.request.get(route)
        progress(`${route}:${response.status()}`)
        assert.ok(response.ok(), `${route} returned ${response.status()}`)
      }
      const anonymous = await browser.newContext({ baseURL: descriptor.base_url })
      const denied = await anonymous.request.post('/v1/gateway', {
        data: { action: 'gateway.remove', params: { name: 'browser-denied' } },
      })
      await anonymous.close()
      assert.ok([401, 403].includes(denied.status()), `unauthorized mutation returned ${denied.status()}`)
      progress(`anonymous-denial:${denied.status()}`)

      await page.goto('/gateways/', { waitUntil: 'domcontentloaded', timeout: 15_000 })
      progress('embedded-ui-loaded')
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth), false)
      progress(`page-errors:${evidence.pageErrors.length}:csp:${evidence.cspViolations.length}`)
      assert.equal(evidence.pageErrors.length, 0)
      assert.equal(evidence.cspViolations.length, 0)

      ownedGatewayId = await addGatewayThroughUi(page, ownedName)
      progress('gateway-added-through-ui')
      await page.reload({ waitUntil: 'domcontentloaded', timeout: 15_000 })
      progress('page-reloaded')
      const persisted = await action(page, csrfToken, 'gateway', 'gateway.server.get', { id: ownedGatewayId })
      progress(`gateway-get:${persisted.status}`)
      assert.equal(persisted.status, 200)
      await assert.doesNotReject(page.locator('a:visible').filter({ hasText: ownedName }).first().waitFor({ state: 'visible', timeout: 10_000 }))

      await stageProtectedRouteThroughUi(page, ownedRoute)
      progress('protected-route-staged-through-ui')
      await removeProtectedRouteThroughUi(page, ownedRoute)
      ownedRouteRemoved = true
      progress('protected-route-removed-through-ui')

      const libraryProbe = await action(page, csrfToken, 'artifacts', 'artifacts.list', { limit: 10 })
      progress(`artifact-library-probe:${libraryProbe.status}:project=${libraryProbe.sessionProjectId}:${JSON.stringify(libraryProbe.body).slice(0, 500)}`)
      assert.equal(libraryProbe.status, 200, `Artifact Library probe failed: ${JSON.stringify(libraryProbe.body)}`)
      await exerciseArtifactLibraryThroughUi(page, csrfToken, `${ownedName}-skill`)
      progress('artifact-lifecycle-through-ui')
      await exerciseActionableImportFailure(page)
      progress('artifact-import-failure-through-ui')

      if (descriptor.stash_supported) {
        await exerciseStashThroughUi(page, descriptor)
        progress('stash-lifecycle-reload-restart-through-ui')
      }

      // A real rapid duplicate reaches backend serialization; the UI must not
      // turn it into two successful state transitions.
      const duplicate = await Promise.all([
        action(page, csrfToken, 'gateway', 'gateway.add', { spec: { name: `${ownedName}-duplicate`, url: 'http://127.0.0.1:9/mcp' } }),
        action(page, csrfToken, 'gateway', 'gateway.add', { spec: { name: `${ownedName}-duplicate`, url: 'http://127.0.0.1:9/mcp' } }),
      ])
      assert.equal(duplicate.filter((result) => result.status === 200).length, 1)
      assert.ok(duplicate.some((result) => result.status === 409 || result.status >= 400))
      progress('duplicate-serialized')
      progress(`requests:count=${evidence.requests.length}:failures=${evidence.requests.filter((request) => (request.status ?? 0) >= 400).length}`)

      assert.ok(evidence.requests.some((request) => request.path === '/auth/session'))
      assert.ok(evidence.requests.some((request) => request.path === '/v1/catalog'))
      assert.ok(evidence.requests.some((request) => request.path === '/v1/gateway' && request.method === 'POST'))
      const scanSecrets = (await import('node:fs/promises')).readFile(descriptor.scan_secrets_path, 'utf8')
        .then((value) => value.split('\n').filter(Boolean))
      assertCanaryFree(await page.locator('body').innerText(), await scanSecrets, 'DOM')
      assertCanaryFree(evidence, await scanSecrets, 'browser evidence')
      progress('evidence-asserted')
      await context.tracing.stop()
      progress('trace-stopped')
    } catch (error) {
      failure = error
      await captureFailureEvidence({ browser, context, page, descriptor, evidence, error, signal })
      throw error
    } finally {
      await runBrowserCleanupIfActive(signal, () => progress('cleanup-deferred-to-owned-root:deadline'), async (mayContinue) => {
      for (const [name, operation] of [
        ...(!ownedRouteRemoved ? [[ownedRoute, 'gateway.protected_route.stage_remove'] as const] : []),
        [`${ownedName}-duplicate`, 'gateway.remove'],
        [ownedGatewayId, 'gateway.remove'],
      ] as const) {
        if (!mayContinue()) break
        const result = await action(page, csrfToken, 'gateway', operation, { name }).catch((error) => ({ status: 0, body: String(error) }))
        progress(`cleanup:${operation}:${result.status}`)
        if (![200, 404].includes(result.status) && operation === 'gateway.remove') {
          if (!mayContinue()) break
          const absent = await action(page, csrfToken, 'gateway', 'gateway.get', { name })
          progress(`cleanup-observe:${name}:${absent.status}`)
          if (absent.status === 404) continue
          if (result.status >= 500 && absent.status >= 500) {
            // The outer Rust supervisor owns and verifies deletion of the
            // complete disposable installation after this real 5xx path.
            progress(`cleanup-deferred-to-owned-root:${name}`)
            continue
          }
        }
        if (![200, 404].includes(result.status)) cleanupFailures.push(`${operation}(${name})=${result.status}`)
      }
      })
      if (!failure) {
        // `/auth/session` rotates the browser cookie. Preserve the current
        // authenticated state for the separately-created mobile context.
        await context.storageState({ path: descriptor.storage_state_path })
        await context.close()
      }
      else await context.close().catch(() => undefined)
      if (!failure) assert.deepEqual(cleanupFailures, [], `live browser cleanup failed: ${cleanupFailures.join(', ')}`)
    }
    })
  }, 'live Gateway Admin journey')
})

test('nightly mobile viewport has no overflow and essential landmarks', {
  concurrency: false,
  skip: liveEnabled && nightlyEnabled ? false : 'nightly live browser coverage is disabled',
}, async () => {
  const descriptor = await readLiveDescriptor()
  assert.ok(descriptor)
  await withAbsoluteDeadline(async (signal) => {
    await useBrowserWithAbort(signal, async () => chromium.launch(await ownedBrowserLaunchOptions(chromium.executablePath())), async (browser) => {
    const context = await browser.newContext({
      baseURL: descriptor.base_url, storageState: descriptor.storage_state_path,
      viewport: { width: 390, height: 844 },
    })
    const page = await context.newPage()
    await page.goto('/gateways/', { waitUntil: 'domcontentloaded', timeout: 15_000 })
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth), false)
    await page.getByRole('main').waitFor({ state: 'visible', timeout: 10_000 })
    const navigationToggle = page.getByRole('button', { name: 'Open navigation' })
    await navigationToggle.waitFor({ state: 'visible', timeout: 10_000 })
    await navigationToggle.click()
    const navigationDialog = page.getByRole('dialog', { name: 'Navigation' })
    await navigationDialog.waitFor({ state: 'visible', timeout: 10_000 })
    await navigationDialog.getByRole('navigation').waitFor({ state: 'visible', timeout: 10_000 })
    await context.close()
    })
  }, 'nightly live Gateway Admin journey')
})
