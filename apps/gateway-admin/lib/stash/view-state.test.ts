import assert from 'node:assert/strict'
import test from 'node:test'
import { acceptGeneration, acceptGrantPage, acceptRecipientSearch, copyUri, mergeFiles, mergeGrants, selectedRecipientId } from './view-state.ts'

const file = (id: string) => ({ file_id: id, uri: `stash://me/files/${id}`, display_name: `${id}.txt`, size_bytes: 1, created_at: 1, updated_at: 1, owned: true })
const grant = (id: string) => ({ grant_id: id, file_id: 'f', grantee_principal_id: 'p', created_at: 1 })

test('cursor pages append in server order and dedupe stable IDs', () => assert.deepEqual(mergeFiles([file('a')], [file('a'), file('b')], true).map(item => item.file_id), ['a', 'b']))
test('a newer search generation rejects a stale response', () => { assert.equal(acceptGeneration(4, 3), false); assert.equal(acceptGeneration(4, 4), true) })
test('grant pages require both current generation and current file', () => { assert.equal(acceptGrantPage(2, 2, 'b', 'a'), false); assert.equal(acceptGrantPage(2, 1, 'a', 'a'), false); assert.equal(acceptGrantPage(2, 2, 'a', 'a'), true) })
test('grant cursor append dedupes grant IDs', () => assert.deepEqual(mergeGrants([grant('g1')], [grant('g1'), grant('g2')], true).map(item => item.grant_id), ['g1', 'g2']))
test('recipient selection accepts only an authoritative search result', () => { assert.equal(selectedRecipientId([{ principal_id: 'p1' }], 'p1'), 'p1'); assert.equal(selectedRecipientId([{ principal_id: 'p1' }], 'invented'), undefined) })
test('recipient search rejects stale generation, file, and normalized query', () => { assert.equal(acceptRecipientSearch(2, 1, 'f', 'f', 'pat', 'pat'), false); assert.equal(acceptRecipientSearch(2, 2, 'g', 'f', 'pat', 'pat'), false); assert.equal(acceptRecipientSearch(2, 2, 'f', 'f', 'new', 'old'), false); assert.equal(acceptRecipientSearch(2, 2, 'f', 'f', ' pat ', 'pat'), true) })
test('clipboard announces only successful writes and returns failures', async () => { assert.deepEqual(await copyUri(async () => {}, 'stash://me/files/a'), { ok: true, announcement: 'Copied stash://me/files/a' }); const result = await copyUri(async () => { throw new Error('denied') }, 'x'); assert.equal(result.ok, false) })
