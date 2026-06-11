'use client'

import Link from 'next/link'
import { ArrowUpRight, Bot, HardDrive } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { useAgentDetail } from '@/lib/hooks/use-usage-drilldown'
import { ToolVolumeChart } from './tool-volume-chart'
import { RecentCallsList } from './recent-calls'
import { DetailDrawer, DrawerSection, DrawerStatGrid, RankRow } from './detail-drawer'
import { ErrorNotice } from './error-notice'
import type { DrillTarget } from './drill'
import {
  WINDOW_LABELS,
  formatCompactNumber,
  successRatePercent,
} from '@/lib/dashboard/dashboard-metrics'
import type { MetricsWindow } from '@/lib/types/metrics'

export function AgentDetailDrawer({
  agentId,
  window,
  onClose,
  onDrill,
}: {
  agentId: string | null
  window: MetricsWindow
  onClose: () => void
  onDrill: (target: DrillTarget) => void
}) {
  const { data, isLoading, error, mutate } = useAgentDetail(agentId, window)
  const detail = data && data.id === agentId ? data : undefined
  const rate = detail ? successRatePercent(detail.calls, detail.failed) : null
  const Icon = detail?.kind === 'device' ? HardDrive : Bot

  return (
    <DetailDrawer
      open={agentId !== null}
      onClose={onClose}
      icon={<Icon className="size-5" />}
      title={detail?.label ?? agentId ?? ''}
      subtitle={`${detail?.kind === 'device' ? 'Device' : 'Agent'} activity · ${WINDOW_LABELS[window]}`}
    >
      {error && !detail ? (
        <ErrorNotice message="Couldn't load activity details." onRetry={() => mutate()} />
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
              { label: 'Success', value: rate !== null ? `${rate}%` : '—' },
              { label: 'Tokens', value: formatCompactNumber(detail.total_tokens) },
            ]}
          />

          <DrawerSection title="Activity">
            <ToolVolumeChart data={detail.timeseries} window={window} />
          </DrawerSection>

          <DrawerSection title="Tools used">
            {detail.tools_used.length === 0 ? (
              <p className="text-sm text-aurora-text-muted">No tools used in this window.</p>
            ) : (
              <div className="flex flex-col">
                {detail.tools_used.slice(0, 8).map((tool) => (
                  <RankRow
                    key={tool.name}
                    label={tool.name}
                    mono
                    value={formatCompactNumber(tool.calls)}
                    onClick={() => onDrill({ type: 'tool', name: tool.name })}
                  />
                ))}
              </div>
            )}
          </DrawerSection>

          <DrawerSection
            title="Recent calls"
            action={
              <Button variant="ghost" size="sm" asChild>
                <Link href={`/usage?agent=${encodeURIComponent(detail.id)}&window=${window}`}>
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
