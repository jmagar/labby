// TypeScript wrapper over the /v1/beads dispatch service.
//
// Beads is read-only: every action runs against the configured Dolt SQL server
// over the MySQL protocol and returns plain JSON. Each Dolt database on the
// server is treated as one "project" and is selected per request via
// `params.project`.

import { beadsActionUrl } from './gateway-config.ts'
import { performServiceAction, type ServiceActionError } from './service-action-client.ts'

export class BeadsApiError extends Error implements ServiceActionError {
  status: number
  code?: string

  constructor(message: string, status: number, code?: string) {
    super(message)
    this.name = 'BeadsApiError'
    this.status = status
    this.code = code
  }
}

async function beadsAction<T>(
  action: string,
  params: Record<string, unknown> = {},
  signal?: AbortSignal,
): Promise<T> {
  return performServiceAction<T, BeadsApiError>({
    action,
    params,
    signal,
    serviceLabel: 'Beads',
    url: beadsActionUrl(),
    createError: (message, status, code) => new BeadsApiError(message, status, code),
  })
}

export type BeadsStatus =
  | 'open'
  | 'in_progress'
  | 'blocked'
  | 'deferred'
  | 'closed'

export const BEADS_STATUS_VALUES: BeadsStatus[] = [
  'open',
  'in_progress',
  'blocked',
  'deferred',
  'closed',
]

export interface Project {
  name: string
}

export interface Issue {
  id: string
  title: string
  description: string
  status: string
  priority: number
  issue_type: string
  assignee?: string | null
  created_by?: string | null
  owner?: string | null
  external_ref?: string | null
  created_at?: string | null
  updated_at?: string | null
  closed_at?: string | null
  started_at?: string | null
  due_at?: string | null
  defer_until?: string | null
  labels: string[]
}

export interface Dependency {
  issue_id: string
  depends_on_id: string
  type: string
  created_at?: string | null
  created_by?: string | null
}

export interface Comment {
  id: string
  author: string
  text: string
  created_at?: string | null
}

export interface IssueDetail {
  issue: Issue
  blocked_by: Dependency[]
  blocks: Dependency[]
  parents: Dependency[]
  children: Dependency[]
  comments: Comment[]
}

export interface DependencyGraph {
  root: string
  nodes: Issue[]
  edges: Dependency[]
}

export interface StatusCount {
  status: string
  count: number
}

export interface StatusSummary {
  project: string
  total: number
  by_status: StatusCount[]
}

export interface BeadsContext {
  project: string
  total_issues: number
  open_issues: number
}

export interface DoltVersion {
  version: string
  dolt_version?: string | null
}

export interface BeadsHealth {
  reachable: boolean
  status: string
  version?: string | null
  default_project?: string | null
  message?: string | null
}

export const beadsApi = {
  health(signal?: AbortSignal) {
    return beadsAction<BeadsHealth>('health.status', {}, signal)
  },
  version(signal?: AbortSignal) {
    return beadsAction<DoltVersion>('version.get', {}, signal)
  },
  projects(signal?: AbortSignal) {
    return beadsAction<Project[]>('project.list', {}, signal)
  },
  context(project?: string, signal?: AbortSignal) {
    return beadsAction<BeadsContext>('context.get', project ? { project } : {}, signal)
  },
  statusSummary(project?: string, signal?: AbortSignal) {
    return beadsAction<StatusSummary>('status.summary', project ? { project } : {}, signal)
  },
  listIssues(
    project: string | undefined,
    status: BeadsStatus | undefined,
    limit: number | undefined,
    signal?: AbortSignal,
  ) {
    const params: Record<string, unknown> = {}
    if (project) params.project = project
    if (status) params.status = status
    if (limit) params.limit = limit
    return beadsAction<Issue[]>('issue.list', params, signal)
  },
  ready(project: string | undefined, limit: number | undefined, signal?: AbortSignal) {
    const params: Record<string, unknown> = {}
    if (project) params.project = project
    if (limit) params.limit = limit
    return beadsAction<Issue[]>('issue.ready', params, signal)
  },
  show(project: string | undefined, id: string, signal?: AbortSignal) {
    const params: Record<string, unknown> = { id }
    if (project) params.project = project
    return beadsAction<IssueDetail>('issue.show', params, signal)
  },
  graph(project: string | undefined, id: string, signal?: AbortSignal) {
    const params: Record<string, unknown> = { id }
    if (project) params.project = project
    return beadsAction<DependencyGraph>('graph.show', params, signal)
  },
}
