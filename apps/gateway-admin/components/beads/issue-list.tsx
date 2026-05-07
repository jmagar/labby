'use client'

import { ArrowRight } from 'lucide-react'
import {
  AURORA_BADGE_LABEL,
  AURORA_DENSE_META,
  AURORA_MEDIUM_PANEL,
  AURORA_MUTED_LABEL,
} from '@/components/aurora/tokens'
import { cn } from '@/lib/utils'
import type { Issue } from '@/lib/api/beads-client'

interface BeadsIssueListProps {
  issues: Issue[]
  loading: boolean
  onOpen: (issue: Issue) => void
}

const STATUS_TONE: Record<string, string> = {
  open: 'border-aurora-accent-primary/40 text-aurora-text-primary',
  in_progress: 'border-aurora-accent-strong/50 text-aurora-text-primary',
  blocked: 'border-aurora-warn/45 text-aurora-warn',
  deferred: 'border-aurora-border-strong text-aurora-text-muted',
  closed: 'border-aurora-border-strong text-aurora-text-muted',
  pinned: 'border-aurora-warn/45 text-aurora-warn',
  tombstone: 'border-aurora-border-strong text-aurora-text-muted',
}

const PRIORITY_LABELS: Record<number, string> = {
  0: 'P0',
  1: 'P1',
  2: 'P2',
  3: 'P3',
  4: 'P4',
}

const PRIORITY_TONE: Record<number, string> = {
  0: 'border-aurora-error/45 text-aurora-error',
  1: 'border-aurora-warn/45 text-aurora-warn',
  2: 'border-aurora-border-strong text-aurora-text-primary',
  3: 'border-aurora-border-strong text-aurora-text-muted',
  4: 'border-aurora-border-strong text-aurora-text-muted',
}

export function BeadsIssueList({ issues, loading, onOpen }: BeadsIssueListProps) {
  if (loading) {
    return (
      <div className="flex flex-col gap-2" data-density="compact">
        {Array.from({ length: 4 }).map((_, idx) => (
          <div
            key={idx}
            className={cn(
              AURORA_MEDIUM_PANEL,
              'h-12 animate-pulse rounded-aurora-2 bg-aurora-control-surface/60',
            )}
          />
        ))}
      </div>
    )
  }

  if (issues.length === 0) {
    return (
      <div className={cn(AURORA_MEDIUM_PANEL, 'px-4 py-5 text-center')}>
        <p className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>
          No issues match this filter.
        </p>
      </div>
    )
  }

  return (
    <div className="flex flex-col gap-1.5" data-density="compact">
      <div className="grid grid-cols-[80px_72px_minmax(0,1fr)_120px_140px_24px] items-center gap-3 px-3 pb-1">
        <span className={AURORA_MUTED_LABEL}>ID</span>
        <span className={AURORA_MUTED_LABEL}>Pri</span>
        <span className={AURORA_MUTED_LABEL}>Title</span>
        <span className={AURORA_MUTED_LABEL}>Status</span>
        <span className={AURORA_MUTED_LABEL}>Updated</span>
        <span className="sr-only">Open</span>
      </div>
      <ul className="flex flex-col gap-1">
        {issues.map((issue) => (
          <li key={issue.id}>
            <button
              type="button"
              onClick={() => onOpen(issue)}
              className={cn(
                AURORA_MEDIUM_PANEL,
                'group grid w-full grid-cols-[80px_72px_minmax(0,1fr)_120px_140px_24px] items-center gap-3 rounded-aurora-2 px-3 py-2 text-left transition-colors hover:bg-aurora-hover-bg',
              )}
            >
              <span
                className={cn(
                  'truncate font-mono text-[12px] font-semibold text-aurora-text-primary',
                )}
              >
                {issue.id}
              </span>
              <PriorityChip priority={issue.priority} />
              <span className="flex min-w-0 flex-col">
                <span className="truncate text-[13px] font-medium text-aurora-text-primary">
                  {issue.title || '(untitled)'}
                </span>
                <span
                  className={cn(
                    AURORA_DENSE_META,
                    'truncate text-aurora-text-muted',
                  )}
                >
                  {issue.issue_type}
                  {issue.assignee ? ` • ${issue.assignee}` : ''}
                  {issue.labels.length > 0 ? ` • ${issue.labels.join(', ')}` : ''}
                </span>
              </span>
              <StatusChip status={issue.status} />
              <span className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>
                {formatRelative(issue.updated_at)}
              </span>
              <ArrowRight
                className="size-3.5 text-aurora-text-muted transition-transform group-hover:translate-x-0.5"
                aria-hidden
              />
            </button>
          </li>
        ))}
      </ul>
    </div>
  )
}

function StatusChip({ status }: { status: string }) {
  const tone = STATUS_TONE[status] ?? 'border-aurora-border-strong text-aurora-text-muted'
  return (
    <span
      className={cn(
        AURORA_BADGE_LABEL,
        'rounded-full border bg-aurora-control-surface px-2 py-1',
        tone,
      )}
    >
      {status.replace('_', ' ')}
    </span>
  )
}

function PriorityChip({ priority }: { priority: number }) {
  const label = PRIORITY_LABELS[priority] ?? `P${priority}`
  const tone =
    PRIORITY_TONE[priority] ?? 'border-aurora-border-strong text-aurora-text-muted'
  return (
    <span
      className={cn(
        AURORA_BADGE_LABEL,
        'rounded-full border bg-aurora-control-surface px-2 py-1',
        tone,
      )}
    >
      {label}
    </span>
  )
}

function formatRelative(value?: string | null): string {
  if (!value) return '—'
  const ts = Date.parse(value)
  if (Number.isNaN(ts)) return value
  const diff = Date.now() - ts
  // Math.floor — buckets shouldn't roll over until a full unit has passed
  // (e.g. 45 minutes is "45m ago", not "1h ago").
  const minutes = Math.floor(diff / 60_000)
  if (minutes < 1) return 'just now'
  if (minutes < 60) return `${minutes}m ago`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours}h ago`
  const days = Math.floor(hours / 24)
  if (days < 7) return `${days}d ago`
  const weeks = Math.floor(days / 7)
  if (weeks < 5) return `${weeks}w ago`
  const date = new Date(ts)
  return date.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })
}
