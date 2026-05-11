import { useEffect, useRef } from 'react'
import useSWR from 'swr'
import { upstreamOauthApi } from '@/lib/api/upstream-oauth-client'
import type { UpstreamEntry, UpstreamOauthStatus } from '@/lib/types/upstream-oauth'

const USE_MOCK_DATA = process.env.NEXT_PUBLIC_MOCK_DATA === 'true'

export function useUpstreamOauthUpstreams() {
  return useSWR<UpstreamEntry[], Error>(
    '/v1/gateway/oauth/upstreams',
    () => {
      if (USE_MOCK_DATA) {
        return Promise.reject(new Error('HTTP 404'))
      }
      return upstreamOauthApi.listUpstreams()
    },
    { revalidateOnFocus: true },
  )
}

export function useUpstreamOauthStatus(
  name: string | null,
  options: { pollWhilePending?: boolean; maxPollDuration?: number } = {},
) {
  const { pollWhilePending = false, maxPollDuration = 300_000 } = options
  const pollStartRef = useRef<number | null>(null)
  const previousPollWhilePendingRef = useRef(pollWhilePending)

  useEffect(() => {
    pollStartRef.current = null
  }, [name])

  useEffect(() => {
    if (pollWhilePending && !previousPollWhilePendingRef.current) {
      pollStartRef.current = null
    }
    if (!pollWhilePending) {
      pollStartRef.current = null
    }
    previousPollWhilePendingRef.current = pollWhilePending
  }, [pollWhilePending])

  return useSWR<UpstreamOauthStatus, Error>(
    name ? `/v1/gateway/oauth/status/${name}` : null,
    () => {
      if (USE_MOCK_DATA) {
        return Promise.reject(new Error('HTTP 404'))
      }
      return upstreamOauthApi.status(name!)
    },
    {
      refreshInterval: (data) => {
        if (!pollWhilePending || data?.authenticated) return 0
        if (pollStartRef.current === null) pollStartRef.current = Date.now()
        return Date.now() - pollStartRef.current < maxPollDuration ? 3_000 : 0
      },
      revalidateOnFocus: true,
    },
  )
}
