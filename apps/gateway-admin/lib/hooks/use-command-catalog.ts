/**
 * SWR hook for the /v1/catalog endpoint.
 *
 * Config rationale:
 * - `revalidateIfStale: false` — catalog only changes on server restart;
 *   prevent refetch on every palette open (>2s idle revalidation).
 * - `revalidateOnFocus: false` — no background refresh on window focus.
 * - `dedupingInterval: 60_000` — 60s dedup window; catalog is stable.
 * - `revalidateOnReconnect: false` — reconnect doesn't invalidate stable catalog.
 * - `fallbackData: []` — prevent flash-of-empty CommandEmpty on first render.
 */

import useSWR from 'swr'

import { fetchCatalog } from '@/lib/command-actions/catalog'
import type { CatalogService } from '@/lib/types/command-catalog'

/** SWR cache key for /v1/catalog. Single constant prevents cache-slot divergence. */
export const COMMAND_CATALOG_KEY = '/v1/catalog'

/**
 * Returns the live catalog of enabled services and their actions.
 *
 * `data` is typed as `CatalogService[]` (empty array while loading or on error).
 * The hook never exposes raw JSON — all data has been validated through the
 * Zod schema at the fetch boundary.
 */
export function useCommandCatalog(): {
  data: CatalogService[]
  isLoading: boolean
  error: unknown
} {
  const { data, isLoading, error } = useSWR<CatalogService[]>(
    COMMAND_CATALOG_KEY,
    () => fetchCatalog().then((r) => r.services),
    {
      revalidateIfStale: false,
      revalidateOnFocus: false,
      dedupingInterval: 60_000,
      revalidateOnReconnect: false,
      fallbackData: [],
    },
  )

  return {
    data: data ?? [],
    isLoading,
    error,
  }
}
