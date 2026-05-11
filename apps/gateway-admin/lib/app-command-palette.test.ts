import assert from 'node:assert/strict'
import test from 'node:test'

import {
  appCommandItems,
  buildAppCommandState,
  findAppCommandItemById,
} from './app-command-palette'

test('app command palette ranks server searches first', () => {
  const state = buildAppCommandState('server')

  assert.equal(state.activeItemId, 'destination-gateways')
  assert.equal(state.groups[0]?.key, 'best-match')
  assert.equal(state.groups[0]?.items[0]?.href, '/gateways')
})

test('app command palette includes core admin destinations', () => {
  const hrefs = new Set(appCommandItems.map((item) => item.href))

  for (const href of [
    '/',
    '/gateways',
    '/marketplace',
    '/chat',
    '/setup',
    '/activity',
    '/logs',
    '/settings',
    '/docs',
  ]) {
    assert.equal(hrefs.has(href), true, `${href} should be searchable`)
  }
})

test('app command palette reports empty state for unmatched queries', () => {
  const state = buildAppCommandState('zzzz-no-match')

  assert.equal(state.activeItemId, null)
  assert.deepEqual(state.items, [])
  assert.deepEqual(state.groups, [])
})

test('findAppCommandItemById returns matching command item', () => {
  const item = findAppCommandItemById('destination-logs', appCommandItems)

  assert.equal(item?.title, 'Logs')
  assert.equal(item?.href, '/logs')
})
