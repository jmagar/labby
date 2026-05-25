import test from 'node:test'
import assert from 'node:assert/strict'

import { nextNavIndex, shouldResetActiveIndex } from './use-list-keyboard'

test('ArrowDown wraps at end', () => {
  assert.equal(nextNavIndex(0, 'ArrowDown', 3), 1)
  assert.equal(nextNavIndex(2, 'ArrowDown', 3), 0)
})

test('ArrowUp wraps at start', () => {
  assert.equal(nextNavIndex(0, 'ArrowUp', 3), 2)
  assert.equal(nextNavIndex(1, 'ArrowUp', 3), 0)
})

test('Home and End jump to ends', () => {
  assert.equal(nextNavIndex(2, 'Home', 5), 0)
  assert.equal(nextNavIndex(0, 'End', 5), 4)
})

test('returns null for empty list', () => {
  assert.equal(nextNavIndex(0, 'ArrowDown', 0), null)
  assert.equal(nextNavIndex(0, 'Home', 0), null)
})

test('returns null for unrelated keys', () => {
  assert.equal(nextNavIndex(0, 'a', 5), null)
  assert.equal(nextNavIndex(0, 'Enter', 5), null)
  assert.equal(nextNavIndex(0, 'Tab', 5), null)
})

test('shouldResetActiveIndex flags shrink-past-active', () => {
  // List shrinks below the active index — hook must reset.
  assert.equal(shouldResetActiveIndex(3, 2), true)
  assert.equal(shouldResetActiveIndex(1, 1), true)
})

test('shouldResetActiveIndex leaves valid index alone', () => {
  assert.equal(shouldResetActiveIndex(0, 5), false)
  assert.equal(shouldResetActiveIndex(4, 5), false)
})

test('shouldResetActiveIndex does not reset on empty list', () => {
  // Empty list is a transient state; preserve the index so the hook does not
  // thrash when the list briefly empties between provider switches.
  assert.equal(shouldResetActiveIndex(0, 0), false)
  assert.equal(shouldResetActiveIndex(3, 0), false)
})
