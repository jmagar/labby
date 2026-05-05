import test from 'node:test'
import assert from 'node:assert/strict'

import {
  MAX_LOCAL_ATTACHMENTS,
  MAX_LOCAL_ATTACHMENT_BYTES,
  fileToSerializableAttachment,
  validateLocalFiles,
} from './local-attachments.ts'

function file(name: string, type: string, body: string): File {
  return new File([body], name, { type })
}

test('validateLocalFiles accepts supported files within count and size limits', () => {
  const files = [
    file('notes.txt', 'text/plain', 'hello'),
    file('diagram.png', 'image/png', 'png-bytes'),
  ]

  const result = validateLocalFiles(files, [])

  assert.deepEqual(result.errors, [])
  assert.equal(result.accepted.length, 2)
})

test('validateLocalFiles rejects unsupported types, oversized files, and count overflow', () => {
  const existing = Array.from({ length: MAX_LOCAL_ATTACHMENTS }, (_, index) =>
    file(`existing-${index}.txt`, 'text/plain', 'x'),
  )
  const unsupported = file('archive.zip', 'application/zip', 'zip')
  const oversized = new File([new Uint8Array(MAX_LOCAL_ATTACHMENT_BYTES + 1)], 'big.txt', {
    type: 'text/plain',
  })

  const result = validateLocalFiles([unsupported, oversized], existing)

  assert.equal(result.accepted.length, 0)
  assert.ok(result.errors.some((message) => message.includes('You can attach up to 5 files')))
  assert.ok(result.errors.some((message) => message.includes('archive.zip has unsupported type application/zip')))
  assert.ok(result.errors.some((message) => message.includes('big.txt is larger than 48 KiB')))
})

test('fileToSerializableAttachment emits text resources for text files', async () => {
  const result = await fileToSerializableAttachment(
    {
      id: 'local-1',
      kind: 'local',
      file: file('notes.txt', 'text/plain', 'hello world'),
      previewUrl: null,
    },
  )

  assert.deepEqual(result, {
    kind: 'local',
    id: 'local-1',
    name: 'notes.txt',
    mimeType: 'text/plain',
    size: 11,
    contentKind: 'text',
    text: 'hello world',
  })
})

test('fileToSerializableAttachment emits base64 blob resources for binary files', async () => {
  const result = await fileToSerializableAttachment(
    {
      id: 'local-2',
      kind: 'local',
      file: new File([new Uint8Array([1, 2, 3])], 'image.png', { type: 'image/png' }),
      previewUrl: 'blob:http://localhost/image',
    },
  )

  assert.deepEqual(result, {
    kind: 'local',
    id: 'local-2',
    name: 'image.png',
    mimeType: 'image/png',
    size: 3,
    contentKind: 'blob',
    base64: 'AQID',
  })
})

test('fileToSerializableAttachment normalizes json MIME casing before content selection', async () => {
  const result = await fileToSerializableAttachment(
    {
      id: 'local-json',
      kind: 'local',
      file: file('data.json', 'Application/JSON', '{"ok":true}'),
      previewUrl: null,
    },
  )

  assert.equal(result.mimeType, 'application/json')
  assert.equal(result.contentKind, 'text')
  assert.equal(result.text, '{"ok":true}')
})
