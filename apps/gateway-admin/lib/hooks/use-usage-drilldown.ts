'use client'

import useSWR from 'swr'
import {
  fetchAgentDetail,
  fetchToolCalls,
  fetchToolDetail,
} from '@/lib/api/metrics-client'
import type {
  AgentDetail,
  MetricsWindow,
  ToolCallPage,
  ToolCallQuery,
  ToolDetail,
} from '@/lib/types/metrics'

/** Single-tool drill-down. Pass `null` name to disable (closed drawer). */
export function useToolDetail(name: string | null, window: MetricsWindow) {
  return useSWR<ToolDetail>(
    name ? ['tool-detail', name, window] : null,
    name ? () => fetchToolDetail(name, window) : null,
    { revalidateOnFocus: false, keepPreviousData: true },
  )
}

/** Single-agent/device drill-down. Pass `null` id to disable. */
export function useAgentDetail(id: string | null, window: MetricsWindow) {
  return useSWR<AgentDetail>(
    id ? ['agent-detail', id, window] : null,
    id ? () => fetchAgentDetail(id, window) : null,
    { revalidateOnFocus: false, keepPreviousData: true },
  )
}

/** Filterable tool-call log for the explorer page. */
export function useToolCalls(query: ToolCallQuery) {
  return useSWR<ToolCallPage>(
    ['tool-calls', query],
    () => fetchToolCalls(query),
    { revalidateOnFocus: false, keepPreviousData: true },
  )
}
