import {
  Cable,
  CirclePlus,
  Clock3,
  Container,
  Bot,
  Activity,
  Logs,
  Warehouse,
  ShieldCheck,
  LayoutDashboard,
  MonitorSmartphone,
  SearchCode,
  GitBranch,
  type LucideIcon,
} from 'lucide-react'

/**
 * Unified Labby + Depot information architecture. Depot owns Discover,
 * Create, and Library; Loadouts and Snippets are Library tabs rather than
 * parallel sidebar products. Workspace collects the agent-facing execution
 * surfaces. Logs and distributed request traces live in a dedicated
 * Observability section so operational investigation is visually distinct
 * from control-plane configuration.
 */

export type ConsoleNavItem = {
  /** Stable key — also the persistence identity for pinning and reordering. */
  id: string
  label: string
  href: string
  icon: LucideIcon
  /** Accelerator shown on hover and bound as ⌘/Ctrl+N. */
  kbd: string
  tooltip: string
  /** Sub-label rendered under the label while the item is the active route. */
  contextLine?: string
  /** Server-projected capability required to expose this destination. */
  capability?: string
}

export type ConsoleNavSection = {
  id: string
  label: string
  items: ConsoleNavItem[]
}

/** Raw item data, before the ⌘N accelerator is attached. */
type ConsoleNavItemSource = Omit<ConsoleNavItem, 'kbd' | 'tooltip'> & {
  /** Text appended after the label in the tooltip, e.g. "upstream MCP servers". */
  tooltipDetail?: string
}

type ConsoleNavSectionSource = {
  id: string
  label: string
  items: ConsoleNavItemSource[]
}

const CONSOLE_NAV_SOURCE: ConsoleNavSectionSource[] = [
  {
    id: 'Control Plane',
    label: 'Control Plane',
    items: [
      { id: 'Overview', label: 'Overview', href: '/', icon: LayoutDashboard, contextLine: '16 servers · 127 calls', capability: 'scope.read' },
      {
        id: 'Gateway',
        label: 'Gateway',
        href: '/gateways',
        icon: Cable,
        tooltipDetail: 'upstream MCP servers',
        capability: 'scope.manage',
      },
      {
        id: 'Labby',
        label: 'Labby',
        href: '/gateway/?id=labby',
        icon: Cable,
        tooltipDetail: 'Labby gateway server',
        capability: 'platform.manage',
      },
      {
        id: 'Browsers',
        label: 'Browsers',
        href: '/browsers',
        icon: MonitorSmartphone,
        tooltipDetail: 'paired WebMCP browser bridges',
        capability: 'platform.manage',
      },
    ],
  },
  {
    id: 'Catalog',
    label: 'Catalog',
    items: [
      {
        id: 'Tools',
        label: 'Tools',
        href: '/tools',
        icon: SearchCode,
        tooltipDetail: 'live Code Mode catalog',
        capability: 'scope.read',
      },
      {
        id: 'Activity',
        label: 'Activity',
        href: '/usage',
        icon: Activity,
        tooltipDetail: 'calls, latency, cost and throughput',
        capability: 'audit.read',
      },
    ],
  },
  {
    id: 'Observability',
    label: 'Observability',
    items: [
      {
        id: 'Logs',
        label: 'Logs',
        href: '/logs',
        icon: Logs,
        tooltipDetail: 'live control-plane and upstream events',
        capability: 'platform.manage',
      },
      {
        id: 'Traces',
        label: 'Traces',
        href: '/traces',
        icon: GitBranch,
        tooltipDetail: 'correlated request flows',
        capability: 'audit.read',
      },
    ],
  },
  {
    id: 'Depot',
    label: 'Depot',
    items: [
      {
        id: 'Discover',
        label: 'Discover',
        href: '/depot',
        icon: SearchCode,
        tooltipDetail: 'search the Depot Bazaar',
      },
      {
        id: 'Create',
        label: 'Create',
        href: '/create',
        icon: CirclePlus,
        tooltipDetail: 'author artifacts and bundles',
        capability: 'scope.create',
      },
      {
        id: 'Library',
        label: 'Library',
        href: '/library',
        icon: Warehouse,
        tooltipDetail: 'artifacts, loadouts and snippets',
        capability: 'scope.read',
      },
      {
        id: 'Administration',
        label: 'Administration',
        href: '/administration',
        icon: ShieldCheck,
        tooltipDetail: 'Depot authority and canonical operations',
      },
    ],
  },
  {
    id: 'Workspace',
    label: 'Workspace',
    items: [
      { id: 'Agents', label: 'Agents', href: '/agents', icon: Bot, capability: 'scope.operate' },
      { id: 'Tasks', label: 'Tasks', href: '/tasks', icon: Clock3, capability: 'scope.operate' },
      { id: 'Dev Containers', label: 'Dev Containers', href: '/dev-containers', icon: Container, capability: 'scope.operate' },
    ],
  },
]

// The ⌘/Ctrl+N handler in console-sidebar.tsx binds N to the Nth item of
// `consoleNavSections.flatMap(section => section.items)`, in section order.
// The accelerator shown here is derived from that same flattened position
// instead of being typed per item, so the two can never drift apart again —
// they previously did: Loadouts was inserted into Control Plane without
// renumbering anything after it, leaving Tools and Loadouts both labelled
// ⌘3, and Usage/Traces both labelled ⌘6, none of which matched what the
// handler actually bound.
let flatIndex = 0
export const consoleNavSections: ConsoleNavSection[] = CONSOLE_NAV_SOURCE.map((section) => ({
  id: section.id,
  label: section.label,
  items: section.items.map((item) => {
    flatIndex += 1
    const kbd = `⌘${flatIndex}`
    return {
      id: item.id,
      label: item.label,
      href: item.href,
      icon: item.icon,
      capability: item.capability,
      contextLine: item.contextLine,
      kbd,
      tooltip: item.tooltipDetail
        ? `${item.label} — ${kbd} · ${item.tooltipDetail}`
        : `${item.label} — ${kbd}`,
    }
  }),
}))

export const consoleNavItems: ConsoleNavItem[] = consoleNavSections.flatMap(
  (section) => section.items,
)

export function capabilityAwareNavSections(capabilities: readonly string[]): ConsoleNavSection[] {
  const allowed = new Set(capabilities)
  let flatIndex = 0
  return consoleNavSections
    .map((section) => ({ ...section, items: section.items.filter((item) => !item.capability || allowed.has(item.capability)) }))
    .filter((section) => section.items.length > 0)
    .map((section) => ({
      ...section,
      items: section.items.map((item) => {
        flatIndex += 1
        const kbd = `⌘${flatIndex}`
        return { ...item, kbd, tooltip: item.tooltip.replace(item.kbd, kbd) }
      }),
    }))
}

export function capabilityForPath(pathname: string): string | null | undefined {
  // The selected Labby gateway uses a query parameter, which usePathname does
  // not expose. Treat every gateway detail route as an installation-admin
  // surface; the backend remains the final authorization boundary.
  if (pathname === '/gateway' || pathname.startsWith('/gateway/')) return 'platform.manage'
  const item = consoleNavItems.find((candidate) => isNavItemActive(candidate.href, pathname))
  if (item) return item.capability ?? null
  if (pathname === '/skills' || pathname.startsWith('/skills/') || pathname === '/loadouts' || pathname === '/snippets') return 'scope.read'
  if (pathname === '/docs' || pathname === '/design-system' || pathname.startsWith('/settings')) return 'platform.manage'
  return undefined
}

/** Section id a given item belongs to — used by the pin affordance's label. */
export function sectionOf(itemId: string): string | undefined {
  return consoleNavSections.find((section) =>
    section.items.some((item) => item.id === itemId),
  )?.id
}

export function isNavItemActive(href: string, pathname: string): boolean {
  if (href === '/') return pathname === '/'
  if (href === '/library') return ['/library', '/loadouts', '/snippets'].some(
    (route) => pathname === route || pathname.startsWith(`${route}/`),
  )
  return pathname === href || pathname.startsWith(`${href}/`)
}
