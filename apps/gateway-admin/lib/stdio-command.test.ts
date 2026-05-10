import test from 'node:test'
import assert from 'node:assert/strict'

import { formatStdioCommandLine, parseStdioCommandLine } from './stdio-command.ts'

test('parseStdioCommandLine splits command and args', () => {
  assert.deepEqual(parseStdioCommandLine('npx -y example-mcp --stdio'), {
    command: 'npx',
    args: ['-y', 'example-mcp', '--stdio'],
  })
})

test('parseStdioCommandLine preserves quoted args with spaces', () => {
  assert.deepEqual(parseStdioCommandLine('uvx example --path "/home/me/My Docs"'), {
    command: 'uvx',
    args: ['example', '--path', '/home/me/My Docs'],
  })
})

test('parseStdioCommandLine converts leading env assignments to env command form', () => {
  assert.deepEqual(parseStdioCommandLine('NODE_ENV=production npx example-mcp --stdio'), {
    command: 'env',
    args: ['NODE_ENV=production', 'npx', 'example-mcp', '--stdio'],
  })
})

test('parseStdioCommandLine reports malformed command lines', () => {
  assert.throws(() => parseStdioCommandLine('npx "unterminated'), /unterminated quote/)
  assert.throws(() => parseStdioCommandLine('npx trailing\\'), /trailing escape/)
  assert.throws(() => parseStdioCommandLine('   '), /Command is required/)
})

test('formatStdioCommandLine quotes tokens that need it', () => {
  assert.equal(
    formatStdioCommandLine('uvx', ['example', '--path', '/home/me/My Docs']),
    "uvx example --path '/home/me/My Docs'",
  )
})
