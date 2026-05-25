import type { ACPRun, ACPRunStatus } from '@/components/chat/types'

const HIDDEN_STATES: ReadonlySet<ACPRunStatus> = new Set(['failed', 'closed'])

export function isHiddenState(status: ACPRunStatus | undefined): boolean {
  return status !== undefined && HIDDEN_STATES.has(status)
}

export function filterVisibleRuns(
  runs: ACPRun[],
  options: { includeHidden: boolean },
): ACPRun[] {
  if (options.includeHidden) return runs
  return runs.filter((run) => !isHiddenState(run.status))
}
