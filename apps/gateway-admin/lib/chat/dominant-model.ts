import type { ACPRun } from '@/components/chat/types'

/**
 * Returns the modelId that exceeds strict majority (> floor(n/2)) across the
 * given runs, or null when no such modelId exists, the list is empty, or the
 * dominant value is null.
 *
 * Single-run lists return that run's modelId — consumers should still render
 * the badge for a lone row since semantic dominance is undefined at N=1.
 */
export function dominantModelId(runs: ACPRun[]): string | null {
  if (runs.length === 0) return null
  if (runs.length === 1) return runs[0]?.modelId ?? null

  const counts = new Map<string | null, number>()
  for (const run of runs) {
    const key = run.modelId ?? null
    counts.set(key, (counts.get(key) ?? 0) + 1)
  }

  const threshold = Math.floor(runs.length / 2) + 1
  for (const [id, count] of counts) {
    if (count >= threshold) return id
  }
  return null
}
