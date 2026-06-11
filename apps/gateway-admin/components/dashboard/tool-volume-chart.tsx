'use client'

import { Area, AreaChart, CartesianGrid, Line, XAxis } from 'recharts'
import {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from '@/components/ui/chart'
import type { MetricsBucket, MetricsWindow } from '@/lib/types/metrics'

const CONFIG: ChartConfig = {
  calls: { label: 'Tool calls', color: 'var(--aurora-accent-primary)' },
  failed: { label: 'Failed', color: 'var(--aurora-error)' },
}

function bucketLabel(ts: number, window: MetricsWindow): string {
  const date = new Date(ts)
  if (window === '7d') {
    return date.toLocaleDateString(undefined, { weekday: 'short' })
  }
  return date.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })
}

export function ToolVolumeChart({
  data,
  window,
}: {
  data: MetricsBucket[]
  window: MetricsWindow
}) {
  const rows = data.map((bucket) => ({
    label: bucketLabel(bucket.ts, window),
    calls: bucket.calls,
    failed: bucket.failed,
  }))

  return (
    <ChartContainer config={CONFIG} className="aspect-auto h-[200px] w-full">
      <AreaChart data={rows} margin={{ left: 4, right: 4, top: 8, bottom: 0 }}>
        <defs>
          <linearGradient id="fill-calls" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="var(--color-calls)" stopOpacity={0.35} />
            <stop offset="100%" stopColor="var(--color-calls)" stopOpacity={0.02} />
          </linearGradient>
        </defs>
        <CartesianGrid vertical={false} strokeDasharray="3 3" />
        <XAxis
          dataKey="label"
          tickLine={false}
          axisLine={false}
          tickMargin={8}
          minTickGap={24}
        />
        <ChartTooltip cursor={false} content={<ChartTooltipContent />} />
        <Area
          dataKey="calls"
          type="natural"
          stroke="var(--color-calls)"
          strokeWidth={2}
          fill="url(#fill-calls)"
          isAnimationActive={false}
        />
        <Line
          dataKey="failed"
          type="natural"
          stroke="var(--color-failed)"
          strokeWidth={1.5}
          dot={false}
          isAnimationActive={false}
        />
      </AreaChart>
    </ChartContainer>
  )
}
