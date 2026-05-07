'use client'

import useSWR from 'swr'
import {
  beadsApi,
  type BeadsContext,
  type BeadsStatus,
  type DependencyGraph,
  type Issue,
  type IssueDetail,
  type Project,
  type StatusSummary,
} from '../api/beads-client'

const KEY_PROJECTS = 'beads:projects'
const KEY_CONTEXT = (project: string) => ['beads:context', project] as const
const KEY_SUMMARY = (project: string) => ['beads:status-summary', project] as const
const KEY_ISSUES = (
  project: string,
  view: 'all' | 'ready',
  status: BeadsStatus | 'any',
  limit: number,
) => ['beads:issues', project, view, status, limit] as const
const KEY_DETAIL = (project: string, id: string) => ['beads:issue-detail', project, id] as const
const KEY_GRAPH = (project: string, id: string) => ['beads:issue-graph', project, id] as const

const POLL_INTERVAL_MS = 30_000

export function useBeadsProjects() {
  return useSWR<Project[]>(KEY_PROJECTS, () => beadsApi.projects(), {
    revalidateOnFocus: false,
    fallbackData: [],
  })
}

export function useBeadsContext(project: string | undefined) {
  return useSWR<BeadsContext | null>(
    project ? KEY_CONTEXT(project) : null,
    () => (project ? beadsApi.context(project) : Promise.resolve(null)),
    { revalidateOnFocus: false, refreshInterval: POLL_INTERVAL_MS },
  )
}

export function useBeadsStatusSummary(project: string | undefined) {
  return useSWR<StatusSummary | null>(
    project ? KEY_SUMMARY(project) : null,
    () => (project ? beadsApi.statusSummary(project) : Promise.resolve(null)),
    { revalidateOnFocus: false, refreshInterval: POLL_INTERVAL_MS },
  )
}

export function useBeadsIssues(
  project: string | undefined,
  view: 'all' | 'ready',
  status: BeadsStatus | 'any',
  limit: number = 100,
) {
  return useSWR<Issue[]>(
    project ? KEY_ISSUES(project, view, status, limit) : null,
    () => {
      if (!project) return Promise.resolve([])
      if (view === 'ready') {
        return beadsApi.ready(project, limit)
      }
      const filter = status === 'any' ? undefined : status
      return beadsApi.listIssues(project, filter, limit)
    },
    {
      revalidateOnFocus: false,
      refreshInterval: POLL_INTERVAL_MS,
      keepPreviousData: true,
    },
  )
}

export function useBeadsIssueDetail(project: string | undefined, id: string | undefined) {
  return useSWR<IssueDetail | null>(
    project && id ? KEY_DETAIL(project, id) : null,
    () => (project && id ? beadsApi.show(project, id) : Promise.resolve(null)),
    { revalidateOnFocus: false },
  )
}

export function useBeadsIssueGraph(project: string | undefined, id: string | undefined) {
  return useSWR<DependencyGraph | null>(
    project && id ? KEY_GRAPH(project, id) : null,
    () => (project && id ? beadsApi.graph(project, id) : Promise.resolve(null)),
    { revalidateOnFocus: false },
  )
}
