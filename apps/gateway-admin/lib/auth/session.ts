'use client'

import { useSyncExternalStore } from 'react'

export {
  __setBrowserSessionStateForTests,
  AUTHORITY_WORKSPACE_CHANGED_EVENT,
  getBrowserSessionState,
  getSessionCsrfToken,
  getSessionAuthority,
  getSessionProjectId,
  loadBrowserSession,
  logoutBrowserSession,
  subscribeToBrowserSession,
  sessionHasCapability,
  selectSessionWorkspace,
  type AuthorityOwner,
  type BrowserSessionState,
  type SessionAuthority,
} from './session-store.ts'
export {
  AUTHORITY_COMPATIBILITY_GENERATION,
  AUTHORITY_SCHEMA_VERSION,
  MalformedAuthorityResponseError,
  authorityCacheKey,
  parseAuthoritySnapshot,
  selectAuthorityWorkspace,
  type AuthorityCacheKey,
  type AuthorityProject,
  type AuthoritySnapshot,
  type AuthorityTeam,
} from './authority.ts'
import { getBrowserSessionState, subscribeToBrowserSession } from './session-store.ts'

export function useBrowserSession() {
  return useSyncExternalStore(
    subscribeToBrowserSession,
    getBrowserSessionState,
    getBrowserSessionState,
  )
}
