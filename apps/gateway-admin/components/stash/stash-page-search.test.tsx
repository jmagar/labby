import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'
import { Window } from 'happy-dom'

const file = (id: string) => ({ file_id: id, uri: `stash://me/files/${id}`, display_name: `${id}.txt`, size_bytes: 1, created_at: 1, updated_at: 1, owned: true })

function installDomBeforeReactDom() {
  const browser = new Window()
  for (const [key, value] of Object.entries({ window: browser, document: browser.document, navigator: browser.navigator, DOMException: browser.DOMException, Node: browser.Node, Element: browser.Element, HTMLElement: browser.HTMLElement, HTMLInputElement: browser.HTMLInputElement, InputEvent: browser.InputEvent, MouseEvent: browser.MouseEvent, PointerEvent: browser.PointerEvent, KeyboardEvent: browser.KeyboardEvent, CustomEvent: browser.CustomEvent, MutationObserver: browser.MutationObserver })) {
    Object.defineProperty(globalThis, key, { configurable: true, value })
  }
  Object.defineProperty(globalThis, 'getComputedStyle', { configurable: true, value: browser.getComputedStyle.bind(browser) })
  Object.defineProperty(globalThis, 'requestAnimationFrame', { configurable: true, value: (callback: FrameRequestCallback) => browser.setTimeout(() => callback(Date.now()), 0) })
  Object.defineProperty(globalThis, 'cancelAnimationFrame', { configurable: true, value: (handle: number) => browser.clearTimeout(handle as unknown as Parameters<typeof browser.clearTimeout>[0]) })
  Object.defineProperty(globalThis, 'IS_REACT_ACT_ENVIRONMENT', { configurable: true, value: true })
}

async function waitFor(assertion: () => void) {
  const deadline = Date.now() + 2_000
  let lastError: unknown
  while (Date.now() < deadline) {
    try { assertion(); return } catch (error) { lastError = error }
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 5)) })
  }
  throw lastError
}

test('search rejects stale responses and does not refresh stats', async () => {
  installDomBeforeReactDom()
  const [{ StashPageContent }, { renderClient }] = await Promise.all([
    import('./stash-page-content.tsx'),
    import('@/lib/testing/dom-test-utils.tsx'),
  ])
  let statsCalls = 0
  const pending = new Map<string, (response: Response) => void>()
  globalThis.fetch = async input => {
    const url = new URL(String(input), 'http://labby.test')
    if (url.pathname.endsWith('/stats')) { statsCalls += 1; return Response.json({ owned_file_count: 1, owned_shared_file_count: 0, owned_committed_bytes: 1, owned_reserved_bytes: 0 }) }
    const query = url.searchParams.get('query') || ''
    if (!query) return Response.json({ files: [file('initial')], next_cursor: null })
    return new Promise<Response>(resolve => pending.set(query, resolve))
  }
  const view = await renderClient(<StashPageContent />)
  await waitFor(() => assert.match(view.container.textContent || '', /initial\.txt/))
  const search = view.container.querySelector('input[placeholder="Search files…"]') as HTMLInputElement
  const setSearch = async (value: string) => act(async () => {
    Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set?.call(search, value)
    search.dispatchEvent(new window.InputEvent('input', { bubbles: true, data: value }) as unknown as Event)
  })
  await setSearch('old')
  await waitFor(() => assert.ok(pending.has('old')))
  await setSearch('new')
  await waitFor(() => assert.ok(pending.has('new')))
  await act(async () => { pending.get('new')!(Response.json({ files: [file('new')], next_cursor: null })) })
  await waitFor(() => assert.match(view.container.textContent || '', /new\.txt/))
  await act(async () => { pending.get('old')!(Response.json({ files: [file('old')], next_cursor: null })) })
  assert.doesNotMatch(view.container.textContent || '', /old\.txt/)
  assert.equal(statsCalls, 1)
  await view.unmount()
})

test('manage mutation errors remain visible inside the active dialog', async () => {
  document.body.replaceChildren()
  const [{ ManageDialog }, { renderClient }] = await Promise.all([
    import('./stash-page-content.tsx'),
    import('@/lib/testing/dom-test-utils.tsx'),
  ])
  globalThis.fetch = async (input) => {
    const url = new URL(String(input), 'http://labby.test')
    if (url.pathname.endsWith('/grants')) return Response.json({ grants: [], next_cursor: null })
    return Response.json({ kind: 'conflict', message: 'Rename conflict' }, { status: 409 })
  }
  const target = file('managed')
  const view = await renderClient(<ManageDialog file={target} onClose={() => {}} onChanged={async () => {}} onError={() => {}} />)
  await waitFor(() => assert.ok(document.body.querySelector('[role="dialog"]')))
  const input = document.body.querySelector('[role="dialog"] input') as HTMLInputElement
  await act(async () => {
    Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set?.call(input, 'renamed.txt')
    input.dispatchEvent(new window.InputEvent('input', { bubbles: true, data: 'renamed.txt' }) as unknown as Event)
  })
  const rename = [...document.body.querySelectorAll('button')].find(button => /^Rename$/.test(button.textContent || ''))!
  await act(async () => { rename.dispatchEvent(new window.MouseEvent('click', { bubbles: true })) })
  await waitFor(() => assert.match(document.body.querySelector('[role="dialog"]')?.textContent || '', /That name is already used/))
  await view.unmount()
})
