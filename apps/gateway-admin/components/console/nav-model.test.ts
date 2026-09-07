import test from 'node:test'
import assert from 'node:assert/strict'

import { capabilityAwareNavSections, capabilityForPath, consoleNavItems, consoleNavSections } from './nav-model'

// bead lab-vl9q6
test('every nav item kbd accelerator matches its position in the flattened list', () => {
  // console-sidebar.tsx's ⌘/Ctrl+N handler jumps to
  // `consoleNavSections.flatMap(section => section.items)[N - 1]` — the
  // displayed accelerator must match that exact position or the hint lies
  // about what pressing it does. This previously drifted: Loadouts was
  // inserted without renumbering what followed it, so Tools/Loadouts both
  // showed ⌘3 and Usage/Traces both showed ⌘6, none matching the real
  // handler once Skills, Usage, and Traces were counted in.
  consoleNavItems.forEach((item, index) => {
    assert.equal(item.kbd, `⌘${index + 1}`, `${item.id} should show ⌘${index + 1}`)
    assert.ok(
      item.tooltip.includes(item.kbd),
      `${item.id} tooltip should reference its own accelerator`,
    )
  })
})

test('every nav item kbd accelerator is unique', () => {
  const seen = new Set<string>()
  for (const item of consoleNavItems) {
    assert.ok(!seen.has(item.kbd), `duplicate accelerator ${item.kbd} on ${item.id}`)
    seen.add(item.kbd)
  }
})

test('consoleNavItems is the flattened consoleNavSections in section order', () => {
  const flat = consoleNavSections.flatMap((section) => section.items)
  assert.deepEqual(
    consoleNavItems.map((item) => item.id),
    flat.map((item) => item.id),
  )
})
test('Depot and Workspace navigation match the unified product information architecture', () => {
  const depot = consoleNavSections.find((section) => section.id === 'Depot')
  const workspace = consoleNavSections.find((section) => section.id === 'Workspace')

  assert.deepEqual(depot?.items.map((item) => item.label), ['Discover', 'Create', 'Library', 'Administration'])
  assert.deepEqual(workspace?.items.map((item) => item.label), [
    'Agents',
    'Tasks',
    'Dev Containers',
  ])
  assert.equal(consoleNavItems.some((item) => item.href === '/stash'), false)
  assert.equal(consoleNavItems.some((item) => item.label === 'Loadouts'), false)
  assert.equal(consoleNavItems.some((item) => item.label === 'Snippets'), false)
})

test('browser bridge is a real control-plane destination', () => {
  const browsers = consoleNavItems.find((item) => item.id === 'Browsers')
  assert.ok(browsers)
  assert.equal(browsers.href, '/browsers')
})

test('one filtered model removes denied links and shortcut targets together', () => {
  const member = capabilityAwareNavSections(['scope.read', 'scope.operate'])
  const items = member.flatMap((section) => section.items)
  const ids = items.map((item) => item.id)
  assert.ok(ids.includes('Agents'))
  assert.ok(ids.includes('Library'))
  assert.ok(!ids.includes('Labby'))
  assert.ok(!ids.includes('Logs'))
  assert.ok(!ids.includes('Create'))
  items.forEach((item, index) => assert.equal(item.kbd, `⌘${index + 1}`))
})

test('direct route manifest fails closed for unknown and settings routes', () => {
  assert.equal(capabilityForPath('/settings/core'), 'platform.manage')
  assert.equal(capabilityForPath('/dev-containers'), 'scope.operate')
  assert.equal(capabilityForPath('/depot'), null)
  assert.equal(capabilityForPath('/gateway'), 'platform.manage')
  assert.equal(capabilityForPath('/not-a-product-route'), undefined)
})
