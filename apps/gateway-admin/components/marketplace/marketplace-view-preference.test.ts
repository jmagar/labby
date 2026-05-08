import test from 'node:test'
import assert from 'node:assert/strict'

import {
  isMarketplaceViewPreference,
  resolveMarketplaceViewMode,
} from './marketplace-view-preference'

test('isMarketplaceViewPreference accepts persisted view modes and rejects invalid values', () => {
  assert.equal(isMarketplaceViewPreference('auto'), true)
  assert.equal(isMarketplaceViewPreference('cards'), true)
  assert.equal(isMarketplaceViewPreference('table'), true)
  assert.equal(isMarketplaceViewPreference('grid'), false)
  assert.equal(isMarketplaceViewPreference(null), false)
})

test('resolveMarketplaceViewMode defaults auto by screen size and honors forced modes', () => {
  assert.equal(resolveMarketplaceViewMode('auto', true), 'cards')
  assert.equal(resolveMarketplaceViewMode('auto', false), 'table')
  assert.equal(resolveMarketplaceViewMode('cards', false), 'cards')
  assert.equal(resolveMarketplaceViewMode('table', true), 'table')
})
