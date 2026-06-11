'use client'

import { METRICS_WINDOWS, type MetricsWindow } from '@/lib/types/metrics'
import { dashPill } from './ui'
import { cn } from '@/lib/utils'

const WINDOW_SHORT: Record<MetricsWindow, string> = {
  '1h': '1h',
  '24h': '24h',
  '7d': '7d',
}

/** Rolling-window pill toggle for the activity section. */
export function WindowSelector({
  value,
  onChange,
}: {
  value: MetricsWindow
  onChange: (window: MetricsWindow) => void
}) {
  return (
    <div
      role="tablist"
      aria-label="Activity window"
      className="inline-flex items-center gap-1 rounded-aurora-2 border border-aurora-border-strong bg-aurora-control-surface p-1"
    >
      {METRICS_WINDOWS.map((window) => {
        const active = window === value
        return (
          <button
            key={window}
            type="button"
            role="tab"
            aria-selected={active}
            onClick={() => onChange(window)}
            className={cn(
              'rounded-aurora-1 border px-3 py-1 text-xs font-semibold tabular-nums transition-colors',
              'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary/40',
              dashPill(active),
            )}
          >
            {WINDOW_SHORT[window]}
          </button>
        )
      })}
    </div>
  )
}
