'use client'

import { useEffect, useMemo, useState } from 'react'
import Link from 'next/link'
import {
  AlertTriangle,
  CircleDashed,
  ExternalLink,
  Filter,
  RefreshCw,
} from 'lucide-react'
import { AppHeader } from '@/components/app-header'
import {
  AURORA_BADGE_LABEL,
  AURORA_DENSE_META,
  AURORA_DISPLAY_1,
  AURORA_DISPLAY_NUMBER,
  AURORA_MEDIUM_PANEL,
  AURORA_MUTED_LABEL,
  AURORA_PAGE_FRAME,
  AURORA_PAGE_SHELL,
  AURORA_STAT_PANEL,
  AURORA_STRONG_PANEL,
} from '@/components/aurora/tokens'
import { Button } from '@/components/ui/button'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import {
  useBeadsContext,
  useBeadsIssues,
  useBeadsProjects,
  useBeadsStatusSummary,
} from '@/lib/hooks/use-beads'
import { BEADS_STATUS_VALUES, type BeadsStatus, type Issue } from '@/lib/api/beads-client'
import { cn, getErrorMessage } from '@/lib/utils'
import { BeadsIssueList } from './issue-list'
import { BeadsIssueDetailSheet } from './issue-detail-sheet'

type StatusFilter = BeadsStatus | 'any'
type IssueView = 'all' | 'ready'

const STATUS_LABELS: Record<StatusFilter, string> = {
  any: 'All statuses',
  open: 'Open',
  in_progress: 'In progress',
  blocked: 'Blocked',
  deferred: 'Deferred',
  closed: 'Closed',
}

export function BeadsShell() {
  const projectsQuery = useBeadsProjects()
  const projects = useMemo(() => projectsQuery.data ?? [], [projectsQuery.data])

  const [project, setProject] = useState<string | undefined>(undefined)
  const [view, setView] = useState<IssueView>('ready')
  const [status, setStatus] = useState<StatusFilter>('any')
  const [openIssueId, setOpenIssueId] = useState<string | null>(null)

  useEffect(() => {
    if (!project && projects.length > 0) {
      setProject(projects[0]?.name)
    }
  }, [projects, project])

  const contextQuery = useBeadsContext(project)
  const summaryQuery = useBeadsStatusSummary(project)
  const issuesQuery = useBeadsIssues(project, view, status, 200)

  const issues = issuesQuery.data ?? []
  const isInitialLoading =
    !projectsQuery.data && !projectsQuery.error && projectsQuery.isLoading

  const issuesError = issuesQuery.error ? getErrorMessage(issuesQuery.error) : null
  const projectsError = projectsQuery.error ? getErrorMessage(projectsQuery.error) : null

  const refresh = () => {
    void projectsQuery.mutate()
    void contextQuery.mutate()
    void summaryQuery.mutate()
    void issuesQuery.mutate()
  }

  return (
    <>
      <AppHeader
        breadcrumbs={[{ label: 'Overview', href: '/' }, { label: 'Beads' }]}
        actions={
          <Button
            variant="outline"
            size="sm"
            onClick={refresh}
            disabled={issuesQuery.isValidating || projectsQuery.isValidating}
            className="gap-2"
          >
            <RefreshCw
              className={cn(
                'size-3.5',
                (issuesQuery.isValidating || projectsQuery.isValidating) && 'animate-spin',
              )}
            />
            Refresh
          </Button>
        }
      />
      <div className={cn(AURORA_PAGE_FRAME, AURORA_PAGE_SHELL, 'min-w-0 [&_*]:min-w-0')}>
        <div className={cn(AURORA_STRONG_PANEL, 'px-4 py-5 sm:px-6 sm:py-6')}>
          <p className={AURORA_MUTED_LABEL}>Issue tracker</p>
          <div className="mt-2 flex flex-col gap-3 sm:flex-row sm:items-end sm:justify-between">
            <div>
              <h1 className={cn(AURORA_DISPLAY_1, 'text-aurora-text-primary')}>Beads</h1>
              <p className="mt-2 max-w-2xl text-sm text-aurora-text-muted">
                Read-only view of the Beads issue tracker, queried directly against your
                Dolt SQL server. Each Dolt database is treated as one project; pick a
                project to scope the issue list, ready queue, and dependency graph.
              </p>
            </div>
            <div className="flex flex-wrap items-center gap-2">
              <span className={AURORA_MUTED_LABEL}>Project</span>
              <Select
                value={project ?? ''}
                onValueChange={(value) => {
                  setProject(value)
                  setOpenIssueId(null)
                }}
                disabled={projects.length === 0}
              >
                <SelectTrigger className="min-w-[200px]">
                  <SelectValue
                    placeholder={
                      projects.length === 0 ? 'No projects available' : 'Select project'
                    }
                  />
                </SelectTrigger>
                <SelectContent>
                  {projects.map((entry) => (
                    <SelectItem key={entry.name} value={entry.name}>
                      {entry.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </div>
        </div>

        {projectsError ? (
          <BeadsErrorPanel
            title="Could not list projects"
            message={projectsError}
            hint="Configure BEADS_DOLT_URL in Settings → Services → Beads to point Lab at your Dolt SQL server."
          />
        ) : null}

        {!projectsError && projects.length === 0 && !isInitialLoading ? (
          <BeadsEmptyState />
        ) : null}

        {project ? (
          <>
            <BeadsSummaryStrip
              project={project}
              total={summaryQuery.data?.total ?? contextQuery.data?.total_issues ?? 0}
              open={contextQuery.data?.open_issues ?? 0}
              byStatus={summaryQuery.data?.by_status ?? []}
            />

            <div className={cn(AURORA_STRONG_PANEL, 'flex flex-col gap-4 px-4 py-4 sm:px-6')}>
              <div className="flex flex-wrap items-center gap-3">
                <ViewToggle value={view} onChange={setView} />
                <div className="flex items-center gap-2">
                  <Filter className="size-3.5 text-aurora-text-muted" aria-hidden />
                  <Select
                    value={status}
                    onValueChange={(value) => setStatus(value as StatusFilter)}
                    disabled={view === 'ready'}
                  >
                    <SelectTrigger className="min-w-[180px]">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="any">All statuses</SelectItem>
                      {BEADS_STATUS_VALUES.map((value) => (
                        <SelectItem key={value} value={value}>
                          {STATUS_LABELS[value]}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
                <span className={cn(AURORA_DENSE_META, 'text-aurora-text-muted ml-auto')}>
                  {issuesQuery.isValidating
                    ? 'Refreshing…'
                    : issues.length
                      ? `${issues.length} ${issues.length === 1 ? 'issue' : 'issues'}`
                      : 'No issues match'}
                </span>
              </div>

              {issuesError ? (
                <BeadsErrorPanel
                  title="Failed to load issues"
                  message={issuesError}
                  hint="Verify that the selected project has the Beads schema (issues, dependencies, comments)."
                />
              ) : (
                <BeadsIssueList
                  issues={issues}
                  loading={issuesQuery.isLoading && !issuesQuery.data}
                  onOpen={(issue: Issue) => setOpenIssueId(issue.id)}
                />
              )}
            </div>
          </>
        ) : null}

        <BeadsIssueDetailSheet
          project={project}
          issueId={openIssueId}
          onOpenChange={(open) => {
            if (!open) setOpenIssueId(null)
          }}
        />
      </div>
    </>
  )
}

interface SummaryStripProps {
  project: string
  total: number
  open: number
  byStatus: Array<{ status: string; count: number }>
}

function BeadsSummaryStrip({ project, total, open, byStatus }: SummaryStripProps) {
  const ranked = useMemo(
    () => byStatus.slice().sort((a, b) => b.count - a.count).slice(0, 5),
    [byStatus],
  )

  return (
    <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
      <div className={AURORA_STAT_PANEL}>
        <p className={AURORA_MUTED_LABEL}>Project</p>
        <p
          className={cn(
            'mt-2 font-display text-[18px] font-extrabold tabular-nums leading-none text-aurora-text-primary',
          )}
        >
          {project}
        </p>
      </div>
      <div className={AURORA_STAT_PANEL}>
        <p className={AURORA_MUTED_LABEL}>Total issues</p>
        <p className={cn(AURORA_DISPLAY_NUMBER, 'mt-2 text-aurora-text-primary')}>
          {total.toLocaleString()}
        </p>
      </div>
      <div className={AURORA_STAT_PANEL}>
        <p className={AURORA_MUTED_LABEL}>Open</p>
        <p className={cn(AURORA_DISPLAY_NUMBER, 'mt-2 text-aurora-text-primary')}>
          {open.toLocaleString()}
        </p>
      </div>
      <div className={cn(AURORA_STAT_PANEL, 'flex flex-col gap-2')}>
        <p className={AURORA_MUTED_LABEL}>Status mix</p>
        <div className="flex flex-wrap gap-1.5">
          {ranked.length === 0 ? (
            <span className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>—</span>
          ) : (
            ranked.map((row) => (
              <span
                key={row.status}
                className={cn(
                  AURORA_BADGE_LABEL,
                  'rounded-full border border-aurora-border-strong bg-aurora-control-surface px-2 py-1 text-aurora-text-primary',
                )}
              >
                {row.status}
                <span className="ml-1.5 text-aurora-text-muted">{row.count}</span>
              </span>
            ))
          )}
        </div>
      </div>
    </div>
  )
}

function ViewToggle({
  value,
  onChange,
}: {
  value: IssueView
  onChange: (next: IssueView) => void
}) {
  return (
    <div
      className={cn(
        AURORA_MEDIUM_PANEL,
        'flex items-center gap-1 rounded-full px-1 py-1',
      )}
    >
      <ToggleButton active={value === 'ready'} onClick={() => onChange('ready')}>
        Ready
      </ToggleButton>
      <ToggleButton active={value === 'all'} onClick={() => onChange('all')}>
        All issues
      </ToggleButton>
    </div>
  )
}

function ToggleButton({
  active,
  onClick,
  children,
}: {
  active: boolean
  onClick: () => void
  children: React.ReactNode
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={active}
      className={cn(
        'rounded-full px-3 py-1 text-[13px] font-semibold transition-colors',
        active
          ? 'bg-aurora-accent-primary/20 text-aurora-text-primary'
          : 'text-aurora-text-muted hover:text-aurora-text-primary',
      )}
    >
      {children}
    </button>
  )
}

function BeadsErrorPanel({
  title,
  message,
  hint,
}: {
  title: string
  message: string
  hint?: string
}) {
  return (
    <div
      className={cn(
        AURORA_MEDIUM_PANEL,
        'flex items-start gap-3 border-aurora-warn/40 px-4 py-3',
      )}
      role="alert"
    >
      <AlertTriangle className="size-4 text-aurora-warn" aria-hidden />
      <div className="flex flex-col gap-1">
        <p className="text-sm font-semibold text-aurora-text-primary">{title}</p>
        <p className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>{message}</p>
        {hint ? (
          <p className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>{hint}</p>
        ) : null}
      </div>
    </div>
  )
}

function BeadsEmptyState() {
  return (
    <div className={cn(AURORA_MEDIUM_PANEL, 'flex flex-col gap-3 px-5 py-6 text-center')}>
      <CircleDashed className="mx-auto size-6 text-aurora-text-muted" aria-hidden />
      <h2 className="font-display text-[17px] font-extrabold leading-tight text-aurora-text-primary">
        No Beads projects found
      </h2>
      <p className="mx-auto max-w-md text-sm text-aurora-text-muted">
        Lab connected to the Dolt server, but no databases were visible. Initialize a
        Beads workspace there with{' '}
        <code className="rounded bg-aurora-control-surface px-1.5 py-0.5">bd init</code>,
        or check that the credentials in Settings have permission to{' '}
        <code className="rounded bg-aurora-control-surface px-1.5 py-0.5">SHOW DATABASES</code>.
      </p>
      <div className="flex justify-center">
        <Button asChild variant="outline" size="sm">
          <Link href="/settings/services/beads" className="gap-2">
            Open Beads settings
            <ExternalLink className="size-3.5" aria-hidden />
          </Link>
        </Button>
      </div>
    </div>
  )
}
