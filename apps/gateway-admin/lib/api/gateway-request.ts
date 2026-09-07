import { beginAuthorityRequest } from '../auth/authority-context.ts'
import { getBrowserSessionEpoch, getSessionAuthority, getSessionCsrfToken, getSessionProjectId } from '../auth/session-store.ts'

export function gatewayHeaders(
  _token?: string,
  _standaloneBearerAuth = false,
): HeadersInit {
  void _token
  void _standaloneBearerAuth
  const headers: HeadersInit = {
    'Content-Type': 'application/json',
  }
  const csrfToken = getSessionCsrfToken()
  if (csrfToken) {
    headers['x-csrf-token'] = csrfToken
  }
  const projectId = getSessionProjectId()
  if (projectId) {
    headers['x-labby-project-id'] = projectId
  }
  const teamId = getSessionAuthority()?.activeTeamId
  if (teamId) headers['x-labby-team-id'] = teamId
  return headers
}

export function captureGatewayAuthority(signal?: AbortSignal, connectionId = 'local') {
  const authority = getSessionAuthority()
  if (!authority) throw new DOMException('Authority is unavailable', 'InvalidStateError')
  return beginAuthorityRequest(authority, getBrowserSessionEpoch(), connectionId, signal)
}

export function assertGatewayAuthorityCurrent(generation: number) {
  if (generation !== getBrowserSessionEpoch()) {
    throw new DOMException('Authority context changed', 'AbortError')
  }
}

export function confirmGatewayParams<T extends object>(params: T): T & { confirm: true } {
  return {
    ...params,
    confirm: true,
  }
}

export function gatewayRequestInit(
  action: string,
  params: object,
  _token?: string,
  signal?: AbortSignal,
  _standaloneBearerAuth = false,
): RequestInit {
  void _token
  void _standaloneBearerAuth
  return {
    method: 'POST',
    headers: gatewayHeaders(),
    body: JSON.stringify({ action, params }),
    cache: 'no-store',
    credentials: 'include',
    signal,
  }
}
