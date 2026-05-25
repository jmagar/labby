'use client'

import * as React from 'react'
import { ChevronDown, ChevronRight, Database, Plus, MoreHorizontal, Search, Sparkles, Circle, AlertCircle } from 'lucide-react'
import { cn } from '@/lib/utils'
import { AURORA_DENSE_META } from '@/components/aurora/tokens'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { Label } from '@/components/ui/label'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'
import { ScrollArea } from '@/components/ui/scroll-area'
import { formatTimeAgo } from './mock-data'
import { ConfirmDialog, type ConfirmState } from '@/components/marketplace/confirm-dialog'
import type { ACPProject, ACPRun, ACPRunStatus } from './types'

interface SessionSidebarProps {
  className?: string
  projects: ACPProject[]
  runs: ACPRun[]
  selectedRunId: string | null
  selectedProjectId: string | null
  onSelectRun: (runId: string, projectId: string) => void
  onNewRun: (projectId: string) => void
  /** Number of failed/closed/cancelled runs hidden from `runs`. */
  hiddenRunCount?: number
  /** Whether the toggle currently includes hidden runs in `runs`. */
  includeHiddenRuns?: boolean
  /** Called when the user clicks the show/hide toggle chip. */
  onToggleIncludeHidden?: () => void
  /** Called when the user confirms the bulk cleanup. Should resolve to closed/failed counts. */
  onBulkCloseHidden?: () => Promise<{ closedCount: number; failedCount: number }>
  /** modelId shared by more than half of the visible runs — used to suppress redundant badges. */
  dominantModelId?: string | null
}

function RunIcon({ status, agentId }: { status: ACPRunStatus; agentId: string }) {
  if (status === 'failed') {
    return <AlertCircle className="size-3.5 shrink-0 text-aurora-error" />
  }
  if (status === 'running') {
    return (
      <span className="relative flex size-3.5 shrink-0 items-center justify-center">
        <span className="absolute size-3.5 animate-ping rounded-full bg-aurora-accent-primary/30" />
        <Sparkles className="relative size-3 text-aurora-accent-primary" />
      </span>
    )
  }
  if (status === 'waiting_for_permission') {
    return <Circle className="size-3.5 shrink-0 animate-pulse text-aurora-warn" />
  }
  if (agentId === 'claude-3-7') {
    return <Sparkles className="size-3.5 shrink-0 text-aurora-text-muted/50" />
  }
  return <Circle className="size-3.5 shrink-0 text-aurora-text-muted/30" />
}

function RunRow({
  run,
  isSelected,
  onSelect,
  dominantModelId,
}: {
  run: ACPRun
  isSelected: boolean
  onSelect: () => void
  dominantModelId: string | null
}) {
  // Hide the per-row model badge when this run matches the dominant model
  // across the visible list — the badge is redundant noise in that case.
  const hideBadge =
    dominantModelId !== null && run.modelId !== null && run.modelId === dominantModelId
  // The screen-reader label always carries the model name, even when the
  // visual badge is suppressed.
  const ariaLabel = run.modelName ? `${run.title} · ${run.modelName}` : run.title
  return (
    <button
      type="button"
      onClick={onSelect}
      aria-label={ariaLabel}
      className={cn(
        'group/run relative flex w-full items-center gap-2 overflow-hidden rounded-aurora-1 px-2 py-1.5 text-left transition-colors',
        isSelected
          ? 'bg-aurora-panel-strong text-aurora-text-primary shadow-[var(--aurora-active-glow)]'
          : 'text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary',
      )}
    >
      {isSelected && (
        <span
          aria-hidden="true"
          className="absolute inset-y-0 left-0 w-0.5 rounded-r bg-aurora-accent-primary"
        />
      )}
      <RunIcon status={run.status} agentId={run.agentId} />
      <span className="min-w-0 flex-1">
        <span className="block truncate text-[13px] leading-[1.2]">{run.title}</span>
        {run.modelName && !hideBadge && (
          <span
            aria-hidden="true"
            className="block truncate text-[11px] leading-[1.2] text-aurora-text-muted/70"
          >
            {run.modelName}
          </span>
        )}
      </span>
      <span className={cn(AURORA_DENSE_META, 'shrink-0 tabular-nums text-aurora-text-muted')}>
        {formatTimeAgo(run.updatedAt)}
      </span>
    </button>
  )
}

function ProjectGroup({
  project,
  runs,
  selectedRunId,
  isActiveProject,
  onSelectRun,
  onNewRun,
  dominantModelId,
}: {
  project: ACPProject
  runs: ACPRun[]
  selectedRunId: string | null
  isActiveProject: boolean
  onSelectRun: (runId: string, projectId: string) => void
  onNewRun: (projectId: string) => void
  dominantModelId: string | null
}) {
  // Seed `open` from `project.collapsed` once per `project.id`; after mount the
  // local row toggle wins so parent prop churn does not clobber user intent.
  const [open, setOpen] = React.useState(!(project.collapsed ?? false))

  return (
    <Collapsible open={open} onOpenChange={setOpen}>
      <div className="group/proj flex items-center gap-1 px-1 py-0.5">
        <CollapsibleTrigger asChild>
          <button
            type="button"
            className={cn('flex min-w-0 flex-1 items-center gap-1.5 rounded px-1 py-1 text-left transition-colors hover:bg-aurora-hover-bg', isActiveProject && 'text-aurora-text-primary')}
          >
            {open ? (
              <ChevronDown className="size-3.5 shrink-0 text-aurora-text-muted/60" />
            ) : (
              <ChevronRight className="size-3.5 shrink-0 text-aurora-text-muted/60" />
            )}
            <span className="min-w-0 truncate text-[13px] font-medium text-aurora-text-primary">
              {project.name}
            </span>
          </button>
        </CollapsibleTrigger>

        <Database className="size-3.5 shrink-0 text-aurora-text-muted/40" />

        <div className="flex shrink-0 items-center gap-0.5 opacity-0 transition-opacity group-hover/proj:opacity-100 group-focus-within/proj:opacity-100">
          <TooltipProvider delayDuration={400}>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon"
                  aria-label={`Start a new session in ${project.name}`}
                  className="size-5 rounded text-aurora-text-muted/60 hover:bg-aurora-hover-bg hover:text-aurora-text-primary"
                  onClick={(e) => { e.stopPropagation(); onNewRun(project.id) }}
                >
                  <Plus className="size-3" />
                </Button>
              </TooltipTrigger>
              <TooltipContent side="top" className="text-xs">New session</TooltipContent>
            </Tooltip>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon"
                  aria-label={`Project options for ${project.name}`}
                  className="size-5 rounded text-aurora-text-muted/60 hover:bg-aurora-hover-bg hover:text-aurora-text-primary"
                  onClick={(e) => e.stopPropagation()}
                >
                  <MoreHorizontal className="size-3" />
                </Button>
              </TooltipTrigger>
              <TooltipContent side="top" className="text-xs">Project options</TooltipContent>
            </Tooltip>
          </TooltipProvider>
        </div>
      </div>

      <CollapsibleContent>
        <div className="ml-3 flex flex-col gap-0.5 border-l border-aurora-border-default/40 pl-3 pb-1">
          {runs.length === 0 ? (
            <p className="px-2 py-1.5 text-[12px] text-aurora-text-muted/50">No threads yet</p>
          ) : (
            runs.map((run) => (
              <RunRow
                key={run.id}
                run={run}
                isSelected={selectedRunId === run.id}
                onSelect={() => onSelectRun(run.id, project.id)}
                dominantModelId={dominantModelId}
              />
            ))
          )}
        </div>
      </CollapsibleContent>
    </Collapsible>
  )
}

export function SessionSidebar({
  className,
  projects,
  runs,
  selectedRunId,
  selectedProjectId,
  onSelectRun,
  onNewRun,
  hiddenRunCount = 0,
  includeHiddenRuns = false,
  onToggleIncludeHidden,
  onBulkCloseHidden,
  dominantModelId = null,
}: SessionSidebarProps) {
  const activeProjectId = selectedProjectId
  const [search, setSearch] = React.useState('')
  const deferredSearch = React.useDeferredValue(search)
  const [confirm, setConfirm] = React.useState<ConfirmState | null>(null)

  const handleCleanup = React.useCallback(() => {
    if (!onBulkCloseHidden || hiddenRunCount === 0) return
    setConfirm({
      title: `Delete ${hiddenRunCount} session${hiddenRunCount === 1 ? '' : 's'}?`,
      description:
        'Sessions in state failed or closed, last active more than 7 days ago, will be permanently removed. This action cannot be undone.',
      confirmLabel: `Delete ${hiddenRunCount} Session${hiddenRunCount === 1 ? '' : 's'}`,
      destructive: true,
      onConfirm: async () => {
        await onBulkCloseHidden()
      },
    })
  }, [hiddenRunCount, onBulkCloseHidden])

  const visibleProjects = React.useMemo(() => {
    const normalizedSearch = deferredSearch.trim().toLowerCase()
    const runsByProject = new Map<string, typeof runs>()
    for (const run of runs) {
      const projectRuns = runsByProject.get(run.projectId)
      if (projectRuns) {
        projectRuns.push(run)
      } else {
        runsByProject.set(run.projectId, [run])
      }
    }

    return projects.flatMap((project) => {
      const projectRuns = runsByProject.get(project.id) ?? []
      if (!normalizedSearch) {
        return [{ project, matchingRuns: projectRuns }]
      }

      const projectMatches = project.name.toLowerCase().includes(normalizedSearch)
      const matchingRuns = projectMatches
        ? projectRuns
        : projectRuns.filter((run) => run.title.toLowerCase().includes(normalizedSearch))

      return projectMatches || matchingRuns.length > 0
        ? [{ project, matchingRuns }]
        : []
    })
  }, [deferredSearch, projects, runs])

  return (
    <div className={cn('flex h-full w-full shrink-0 flex-col border-r border-aurora-border-default bg-aurora-nav-bg md:w-[272px]', className)}>
      {/* Search */}
      <div className="shrink-0 px-3 py-3">
        <div className="relative">
          <Label htmlFor="session-search" className="sr-only">
            Search sessions
          </Label>
          <Search className="absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-aurora-text-muted/50" />
          <Input
            id="session-search"
            placeholder="Search..."
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            className="h-8 rounded-aurora-1 border-aurora-border-default bg-aurora-control-surface pl-8 text-[13px] text-aurora-text-primary placeholder:text-aurora-text-muted/50 focus-visible:ring-aurora-accent-primary/30"
          />
        </div>
      </div>

      {/* Hidden-session controls */}
      {hiddenRunCount > 0 && (
        <div className="flex items-center justify-between gap-2 px-3 pb-2 text-[11px] text-aurora-text-muted">
          <button
            type="button"
            onClick={onToggleIncludeHidden}
            className="hover:text-aurora-text-primary transition-colors"
          >
            {includeHiddenRuns
              ? `Hide ${hiddenRunCount} closed/failed`
              : `Show ${hiddenRunCount} closed/failed`}
          </button>
          {onBulkCloseHidden && (
            <button
              type="button"
              onClick={handleCleanup}
              className="hover:text-aurora-error transition-colors"
            >
              Clean up
            </button>
          )}
        </div>
      )}

      {/* Project list */}
      <ScrollArea className="min-h-0 flex-1">
        <div className="flex flex-col gap-1 px-2 pb-4">
          {visibleProjects.map(({ project, matchingRuns }) => (
            <ProjectGroup
              key={project.id}
              project={project}
              runs={matchingRuns}
              selectedRunId={selectedRunId}
              isActiveProject={activeProjectId === project.id}
              onSelectRun={onSelectRun}
              onNewRun={onNewRun}
              dominantModelId={dominantModelId}
            />
          ))}
        </div>
      </ScrollArea>

      <ConfirmDialog
        state={confirm}
        onOpenChange={(open) => {
          if (!open) setConfirm(null)
        }}
      />
    </div>
  )
}
