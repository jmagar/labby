'use client'

import { Bar, BarChart, CartesianGrid, XAxis, YAxis } from 'recharts'
import {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from '@/components/ui/chart'
import type { ToolUsageEntry } from '@/lib/types/metrics'

const CONFIG: ChartConfig = {
  calls: { label: 'Calls', color: 'var(--aurora-accent-primary)' },
}

export function TopToolsChart({
  tools,
  onSelect,
}: {
  tools: ToolUsageEntry[]
  onSelect?: (tool: string) => void
}) {
  const rows = tools.map((tool) => ({ name: tool.name, calls: tool.calls }))

  return (
    <ChartContainer config={CONFIG} className="aspect-auto h-[200px] w-full">
      <BarChart
        data={rows}
        layout="vertical"
        margin={{ left: 8, right: 16, top: 4, bottom: 0 }}
      >
        <CartesianGrid horizontal={false} strokeDasharray="3 3" />
        <XAxis type="number" hide />
        <YAxis
          dataKey="name"
          type="category"
          tickLine={false}
          axisLine={false}
          width={96}
          tickMargin={6}
          className="font-mono text-[11px]"
        />
        <ChartTooltip cursor={false} content={<ChartTooltipContent />} />
        <Bar
          dataKey="calls"
          fill="var(--color-calls)"
          radius={[0, 6, 6, 0]}
          barSize={18}
          isAnimationActive={false}
          className={onSelect ? 'cursor-pointer' : undefined}
          onClick={onSelect ? (entry: { name?: string }) => entry?.name && onSelect(entry.name) : undefined}
        />
      </BarChart>
    </ChartContainer>
  )
}
