import test from 'node:test'
import assert from 'node:assert/strict'

import {
  buildGatewayCreatePayload,
  buildGatewayPatch,
  buildGatewayUpdatePayload,
  exposurePolicyFromConfig,
  gatewayInputToSpec,
  normalizeGateway,
  normalizeServerView,
  previewExposurePolicy,
  probeStatusFromRuntime,
} from './gateway-adapter.ts'
import { EXPOSE_NONE_PATTERN } from '../api/tool-exposure-draft.ts'

test('normalizeGateway maps backend views into UI gateway shape', () => {
  const gateway = normalizeGateway(
    {
      config: {
        name: 'fixture-http',
        url: 'http://127.0.0.1:9001/mcp',
        bearer_token_env: 'FIXTURE_TOKEN',
        proxy_resources: true,
      },
      runtime: {
        name: 'fixture-http',
        tool_count: 3,
        resource_count: 1,
        prompt_count: 1,
      },
    },
    {
      connected: true,
      healthy: true,
    },
    {
      tools: ['alpha.read', 'beta.write', 'gamma.run'],
      resources: ['resource://one'],
      prompts: ['prompt-one'],
    }
  )

  assert.equal(gateway.id, 'fixture-http')
  assert.equal(gateway.transport, 'http')
  assert.equal(gateway.status.connected, true)
  assert.equal(gateway.status.healthy, true)
  assert.equal(gateway.source, 'custom_gateway')
  assert.equal(gateway.surfaces?.mcp.enabled, true)
  assert.equal(gateway.status.discovered_tool_count, 3)
  assert.equal(gateway.status.exposed_tool_count, 3)
  assert.deepEqual(
    gateway.discovery.tools.map((tool) => ({ name: tool.name, exposed: tool.exposed, matched_by: tool.matched_by })),
    [
      { name: 'alpha.read', exposed: true, matched_by: '*' },
      { name: 'beta.write', exposed: true, matched_by: '*' },
      { name: 'gamma.run', exposed: true, matched_by: '*' },
    ]
  )
  assert.deepEqual(gateway.discovery.resources, [
    {
      name: 'resource://one',
      uri: 'resource://one',
      exposed: true,
    },
  ])
  assert.deepEqual(gateway.discovery.prompts, [{ name: 'prompt-one', exposed: true }])
})

test('buildGatewayCreatePayload generates an auth env var when a bearer token is pasted', () => {
  const payload = buildGatewayCreatePayload({
    name: 'github',
    transport: 'http',
    config: {
      url: 'https://api.githubcopilot.com/mcp/',
      bearer_token_value: 'ghp_secret',
    },
  })

  assert.deepEqual(payload, {
    spec: {
      name: 'github',
      url: 'https://api.githubcopilot.com/mcp/',
      command: null,
      args: [],
      bearer_token_env: 'LAB_GW_GITHUB_AUTH_HEADER',
      proxy_resources: false,
      proxy_prompts: true,
      expose_tools: null,
      expose_resources: null,
      expose_prompts: null,
    },
    bearer_token_value: 'ghp_secret',
  })
})

test('buildGatewayCreatePayload builds a stdio spec without any ack flag', () => {
  const payload = buildGatewayCreatePayload({
    name: 'fixture-stdio',
    transport: 'stdio',
    config: {
      command: 'example-mcp-server',
      args: ['--stdio'],
    },
  })

  assert.equal('allow_stdio' in payload, false)
  assert.deepEqual(payload.spec, {
    name: 'fixture-stdio',
    url: null,
    command: 'example-mcp-server',
    args: ['--stdio'],
    bearer_token_env: null,
    proxy_resources: false,
    proxy_prompts: true,
    expose_tools: null,
    expose_resources: null,
    expose_prompts: null,
  })
})

test('buildGatewayCreatePayload includes stdio environment variables', () => {
  const payload = buildGatewayCreatePayload({
    name: 'searxng',
    transport: 'stdio',
    config: {
      command: 'npx',
      args: ['-y', 'mcp-searxng'],
      env: { SEARXNG_URL: 'https://s.tootie.tv' },
    },
  })

  assert.deepEqual(payload.spec, {
    name: 'searxng',
    url: null,
    command: 'npx',
    args: ['-y', 'mcp-searxng'],
    env: { SEARXNG_URL: 'https://s.tootie.tv' },
    bearer_token_env: null,
    proxy_resources: false,
    proxy_prompts: true,
    expose_tools: null,
    expose_resources: null,
    expose_prompts: null,
  })
})

test('buildGatewayUpdatePayload clears auth when bearer_token_env is blanked', () => {
  const payload = buildGatewayUpdatePayload('github', {
    transport: 'http',
    config: {
      bearer_token_env: '',
    },
  })

  assert.deepEqual(payload, {
    name: 'github',
    patch: {
      url: null,
      command: null,
      args: [],
      bearer_token_env: null,
    },
  })
})

test('buildGatewayUpdatePayload builds a stdio patch without any ack flag', () => {
  const payload = buildGatewayUpdatePayload('fixture-stdio', {
    transport: 'stdio',
    config: {
      command: 'example-mcp-server',
      args: ['--stdio'],
    },
  })

  assert.deepEqual(payload, {
    name: 'fixture-stdio',
    patch: {
      url: null,
      command: 'example-mcp-server',
      args: ['--stdio'],
    },
  })
})

test('normalizeGateway applies allowlist exposure patterns', () => {
  const gateway = normalizeGateway(
    {
      config: {
        name: 'fixture-stdio',
        command: 'npx',
        args: ['-y', 'server'],
        proxy_resources: false,
        expose_tools: ['alpha.*', 'gamma.run'],
      },
      runtime: {
        name: 'fixture-stdio',
        tool_count: 3,
        resource_count: 0,
        prompt_count: 0,
      },
    },
    {
      connected: false,
      healthy: false,
      last_error: 'connection refused',
    },
    {
      tools: ['alpha.read', 'beta.write', 'gamma.run'],
      resources: [],
      prompts: [],
    }
  )

  assert.equal(gateway.transport, 'stdio')
  assert.equal(gateway.status.connected, false)
  assert.equal(gateway.status.last_error, 'connection refused')
  assert.equal(gateway.status.exposed_tool_count, 2)
  assert.deepEqual(
    gateway.discovery.tools.map((tool) => ({ name: tool.name, exposed: tool.exposed, matched_by: tool.matched_by })),
    [
      { name: 'alpha.read', exposed: true, matched_by: 'alpha.*' },
      { name: 'beta.write', exposed: false, matched_by: null },
      { name: 'gamma.run', exposed: true, matched_by: 'gamma.run' },
    ]
  )
})

test('normalizeGateway preserves discovered tool descriptions when provided', () => {
  const gateway = normalizeGateway(
    {
      config: {
        name: 'noxa',
        command: 'noxa',
        args: ['mcp'],
        proxy_resources: true,
      },
      runtime: {
        name: 'noxa',
        tool_count: 2,
        resource_count: 0,
        prompt_count: 0,
      },
    },
    {
      connected: true,
      healthy: true,
    },
    {
      tools: [
        { name: 'scrape', description: 'Fetch a single page as markdown', exposed: true, matched_by: '*' },
        { name: 'crawl', description: 'Recursively crawl a site', exposed: true, matched_by: '*' },
      ],
      resources: [],
      prompts: [],
    }
  )

  assert.deepEqual(
    gateway.discovery.tools.map((tool) => ({
      name: tool.name,
      description: tool.description,
    })),
    [
      { name: 'scrape', description: 'Fetch a single page as markdown' },
      { name: 'crawl', description: 'Recursively crawl a site' },
    ]
  )
})

test('normalizeServerView maps unified server rows into list-friendly gateway cards', () => {
  const gateway = normalizeServerView({
    id: 'plex',
    name: 'plex',
    source: 'in_process',
    configured: true,
    enabled: false,
    connected: false,
    warnings: [],
    config_summary: {
      transport: 'in_process',
      target: 'plex',
    },
  })

  assert.equal(gateway.id, 'plex')
  assert.equal(gateway.transport, 'in_process')
  assert.equal(gateway.source, 'in_process')
  assert.equal(gateway.enabled, false)
  assert.equal(gateway.surfaces?.mcp.enabled, false)
  assert.equal(gateway.status.connected, false)
  assert.equal(gateway.config.url, undefined)
  assert.equal(gateway.config.command, undefined)
})

test('normalizeServerView can include compiled lab-service tools', () => {
  const gateway = normalizeServerView(
    {
      id: 'plex',
      name: 'plex',
      source: 'in_process',
      configured: true,
      enabled: true,
      connected: true,
      warnings: [],
      config_summary: {
        transport: 'in_process',
        target: 'plex',
      },
    },
    {
      tools: [
        { name: 'server.info', description: 'Show server metadata', destructive: false },
        { name: 'library.list', description: 'List available libraries', destructive: false },
      ],
    }
  )

  assert.equal(gateway.status.discovered_tool_count, 2)
  assert.equal(gateway.status.exposed_tool_count, 2)
  assert.deepEqual(
    gateway.discovery.tools.map((tool) => ({
      name: tool.name,
      description: tool.description,
      exposed: tool.exposed,
      matched_by: tool.matched_by,
    })),
    [
      {
        name: 'server.info',
        description: 'Show server metadata',
        exposed: true,
        matched_by: '*',
      },
      {
        name: 'library.list',
        description: 'List available libraries',
        exposed: true,
        matched_by: '*',
      },
    ]
  )
})

test('normalizeServerView applies virtual-server MCP policy to lab-service tools', () => {
  const gateway = normalizeServerView(
    {
      id: 'github-chat',
      name: 'github-chat',
      source: 'in_process',
      configured: true,
      enabled: true,
      connected: true,
      discovered_tool_count: 2,
      exposed_tool_count: 1,
      surfaces: {
        cli: { enabled: false, connected: false },
        api: { enabled: false, connected: false },
        mcp: { enabled: true, connected: true },
        webui: { enabled: false, connected: false },
      },
      warnings: [],
      config_summary: {
        transport: 'in_process',
        target: 'github-chat',
      },
    },
    {
      tools: [
        { name: 'index_repository', description: 'Index a GitHub repository', destructive: false },
        { name: 'query_repository', description: 'Query a GitHub repository', destructive: false },
      ],
      allowed_actions: ['query_repository'],
    }
  )

  assert.equal(gateway.status.discovered_tool_count, 2)
  assert.equal(gateway.status.exposed_tool_count, 1)
  assert.deepEqual(
    gateway.discovery.tools.map((tool) => ({
      name: tool.name,
      exposed: tool.exposed,
      matched_by: tool.matched_by,
    })),
    [
      {
        name: 'index_repository',
        exposed: false,
        matched_by: null,
      },
      {
        name: 'query_repository',
        exposed: true,
        matched_by: 'query_repository',
      },
    ]
  )
})

test('normalizeServerView hides lab-service tools when MCP surface is disabled', () => {
  const gateway = normalizeServerView(
    {
      id: 'github-chat',
      name: 'github-chat',
      source: 'in_process',
      configured: true,
      enabled: true,
      connected: false,
      discovered_tool_count: 2,
      exposed_tool_count: 0,
      surfaces: {
        cli: { enabled: false, connected: false },
        api: { enabled: false, connected: false },
        mcp: { enabled: false, connected: false },
        webui: { enabled: false, connected: false },
      },
      warnings: [],
      config_summary: {
        transport: 'in_process',
        target: 'github-chat',
      },
    },
    {
      tools: [
        { name: 'index_repository', description: 'Index a GitHub repository', destructive: false },
        { name: 'query_repository', description: 'Query a GitHub repository', destructive: false },
      ],
      allowed_actions: [],
    }
  )

  assert.deepEqual(
    gateway.discovery.tools.map((tool) => tool.exposed),
    [false, false]
  )
})

test('normalizeServerView can include discovered custom-gateway tools for summary counts', () => {
  const gateway = normalizeServerView(
    {
      id: 'noxa',
      name: 'noxa',
      source: 'custom_gateway',
      configured: true,
      enabled: true,
      connected: true,
      discovered_tool_count: 5,
      exposed_tool_count: 3,
      discovered_resource_count: 4,
      exposed_resource_count: 2,
      discovered_prompt_count: 6,
      exposed_prompt_count: 6,
      warnings: [],
      config_summary: {
        transport: 'stdio',
        target: 'noxa',
      },
    },
    {
      tools: [
        { name: 'scrape', description: 'Fetch a single page as markdown', destructive: false },
        { name: 'crawl', description: 'Recursively crawl a site', destructive: false },
        { name: 'search', description: 'Search the web', destructive: false },
      ],
    }
  )

  assert.equal(gateway.status.discovered_tool_count, 5)
  assert.equal(gateway.status.exposed_tool_count, 3)
  assert.equal(gateway.status.discovered_resource_count, 4)
  assert.equal(gateway.status.exposed_resource_count, 2)
  assert.equal(gateway.status.discovered_prompt_count, 6)
  assert.equal(gateway.status.exposed_prompt_count, 6)
  assert.deepEqual(
    gateway.discovery.tools.map((tool) => ({
      name: tool.name,
      description: tool.description,
    })),
    [
      { name: 'scrape', description: 'Fetch a single page as markdown' },
      { name: 'crawl', description: 'Recursively crawl a site' },
      { name: 'search', description: 'Search the web' },
    ]
  )
})

test('gatewayInputToSpec converts UI input into backend spec payload', () => {
  const spec = gatewayInputToSpec({
    name: 'fixture-http',
    transport: 'http',
    config: {
      url: 'http://127.0.0.1:9001/mcp',
      bearer_token_env: 'FIXTURE_TOKEN',
      proxy_resources: true,
      expose_tools: ['alpha.*'],
    },
  })

  assert.deepEqual(spec, {
    name: 'fixture-http',
    url: 'http://127.0.0.1:9001/mcp',
    command: null,
    args: [],
    bearer_token_env: 'FIXTURE_TOKEN',
    proxy_resources: true,
    proxy_prompts: true,
    expose_tools: ['alpha.*'],
    expose_resources: null,
    expose_prompts: null,
  })
})

test('buildGatewayPatch clears the opposite transport fields when switching to stdio', () => {
  const patch = buildGatewayPatch({
    name: 'fixture-stdio',
    transport: 'stdio',
    config: {
      command: 'npx',
      args: ['-y', 'server'],
      bearer_token_env: '',
      proxy_resources: false,
    },
  })

  assert.deepEqual(patch, {
    name: 'fixture-stdio',
    url: null,
    command: 'npx',
    args: ['-y', 'server'],
    bearer_token_env: null,
    proxy_resources: false,
  })
})

test('buildGatewayPatch preserves resource and prompt exposure edits', () => {
  const patch = buildGatewayPatch({
    config: {
      expose_resources: ['resource://one'],
      expose_prompts: ['prompt-one'],
    },
  })

  assert.deepEqual(patch, {
    expose_resources: ['resource://one'],
    expose_prompts: ['prompt-one'],
  })
})

test('previewExposurePolicy reports matches filtered tools and unmatched patterns', () => {
  const preview = previewExposurePolicy(
    ['alpha.read', 'beta.write', 'gamma.run'],
    ['alpha.*', 'gamma.run', 'missing.*']
  )

  assert.deepEqual(preview, {
    matched_tools: [
      { name: 'alpha.read', matched_by: 'alpha.*' },
      { name: 'gamma.run', matched_by: 'gamma.run' },
    ],
    unmatched_patterns: ['missing.*'],
    filtered_tools: ['beta.write'],
    exposed_count: 2,
    filtered_count: 1,
  })
})

test('previewExposurePolicy treats an empty allowlist as expose-all', () => {
  const preview = previewExposurePolicy(
    ['alpha.read', 'beta.write'],
    []
  )

  assert.deepEqual(preview, {
    matched_tools: [
      { name: 'alpha.read', matched_by: '*' },
      { name: 'beta.write', matched_by: '*' },
    ],
    unmatched_patterns: [],
    filtered_tools: [],
    exposed_count: 2,
    filtered_count: 0,
  })
})

test('previewExposurePolicy supports leading wildcard patterns', () => {
  const preview = previewExposurePolicy(
    ['github.search_repos', 'gitlab.search_projects'],
    ['*search_*']
  )

  assert.deepEqual(preview, {
    matched_tools: [
      { name: 'github.search_repos', matched_by: '*search_*' },
      { name: 'gitlab.search_projects', matched_by: '*search_*' },
    ],
    unmatched_patterns: [],
    filtered_tools: [],
    exposed_count: 2,
    filtered_count: 0,
  })
})

test('exposurePolicyFromConfig preserves expose none sentinel as an empty allowlist', () => {
  assert.deepEqual(
    exposurePolicyFromConfig({
      name: 'fixture-http',
      expose_tools: [EXPOSE_NONE_PATTERN],
    }),
    {
      mode: 'allowlist',
      patterns: [],
    },
  )
})

test('probeStatusFromRuntime marks zero-capability gateways unhealthy', () => {
  assert.deepEqual(
    probeStatusFromRuntime({
      name: 'swag',
      tool_count: 0,
      resource_count: 0,
      prompt_count: 0,
      last_error: 'stdio handshake failed: unexpected HTTP response on stdout',
    }),
    {
      connected: false,
      healthy: false,
      last_error: 'stdio handshake failed: unexpected HTTP response on stdout',
    }
  )

  assert.deepEqual(
    probeStatusFromRuntime({
      name: 'fixture-stdio',
      tool_count: 3,
      resource_count: 0,
      prompt_count: 0,
    }),
    {
      connected: true,
      healthy: true,
    }
  )
})

test('probeStatusFromRuntime treats resource and prompt only gateways as connected', () => {
  assert.deepEqual(
    probeStatusFromRuntime({
      name: 'fixture-resources',
      tool_count: 0,
      resource_count: 2,
      prompt_count: 1,
    }),
    {
      connected: true,
      healthy: true,
    }
  )
})

test('normalizeGateway turns specific probe failures into actionable warnings', () => {
  const gateway = normalizeGateway(
    {
      config: {
        name: 'syslog-http',
        url: 'http://127.0.0.1:3100/mcp',
        proxy_resources: true,
      },
      runtime: {
        name: 'syslog-http',
        tool_count: 0,
        resource_count: 0,
        prompt_count: 0,
      },
    },
    {
      connected: false,
      healthy: false,
      last_error: 'Connection refused while probing http://127.0.0.1:3100/mcp',
    },
    {
      tools: [],
      resources: [],
      prompts: [],
    }
  )

  assert.equal(gateway.status.last_error, 'Connection refused while probing http://127.0.0.1:3100/mcp')
  assert.deepEqual(gateway.warnings, [
    {
      code: 'connection_error',
      message: 'Connection refused while probing http://127.0.0.1:3100/mcp',
      timestamp: gateway.warnings[0]?.timestamp,
    },
  ])
})

test('normalizeGateway humanizes auth failures for operator-facing UI', () => {
  const gateway = normalizeGateway(
    {
      config: {
        name: 'swag',
        url: 'https://swag.tootie.tv/mcp',
        proxy_resources: true,
      },
      runtime: {
        name: 'swag',
        tool_count: 0,
        resource_count: 0,
        prompt_count: 0,
      },
    },
    {
      connected: false,
      healthy: false,
      last_error:
        'Send message error Transport [rmcp::transport::worker::WorkerTransport<rmcp::transport::streamable_http_client::StreamableHttpClientWorker<reqwest::async_impl::client::Client>>] error: Auth required, when send initialize request',
    },
    {
      tools: [],
      resources: [],
      prompts: [],
    }
  )

  assert.equal(
    gateway.status.last_error,
    'Authentication is required by https://swag.tootie.tv/mcp. Configure `bearer_token_env` with a valid upstream token, then reload this gateway.'
  )
  assert.equal(gateway.warnings[0]?.code, 'auth_required')
})

test('normalizeGateway ignores resource discovery method-not-found for health and warnings', () => {
  const gateway = normalizeGateway(
    {
      config: {
        name: 'chrome-dev-tools',
        command: 'npx',
        args: ['chrome-devtools-mcp'],
        proxy_resources: true,
      },
      runtime: {
        name: 'chrome-dev-tools',
        tool_count: 29,
        resource_count: 0,
        prompt_count: 0,
      },
    },
    {
      connected: true,
      healthy: true,
      last_error: 'failed to list resources from upstream: Mcp error: -32601: Method not found',
    },
    {
      tools: [],
      resources: [],
      prompts: [],
    }
  )

  assert.equal(
    gateway.status.last_error,
    undefined
  )
  assert.equal(gateway.status.connected, true)
  assert.equal(gateway.status.healthy, true)
  assert.deepEqual(gateway.warnings, [])
})

test('normalizeServerView ignores custom gateway resource discovery method-not-found warnings', () => {
  const gateway = normalizeServerView({
    id: 'claude-in-mobile',
    name: 'claude-in-mobile',
    source: 'custom_gateway',
    configured: true,
    enabled: true,
    connected: true,
    discovered_tool_count: 81,
    exposed_tool_count: 81,
    discovered_resource_count: 0,
    exposed_resource_count: 0,
    discovered_prompt_count: 0,
    exposed_prompt_count: 0,
    warnings: [
      {
        code: 'connection_error',
        message: 'failed to list resources from upstream: Mcp error: -32601: Method not found',
      },
    ],
    config_summary: {
      transport: 'stdio',
      target: 'npx',
    },
  })

  assert.equal(gateway.status.connected, true)
  assert.equal(gateway.status.healthy, true)
  assert.equal(gateway.status.last_error, undefined)
  assert.deepEqual(gateway.warnings, [])
})
