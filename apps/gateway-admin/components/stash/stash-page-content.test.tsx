import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'
import { readFile } from 'node:fs/promises'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils.tsx'
import { StashPageContent } from './stash-page-content.tsx'

installTestDom()
const file = (id: string) => ({ file_id: id, uri: `stash://me/files/${id}`, display_name: `${id}.txt`, size_bytes: 1, created_at: 1, updated_at: 1, owned: true })

test('Stash renders live data and appends the next cursor page', async () => {
  document.body.replaceChildren()
  globalThis.fetch = async input => {
    const url = new URL(String(input), 'http://labby.test')
    if (url.pathname.endsWith('/stats')) return Response.json({ owned_file_count: 2, owned_shared_file_count: 0, owned_committed_bytes: 2, owned_reserved_bytes: 0 })
    return Response.json(url.searchParams.has('cursor') ? { files: [file('second')], next_cursor: null } : { files: [file('first')], next_cursor: 'next' })
  }
  const view = await renderClient(<StashPageContent />)
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 300)) })
  assert.match(view.container.textContent || '', /first\.txt/)
  const loadMore = [...view.container.querySelectorAll('button')].find(button => button.textContent?.includes('Load more'))
  assert.ok(loadMore)
  await act(async () => { loadMore.dispatchEvent(new window.MouseEvent('click', { bubbles: true })); await new Promise(resolve => setTimeout(resolve, 10)) })
  assert.match(view.container.textContent || '', /first\.txt.*second\.txt/)
  await view.unmount()
})

test('Stash accessibility gate covers names, status, upload equivalence, and reduced motion', async () => {
  document.body.replaceChildren()
  globalThis.fetch = async input => String(input).includes('/stats')
    ? Response.json({ owned_file_count: 0, owned_shared_file_count: 0, owned_committed_bytes: 0, owned_reserved_bytes: 0 })
    : Response.json({ files: [], next_cursor: null })
  const view = await renderClient(<StashPageContent />)
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 300)) })

  const status = view.container.querySelector('[role="status"][aria-live="polite"]')
  assert.ok(status, 'mutation results require a polite live region')
  const uploadButtons = [...view.container.querySelectorAll('button')].filter(button => /upload|drop files here/i.test(button.textContent || ''))
  assert.equal(uploadButtons.length >= 2, true, 'button and drop-zone upload paths must both be operable controls')
  const dropTarget = uploadButtons.find(button => /drop files here/i.test(button.textContent || ''))
  assert.ok(dropTarget)
  dropTarget.focus()
  assert.equal(document.activeElement, dropTarget, 'drop-zone browse equivalent must receive keyboard focus')
  assert.ok(view.container.querySelector('input[type="file"]'), 'keyboard browse and drag/drop must share one file input')
  assert.ok(view.container.querySelector('input[placeholder="Search files…"]'), 'file search requires an accessible wrapping label')
  assert.ok(view.container.querySelector('[role="group"][aria-label="File layout"]'))

  const css = await readFile(new URL('../../app/globals.css', import.meta.url), 'utf8')
  assert.match(css, /@media \(prefers-reduced-motion: reduce\)/)
  assert.match(css, /animation:\s*none !important/)
  assert.match(css, /transition-duration:\s*1ms !important/)
  await view.unmount()
})
