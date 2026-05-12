import type { Gateway, ProtectedMcpRoute } from './types/gateway'

export type GatewayAuthMode = 'none' | 'bearer' | 'oauth'

export function normalizeProtectedPublicPath(raw: string): string {
  const trimmed = raw.trim()
  if (!trimmed) return ''

  let withoutOrigin: string
  if (trimmed.startsWith('http://') || trimmed.startsWith('https://')) {
    try {
      withoutOrigin = new URL(trimmed).pathname
    } catch {
      throw new Error('Enter a valid URL or a path starting with /')
    }
  } else {
    withoutOrigin = trimmed
  }
  const withSlash = withoutOrigin.startsWith('/') ? withoutOrigin : `/${withoutOrigin}`
  const normalized = withSlash.replace(/\/+/g, '/').replace(/\/$/, '')

  if (!normalized || normalized === '/') {
    throw new Error('Use a non-root path such as tools')
  }
  if (!/^\/[A-Za-z0-9][A-Za-z0-9._/-]*$/.test(normalized)) {
    throw new Error('Use letters, numbers, dots, underscores, hyphens, and slashes')
  }
  if (normalized.split('/').some((segment) => segment === '..')) {
    throw new Error('Path cannot contain .. segments')
  }
  return normalized
}

export function protectedRouteForGateway(
  gateway: Gateway | null,
  routes: ProtectedMcpRoute[],
  publicHost: string,
): ProtectedMcpRoute | null {
  if (!gateway || gateway.source === 'in_process') return null

  const gatewayName = gateway.name
  const host = publicHost.toLowerCase()
  const matches = routes.filter((route) => {
    if (!route.enabled) return false
    if (route.public_host.toLowerCase() !== host) return false
    return route.upstream === gatewayName || route.name === gatewayName
  })

  return matches.find((route) => route.upstream === gatewayName) ?? matches[0] ?? null
}

export function protectedRoutePathInputValue(route: ProtectedMcpRoute | null): string {
  return route?.public_path.replace(/^\//, '') ?? ''
}

export function initialGatewayAuthMode(
  gateway: Gateway,
  protectedRoute: ProtectedMcpRoute | null,
): GatewayAuthMode {
  // bearer_token_env and oauth_enabled are explicit gateway-level auth config;
  // check them first so that a protected-route publication (which is orthogonal
  // to upstream auth) cannot silently clear bearer credentials on re-save.
  if (gateway.config.oauth_enabled) return 'oauth'
  if (gateway.config.bearer_token_env) return 'bearer'
  if (protectedRoute) return 'oauth'
  return 'none'
}
