'use client'

import { Loader2 } from 'lucide-react'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import {
  AURORA_BADGE_LABEL,
  AURORA_DENSE_META,
  AURORA_MEDIUM_PANEL,
  AURORA_MUTED_LABEL,
} from '@/components/aurora/tokens'
import { useBeadsIssueDetail } from '@/lib/hooks/use-beads'
import type { Comment, Dependency } from '@/lib/api/beads-client'
import { cn, getErrorMessage } from '@/lib/utils'

interface BeadsIssueDetailSheetProps {
  project: string | undefined
  issueId: string | null
  onOpenChange: (open: boolean) => void
}

export function BeadsIssueDetailSheet({
  project,
  issueId,
  onOpenChange,
}: BeadsIssueDetailSheetProps) {
  const open = Boolean(project && issueId)
  const detailQuery = useBeadsIssueDetail(project, issueId ?? undefined)
  const detail = detailQuery.data
  const error = detailQuery.error ? getErrorMessage(detailQuery.error) : null

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent
        side="right"
        className="flex w-full flex-col gap-0 overflow-hidden bg-aurora-page-bg p-0 sm:max-w-xl"
      >
        <SheetHeader className="border-b border-aurora-border-strong/60 px-5 py-4">
          <p className={AURORA_MUTED_LABEL}>{project ?? '—'}</p>
          <SheetTitle className="font-display text-[19px] font-bold text-aurora-text-primary">
            {detail?.issue.title ?? issueId ?? 'Issue detail'}
          </SheetTitle>
          <SheetDescription className="text-sm text-aurora-text-muted">
            {detail
              ? `${detail.issue.id} • ${detail.issue.issue_type} • ${detail.issue.status}`
              : 'Loading…'}
          </SheetDescription>
        </SheetHeader>

        <div className="aurora-scrollbar flex-1 overflow-y-auto px-5 py-4">
          {detailQuery.isLoading && !detail ? (
            <div className="flex items-center gap-2 text-sm text-aurora-text-muted">
              <Loader2 className="size-4 animate-spin" aria-hidden /> Loading issue…
            </div>
          ) : error ? (
            <div
              className={cn(
                AURORA_MEDIUM_PANEL,
                'border-aurora-warn/40 px-4 py-3 text-sm text-aurora-text-muted',
              )}
              role="alert"
            >
              {error}
            </div>
          ) : detail ? (
            <div className="flex flex-col gap-5">
              <MetaGrid detail={detail} />

              {detail.issue.description ? (
                <Section title="Description">
                  <p className="whitespace-pre-wrap text-sm text-aurora-text-primary">
                    {detail.issue.description}
                  </p>
                </Section>
              ) : null}

              {detail.issue.labels.length > 0 ? (
                <Section title="Labels">
                  <div className="flex flex-wrap gap-1.5">
                    {detail.issue.labels.map((label) => (
                      <span
                        key={label}
                        className={cn(
                          AURORA_BADGE_LABEL,
                          'rounded-full border border-aurora-border-strong bg-aurora-control-surface px-2 py-1 text-aurora-text-primary',
                        )}
                      >
                        {label}
                      </span>
                    ))}
                  </div>
                </Section>
              ) : null}

              <DependencySection
                title="Blocked by"
                deps={detail.blocked_by}
                idField="depends_on_id"
                emptyHint="Nothing currently blocking this issue."
              />
              <DependencySection
                title="Blocks"
                deps={detail.blocks}
                idField="issue_id"
                emptyHint="No downstream issues depend on this one."
              />
              <DependencySection
                title="Parents"
                deps={detail.parents}
                idField="depends_on_id"
                emptyHint="No parent issues."
              />
              <DependencySection
                title="Children"
                deps={detail.children}
                idField="issue_id"
                emptyHint="No child issues."
              />

              <CommentSection comments={detail.comments} />
            </div>
          ) : null}
        </div>
      </SheetContent>
    </Sheet>
  )
}

function MetaGrid({
  detail,
}: {
  detail: NonNullable<ReturnType<typeof useBeadsIssueDetail>['data']>
}) {
  const items: Array<{ label: string; value: string }> = [
    { label: 'Status', value: detail.issue.status },
    { label: 'Priority', value: `P${detail.issue.priority}` },
    { label: 'Type', value: detail.issue.issue_type },
    { label: 'Assignee', value: detail.issue.assignee || '—' },
    { label: 'Owner', value: detail.issue.owner || '—' },
    { label: 'Created', value: formatTimestamp(detail.issue.created_at) },
    { label: 'Updated', value: formatTimestamp(detail.issue.updated_at) },
    { label: 'Closed', value: formatTimestamp(detail.issue.closed_at) },
  ]
  return (
    <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
      {items.map((item) => (
        <div key={item.label} className="flex flex-col gap-1">
          <span className={AURORA_MUTED_LABEL}>{item.label}</span>
          <span className="truncate text-sm font-medium text-aurora-text-primary">
            {item.value}
          </span>
        </div>
      ))}
    </div>
  )
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-2">
      <h3 className="font-display text-[15px] font-extrabold tracking-[-0.02em] text-aurora-text-primary">
        {title}
      </h3>
      {children}
    </div>
  )
}

function DependencySection({
  title,
  deps,
  idField,
  emptyHint,
}: {
  title: string
  deps: Dependency[]
  idField: 'issue_id' | 'depends_on_id'
  emptyHint: string
}) {
  return (
    <Section title={title}>
      {deps.length === 0 ? (
        <p className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>{emptyHint}</p>
      ) : (
        <ul className="flex flex-col gap-1">
          {deps.map((dep) => {
            const id = dep[idField]
            return (
              <li
                key={`${dep.issue_id}-${dep.depends_on_id}-${dep.type}`}
                className={cn(
                  AURORA_MEDIUM_PANEL,
                  'flex items-center justify-between gap-3 rounded-aurora-2 px-3 py-2',
                )}
              >
                <span className="font-mono text-[12px] font-semibold text-aurora-text-primary">
                  {id}
                </span>
                <span
                  className={cn(
                    AURORA_BADGE_LABEL,
                    'rounded-full border border-aurora-border-strong bg-aurora-control-surface px-2 py-1 text-aurora-text-muted',
                  )}
                >
                  {dep.type}
                </span>
              </li>
            )
          })}
        </ul>
      )}
    </Section>
  )
}

function CommentSection({ comments }: { comments: Comment[] }) {
  if (comments.length === 0) {
    return (
      <Section title="Comments">
        <p className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>
          No comments yet.
        </p>
      </Section>
    )
  }
  return (
    <Section title={`Comments (${comments.length})`}>
      <ul className="flex flex-col gap-2">
        {comments.map((comment) => (
          <li
            key={comment.id}
            className={cn(
              AURORA_MEDIUM_PANEL,
              'flex flex-col gap-1 rounded-aurora-2 px-3 py-2',
            )}
          >
            <div className="flex items-center justify-between gap-2">
              <span className="text-[12px] font-semibold text-aurora-text-primary">
                {comment.author}
              </span>
              <span className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>
                {formatTimestamp(comment.created_at)}
              </span>
            </div>
            <p className="whitespace-pre-wrap text-sm text-aurora-text-primary">
              {comment.text}
            </p>
          </li>
        ))}
      </ul>
    </Section>
  )
}

function formatTimestamp(value?: string | null): string {
  if (!value) return '—'
  const ts = Date.parse(value)
  if (Number.isNaN(ts)) return value
  return new Date(ts).toLocaleString()
}
