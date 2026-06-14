import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { buildFileTree, PluginFilesPanel } from './plugin-files-panel'
import type { Artifact } from '@/lib/types/marketplace'

const artifacts: Artifact[] = [
  {
    path: 'plugin.json',
    content: '{"name":"demo-plugin"}',
    lang: 'json',
  },
  {
    path: '.claude-plugin/plugin.json',
    content: '{"name":"demo-plugin"}',
    lang: 'json',
  },
]

test('renders marketplace file tree and shared editor actions', () => {
  const markup = renderToStaticMarkup(
    <PluginFilesPanel pluginId="demo-plugin@demo-market" artifacts={artifacts} />,
  )

  assert.match(markup, /Files/)
  assert.match(markup, /plugin\.json/)
  assert.match(markup, /Save/)
  assert.match(markup, /Deploy/)
  assert.match(markup, /Fork to Stash/)
  assert.match(markup, /aurora-text-surface/)
})

test('builds a nested tree and bubbles dirty state to parent directories', () => {
  const tree = buildFileTree([
    {
      path: 'agents/reviewer/agent.md',
      content: '---\nname: reviewer\n---',
      savedContent: '---\nname: reviewer\n---',
      lang: 'markdown',
      dirty: true,
    },
    {
      path: 'plugin.json',
      content: '{"name":"demo-plugin"}',
      savedContent: '{"name":"demo-plugin"}',
      lang: 'json',
      dirty: false,
    },
  ])

  assert.equal(tree[0]?.kind, 'dir')
  assert.equal(tree[0]?.path, 'agents')
  assert.equal(tree[0]?.dirty, true)
  assert.equal(tree[0]?.children?.[0]?.path, 'agents/reviewer')
  assert.equal(tree[1]?.path, 'plugin.json')
})
