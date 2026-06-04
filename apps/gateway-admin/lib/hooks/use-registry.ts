'use client'

import useSWR from 'swr'
import { listServers, getServer } from '@/lib/api/mcpregistry-client'
import type { ServerListResponse, ServerResponse } from '@/lib/types/registry'

export const REGISTRY_SERVERS_KEY = '/registry/servers'
export const REGISTRY_SERVER_KEY = '/registry/server'

export const registryServersKey = (
  query: string,
  cursor: string | null,
  version?: string,
  updatedSince?: string,
  featured?: boolean,
  reviewed?: boolean,
  recommended?: boolean,
  hidden?: boolean,
  tag?: string,
): RegistryServersKey => [
  REGISTRY_SERVERS_KEY,
  query,
  cursor,
  version || undefined,
  updatedSince || undefined,
  featured ?? undefined,
  reviewed ?? undefined,
  recommended ?? undefined,
  hidden ?? undefined,
  tag || undefined,
]

export const registryServerKey = (name: string): [string, string] => [
  REGISTRY_SERVER_KEY,
  name,
]

export type RegistryServersKey = [string, string, string | null, string?, string?, boolean?, boolean?, boolean?, boolean?, string?]

// Fetcher exported so bead 3 can wrap it with an AbortController ref.
export function fetchRegistryServers(
  [, query, cursor, version, updatedSince, featured, reviewed, recommended, hidden, tag]: RegistryServersKey,
  signal?: AbortSignal,
): Promise<ServerListResponse> {
  return listServers(
    {
      search: query || undefined,
      limit: 20,
      cursor: cursor ?? undefined,
      version,
      updated_since: updatedSince && /^\d{4}-\d{2}-\d{2}$/.test(updatedSince)
        ? `${updatedSince}T00:00:00Z`
        : updatedSince,
      featured,
      reviewed,
      recommended,
      hidden,
      tag,
    },
    signal,
  )
}

export function useRegistryServers(
  query: string,
  cursor: string | null = null,
  version?: string,
  updatedSince?: string,
  featured?: boolean,
  reviewed?: boolean,
  recommended?: boolean,
  hidden?: boolean,
  tag?: string,
) {
  return useSWR<ServerListResponse>(
    registryServersKey(query, cursor, version, updatedSince, featured, reviewed, recommended, hidden, tag),
    (key: RegistryServersKey) =>
      fetchRegistryServers(key, undefined),
    { revalidateOnFocus: false },
  )
}

export function useRegistryServer(name: string | null) {
  return useSWR<ServerResponse>(
    name ? registryServerKey(name) : null,
    name ? ([, n]: [string, string]) => getServer(n) : null,
    { revalidateOnFocus: false },
  )
}
