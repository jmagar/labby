import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { act } from 'react'
import { Window } from 'happy-dom'
import { renderToStaticMarkup } from 'react-dom/server'

import { SettingsScalarSection } from './SettingsScalarSection'
import { setupApi, type SettingsFieldSpec, type SettingsState, type SettingsUpdateEntry } from '@/lib/api/setup-client'

const fields: SettingsFieldSpec[] = [
  { key: 'LAB_LOG', label: 'Log filter', description: '', section: 'core', backend: 'env', control: 'text', risk: 'restart', write_policy: 'editable', apply_mode: 'restart', secret: false, required: false, env_override: null, min: null, max: null, options: [], example: null },
]

function installDom() {
  const window = new Window()
  Object.defineProperty(globalThis, 'window', { configurable: true, value: window })
  Object.defineProperty(globalThis, 'self', { configurable: true, value: window })
  Object.defineProperty(globalThis, 'document', { configurable: true, value: window.document })
  Object.defineProperty(globalThis, 'navigator', { configurable: true, value: window.navigator })
  Object.defineProperty(globalThis, 'HTMLElement', { configurable: true, value: window.HTMLElement })
  Object.defineProperty(globalThis, 'HTMLButtonElement', { configurable: true, value: window.HTMLButtonElement })
  Object.defineProperty(globalThis, 'HTMLInputElement', { configurable: true, value: window.HTMLInputElement })
  Object.defineProperty(globalThis, 'Node', { configurable: true, value: window.Node })
  Object.defineProperty(globalThis, 'Event', { configurable: true, value: window.Event })
  Object.defineProperty(globalThis, 'MouseEvent', { configurable: true, value: window.MouseEvent })
  Object.defineProperty(globalThis, 'PointerEvent', { configurable: true, value: window.PointerEvent })
  Object.defineProperty(globalThis, 'KeyboardEvent', { configurable: true, value: window.KeyboardEvent })
  Object.defineProperty(globalThis, 'CustomEvent', { configurable: true, value: window.CustomEvent })
  Object.defineProperty(globalThis, 'DOMException', { configurable: true, value: window.DOMException })
  Object.defineProperty(globalThis, 'MutationObserver', {
    configurable: true,
    value: window.MutationObserver,
  })
  Object.defineProperty(globalThis, 'requestAnimationFrame', {
    configurable: true,
    value: (callback: FrameRequestCallback) => window.setTimeout(() => callback(Date.now()), 0),
  })
  Object.defineProperty(globalThis, 'cancelAnimationFrame', {
    configurable: true,
    value: (handle: number) => window.clearTimeout(handle as unknown as Parameters<typeof window.clearTimeout>[0]),
  })
  Object.defineProperty(globalThis, 'getComputedStyle', {
    configurable: true,
    value: window.getComputedStyle.bind(window),
  })
  Object.defineProperty(globalThis, 'IS_REACT_ACT_ENVIRONMENT', { configurable: true, value: true })
}

async function renderClient(element: React.ReactElement) {
  const { createRoot } = await import('react-dom/client')
  const container = document.createElement('div')
  document.body.appendChild(container)
  const root = createRoot(container)

  await act(async () => {
    root.render(element)
  })

  return {
    container,
    unmount: async () => {
      await act(async () => root.unmount())
      container.remove()
    },
  }
}

async function waitFor(assertion: () => void) {
  const deadline = Date.now() + 2_000
  let lastError: unknown
  while (Date.now() < deadline) {
    try {
      assertion()
      return
    } catch (error) {
      lastError = error
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 20))
      })
    }
  }
  throw lastError
}

async function click(element: Element | null) {
  assert.ok(element)
  await act(async () => {
    element.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    await Promise.resolve()
  })
}

async function setInputValue(input: HTMLInputElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set
  assert.ok(setter)
  await act(async () => {
    setter.call(input, value)
    input.dispatchEvent(new Event('input', { bubbles: true }))
    await Promise.resolve()
  })
}

const state: SettingsState = {
  schema_version: 1,
  config_path: '/tmp/config.toml',
  env_path: '/tmp/.env',
  section: 'core',
  values: { LAB_LOG: 'lab=info' },
  sources: { LAB_LOG: { source: 'env', overridden_by_env: null } },
}

test('SettingsScalarSection renders reset and save controls', () => {
  const html = renderToStaticMarkup(
    <SettingsScalarSection title="Core" description="" section="core" state={state} fields={fields} onSaved={() => undefined} />,
  )
  assert.match(html, /Core/)
  assert.match(html, /Reset/)
  assert.match(html, /Save changes/)
})

test('SettingsScalarSection sends previous values on confirmed env save', async () => {
  installDom()
  const originalEnvUpdate = setupApi.settingsEnvUpdate
  const originalConfigUpdate = setupApi.settingsConfigUpdate
  let receivedEntries: SettingsUpdateEntry[] = []
  let receivedConfirm = false
  setupApi.settingsEnvUpdate = async (_section, entries, confirm) => {
    receivedEntries = entries
    receivedConfirm = confirm
    return { ...state, values: { LAB_LOG: 'lab=debug' } }
  }
  setupApi.settingsConfigUpdate = async () => {
    throw new Error('config update should not be called')
  }

  try {
    const view = await renderClient(
      <SettingsScalarSection title="Core" description="" section="core" state={state} fields={fields} onSaved={() => undefined} />,
    )
    const input = view.container.querySelector('input')
    assert.ok(input)
    await setInputValue(input, 'lab=debug')
    await click(view.container.querySelector('[data-slot="checkbox"]'))
    await click([...view.container.querySelectorAll('button')].find((button) => button.textContent?.includes('Save changes')) ?? null)

    await waitFor(() => assert.deepEqual(receivedEntries, [
      { key: 'LAB_LOG', value: 'lab=debug', previous: 'lab=info' },
    ]))
    assert.equal(receivedConfirm, true)
    await view.unmount()
  } finally {
    setupApi.settingsEnvUpdate = originalEnvUpdate
    setupApi.settingsConfigUpdate = originalConfigUpdate
  }
})

test('SettingsScalarSection blocks mixed env and config saves', async () => {
  installDom()
  const configField: SettingsFieldSpec = {
    ...fields[0],
    key: 'output.format',
    label: 'Output format',
    backend: 'config_toml',
    env_override: null,
  }
  const mixedState: SettingsState = {
    ...state,
    values: { LAB_LOG: 'lab=info', 'output.format': 'table' },
    sources: {
      LAB_LOG: { source: 'env', overridden_by_env: null },
      'output.format': { source: 'config_toml', overridden_by_env: null },
    },
  }
  const originalEnvUpdate = setupApi.settingsEnvUpdate
  const originalConfigUpdate = setupApi.settingsConfigUpdate
  let calls = 0
  setupApi.settingsEnvUpdate = async () => {
    calls += 1
    return mixedState
  }
  setupApi.settingsConfigUpdate = async () => {
    calls += 1
    return { state: mixedState, backup_path: null }
  }

  try {
    const view = await renderClient(
      <SettingsScalarSection title="Core" description="" section="core" state={mixedState} fields={[fields[0], configField]} onSaved={() => undefined} />,
    )
    const inputs = [...view.container.querySelectorAll('input')]
    assert.equal(inputs.length, 2)
    await setInputValue(inputs[0], 'lab=debug')
    await setInputValue(inputs[1], 'json')
    await click(view.container.querySelector('[data-slot="checkbox"]'))
    await click([...view.container.querySelectorAll('button')].find((button) => button.textContent?.includes('Save changes')) ?? null)

    await waitFor(() => assert.match(view.container.textContent ?? '', /Save \.env and config\.toml settings separately/))
    assert.equal(calls, 0)
    await view.unmount()
  } finally {
    setupApi.settingsEnvUpdate = originalEnvUpdate
    setupApi.settingsConfigUpdate = originalConfigUpdate
  }
})

test('SettingsScalarSection sends unset for cleared optional config field', async () => {
  installDom()
  const configField: SettingsFieldSpec = {
    ...fields[0],
    key: 'workspace.root',
    label: 'Workspace root',
    backend: 'config_toml',
    env_override: null,
  }
  const configState: SettingsState = {
    ...state,
    values: { 'workspace.root': '/srv/lab' },
    sources: {
      'workspace.root': { source: 'config_toml', overridden_by_env: null },
    },
  }
  const originalEnvUpdate = setupApi.settingsEnvUpdate
  const originalConfigUpdate = setupApi.settingsConfigUpdate
  let receivedEntries: SettingsUpdateEntry[] = []
  setupApi.settingsEnvUpdate = async () => {
    throw new Error('env update should not be called')
  }
  setupApi.settingsConfigUpdate = async (_section, entries) => {
    receivedEntries = entries
    return { state: configState, backup_path: null }
  }

  try {
    const view = await renderClient(
      <SettingsScalarSection title="Core" description="" section="core" state={configState} fields={[configField]} onSaved={() => undefined} />,
    )
    const input = view.container.querySelector('input')
    assert.ok(input)
    await setInputValue(input, '')
    await click(view.container.querySelector('[data-slot="checkbox"]'))
    await click([...view.container.querySelectorAll('button')].find((button) => button.textContent?.includes('Save changes')) ?? null)

    await waitFor(() => assert.deepEqual(receivedEntries, [
      { key: 'workspace.root', value: null, previous: '/srv/lab', unset: true },
    ]))
    await view.unmount()
  } finally {
    setupApi.settingsEnvUpdate = originalEnvUpdate
    setupApi.settingsConfigUpdate = originalConfigUpdate
  }
})

test('SettingsScalarSection blocks invalid numeric input before save', async () => {
  installDom()
  const numberField: SettingsFieldSpec = {
    ...fields[0],
    key: 'mcp.session_ttl_secs',
    label: 'Session TTL',
    backend: 'config_toml',
    control: 'number',
    min: 1,
    max: 86_400,
    env_override: null,
  }
  const numberState: SettingsState = {
    ...state,
    values: { 'mcp.session_ttl_secs': 3600 },
    sources: {
      'mcp.session_ttl_secs': { source: 'config_toml', overridden_by_env: null },
    },
  }
  const originalConfigUpdate = setupApi.settingsConfigUpdate
  let calls = 0
  setupApi.settingsConfigUpdate = async () => {
    calls += 1
    return { state: numberState, backup_path: null }
  }

  try {
    const view = await renderClient(
      <SettingsScalarSection title="Surfaces" description="" section="surfaces" state={numberState} fields={[numberField]} onSaved={() => undefined} />,
    )
    const input = view.container.querySelector('input')
    assert.ok(input)
    await setInputValue(input, '90000')
    await click(view.container.querySelector('[data-slot="checkbox"]'))
    await click([...view.container.querySelectorAll('button')].find((button) => button.textContent?.includes('Save changes')) ?? null)

    await waitFor(() => assert.match(view.container.textContent ?? '', /Must be at most 86400/))
    assert.equal(calls, 0)
    await view.unmount()
  } finally {
    setupApi.settingsConfigUpdate = originalConfigUpdate
  }
})
