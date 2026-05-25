import * as React from 'react'

const NAV_KEYS = new Set(['ArrowDown', 'ArrowUp', 'Home', 'End'])

/**
 * Pure navigation reducer for keyboard list pickers. Returns the next index
 * for a given key press, or null if the key is not a navigation key. Wraps
 * at both ends. Returns null when count is 0.
 *
 * Callers wire this up alongside DOM focus management (e.g., focusing the
 * underlying option button) — keeping that side-effect outside the helper
 * keeps the helper deterministic and unit-testable.
 */
export function nextNavIndex(current: number, key: string, count: number): number | null {
  if (count <= 0 || !NAV_KEYS.has(key)) return null
  switch (key) {
    case 'ArrowDown':
      return (current + 1) % count
    case 'ArrowUp':
      return (current - 1 + count) % count
    case 'Home':
      return 0
    case 'End':
      return count - 1
    default:
      return null
  }
}

export interface ListKeyboardHandle {
  activeIndex: number
  setActiveIndex: React.Dispatch<React.SetStateAction<number>>
}

/**
 * Shared state shape for keyboard-navigable lists. Tracks an active index and
 * resets to 0 when `count` shrinks past the current index (e.g., the list
 * collapses on a provider switch). Pair with `nextNavIndex` to react to keys.
 */
export function useListKeyboard({
  count,
  initialIndex = 0,
}: {
  count: number
  initialIndex?: number
}): ListKeyboardHandle {
  const [activeIndex, setActiveIndex] = React.useState(initialIndex)

  React.useEffect(() => {
    if (count > 0 && activeIndex >= count) setActiveIndex(0)
  }, [count, activeIndex])

  return { activeIndex, setActiveIndex }
}
