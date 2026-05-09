export interface UpstreamEntry {
  name: string
}

export interface UpstreamOauthStatus {
  authenticated: boolean
  upstream: string
  expires_within_5m: boolean
  state?: 'connected' | 'expiring' | 'expired' | 'refresh_failed' | 'discovery_failed' | 'disconnected'
  access_token_expires_at?: number
  seconds_until_expiry?: number
  refresh_token_present?: boolean
  refresh_attempted?: boolean
  refreshed?: boolean
  refresh_error_kind?: string
  refresh_error?: string
  discovery_checked?: boolean
  discovered_tool_count?: number
  exposed_tool_count?: number
  discovery_error?: string
}

export interface StartResponse {
  authorization_url: string
}

export interface ProbeResponse {
  upstream: string
  url: string
  oauth_discovered: boolean
  issuer?: string
  scopes?: string[]
  registration_strategy?: 'dynamic' | 'preregistered' | 'client_metadata_document'
}

export type OAuthConnectState =
  | { kind: 'idle' }
  | { kind: 'probing' }
  | { kind: 'discovered'; upstream: string; issuer?: string; scopes?: string[] }
  | { kind: 'authorizing'; upstream: string }
  | { kind: 'connected'; upstream: string; registration_strategy: string; scopes?: string[] }
  | { kind: 'error'; message: string }
