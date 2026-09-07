export const AUTHORITY_SCHEMA_VERSION = 1 as const
export const AUTHORITY_COMPATIBILITY_GENERATION = 1 as const
const opaqueValues = new Map<string, string>()
let opaqueCounter = 0
const MAX_OPAQUE_VALUES = 256

export type AuthorityOwner =
  | { kind: 'installation'; id: string }
  | { kind: 'team'; id: string }
  | { kind: 'project'; id: string }
  | { kind: 'personal'; id: string }

export type AuthorityTeam = { id: string; role: string; membershipEpoch: number; policyEpoch: number }
export type AuthorityProject = { id: string; role: string }

export type AuthoritySnapshot = {
  schemaVersion: typeof AUTHORITY_SCHEMA_VERSION
  compatibilityGeneration: typeof AUTHORITY_COMPATIBILITY_GENERATION
  principalId: string
  organizationId: string
  activeOwner: AuthorityOwner
  activeTeamId?: string
  activeProjectId?: string
  teams: readonly AuthorityTeam[]
  projects: readonly AuthorityProject[]
  capabilities: readonly string[]
  generation: number
}

export type AuthorityCacheKey = readonly [
  'authority',
  number,
  number,
  string,
  AuthorityOwner['kind'],
  string,
  string,
]

export class MalformedAuthorityResponseError extends Error {
  constructor(message: string) {
    super(`Malformed authority response: ${message}`)
    this.name = 'MalformedAuthorityResponseError'
  }
}

export function authorityCacheKey(snapshot: AuthoritySnapshot, connectionId = 'local'): AuthorityCacheKey {
  return ['authority', snapshot.compatibilityGeneration, snapshot.generation, opaque(snapshot.principalId), snapshot.activeOwner.kind, opaque(snapshot.activeOwner.id), opaque(connectionId)]
}

export function selectAuthorityWorkspace(
  snapshot: AuthoritySnapshot,
  selection: { teamId?: string | null; projectId?: string | null },
): AuthoritySnapshot {
  const teamId = clean(selection.teamId)
  const projectId = clean(selection.projectId)
  if (teamId && !snapshot.teams.some((team) => team.id === teamId)) throw new Error('Selected team is not available')
  if (projectId && !snapshot.projects.some((project) => project.id === projectId)) throw new Error('Selected project is not available')
  if (projectId) return { ...snapshot, activeTeamId: teamId, activeProjectId: projectId, activeOwner: { kind: 'project', id: projectId } }
  if (teamId) return { ...snapshot, activeTeamId: teamId, activeProjectId: undefined, activeOwner: { kind: 'team', id: teamId } }
  return { ...snapshot, activeTeamId: undefined, activeProjectId: undefined, activeOwner: { kind: 'personal', id: snapshot.principalId } }
}

export function parseAuthoritySnapshot(payload: Record<string, unknown>): AuthoritySnapshot {
  const owner = parseOwner(payload.active_owner ?? payload.owner)
  const principalId = requiredString(payload.principal_id ?? (owner?.kind === 'personal' ? owner.id : undefined), 'principal_id')
  const organizationId = requiredString(payload.organization_id, 'organization_id')
  const generation = nonNegativeInteger(payload.authority_generation, 'authority_generation')
  const teams = parseTeams(payload.teams)
  const projects = parseProjects(payload.projects)
  const capabilities = stringArray(payload.capabilities, 'capabilities')
  const activeProjectId = optionalString(payload.active_project_id ?? payload.project_id ?? payload.project)
  const activeTeamId = optionalString(payload.active_team_id)
  if (activeTeamId && !teams.some((team) => team.id === activeTeamId)) throw malformed('active_team_id is unavailable')
  if (activeProjectId && !projects.some((project) => project.id === activeProjectId)) throw malformed('active_project_id is unavailable')

  return {
    schemaVersion: AUTHORITY_SCHEMA_VERSION,
    compatibilityGeneration: AUTHORITY_COMPATIBILITY_GENERATION,
    principalId,
    organizationId,
    activeOwner: owner ?? { kind: 'personal', id: principalId },
    activeTeamId,
    activeProjectId,
    teams,
    projects,
    capabilities: [...new Set(capabilities)].sort(),
    generation,
  }
}

function parseOwner(value: unknown): AuthorityOwner | undefined {
  if (value == null) return undefined
  if (!isObject(value)) throw malformed('owner must be an object')
  const kind = value.kind
  const id = requiredString(value.id, 'owner.id')
  if (kind !== 'installation' && kind !== 'team' && kind !== 'project' && kind !== 'personal') throw malformed('owner.kind is unsupported')
  return { kind, id }
}

function parseTeams(value: unknown): AuthorityTeam[] {
  if (!Array.isArray(value)) throw malformed('teams must be an array')
  return value.map((item) => {
    if (!isObject(item)) throw malformed('team must be an object')
    return { id: requiredString(item.id, 'team.id'), role: requiredString(item.role, 'team.role'), membershipEpoch: nonNegativeInteger(item.membership_epoch, 'team.membership_epoch'), policyEpoch: nonNegativeInteger(item.policy_epoch, 'team.policy_epoch') }
  })
}

function parseProjects(value: unknown): AuthorityProject[] {
  if (!Array.isArray(value)) throw malformed('projects must be an array')
  return value.map((item) => {
    if (!isObject(item)) throw malformed('project must be an object')
    return { id: requiredString(item.id, 'project.id'), role: requiredString(item.role, 'project.role') }
  })
}

function stringArray(value: unknown, field: string) {
  if (!Array.isArray(value) || value.some((item) => clean(item) === undefined)) throw malformed(`${field} must contain non-empty strings`)
  return value as string[]
}
function requiredString(value: unknown, field: string) { const result = clean(value); if (!result) throw malformed(`${field} is required`); return result }
function optionalString(value: unknown) { if (value == null) return undefined; const result = clean(value); if (!result) throw malformed('selector must be a non-empty string'); return result }
function nonNegativeInteger(value: unknown, field: string) { if (!Number.isSafeInteger(value) || Number(value) < 0) throw malformed(`${field} must be a non-negative integer`); return Number(value) }
function clean(value: unknown) { return typeof value === 'string' && value.trim() ? value.trim() : undefined }
function isObject(value: unknown): value is Record<string, unknown> { return typeof value === 'object' && value !== null && !Array.isArray(value) }
function malformed(message: string) { return new MalformedAuthorityResponseError(message) }
function opaque(value: string) {
  const existing = opaqueValues.get(value)
  if (existing) return existing
  const token = globalThis.crypto?.randomUUID?.() ?? `context-${++opaqueCounter}`
  opaqueValues.set(value, token)
  if (opaqueValues.size > MAX_OPAQUE_VALUES) opaqueValues.delete(opaqueValues.keys().next().value!)
  return token
}
