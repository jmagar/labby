import { formatCompactNumber, formatDuration, formatRelativeTime } from '@/lib/dashboard/dashboard-metrics'
import type { CallOutcome, CallSurface, ToolCallRecord } from '@/lib/types/metrics'
import { cn } from '@/lib/utils'

export function OutcomeDot({ outcome }: { outcome: CallOutcome }) {
  return (
    <span
      aria-label={outcome}
      className={cn(
        'size-2 shrink-0 rounded-full',
        outcome === 'ok' ? 'bg-aurora-success' : 'bg-aurora-error',
      )}
    />
  )
}

const SURFACE_LABEL: Record<CallSurface, string> = {
  mcp: 'MCP',
  api: 'API',
  cli: 'CLI',
  web: 'Web',
}

export function SurfaceTag({ surface }: { surface: CallSurface }) {
  return (
    <span className="rounded-aurora-1 border border-aurora-border-default px-1.5 py-px font-mono text-[10px] uppercase tracking-wide text-aurora-text-muted">
      {SURFACE_LABEL[surface]}
    </span>
  )
}

/** Compact stacked call rows for drawers. */
export function RecentCallsList({ calls }: { calls: ToolCallRecord[] }) {
  if (calls.length === 0) {
    return <p className="text-sm text-aurora-text-muted">No calls in this window.</p>
  }
  return (
    <ul className="divide-y divide-aurora-border-default/60">
      {calls.map((call) => (
        <li key={call.id} className="flex items-start gap-3 py-2.5">
          <OutcomeDot outcome={call.outcome} />
          <div className="min-w-0 flex-1">
            <div className="flex items-baseline gap-1.5">
              <span className="truncate font-mono text-[13px] text-aurora-text-primary">{call.tool}</span>
              {call.action ? (
                <span className="truncate font-mono text-[12px] text-aurora-text-muted">.{call.action}</span>
              ) : null}
            </div>
            <div className="mt-0.5 flex flex-wrap items-center gap-1.5 text-xs text-aurora-text-muted">
              <span className="truncate">{call.agent_label}</span>
              <SurfaceTag surface={call.surface} />
              {call.error_kind ? (
                <span className="font-mono text-[11px] text-aurora-error">{call.error_kind}</span>
              ) : null}
            </div>
          </div>
          <div className="shrink-0 text-right">
            <p className="text-xs font-medium tabular-nums text-aurora-text-primary">
              {formatCompactNumber(call.input_tokens + call.output_tokens)} tok
            </p>
            <p className="text-[11px] tabular-nums text-aurora-text-muted">
              {formatDuration(call.elapsed_ms)} · {formatRelativeTime(call.ts)}
            </p>
          </div>
        </li>
      ))}
    </ul>
  )
}
