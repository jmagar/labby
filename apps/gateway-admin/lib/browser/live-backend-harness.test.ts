import assert from 'node:assert/strict'
import { spawn, type ChildProcess } from 'node:child_process'
import { once } from 'node:events'
import { mkdtemp, mkdir, readFile, readdir, realpath, rm, symlink, writeFile } from 'node:fs/promises'
import { createConnection } from 'node:net'
import os from 'node:os'
import path from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

import type { Browser, BrowserContext, Page } from 'playwright'

import {
  captureFailureEvidence,
  launchBrowserWithAbort,
  observeLivePage,
  MAX_EVIDENCE_TEXT_BYTES,
  MAX_ARTIFACT_BYTES,
  readLiveDescriptorAt,
  runBrowserCleanupIfActive,
  scanArtifact,
  useBrowserWithAbort,
  withAbsoluteDeadline,
  type LiveBackendDescriptor,
  type LiveBrowserEvidence,
} from './live-backend-harness.ts'

test('absolute deadline aborts and drains the timed-out operation', async () => {
  let settled = false
  let observedAbort = false
  await assert.rejects(
    withAbsoluteDeadline(
      async (signal) => {
        await new Promise<void>((resolve) =>
          signal.addEventListener(
            'abort',
            () => {
              observedAbort = true
              setTimeout(resolve, 10)
            },
            { once: true },
          ),
        )
        settled = true
      },
      'cooperative operation',
      5,
    ),
    /cooperative operation exceeded 5ms/,
  )
  assert.equal(observedAbort, true)
  assert.equal(settled, true, 'deadline must drain cooperative teardown before returning')
})

async function settleOwnedSupervisor(child: ChildProcess): Promise<void> {
  // A reaped supervisor's recorded numeric PGID is not retained authority.
  // Only the live ChildProcess handle may be signaled; its TERM trap owns the
  // admission registry and descendant cleanup.
  if (child.exitCode !== null || child.signalCode !== null) return
  const exited = once(child, 'exit')
  child.kill('SIGTERM')
  try {
    await withAbsoluteDeadline(async () => exited, 'owned supervisor cleanup', 8_000, 0)
  } catch (error) {
    child.kill('SIGKILL')
    try {
      await withAbsoluteDeadline(async () => exited, 'owned supervisor final reap', 1_000, 0)
    } catch (settlementError) {
      throw new AggregateError([error, settlementError], 'supervisor cleanup and settlement failed', { cause: settlementError })
    }
    // Direct-child settlement does not prove descendant cleanup after KILL.
    throw error
  }
}

async function settleSupervisorAfterFailure(child: ChildProcess, failure: unknown): Promise<never> {
  try {
    await settleOwnedSupervisor(child)
  } catch (cleanupError) {
    throw new AggregateError([failure, cleanupError], 'supervisor fixture and cleanup failed', { cause: cleanupError })
  }
  throw failure
}

test('a reaped supervisor never authorizes a fallback signal', async () => {
  const child = spawn(process.execPath, ['-e', 'process.exit(0)'], { stdio: 'ignore' })
  await withAbsoluteDeadline(async () => once(child, 'exit'), 'supervisor fixture exit', 3_000, 0)
    .catch(async (error: unknown) => settleSupervisorAfterFailure(child, error))
  let signaled = false
  child.kill = () => { signaled = true; return false }
  await settleOwnedSupervisor(child)
  assert.equal(signaled, false, 'a stale child must not trigger any signal callback')
})

test('live supervisor cleanup uses its owned TERM handler before settlement', async () => {
  const child = spawn(process.execPath, ['-e', `
    process.on('SIGTERM', () => process.exit(0))
    process.stdout.write('ready')
    setInterval(() => {}, 1_000)
  `], { stdio: ['ignore', 'pipe', 'ignore'] })
  await withAbsoluteDeadline(async () => once(child.stdout!, 'data'), 'supervisor fixture readiness', 3_000, 0)
    .catch(async (error: unknown) => settleSupervisorAfterFailure(child, error))
  await settleOwnedSupervisor(child)
  assert.equal(child.exitCode, 0)
  assert.equal(child.signalCode, null)
})

for (const leaderExit of [false, true]) {
test(`outer cancellation reaps Playwright detached browser despite a wedged Node owner (leader exits: ${leaderExit})`, { skip: process.platform === 'win32' }, async () => {
  const parent = await mkdtemp(path.join(os.tmpdir(), 'labby-browser-containment-'))
  const root = path.join(parent, 'run')
  const script = fileURLToPath(new URL('../../../../scripts/ci/labby-live-e2e.sh', import.meta.url))
  const child = spawn('bash', [script, 'pr', '1'], {
    env: {
      ...process.env,
      PATH: `${path.dirname(process.execPath)}:${process.env.PATH ?? '/usr/bin:/bin'}`,
      LABBY_E2E_RUN_ROOT: root,
      LABBY_E2E_ESCAPED_BROWSER_SELFTEST: '1',
      LABBY_E2E_BROWSER_LEADER_EXIT: leaderExit ? '1' : '0',
      LABBY_NODE_BIN: process.execPath,
      LABBY_E2E_PREBUILT: '1',
      LABBY_E2E_BINARY: '/usr/bin/true',
      LABBY_E2E_SHARD_TIMEOUT_SECONDS: '3',
      LABBY_E2E_RUN_TIMEOUT_SECONDS: '6',
    },
    stdio: 'ignore',
  })
  let primaryFailure: { error: unknown } | undefined
  try {
    const [code] = await withAbsoluteDeadline(async () => once(child, 'exit'), 'detached browser supervisor', 15_000, 0)
    assert.notEqual(code, 0, 'cancelled qualification must fail')
    const { pid, port } = JSON.parse(await readFile(path.join(root, 'detached-browser.json'), 'utf8')) as { pid: number; port: number }
    assert.throws(() => process.kill(pid, 0), { code: 'ESRCH' }, 'registered browser process must be absent')
    await new Promise<void>((resolve, reject) => {
      const socket = createConnection({ host: '127.0.0.1', port })
      socket.once('connect', () => { socket.destroy(); reject(new Error('owned browser listener survived')) })
      socket.once('error', (error) => {
        socket.destroy()
        if ((error as NodeJS.ErrnoException).code === 'ECONNREFUSED') resolve()
        else reject(error)
      })
      socket.setTimeout(1_000, () => { socket.destroy(); reject(new Error('listener absence could not be verified')) })
    })
    const status = JSON.parse(await readFile(path.join(root, 'artifacts/status.json'), 'utf8'))
    assert.equal(status.primary, 1)
    assert.equal(status.cleanup, 0)
  } catch (error) {
    primaryFailure = { error }
  }
  try {
    await settleOwnedSupervisor(child)
    await rm(parent, { recursive: true, force: true })
  } catch (cleanupError) {
    if (primaryFailure) throw new AggregateError([primaryFailure.error, cleanupError], 'browser assertion and owned cleanup failed', { cause: cleanupError })
    throw cleanupError
  }
  if (primaryFailure) throw primaryFailure.error
})
}

test('absolute deadline returns after bounded grace when operation never settles', async () => {
  await assert.rejects(
    withAbsoluteDeadline(
      async () => {
        await new Promise<never>(() => undefined)
      },
      'non-cooperative operation',
      0,
      0,
    ),
    /non-cooperative operation exceeded 0ms/,
  )
})

test('already aborted browser ownership never starts a launch', async () => {
  let launched = false
  await assert.rejects(launchBrowserWithAbort(AbortSignal.abort(new Error('expired')), async () => {
    launched = true
    return { close: async () => undefined }
  }), /expired/)
  assert.equal(launched, false)
})

test('request paths share the aggregate evidence byte budget', () => {
  const handlers = new Map<string, (...args: never[]) => void>()
  const page = { on: (event: string, handler: (...args: never[]) => void) => handlers.set(event, handler) } as unknown as Page
  const evidence = observeLivePage(page, 'http://localhost')
  const request = { url: () => `http://localhost/${'x'.repeat(MAX_EVIDENCE_TEXT_BYTES)}`, method: () => 'GET' }
  handlers.get('request')!(request as never)
  assert.deepEqual(evidence.requests, [])
})

test('browser produced after abort during launch is closed before use', async () => {
  const controller = new AbortController()
  let resolveLaunch!: (browser: Pick<Browser, 'close'>) => void
  const launch = new Promise<Pick<Browser, 'close'>>((resolve) => {
    resolveLaunch = resolve
  })
  let closes = 0
  const pending = launchBrowserWithAbort(controller.signal, () => launch)
  controller.abort(new Error('launch deadline expired'))
  resolveLaunch({
    close: async () => {
      closes += 1
    },
  })

  await assert.rejects(pending, /launch deadline expired/)
  assert.equal(closes, 1)
})

test('launch-time abort remains primary when browser close rejects', async () => {
  const controller = new AbortController()
  let resolveLaunch!: (browser: Pick<Browser, 'close'>) => void
  const launch = new Promise<Pick<Browser, 'close'>>((resolve) => {
    resolveLaunch = resolve
  })
  const pending = launchBrowserWithAbort(controller.signal, () => launch)
  controller.abort(new Error('launch deadline expired'))
  resolveLaunch({
    close: async () => {
      throw new Error('close failed')
    },
  })

  await assert.rejects(pending, /launch deadline expired/)
})

test('browser cleanup owns failures from the first post-launch initialization step', async () => {
  const controller = new AbortController()
  let closes = 0
  await assert.rejects(
    useBrowserWithAbort(
      controller.signal,
      async () => ({
        close: async () => {
          closes += 1
        },
      }),
      async () => {
        throw new Error('context initialization failed')
      },
    ),
    /context initialization failed/,
  )
  assert.equal(closes, 1)
})

test('browser abort and finalization share one close operation', async () => {
  const controller = new AbortController()
  let closes = 0
  await assert.rejects(
    useBrowserWithAbort(
      controller.signal,
      async () => ({
        close: async () => {
          closes += 1
        },
      }),
      async () => {
        controller.abort(new Error('journey aborted'))
        controller.signal.throwIfAborted()
      },
    ),
    /journey aborted/,
  )
  assert.equal(closes, 1)
})

test('browser close failure does not replace the primary operation error', async () => {
  const controller = new AbortController()
  let combined: AggregateError | undefined
  try {
    await useBrowserWithAbort(
      controller.signal,
      async () => ({
        close: async () => {
          throw new Error('close failed')
        },
      }),
      async () => {
        throw new Error('primary journey failed')
      },
    )
    assert.fail('operation unexpectedly succeeded')
  } catch (error) {
    assert.ok(error instanceof AggregateError)
    combined = error
  }
  assert.ok(combined)
  assert.match(combined.message, /browser operation and cleanup both failed/)
  assert.deepEqual(
    combined.errors.map((error) => (error instanceof Error ? error.message : String(error))),
    ['primary journey failed', 'close failed'],
  )
  await assert.rejects(
    useBrowserWithAbort(
      controller.signal,
      async () => ({
        close: async () => {
          throw new Error('close failed')
        },
      }),
      async () => undefined,
    ),
    /close failed/,
  )
})

test('aborted browser cleanup is deferred without starting mutations', async () => {
  const controller = new AbortController()
  controller.abort(new Error('journey deadline expired'))
  let deferrals = 0
  let mutations = 0

  await runBrowserCleanupIfActive(
    controller.signal,
    () => {
      deferrals += 1
    },
    async () => {
      mutations += 1
    },
  )

  assert.equal(deferrals, 1)
  assert.equal(mutations, 0, 'an aborted journey must leave owned-root cleanup to the outer supervisor')
})

test('active browser cleanup executes exactly once', async () => {
  const controller = new AbortController()
  let deferrals = 0
  let mutations = 0

  await runBrowserCleanupIfActive(
    controller.signal,
    () => {
      deferrals += 1
    },
    async () => {
      mutations += 1
    },
  )

  assert.equal(deferrals, 0)
  assert.equal(mutations, 1)
})

test('aborted nightly journey does not start context cleanup', async () => {
  const controller = new AbortController()
  controller.abort(new Error('nightly journey deadline expired'))
  let contextCloses = 0

  await runBrowserCleanupIfActive(
    controller.signal,
    () => undefined,
    async () => {
      contextCloses += 1
    },
  )

  assert.equal(contextCloses, 0)
})

test('abort during cleanup prevents every later mutation without throwing', async () => {
  const controller = new AbortController()
  let deferrals = 0
  let firstMutations = 0
  let laterMutations = 0
  let contextCloses = 0

  await runBrowserCleanupIfActive(
    controller.signal,
    () => {
      deferrals += 1
    },
    async (mayContinue) => {
      assert.equal(mayContinue(), true)
      firstMutations += 1
      await Promise.resolve()
      controller.abort(new Error('cleanup deadline expired'))
      if (!mayContinue()) return
      laterMutations += 1
      if (!mayContinue()) return
      contextCloses += 1
    },
  )

  assert.equal(firstMutations, 1)
  assert.equal(laterMutations, 0)
  assert.equal(contextCloses, 0)
  assert.equal(deferrals, 1)
})

async function privateFile(filePath: string, value: string) {
  await writeFile(filePath, value, { mode: 0o600 })
  return filePath
}

async function descriptorFixture() {
  const parent = await mkdtemp(path.join(os.tmpdir(), 'labby-browser-descriptor-'))
  const runRoot = path.join(parent, 'owned-run')
  const evidenceDir = path.join(runRoot, 'evidence')
  await mkdir(evidenceDir, { recursive: true, mode: 0o700 })
  const storageState = await privateFile(path.join(runRoot, 'storage.json'), '{}')
  const csrfState = await privateFile(path.join(runRoot, 'csrf.json'), '{"csrf_token":"0123456789abcdef"}')
  const scanSecrets = await privateFile(path.join(runRoot, 'scan-secrets'), 'secret-canary\n')
  const descriptorPath = path.join(runRoot, 'descriptor.json')
  const descriptor: LiveBackendDescriptor = {
    version: 1,
    run_id: 'browser_test_run',
    base_url: 'http://127.0.0.1:40123',
    run_root: runRoot,
    storage_state_path: storageState,
    csrf_state_path: csrfState,
    evidence_dir: evidenceDir,
    scan_secrets_path: scanSecrets,
    restart_request_path: path.join(runRoot, 'restart.request'),
    restart_complete_path: path.join(runRoot, 'restart.complete'),
    stash_supported: true,
    recipient_principal_id: 'browser-stash-recipient',
  }
  await privateFile(descriptorPath, JSON.stringify(descriptor))
  return { parent, runRoot, evidenceDir, descriptorPath, descriptor }
}

test('live descriptor accepts only canonical paths below its run-owned root', async () => {
  const fixture = await descriptorFixture()
  const parsed = await readLiveDescriptorAt(fixture.descriptorPath)
  assert.equal(parsed.run_root, await realpath(fixture.runRoot))
  assert.equal(parsed.evidence_dir, await realpath(fixture.evidenceDir))

  const outside = await privateFile(path.join(fixture.parent, 'outside.json'), '{}')
  await privateFile(fixture.descriptorPath, JSON.stringify({ ...fixture.descriptor, storage_state_path: outside }))
  await assert.rejects(readLiveDescriptorAt(fixture.descriptorPath), /below the run-owned root/)
})

test('live descriptor rejects symlink leaves and symlinked path components', async () => {
  const fixture = await descriptorFixture()
  const outside = await privateFile(path.join(fixture.parent, 'outside.json'), '{}')
  const leafLink = path.join(fixture.runRoot, 'linked-storage.json')
  await symlink(outside, leafLink)
  await privateFile(fixture.descriptorPath, JSON.stringify({ ...fixture.descriptor, storage_state_path: leafLink }))
  await assert.rejects(readLiveDescriptorAt(fixture.descriptorPath), /must not be a symlink/)

  const linkedDirectory = path.join(fixture.runRoot, 'linked-directory')
  await symlink(fixture.parent, linkedDirectory)
  await privateFile(
    fixture.descriptorPath,
    JSON.stringify({
      ...fixture.descriptor,
      storage_state_path: path.join(linkedDirectory, 'outside.json'),
    }),
  )
  await assert.rejects(readLiveDescriptorAt(fixture.descriptorPath), /symlink components|below the run-owned root/)
})

test('failed secret scan deletes only invocation-created evidence and preserves decoys', async () => {
  const fixture = await descriptorFixture()
  const decoy = path.join(fixture.evidenceDir, 'preexisting-decoy.txt')
  await privateFile(decoy, 'must survive')
  const evidence: LiveBrowserEvidence = {
    requests: [],
    console: [],
    pageErrors: [],
    failedRequests: [],
    cspViolations: [],
  }
  const page = {
    screenshot: async () => Buffer.from('secret-canary'),
  } as unknown as Page
  const context = {
    tracing: { stop: async () => undefined },
  } as unknown as BrowserContext

  await assert.rejects(
    captureFailureEvidence({
      browser: {} as Browser,
      context,
      page,
      descriptor: fixture.descriptor,
      evidence,
      error: new Error('expected failure'),
    }),
    /contained scan-only secret material/,
  )
  assert.equal(await readFile(decoy, 'utf8'), 'must survive')
})

test('aborted non-cooperative capture has no path it can mutate later', async () => {
  const fixture = await descriptorFixture()
  const controller = new AbortController()
  let release!: () => void
  const gate = new Promise<void>((resolve) => {
    release = resolve
  })
  let markStarted!: () => void
  const started = new Promise<void>((resolve) => {
    markStarted = resolve
  })
  let receivedPath = false
  let traceStops = 0
  const page = {
    screenshot: async (options: { path?: string }) => {
      receivedPath = options.path !== undefined
      markStarted()
      await gate
      if (options.path) await privateFile(options.path, 'late mutation')
      return Buffer.from('late screenshot bytes')
    },
  } as unknown as Page
  const context = {
    tracing: {
      stop: async () => {
        traceStops += 1
      },
    },
  } as unknown as BrowserContext
  const pending = captureFailureEvidence({
    browser: {} as Browser,
    context,
    page,
    descriptor: fixture.descriptor,
    evidence: { requests: [], console: [], pageErrors: [], failedRequests: [], cspViolations: [] },
    error: new Error('journey failed'),
    signal: controller.signal,
  })
  await started
  controller.abort(new Error('journey deadline expired'))
  await assert.rejects(pending, /journey deadline expired/)
  release()
  await new Promise((resolve) => setImmediate(resolve))

  assert.equal(receivedPath, false)
  assert.equal(traceStops, 0, 'an aborted screenshot must not start trace shutdown')
  const [invocation] = await readdir(fixture.evidenceDir)
  assert.ok(invocation)
  assert.deepEqual(await readdir(path.join(fixture.evidenceDir, invocation)), [])
})

test('capture failures are recorded in the retained report and reject the operation', async () => {
  const fixture = await descriptorFixture()
  const evidence: LiveBrowserEvidence = {
    requests: [],
    console: [],
    pageErrors: [],
    failedRequests: [],
    cspViolations: [],
  }
  const page = {
    screenshot: async () => {
      throw new Error('screenshot backend unavailable')
    },
  } as unknown as Page
  const context = {
    tracing: {
      stop: async () => {
        throw new Error('trace already stopped')
      },
    },
  } as unknown as BrowserContext

  await assert.rejects(
    captureFailureEvidence({
      browser: {} as Browser,
      context,
      page,
      descriptor: fixture.descriptor,
      evidence,
      error: new Error('journey failed'),
    }),
    /browser failure evidence capture was incomplete/,
  )

  const [invocation] = await readdir(fixture.evidenceDir)
  assert.ok(invocation)
  const report = JSON.parse(await readFile(path.join(fixture.evidenceDir, invocation, 'failure.json'), 'utf8'))
  assert.deepEqual(report.captures, {
    screenshot: { status: 'failed', error: 'screenshot backend unavailable' },
    trace: { status: 'failed', error: 'trace already stopped' },
  })
})

test('oversized screenshots are rejected before file publication', async () => {
  const fixture = await descriptorFixture()
  await assert.rejects(captureFailureEvidence({
    browser: {} as Browser,
    context: { tracing: { stop: async () => undefined } } as unknown as BrowserContext,
    page: { screenshot: async () => Buffer.alloc(MAX_ARTIFACT_BYTES + 1) } as unknown as Page,
    descriptor: fixture.descriptor,
    evidence: { requests: [], console: [], pageErrors: [], failedRequests: [], cspViolations: [] },
    error: new Error('journey failed'),
  }), /capture was incomplete/)
  const [invocation] = await readdir(fixture.evidenceDir)
  assert.ok(invocation)
  assert.deepEqual(await readdir(path.join(fixture.evidenceDir, invocation)), ['failure.json'])
})

test('artifact scanning tolerates only an explicitly optional missing file', async () => {
  const fixture = await descriptorFixture()
  const missing = path.join(fixture.runRoot, 'missing-artifact')
  await assert.rejects(scanArtifact(missing, [Buffer.from('secret-canary')]), {
    code: 'ENOENT',
  })
  await assert.doesNotReject(scanArtifact(missing, [Buffer.from('secret-canary')], { optional: true }))
  await assert.rejects(scanArtifact(fixture.evidenceDir, [Buffer.from('secret-canary')]), /EISDIR|illegal operation/)
})

test('artifact scanning enforces actual streamed bytes and an absolute deadline', async () => {
  const fixture = await descriptorFixture()
  const apparentlySmall = await privateFile(path.join(fixture.runRoot, 'apparently-small-artifact'), 'safe')
  await assert.rejects(
    scanArtifact(apparentlySmall, [Buffer.from('secret-canary')], {
      timeoutMs: 1_000,
      openStream: async function* () {
        yield Buffer.alloc(16 * 1024 * 1024)
        yield Buffer.alloc(1)
      },
    }),
    /exceeded artifact cap while streaming/,
  )

  const ordinary = await privateFile(path.join(fixture.runRoot, 'ordinary-artifact'), 'safe')
  await assert.rejects(
    scanArtifact(ordinary, [Buffer.from('secret-canary')], {
      timeoutMs: 0,
      openStream: async function* (_artifact, signal) {
        await new Promise<void>((_resolve, reject) =>
          signal.addEventListener(
            'abort',
            () => {
              reject(new Error('stream aborted'))
            },
            { once: true },
          ),
        )
        yield Buffer.alloc(0)
      },
    }),
    /artifact scan deadline exceeded/,
  )
})

test('CI uploads only the exclusive current run-attempt evidence directory', async () => {
  const workflowPath = path.resolve(import.meta.dirname, '../../../../.github/workflows/ci.yml')
  const workflow = await readFile(workflowPath, 'utf8')
  assert.match(
    workflow,
    /run_root="\$\{RUNNER_TEMP\}\/labby-live-e2e-\$\{GITHUB_RUN_ID\}-\$\{GITHUB_RUN_ATTEMPT\}-\$\{GITHUB_JOB\}"/,
  )
  assert.match(
    workflow,
    /path: \$\{\{ runner\.temp \}\}\/labby-live-e2e-\$\{\{ github\.run_id \}\}-\$\{\{ github\.run_attempt \}\}-live-e2e-core\/artifacts\//,
  )
  assert.doesNotMatch(workflow, /\/tmp\/labby-live-e2e\.\*\/artifacts/)
})
