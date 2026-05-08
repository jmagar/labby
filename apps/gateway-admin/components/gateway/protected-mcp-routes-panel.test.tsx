import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { SWRConfig } from 'swr'

import { ProtectedMcpRoutesPanel } from './protected-mcp-routes-panel'

test('protected MCP routes panel renders management controls', () => {
  const markup = renderToStaticMarkup(
    <SWRConfig value={{ provider: () => new Map() }}>
      <ProtectedMcpRoutesPanel />
    </SWRConfig>,
  )

  assert.match(markup, /Protected MCP routes/)
  assert.match(markup, /New route/)
  assert.match(markup, /Public host/)
  assert.match(markup, /Backend URL/)
  assert.match(markup, /Test/)
})
