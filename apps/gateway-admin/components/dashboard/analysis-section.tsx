import { Skeleton } from '@/components/ui/skeleton'
import {
  FailuresPanel,
  HourlyHeatPanel,
  LatencyPanel,
  SurfacesPanel,
  ThroughputPanel,
  TokensByToolPanel,
  UpstreamsPanel,
} from './analysis-panels'
import type { DashboardMetrics } from '@/lib/types/metrics'

function PanelSkeleton({ height }: { height: string }) {
  return <Skeleton className={`w-full rounded-aurora-3 ${height}`} />
}

/** Performance / cost / rhythm analytics. Renders skeletons until metrics load. */
export function AnalysisSection({
  metrics,
  onSelectTool,
}: {
  metrics?: DashboardMetrics
  onSelectTool: (tool: string) => void
}) {
  if (!metrics) {
    return (
      <div className="flex flex-col gap-4">
        <div className="grid gap-4 lg:grid-cols-3">
          {[0, 1, 2].map((i) => (
            <PanelSkeleton key={i} height="h-[232px]" />
          ))}
        </div>
        <div className="grid gap-4 lg:grid-cols-3">
          {[0, 1, 2].map((i) => (
            <PanelSkeleton key={i} height="h-[268px]" />
          ))}
        </div>
        <PanelSkeleton height="h-[140px]" />
      </div>
    )
  }

  return (
    <div className="flex flex-col gap-4">
      <div className="grid gap-4 lg:grid-cols-3">
        <LatencyPanel latency={metrics.latency} />
        <FailuresPanel errors={metrics.errors} />
        <SurfacesPanel surfaces={metrics.surfaces} />
      </div>
      <div className="grid gap-4 lg:grid-cols-3">
        <TokensByToolPanel tokens={metrics.tokens_by_tool} onSelect={onSelectTool} />
        <UpstreamsPanel upstreams={metrics.upstreams} />
        <ThroughputPanel throughput={metrics.throughput} agentsSeen={metrics.agents_seen} />
      </div>
      <HourlyHeatPanel hourly={metrics.hourly} busiestHour={metrics.throughput.busiest_hour} />
    </div>
  )
}
