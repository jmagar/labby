const DEV_PREFIX = '/dev'

const READ_ONLY_ACTIONS = new Set([
  'help',
  'schema',
  'sources.list',
  'plugins.list',
  'plugin.get',
  'plugin.artifacts',
  'plugin.workspace',
  'plugin.components',
  'plugin.deploy.preview',
  'artifact.list',
  'agent.list',
  'agent.get',
  'mcp.config',
  'mcp.list',
  'mcp.get',
  'mcp.versions',
  'mcp.meta.get',
])

export class DevPreviewReadOnlyError extends Error {
  status = 403
  code = 'dev_preview_read_only'

  constructor(action: string) {
    super(`Dev preview is read-only. Action \`${action}\` was not sent.`)
    this.name = 'DevPreviewReadOnlyError'
  }
}

export function isDevPreviewRoute(): boolean {
  if (typeof window === 'undefined') return false
  return window.location.pathname === DEV_PREFIX || window.location.pathname.startsWith(`${DEV_PREFIX}/`)
}

export function isDevPreviewReadOnlyAction(action: string): boolean {
  return READ_ONLY_ACTIONS.has(action)
}

export function assertDevPreviewCanRunAction(action: string): void {
  if (isDevPreviewRoute() && !isDevPreviewReadOnlyAction(action)) {
    throw new DevPreviewReadOnlyError(action)
  }
}

export function devPreviewActionUrl(url: string): string {
  if (!isDevPreviewRoute()) return url
  if (url === '/v1/marketplace') return '/dev/api/marketplace'
  return url
}
