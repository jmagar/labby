import test from 'node:test'
import assert from 'node:assert/strict'

import type { Marketplace, Plugin } from '@/lib/types/marketplace'
import type { AcpAgent, McpServer } from '@/lib/marketplace/types'
import {
  buildMarketplaceCatalogItems,
  filterMarketplaceCatalogItems,
  marketplaceCatalogSummary,
  sortMarketplaceCatalogItems,
  type MarketplaceCatalogFilterState,
} from './marketplace-state'

const sources: Marketplace[] = [
  {
    id: 'official',
    name: 'Official',
    owner: 'Labby',
    description: 'Curated plugins',
    autoUpdateEnabled: true,
    pluginCount: 2,
    lastUpdatedAt: '2026-04-21T12:00:00Z',
    source: 'github',
    repository: 'labby/marketplace',
    githubOwner: 'labby',
  },
  {
    id: 'acp-registry',
    name: 'ACP Registry',
    owner: 'Agent Client Protocol',
    description: 'Official ACP agent registry mirrored into Marketplace.',
    autoUpdateEnabled: true,
    pluginCount: 1,
    lastUpdatedAt: '',
    source: 'git',
    remoteUrl: 'https://cdn.agentclientprotocol.com',
  },
]

const plugins: Plugin[] = [
  {
    id: 'gateway-audit',
    name: 'Gateway Audit',
    marketplaceId: 'official',
    version: '1.0.0',
    description: 'Audit gateway exposure',
    tags: ['gateway'],
    installed: true,
    hasUpdate: true,
    updatedAt: '2026-04-22T10:00:00Z',
    runtime: 'claude',
  },
  {
    id: 'codex-helper',
    name: 'Codex Helper',
    marketplaceId: 'official',
    version: '0.2.0',
    description: 'Codex workflow helpers',
    tags: ['codex'],
    installed: false,
    updatedAt: '2026-04-21T10:00:00Z',
    runtime: 'codex',
    components: [
      {
        kind: 'agents',
        path: 'agents/reviewer.md',
        name: 'code-reviewer',
        metadata: {
          name: 'code-reviewer',
          description: 'Reviews repository changes.',
          model: 'sonnet',
        },
      },
      {
        kind: 'skills',
        path: 'skills/tdd/SKILL.md',
        name: 'TDD Skill',
      },
      {
        kind: 'commands',
        path: 'commands/ship.md',
        name: 'Ship Command',
      },
      {
        kind: 'mcp_servers',
        path: '.mcp.json',
        name: 'Plugin MCP Servers',
      },
      {
        kind: 'lsp_servers',
        path: '.lsp.json',
        name: 'Plugin LSP Servers',
      },
      {
        kind: 'monitors',
        path: 'monitors/monitors.json',
        name: 'Background Monitors',
      },
      {
        kind: 'bin',
        path: 'bin/review',
        name: 'Review Helper',
      },
      {
        kind: 'settings',
        path: 'settings.json',
        name: 'Default Settings',
      },
      {
        kind: 'output_styles',
        path: 'output-styles/reviewer.md',
        name: 'Reviewer Style',
      },
      {
        kind: 'themes',
        path: 'themes/dim.json',
        name: 'Dim Theme',
      },
      {
        kind: 'channels',
        path: 'channels.json',
        name: 'Team Channel',
      },
    ],
  },
]

const mcpServers: McpServer[] = [
  {
    name: 'filesystem',
    description: 'Filesystem MCP server',
    version: '1.3.0',
    package: '@modelcontextprotocol/server-filesystem',
    transport: ['stdio'],
  },
]

const registryMcpServers = [
  {
    server: {
      name: 'github',
      description: 'GitHub MCP server',
      version: '0.4.1',
      packages: [{ registryType: 'npm', identifier: '@modelcontextprotocol/server-github' }],
    },
    _meta: {
      'io.modelcontextprotocol.registry/official': {
        updatedAt: '2026-04-24T10:00:00Z',
      },
    },
  },
  {
    server: {
      name: 'github',
      description: 'Older GitHub MCP server',
      version: '0.3.0',
      packages: [{ registryType: 'npm', identifier: '@modelcontextprotocol/server-github' }],
    },
    _meta: {
      'io.modelcontextprotocol.registry/official': {
        updatedAt: '2026-04-19T10:00:00Z',
        isLatest: false,
      },
    },
  },
  {
    server: {
      name: 'brave-search',
      description: 'Search MCP server',
      version: '0.2.0',
      packages: [{ registryType: 'npm', identifier: '@modelcontextprotocol/server-brave-search' }],
    },
    _meta: {
      'io.modelcontextprotocol.registry/official': {
        updatedAt: '2026-04-20T10:00:00Z',
      },
    },
  },
]

const acpAgents: AcpAgent[] = [
  {
    id: 'codex-cli',
    name: 'Codex CLI',
    version: '0.5.0',
    description: 'Agent implementing ACP',
    distribution: { binary: {} },
    installed: true,
    installedAt: '2026-04-23T10:00:00Z',
    repository: 'https://github.com/openai/codex',
  },
]

const baseFilters: MarketplaceCatalogFilterState = {
  lens: 'all',
  search: '',
  types: [],
  installStates: [],
  ecosystems: [],
  sourceIds: [],
  distributions: [],
  sort: 'updated',
}

test('buildMarketplaceCatalogItems creates unified plugin mcp agent and source rows', () => {
  const items = buildMarketplaceCatalogItems({ plugins, sources, mcpServers, acpAgents })

  assert.deepEqual(items.map((item) => item.kind), ['plugin', 'plugin', 'agent', 'skill', 'command', 'mcp_server', 'lsp_server', 'monitor', 'executable', 'settings', 'output_style', 'theme', 'channel', 'mcp_server', 'acp_agent', 'source', 'source'])
  assert.equal(items.find((item) => item.id === 'plugin:gateway-audit')?.ecosystem, 'Claude')
  assert.equal(items.find((item) => item.id === 'component:codex-helper:agents:agents/reviewer.md')?.kind, 'agent')
  assert.equal(items.find((item) => item.id === 'component:codex-helper:agents:agents/reviewer.md')?.name, 'code-reviewer')
  assert.equal(items.find((item) => item.id === 'component:codex-helper:agents:agents/reviewer.md')?.description, 'Reviews repository changes.')
  assert.equal(items.find((item) => item.id === 'component:codex-helper:skills:skills/tdd/SKILL.md')?.kind, 'skill')
  assert.equal(items.find((item) => item.id === 'component:codex-helper:commands:commands/ship.md')?.kind, 'command')
  assert.equal(items.find((item) => item.id === 'component:codex-helper:mcp_servers:.mcp.json')?.kind, 'mcp_server')
  assert.equal(items.find((item) => item.id === 'component:codex-helper:lsp_servers:.lsp.json')?.kind, 'lsp_server')
  assert.equal(items.find((item) => item.id === 'component:codex-helper:monitors:monitors/monitors.json')?.kind, 'monitor')
  assert.equal(items.find((item) => item.id === 'component:codex-helper:bin:bin/review')?.kind, 'executable')
  assert.equal(items.find((item) => item.id === 'component:codex-helper:settings:settings.json')?.kind, 'settings')
  assert.equal(items.find((item) => item.id === 'component:codex-helper:output_styles:output-styles/reviewer.md')?.kind, 'output_style')
  assert.equal(items.find((item) => item.id === 'component:codex-helper:themes:themes/dim.json')?.kind, 'theme')
  assert.equal(items.find((item) => item.id === 'component:codex-helper:channels:channels.json')?.kind, 'channel')
  assert.equal(items.find((item) => item.id === 'mcp:filesystem')?.distribution, 'npm')
  assert.equal(items.find((item) => item.id === 'agent:codex-cli')?.ecosystem, 'ACP')
  assert.equal(items.find((item) => item.id === 'agent:codex-cli')?.sourceId, 'acp-registry')
  assert.equal(items.find((item) => item.id === 'agent:codex-cli')?.sourceName, 'ACP Registry')
  assert.deepEqual(items.find((item) => item.id === 'plugin:gateway-audit')?.avatar, { kind: 'github', owner: 'labby' })
  assert.deepEqual(items.find((item) => item.id === 'component:codex-helper:agents:agents/reviewer.md')?.avatar, { kind: 'github', owner: 'labby' })
  assert.deepEqual(items.find((item) => item.id === 'agent:codex-cli')?.avatar, { kind: 'github', owner: 'openai' })
  assert.deepEqual(items.find((item) => item.id === 'source:official')?.avatar, { kind: 'github', owner: 'labby' })
})

test('buildMarketplaceCatalogItems accepts MCP Registry server envelopes', () => {
  const items = buildMarketplaceCatalogItems({
    plugins: [],
    sources: [],
    mcpServers: [
      {
        server: {
          name: 'io.github.acme/widgets',
          title: 'Acme Widgets',
          description: 'Widget MCP server',
          version: '2.1.0',
          packages: [{ registryType: 'npm', identifier: '@acme/widgets-mcp', transport: { type: 'stdio' } }],
        },
        _meta: {
          'io.modelcontextprotocol.registry/official': {
            updatedAt: '2026-04-24T10:00:00Z',
            isLatest: true,
            status: 'active',
          },
        },
      },
    ],
    acpAgents: [],
  })

  assert.equal(items[0]?.id, 'mcp:io.github.acme/widgets')
  assert.equal(items[0]?.name, 'Acme Widgets')
  assert.equal(items[0]?.subtitle, '@acme/widgets-mcp')
  assert.equal(items[0]?.distribution, 'npm')
  assert.equal(items[0]?.updatedAt, '2026-04-24T10:00:00Z')
  assert.deepEqual(items[0]?.avatar, { kind: 'github', owner: 'acme' })
})

test('buildMarketplaceCatalogItems derives GitHub avatars from repository urls when owner fields are absent', () => {
  const items = buildMarketplaceCatalogItems({
    plugins: [
      {
        id: 'repo-avatar',
        name: 'Repo Avatar',
        marketplaceId: 'repo-source',
        version: '1.0.0',
        description: 'Uses repo owner avatar',
        tags: [],
        installed: false,
      },
    ],
    sources: [
      {
        id: 'repo-source',
        name: 'Repo Source',
        owner: 'Fallback',
        description: 'Source without githubOwner',
        autoUpdateEnabled: true,
        pluginCount: 1,
        lastUpdatedAt: '2026-04-21T12:00:00Z',
        source: 'github',
        repository: 'octo-org/plugins',
      },
    ],
    mcpServers: [
      {
        server: {
          name: 'widgets',
          description: 'Widget server',
          version: '1.0.0',
          repository: { url: 'https://github.com/mcp-org/widgets' },
        },
      },
    ],
    acpAgents: [],
  })

  assert.deepEqual(items.find((item) => item.id === 'plugin:repo-avatar')?.avatar, { kind: 'github', owner: 'octo-org' })
  assert.deepEqual(items.find((item) => item.id === 'source:repo-source')?.avatar, { kind: 'github', owner: 'octo-org' })
  assert.deepEqual(items.find((item) => item.id === 'mcp:widgets')?.avatar, { kind: 'github', owner: 'mcp-org' })
})

test('buildMarketplaceCatalogItems dedupes MCP Registry versions by server identifier', () => {
  const items = buildMarketplaceCatalogItems({
    plugins: [],
    sources: [],
    mcpServers: registryMcpServers,
    acpAgents: [],
  })

  assert.deepEqual(items.map((item) => item.id), ['mcp:github', 'mcp:brave-search'])
  assert.equal(items.find((item) => item.id === 'mcp:github')?.version, '0.4.1')
  assert.equal(items.find((item) => item.id === 'mcp:github')?.updatedAt, '2026-04-24T10:00:00Z')
})

test('marketplaceCatalogSummary counts lenses from unified rows', () => {
  const items = buildMarketplaceCatalogItems({ plugins, sources, mcpServers, acpAgents })
  const summary = marketplaceCatalogSummary(items)

  assert.deepEqual(summary, {
    all: 17,
    installed: 2,
    plugins: 2,
    agents: 1,
    skills: 1,
    commands: 1,
    mcpServers: 2,
    lspServers: 1,
    acpAgents: 1,
    sources: 2,
    updates: 1,
  })
})

test('marketplaceCatalogSummary does not count bundled components as plugins', () => {
  const installedPluginWithComponents: Plugin = {
    id: 'ops-pack',
    name: 'Ops Pack',
    marketplaceId: 'official',
    version: '1.0.0',
    description: 'Installed operator component bundle',
    tags: [],
    installed: true,
    components: [
      { kind: 'agents', path: 'agents/ops.md', name: 'Ops Agent' },
      { kind: 'skills', path: 'skills/ops/SKILL.md', name: 'Ops Skill' },
      { kind: 'commands', path: 'commands/ops.md', name: 'Ops Command' },
    ],
  }
  const items = buildMarketplaceCatalogItems({
    plugins: [installedPluginWithComponents],
    sources,
    mcpServers: [],
    acpAgents: [],
  })
  const summary = marketplaceCatalogSummary(items)

  assert.equal(summary.plugins, 1)
  assert.equal(summary.agents, 1)
  assert.equal(summary.skills, 1)
  assert.equal(summary.commands, 1)
  assert.equal(summary.mcpServers, 0)
  assert.equal(summary.lspServers, 0)
  assert.deepEqual(
    items.filter((item) => item.installed).map((item) => item.kind),
    ['plugin', 'agent', 'skill', 'command'],
  )
})

test('filterMarketplaceCatalogItems combines lens search and facets', () => {
  const items = buildMarketplaceCatalogItems({ plugins, sources, mcpServers, acpAgents })
  const filtered = filterMarketplaceCatalogItems(items, {
    ...baseFilters,
    lens: 'plugins',
    search: 'codex',
    installStates: ['not_installed'],
    ecosystems: ['Codex'],
  })

  assert.deepEqual(filtered.map((item) => item.id), ['plugin:codex-helper'])
})

test('filterMarketplaceCatalogItems treats plugin-distributed components as their own kinds', () => {
  const items = buildMarketplaceCatalogItems({ plugins, sources, mcpServers, acpAgents })

  assert.deepEqual(
    filterMarketplaceCatalogItems(items, { ...baseFilters, lens: 'agents' }).map((item) => item.id),
    ['component:codex-helper:agents:agents/reviewer.md'],
  )
  assert.deepEqual(
    filterMarketplaceCatalogItems(items, { ...baseFilters, types: ['skill'] }).map((item) => item.id),
    ['component:codex-helper:skills:skills/tdd/SKILL.md'],
  )
  assert.deepEqual(
    filterMarketplaceCatalogItems(items, { ...baseFilters, search: 'ship' }).map((item) => item.id),
    ['component:codex-helper:commands:commands/ship.md'],
  )
})

test('filterMarketplaceCatalogItems treats sources as catalog items and facets', () => {
  const items = buildMarketplaceCatalogItems({ plugins, sources, mcpServers, acpAgents })
  const sourceLens = filterMarketplaceCatalogItems(items, { ...baseFilters, lens: 'sources' })
  const sourceFacet = filterMarketplaceCatalogItems(items, { ...baseFilters, sourceIds: ['official'] })

  assert.deepEqual(sourceLens.map((item) => item.id), ['source:official', 'source:acp-registry'])
  assert.deepEqual(sourceFacet.map((item) => item.id), [
    'plugin:gateway-audit',
    'plugin:codex-helper',
    'component:codex-helper:agents:agents/reviewer.md',
    'component:codex-helper:skills:skills/tdd/SKILL.md',
    'component:codex-helper:commands:commands/ship.md',
    'component:codex-helper:mcp_servers:.mcp.json',
    'component:codex-helper:lsp_servers:.lsp.json',
    'component:codex-helper:monitors:monitors/monitors.json',
    'component:codex-helper:bin:bin/review',
    'component:codex-helper:settings:settings.json',
    'component:codex-helper:output_styles:output-styles/reviewer.md',
    'component:codex-helper:themes:themes/dim.json',
    'component:codex-helper:channels:channels.json',
    'source:official',
  ])
})

test('filterMarketplaceCatalogItems lets source filters target MCP Registry rows', () => {
  const items = buildMarketplaceCatalogItems({
    plugins,
    sources,
    mcpServers: registryMcpServers,
    acpAgents,
  })

  assert.deepEqual(
    filterMarketplaceCatalogItems(items, { ...baseFilters, sourceIds: ['mcp-registry'] }).map((item) => item.id),
    ['mcp:github', 'mcp:brave-search'],
  )
  assert.deepEqual(
    filterMarketplaceCatalogItems(items, { ...baseFilters, sourceIds: ['official'] }).map((item) => item.id),
    [
      'plugin:gateway-audit',
      'plugin:codex-helper',
      'component:codex-helper:agents:agents/reviewer.md',
      'component:codex-helper:skills:skills/tdd/SKILL.md',
      'component:codex-helper:commands:commands/ship.md',
      'component:codex-helper:mcp_servers:.mcp.json',
      'component:codex-helper:lsp_servers:.lsp.json',
      'component:codex-helper:monitors:monitors/monitors.json',
      'component:codex-helper:bin:bin/review',
      'component:codex-helper:settings:settings.json',
      'component:codex-helper:output_styles:output-styles/reviewer.md',
      'component:codex-helper:themes:themes/dim.json',
      'component:codex-helper:channels:channels.json',
      'source:official',
    ],
  )
})

test('filterMarketplaceCatalogItems lets source filters target ACP Registry rows', () => {
  const items = buildMarketplaceCatalogItems({ plugins, sources, mcpServers, acpAgents })

  assert.deepEqual(
    filterMarketplaceCatalogItems(items, { ...baseFilters, sourceIds: ['acp-registry'] }).map((item) => item.id),
    ['agent:codex-cli', 'source:acp-registry'],
  )
})

test('sortMarketplaceCatalogItems supports install state and source sorting', () => {
  const items = buildMarketplaceCatalogItems({ plugins, sources, mcpServers, acpAgents })

  assert.deepEqual(
    sortMarketplaceCatalogItems(items, 'installed').map((item) => item.id).slice(0, 2),
    ['plugin:gateway-audit', 'agent:codex-cli'],
  )
  assert.deepEqual(
    sortMarketplaceCatalogItems(items, 'source').map((item) => item.id).slice(0, 2),
    ['source:acp-registry', 'agent:codex-cli'],
  )
})

test('sortMarketplaceCatalogItems ranks search matches before the selected sort order', () => {
  const items = buildMarketplaceCatalogItems({ plugins, sources, mcpServers, acpAgents })

  assert.deepEqual(
    sortMarketplaceCatalogItems(items, 'updated', 'codex').map((item) => item.id).slice(0, 2),
    ['plugin:codex-helper', 'agent:codex-cli'],
  )
})

test('sortMarketplaceCatalogItems puts recently updated catalog entries first', () => {
  const items = buildMarketplaceCatalogItems({
    plugins,
    sources,
    mcpServers: registryMcpServers,
    acpAgents,
  })

  assert.deepEqual(
    sortMarketplaceCatalogItems(items, 'updated').map((item) => item.id).slice(0, 3),
    ['mcp:github', 'agent:codex-cli', 'plugin:gateway-audit'],
  )
})
