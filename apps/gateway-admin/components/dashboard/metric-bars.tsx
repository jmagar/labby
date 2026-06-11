import { cn } from '@/lib/utils'

export interface MetricBarItem {
  key: string
  label: string
  value: number
  /** Optional right-aligned formatted value (defaults to the number). */
  display?: string
  onSelect?: () => void
}

const BAR_TONE = {
  accent: 'bg-aurora-accent-primary',
  strong: 'bg-aurora-accent-strong',
  error: 'bg-aurora-error',
  warn: 'bg-aurora-warn',
} as const

/** A ranked list of proportional horizontal bars. */
export function MetricBarList({
  items,
  tone = 'accent',
  mono = false,
  empty = 'No data in this window.',
}: {
  items: MetricBarItem[]
  tone?: keyof typeof BAR_TONE
  mono?: boolean
  empty?: string
}) {
  if (items.length === 0) {
    return <p className="text-sm text-aurora-text-muted">{empty}</p>
  }
  const max = Math.max(1, ...items.map((i) => i.value))

  return (
    <ul className="flex flex-col gap-2.5">
      {items.map((item) => {
        const pct = Math.round((item.value / max) * 100)
        const body = (
          <>
            <div className="flex items-baseline justify-between gap-3">
              <span
                className={cn(
                  'min-w-0 truncate text-aurora-text-primary',
                  mono ? 'font-mono text-[13px]' : 'text-sm',
                )}
              >
                {item.label}
              </span>
              <span className="shrink-0 text-sm font-semibold tabular-nums text-aurora-text-muted">
                {item.display ?? item.value}
              </span>
            </div>
            <div className="mt-1.5 h-1.5 w-full overflow-hidden rounded-full bg-aurora-control-surface">
              <div className={cn('h-full rounded-full', BAR_TONE[tone])} style={{ width: `${pct}%` }} />
            </div>
          </>
        )
        return (
          <li key={item.key}>
            {item.onSelect ? (
              <button
                type="button"
                onClick={item.onSelect}
                className="block w-full rounded-aurora-1 px-1 py-0.5 text-left transition-colors hover:bg-aurora-hover-bg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary/40"
              >
                {body}
              </button>
            ) : (
              <div className="px-1 py-0.5">{body}</div>
            )}
          </li>
        )
      })}
    </ul>
  )
}
