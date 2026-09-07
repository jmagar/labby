import type { AuthorityCacheKey, AuthoritySnapshot } from './authority.ts'
import { authorityCacheKey } from './authority.ts'

const controllers = new Map<number, Set<AbortController>>()
const retainedGenerations: number[] = []
const MAX_RETAINED_GENERATIONS = 3

export function beginAuthorityRequest(snapshot: AuthoritySnapshot, contextGeneration: number, connectionId = 'local', callerSignal?: AbortSignal) {
  const controller = new AbortController()
  const generation = contextGeneration
  let bucket = controllers.get(generation)
  if (!bucket) { bucket = new Set(); controllers.set(generation, bucket) }
  bucket.add(controller)
  if (callerSignal) {
    if (callerSignal.aborted) controller.abort(callerSignal.reason)
    else callerSignal.addEventListener('abort', () => controller.abort(callerSignal.reason), { once: true })
  }
  return { generation, cacheKey: authorityCacheKey(snapshot, connectionId), signal: controller.signal, finish: () => bucket?.delete(controller) }
}

export function invalidateAuthorityRequests(currentGeneration: number) {
  for (const [generation, bucket] of controllers) {
    if (generation !== currentGeneration) {
      for (const controller of bucket) controller.abort(new DOMException('Authority context changed', 'AbortError'))
      controllers.delete(generation)
    }
  }
  if (!retainedGenerations.includes(currentGeneration)) retainedGenerations.push(currentGeneration)
  while (retainedGenerations.length > MAX_RETAINED_GENERATIONS) {
    const expired = retainedGenerations.shift()
    if (expired !== undefined) controllers.delete(expired)
  }
}

export function authorityResultIsCurrent(capturedGeneration: number, snapshot?: AuthoritySnapshot) {
  return snapshot?.generation === capturedGeneration
}

export function qualifyAuthorityCacheKey(key: string, authority: AuthorityCacheKey) {
  return [key, ...authority] as const
}

export function __resetAuthorityContextForTests() {
  for (const bucket of controllers.values()) for (const controller of bucket) controller.abort()
  controllers.clear()
  retainedGenerations.splice(0)
}

export function __authorityContextStatsForTests() {
  return { activeGenerations: controllers.size, retainedGenerations: retainedGenerations.length }
}
