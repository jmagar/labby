'use client'

import Link from 'next/link'
import { ArrowUpRight, Wrench } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { useToolDetail } from '@/lib/hooks/use-usage-drilldown'
import { ToolVolumeChart } from './tool-volume-chart'
import { RecentCallsList } from './recent-calls'
import { DetailDrawer, DrawerSection, DrawerStatGrid, RankRow } from './detail-drawer'
import { ErrorNotice } from './error-notice'
import type { DrillTarget } from './drill'
import {
  WINDOW_LABELS,
  formatCompactNumber,
  formatDuration,
  successRatePercent,
} from '@/lib/dashboard/dashboard-metrics'
import type { MetricsWindow } from '@/lib/types/metrics'

export function ToolDetailDrawer({
  tool,
  window,
  onClose,
  onDrill,
}: {
  tool: string | null
  window: MetricsWindow
  onClose: () => void
  onDrill: (target: DrillTarget) => void
}) {
  const { data, isLoading, error, mutate } = useToolDetail(tool, window)
  const detail = data && data.name === tool ? data : undefined
  const rate = detail ? successRatePercent(detail.calls, detail.failed) : null

  return (
    <DetailDrawer
      open={tool !== null}
      onClose={onClose}
      icon={<Wrench className="size-5" />}
      title={<span className="font-mono">{tool ?? ''}</span>}
      subtitle={`Tool usage · ${WINDOW_LABELS[window]}`}
    >
      {error && !detail ? (
        <ErrorNotice message="Couldn't load tool details." onRetry={() => mutate()} />
      ) : !detail || isLoading ? (
        <div className="flex flex-col gap-6">
          <Skeleton className="h-20 w-full" />
          <Skeleton className="h-[180px] w-full" />
          <Skeleton className="h-40 w-full" />
        </div>
      ) : (
        <>
          <DrawerStatGrid
            items={[
              { label: 'Tool calls', value: formatCompactNumber(detail.calls) },
              {
                label: 'Failed',
                value: formatCompactNumber(detail.failed),
                tone: detail.failed > 0 ? 'error' : 'success',
              },
              { label: 'Avg tokens', value: formatCompactNumber(detail.avg_tokens) },
              { label: 'Avg latency', value: formatDuration(detail.avg_elapsed_ms) },
            ]}
          />

          <DrawerSection
            title="Call volume"
            action={
              <span className="text-xs text-aurora-text-muted">
                {rate !== null ? `${rate}% success` : '—'}
              </span>
            }
          >
            <ToolVolumeChart data={detail.timeseries} window={window} />
          </DrawerSection>

          <DrawerSection title="Top callers">
            {detail.top_callers.length === 0 ? (
              <p className="text-sm text-aurora-text-muted">No callers in this window.</p>
            ) : (
              <div className="flex flex-col">
                {detail.top_callers.map((caller) => (
                  <RankRow
                    key={caller.id}
                    label={caller.label}
                    value={formatCompactNumber(caller.calls)}
                    onClick={() => onDrill({ type: 'agent', id: caller.id })}
                  />
                ))}
              </div>
            )}
          </DrawerSection>

          <DrawerSection
            title="Recent calls"
            action={
              <Button variant="ghost" size="sm" asChild>
                <Link href={`/usage?tool=${encodeURIComponent(detail.name)}&window=${window}`}>
                  Open in explorer
                  <ArrowUpRight className="ml-1 size-3.5" />
                </Link>
              </Button>
            }
          >
            <RecentCallsList calls={detail.recent} />
          </DrawerSection>
        </>
      )}
    </DetailDrawer>
  )
}
