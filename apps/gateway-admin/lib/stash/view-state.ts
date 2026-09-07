import type { StashFile, StashGrant } from './types'

export function mergeFiles(previous: StashFile[], incoming: StashFile[], append: boolean): StashFile[] {
  if (!append) return incoming
  const seen = new Set(previous.map(file => file.file_id))
  return [...previous, ...incoming.filter(file => !seen.has(file.file_id))]
}

export function acceptGeneration(current: number, response: number): boolean {
  return current === response
}

export function acceptGrantPage(currentGeneration: number, responseGeneration: number, currentFileId: string | undefined, responseFileId: string): boolean {
  return currentGeneration === responseGeneration && currentFileId === responseFileId
}

export function acceptRecipientSearch(currentGeneration: number, responseGeneration: number, currentFileId: string | undefined, responseFileId: string, currentQuery: string, responseQuery: string): boolean {
  return currentGeneration === responseGeneration && currentFileId === responseFileId && currentQuery.trim() === responseQuery
}

export function selectedRecipientId(candidates: Array<{ principal_id: string }>, principalId: string): string | undefined {
  return candidates.some(candidate => candidate.principal_id === principalId) ? principalId : undefined
}

export async function copyUri(write: (value: string) => Promise<void>, uri: string): Promise<{ ok: true; announcement: string } | { ok: false; error: unknown }> {
  try { await write(uri); return { ok: true, announcement: `Copied ${uri}` } }
  catch (error) { return { ok: false, error } }
}

export function mergeGrants(previous: StashGrant[], incoming: StashGrant[], append: boolean): StashGrant[] {
  if (!append) return incoming
  const seen = new Set(previous.map(grant => grant.grant_id))
  return [...previous, ...incoming.filter(grant => !seen.has(grant.grant_id))]
}
