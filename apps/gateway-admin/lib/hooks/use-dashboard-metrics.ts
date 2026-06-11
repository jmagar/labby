'use client'

import useSWR from 'swr'
import { fetchDashboardMetrics } from '@/lib/api/metrics-client'
import type { DashboardMetrics, MetricsWindow } from '@/lib/types/metrics'

export const dashboardMetricsKey = (window: MetricsWindow) =>
  `/dashboard-metrics/${window}`

/** Windowed gateway activity metrics. Polls every 15s; pauses off-focus. */
export function useDashboardMetrics(window: MetricsWindow) {
  return useSWR<DashboardMetrics>(
    dashboardMetricsKey(window),
    () => fetchDashboardMetrics(window),
    {
      revalidateOnFocus: false,
      refreshInterval: 15_000,
      keepPreviousData: true,
    },
  )
}
