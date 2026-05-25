import type { ACPModelOption } from '@/components/chat/types'

export type Effort = 'low' | 'medium' | 'high' | 'xhigh'

const EFFORT_ORDER: readonly Effort[] = ['low', 'medium', 'high', 'xhigh']

export function parseModelId(id: string): { base: string; effort: Effort } | null {
  const normalized = id.replace(/\s*\(([^)]+)\)\s*$/, ' $1')
  const match = /^(.+?)[\s/]\s*(low|medium|high|xhigh)$/i.exec(normalized)
  if (!match) return null
  const base = match[1].trim()
  const effort = match[2].toLowerCase() as Effort
  if (base.includes('/')) return null
  return { base, effort }
}

export interface GroupedOption {
  base: string
  variants: Array<{ effort: Effort; option: ACPModelOption }>
}

export type GroupingResult =
  | { kind: 'flat'; options: ACPModelOption[] }
  | { kind: 'grouped'; groups: GroupedOption[] }

export function groupModels(options: ACPModelOption[]): GroupingResult {
  if (options.length <= 1) return { kind: 'flat', options }

  const parsed = options.map((option) => ({ option, parsed: parseModelId(option.id) }))
  if (parsed.some((p) => p.parsed === null)) {
    return { kind: 'flat', options }
  }

  const groupMap = new Map<string, GroupedOption>()
  for (const { option, parsed: p } of parsed) {
    if (!p) continue
    const existing = groupMap.get(p.base) ?? { base: p.base, variants: [] }
    existing.variants.push({ effort: p.effort, option })
    groupMap.set(p.base, existing)
  }

  for (const group of groupMap.values()) {
    group.variants.sort(
      (a, b) => EFFORT_ORDER.indexOf(a.effort) - EFFORT_ORDER.indexOf(b.effort),
    )
  }

  return { kind: 'grouped', groups: Array.from(groupMap.values()) }
}
