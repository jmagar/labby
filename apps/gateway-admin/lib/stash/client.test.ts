import assert from 'node:assert/strict'
import test from 'node:test'

import { __setBrowserSessionStateForTests } from '../auth/session-store.ts'
import { createGrant, deleteFile, downloadUrl, listFiles, renameFile, searchRecipients, uploadFile } from './client.ts'

test.beforeEach(() => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf-stash', isAdmin: false })
})

test('list preserves opaque cursor and search while using the server page default', async () => {
  let requested: Request | undefined
  globalThis.fetch = async (input, init) => {
    requested = new Request(new URL(String(input), 'http://labby.test'), init)
    return Response.json({ files: [], next_cursor: null })
  }
  await listFiles('opaque cursor', undefined, 'needle')
  assert.equal(new URL(requested?.url || '').pathname, '/v1/stash/')
  assert.equal(new URL(requested?.url || '').searchParams.has('limit'), false)
  assert.equal(new URL(requested?.url || '').searchParams.get('cursor'), 'opaque cursor')
  assert.equal(new URL(requested?.url || '').searchParams.get('query'), 'needle')
  assert.equal(requested?.credentials, 'include')
})

test('binary upload passes the File body and csrf without JSON wrapping', async () => {
  let requested: Request | undefined
  globalThis.fetch = async (input, init) => {
    requested = new Request(new URL(String(input), 'http://labby.test'), init)
    return Response.json({ file_id: '01J', uri: 'stash://me/files/01J' }, { status: 201 })
  }
  const file = new File(['hello'], 'notes & work.md')
  await uploadFile(file)
  const url = new URL(requested?.url || '')
  assert.equal(url.search, '')
  assert.equal(decodeURIComponent(requested?.headers.get('x-labby-stash-filename') || ''), file.name)
  assert.equal(requested?.headers.get('x-csrf-token'), 'csrf-stash')
  assert.equal(requested?.body instanceof ReadableStream, true)
  assert.equal(await requested?.text(), 'hello')
})

test('mutations use encoded identifiers, csrf, and the documented bodies', async () => {
  const requests: Request[] = []
  globalThis.fetch = async (input, init) => {
    const request = new Request(new URL(String(input), 'http://labby.test'), init); requests.push(request)
    if (request.method === 'DELETE') return new Response(null, { status: 204 })
    if (request.method === 'POST') return Response.json({ grant_id: 'g', file_id: 'a/b', grantee_principal_id: 'p', created_at: 1 }, { status: 201 })
    return Response.json({ file_id: 'a/b', uri: 'stash://me/files/a%2Fb', display_name: 'new', size_bytes: 1, created_at: 1, updated_at: 2, owned: true })
  }
  await renameFile('a/b', 'new')
  await createGrant('a/b', 'p')
  await deleteFile('a/b')
  assert.ok(requests.every(request => request.url.includes('a%2Fb')))
  assert.ok(requests.every(request => request.headers.get('x-csrf-token') === 'csrf-stash'))
  assert.deepEqual(JSON.parse(await requests[0]!.text()), { display_name: 'new' })
  assert.deepEqual(JSON.parse(await requests[1]!.text()), { grantee_principal_id: 'p' })
})

test('download URLs are same-origin attachments and encode opaque IDs', () => {
  assert.equal(downloadUrl('a/b'), '/v1/stash/files/a%2Fb/content')
})

test('recipient discovery keeps identity queries out of URLs and requires csrf', async () => {
  let requested: Request | undefined
  globalThis.fetch = async (input, init) => { requested = new Request(new URL(String(input), 'http://labby.test'), init); return Response.json({ recipients: [] }) }
  await searchRecipients('private person')
  assert.equal(new URL(requested?.url || '').search, '')
  assert.equal(requested?.method, 'POST')
  assert.equal(requested?.headers.get('x-csrf-token'), 'csrf-stash')
  assert.deepEqual(JSON.parse(await requested!.text()), { query: 'private person' })
})
