import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { GatewayFilters } from './gateway-filters'

test('gateway filters render aurora checkbox groups and clear state affordance', () => {
  const markup = renderToStaticMarkup(
    <GatewayFilters
      mode="gateways"
      search="plex"
      gatewayFilters={{ status: ['configured'], source: ['lab'], transport: ['stdio'] }}
      toolFilters={{ search: '', gatewayIds: [], exposure: 'all', source: [], transport: [] }}
      gatewayOptions={[]}
      mobileSheetOpen={false}
      onMobileSheetOpenChange={() => {}}
      onSearchChange={() => {}}
      onGatewayFilterToggle={() => {}}
      onToolFilterToggle={() => {}}
      onExposureChange={() => {}}
      onClearFilters={() => {}}
    />,
  )

  assert.match(markup, /bg-aurora-panel-medium/)
  assert.match(markup, /bg-aurora-control-surface/)
  assert.match(markup, /data-mobile-search="gateways"/)
  assert.match(markup, /Search servers/)
  assert.match(markup, /Clear filters/i)
  assert.match(markup, /aria-label="Open filters"/)
  assert.match(markup, /type="checkbox"/)
  assert.match(markup, /Configured/)
  assert.doesNotMatch(markup, /role="combobox"/)
})

test('tools filters render exposure segmented control and gateway facets', () => {
  const markup = renderToStaticMarkup(
    <GatewayFilters
      mode="tools"
      search="uni"
      gatewayFilters={{ status: [], source: [], transport: [] }}
      toolFilters={{ search: '', gatewayIds: ['gw_1'], exposure: 'exposed', source: ['lab'], transport: ['stdio'] }}
      gatewayOptions={[{ value: 'gw_1', label: 'Lab Core' }]}
      mobileSheetOpen={true}
      onMobileSheetOpenChange={() => {}}
      onSearchChange={() => {}}
      onGatewayFilterToggle={() => {}}
      onToolFilterToggle={() => {}}
      onExposureChange={() => {}}
      onClearFilters={() => {}}
    />,
  )

  assert.match(markup, /Exposed only/)
  assert.match(markup, /Lab Core/)
  assert.match(markup, /data-mobile-search="tools"/)
  assert.match(markup, /aria-label="Open filters"/)
  assert.match(markup, /Search tools, descriptions, or servers/)
})
