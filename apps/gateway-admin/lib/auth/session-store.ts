import { invalidateAuthorityRequests } from './authority-context.ts'
import { MalformedAuthorityResponseError, parseAuthoritySnapshot, selectAuthorityWorkspace, type AuthorityOwner, type AuthoritySnapshot } from './authority.ts'

export type SessionAuthority = AuthoritySnapshot
export type { AuthorityOwner, AuthoritySnapshot }

export type BrowserSessionState =
  | { status: 'loading' }
  | {
      status: 'authenticated'
      user: {
        sub: string
        email?: string | null
      }
      expiresAt: number
      csrfToken: string
      authority?: SessionAuthority
      /** Compatibility presentation flag derived only from server-projected capabilities. */
      isAdmin?: boolean
      projectId?: string
    }
  | { status: 'unauthenticated' }
  | {
      status: 'auth_error'
      kind?: string
      message: string
      requestId?: string
    }

type SessionPayload =
  | {
      authenticated: true
      user: {
        sub: string
        email?: string | null
      }
      expires_at: number
      csrf_token: string
      project_id?: string | null
      principal_id?: string | null
      active_owner?: { kind?: string; id?: string } | null
      active_team_id?: string | null
      active_project_id?: string | null
      capabilities?: unknown
      authority_generation?: number | null
      owner?: unknown
      organization_id?: unknown
      teams?: unknown
      projects?: unknown
      project?: unknown
    }
  | {
      authenticated: false
    }

type SessionErrorPayload = {
  kind?: string
  message?: string
}

let currentState: BrowserSessionState = { status: 'loading' }
export const AUTHORITY_WORKSPACE_CHANGED_EVENT = 'labby:authority-workspace-changed'
let sessionGeneration = 0
const listeners = new Set<() => void>()

function emit() {
  for (const listener of listeners) {
    listener()
  }
}

function setState(next: BrowserSessionState) {
  const previousIdentity = sessionIdentity(currentState)
  const nextIdentity = sessionIdentity(next)
  if (previousIdentity !== nextIdentity) {
    sessionGeneration += 1
    invalidateAuthorityRequests(sessionGeneration)
  }
  currentState = next
  emit()
}

function sessionIdentity(state: BrowserSessionState) {
  if (state.status !== 'authenticated') return state.status
  const authority = state.authority
  const authorityIdentity = authority
    ? [
        authority.principalId,
        authority.activeOwner.kind,
        authority.activeOwner.id,
        authority.activeTeamId ?? '',
        authority.activeProjectId ?? '',
        [...authority.capabilities].sort().join(','),
        authority.generation,
      ].join(':')
    : 'authority-unavailable'
  return `authenticated:${state.user.sub}:${authorityIdentity}:${state.csrfToken}:${state.expiresAt}`
}

function normalizeAuthority(payload: Extract<SessionPayload, { authenticated: true }>): SessionAuthority | undefined {
  const hasProjection = payload.authority_generation !== undefined || payload.organization_id !== undefined || payload.owner !== undefined || payload.active_owner !== undefined
  return hasProjection ? parseAuthoritySnapshot(payload as unknown as Record<string, unknown>) : undefined
}

function normalizePayload(payload: SessionPayload): BrowserSessionState {
  if (!payload.authenticated) {
    return { status: 'unauthenticated' }
  }
  const authority = normalizeAuthority(payload)
  return {
    status: 'authenticated',
    user: payload.user,
    expiresAt: payload.expires_at,
    csrfToken: payload.csrf_token,
    authority,
    isAdmin: authority?.capabilities.includes('platform.manage') ?? false,
    projectId: authority?.activeProjectId,
  }
}

export function subscribeToBrowserSession(listener: () => void) {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

export function getBrowserSessionState() {
  return currentState
}

export function getSessionCsrfToken() {
  return currentState.status === 'authenticated' ? currentState.csrfToken : undefined
}

export function getSessionProjectId() {
  return currentState.status === 'authenticated' ? currentState.projectId : undefined
}

export function getSessionAuthority() {
  return currentState.status === 'authenticated' ? currentState.authority : undefined
}

export function sessionHasCapability(capability: string) {
  return getSessionAuthority()?.capabilities.includes(capability) ?? false
}

export function selectSessionWorkspace(selection: { teamId?: string | null; projectId?: string | null }) {
  if (currentState.status !== 'authenticated' || !currentState.authority) throw new Error('Authority is unavailable')
  const authority = selectAuthorityWorkspace(currentState.authority, selection)
  setState({ ...currentState, authority, projectId: authority.activeProjectId })
  if (typeof window !== 'undefined') window.dispatchEvent(new CustomEvent(AUTHORITY_WORKSPACE_CHANGED_EVENT))
  return authority
}

/** Authority-adjacent cache generation. Never expose the subject in cache keys. */
export function getBrowserSessionEpoch() {
  return sessionGeneration
}

export async function loadBrowserSession() {
  const generationAtStart = sessionGeneration
  let next: BrowserSessionState

  try {
    const response = await fetch('/auth/session', {
      cache: 'no-store',
      credentials: 'include',
    })

    if (response.ok) {
      const payload = (await response.json()) as SessionPayload
      next = normalizePayload(payload)
    } else if (response.status === 401 || response.status === 403) {
      next = { status: 'unauthenticated' }
    } else {
      const payload = (await response.json().catch(() => null)) as SessionErrorPayload | null
      next = {
        status: 'auth_error',
        kind: payload?.kind,
        message: payload?.message || SESSION_ERROR_MESSAGE,
        requestId: response.headers.get('x-request-id') ?? undefined,
      }
    }
  } catch (error) {
    if (error instanceof MalformedAuthorityResponseError) {
      next = { status: 'auth_error', kind: 'incompatible_authority', message: error.message }
    } else {
    next = {
      status: 'auth_error',
      kind: 'network_error',
      message: SESSION_ERROR_MESSAGE,
    }
    }
  }

  if (generationAtStart !== sessionGeneration) {
    return currentState
  }

  setState(next)
  return next
}

export async function logoutBrowserSession() {
  const csrfToken = getSessionCsrfToken()
  const response = await fetch('/auth/logout', {
    method: 'POST',
    cache: 'no-store',
    credentials: 'include',
    headers: csrfToken
      ? {
          'x-csrf-token': csrfToken,
        }
      : undefined,
  })

  if (!response.ok) {
    throw new Error('Failed to logout browser session')
  }

  sessionGeneration += 1
  setState({ status: 'unauthenticated' })
}

export function __setBrowserSessionStateForTests(state: BrowserSessionState) {
  sessionGeneration += 1
  currentState = state
}
const SESSION_ERROR_MESSAGE = 'Unable to reach the authentication service. Try again.'
