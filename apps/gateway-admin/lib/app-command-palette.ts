export type AppCommandKind = 'destination' | 'action'

export type AppCommandGroupKey = 'best-match' | 'actions' | 'destinations'

export type AppCommandIconKey =
  | 'activity'
  | 'chat'
  | 'docs'
  | 'gateway'
  | 'logs'
  | 'marketplace'
  | 'overview'
  | 'settings'
  | 'setup'

export type AppCommandItem = {
  id: string
  kind: AppCommandKind
  title: string
  description: string
  keywords: string[]
  group: AppCommandGroupKey
  icon: AppCommandIconKey
  href: string
  actionHint: string
  priority: number
}

export type AppCommandGroup = {
  key: AppCommandGroupKey
  label: string
  items: AppCommandItem[]
}

export type AppCommandState = {
  items: AppCommandItem[]
  groups: AppCommandGroup[]
  activeItemId: string | null
}

const GROUP_LABELS: Record<AppCommandGroupKey, string> = {
  'best-match': 'Best match',
  actions: 'Actions',
  destinations: 'Destinations',
}

export const appCommandItems: AppCommandItem[] = [
  {
    id: 'destination-overview',
    kind: 'destination',
    title: 'Overview',
    description: 'Open the Labby dashboard with server health, activity, and quick actions.',
    keywords: ['home', 'dashboard', 'overview', 'summary'],
    group: 'destinations',
    icon: 'overview',
    href: '/',
    actionHint: 'Open',
    priority: 100,
  },
  {
    id: 'destination-gateways',
    kind: 'destination',
    title: 'Servers',
    description: 'Manage upstream servers, policies, and runtime exposure.',
    keywords: ['server', 'servers', 'gateway', 'gateways', 'routes', 'upstream', 'policy'],
    group: 'destinations',
    icon: 'gateway',
    href: '/gateways',
    actionHint: 'Open',
    priority: 98,
  },
  {
    id: 'destination-marketplace',
    kind: 'destination',
    title: 'Marketplace',
    description: 'Browse available plugins, MCP servers, ACP agents, and registry-backed catalog entries.',
    keywords: ['marketplace', 'plugin', 'plugins', 'install', 'agents', 'mcp', 'registry', 'servers', 'catalog', 'packages', 'acp'],
    group: 'destinations',
    icon: 'marketplace',
    href: '/marketplace',
    actionHint: 'Open',
    priority: 92,
  },
  {
    id: 'destination-chat',
    kind: 'destination',
    title: 'Chat',
    description: 'Open the ACP chat workspace for agent sessions and tool activity.',
    keywords: ['chat', 'agent', 'assistant', 'acp', 'session'],
    group: 'destinations',
    icon: 'chat',
    href: '/chat',
    actionHint: 'Open',
    priority: 88,
  },
  {
    id: 'destination-setup',
    kind: 'destination',
    title: 'Setup',
    description: 'Run environment discovery, scan local services, and review setup results.',
    keywords: ['setup', 'onboarding', 'doctor', 'extract', 'environment', 'env'],
    group: 'destinations',
    icon: 'setup',
    href: '/setup',
    actionHint: 'Open',
    priority: 86,
  },
  {
    id: 'destination-activity',
    kind: 'destination',
    title: 'Activity',
    description: 'Review recent server events, jobs, and operator activity.',
    keywords: ['activity', 'events', 'jobs', 'review', 'history'],
    group: 'destinations',
    icon: 'activity',
    href: '/activity',
    actionHint: 'Open',
    priority: 84,
  },
  {
    id: 'destination-logs',
    kind: 'destination',
    title: 'Logs',
    description: 'Open the operational log stream with filtering and event inspection.',
    keywords: ['logs', 'log', 'tail', 'events', 'observability', 'errors'],
    group: 'destinations',
    icon: 'logs',
    href: '/logs',
    actionHint: 'Open',
    priority: 82,
  },
  {
    id: 'destination-settings',
    kind: 'destination',
    title: 'Settings',
    description: 'Review auth mode, environment configuration, and control-plane defaults.',
    keywords: ['settings', 'config', 'configuration', 'auth', 'preferences'],
    group: 'destinations',
    icon: 'settings',
    href: '/settings',
    actionHint: 'Open',
    priority: 80,
  },
  {
    id: 'destination-docs',
    kind: 'destination',
    title: 'Documentation',
    description: 'Read Labby docs, setup guidance, conventions, and operator references.',
    keywords: ['docs', 'documentation', 'help', 'reference', 'guide'],
    group: 'destinations',
    icon: 'docs',
    href: '/docs',
    actionHint: 'Open',
    priority: 78,
  },
  {
    id: 'action-tail-logs',
    kind: 'action',
    title: 'Tail logs',
    description: 'Jump directly to the log console to continue watching runtime events.',
    keywords: ['tail', 'logs', 'stream', 'follow', 'errors'],
    group: 'actions',
    icon: 'logs',
    href: '/logs',
    actionHint: 'Run',
    priority: 89,
  },
  {
    id: 'action-review-gateways',
    kind: 'action',
    title: 'Review servers',
    description: 'Open server management to inspect upstreams and exposure state.',
    keywords: ['review', 'server', 'servers', 'gateway', 'gateways', 'health', 'runtime'],
    group: 'actions',
    icon: 'gateway',
    href: '/gateways',
    actionHint: 'Run',
    priority: 87,
  },
  {
    id: 'action-check-setup',
    kind: 'action',
    title: 'Check setup',
    description: 'Open setup validation and environment discovery results.',
    keywords: ['check', 'setup', 'doctor', 'validate', 'env'],
    group: 'actions',
    icon: 'setup',
    href: '/setup',
    actionHint: 'Run',
    priority: 85,
  },
]

function normalize(value: string): string {
  return value.trim().toLowerCase()
}

function scoreItem(item: AppCommandItem, query: string): { baseScore: number; totalScore: number } {
  if (!query) {
    return { baseScore: 0, totalScore: item.priority }
  }

  const normalizedTitle = item.title.toLowerCase()
  const normalizedDescription = item.description.toLowerCase()
  let baseScore = 0
  let matched = false

  if (normalizedTitle === query) {
    baseScore += 220
    matched = true
  }
  if (normalizedTitle.startsWith(query)) {
    baseScore += 130
    matched = true
  }
  if (normalizedTitle.includes(query)) {
    baseScore += 80
    matched = true
  }
  if (normalizedDescription.includes(query)) {
    baseScore += 20
    matched = true
  }

  for (const keyword of item.keywords) {
    const normalizedKeyword = keyword.toLowerCase()
    if (normalizedKeyword === query) {
      baseScore += 100
      matched = true
    } else if (normalizedKeyword.startsWith(query)) {
      baseScore += 58
      matched = true
    } else if (normalizedKeyword.includes(query)) {
      baseScore += 32
      matched = true
    }
  }

  if (!matched) return { baseScore: 0, totalScore: 0 }

  let totalScore = baseScore + item.priority
  if (item.kind === 'destination') totalScore += 6
  if (item.kind === 'action') totalScore += 3

  return { baseScore, totalScore }
}

function filterItems(query: string, items: AppCommandItem[]): AppCommandItem[] {
  const normalizedQuery = normalize(query)
  if (!normalizedQuery) {
    return [...items].sort((a, b) => b.priority - a.priority)
  }

  return [...items]
    .map((item) => ({ item, ...scoreItem(item, normalizedQuery) }))
    .filter(({ baseScore }) => baseScore > 40)
    .sort((a, b) => b.totalScore - a.totalScore)
    .map(({ item }) => item)
}

export function buildAppCommandState(
  query: string,
  items: AppCommandItem[] = appCommandItems,
): AppCommandState {
  const ranked = filterItems(query, items)
  if (!ranked.length) {
    return {
      items: [],
      groups: [],
      activeItemId: null,
    }
  }

  const [bestMatch, ...rest] = ranked
  const grouped = new Map<AppCommandGroupKey, AppCommandItem[]>([
    ['best-match', [bestMatch]],
    ['actions', []],
    ['destinations', []],
  ])

  for (const item of rest) {
    grouped.get(item.group)?.push(item)
  }

  const groups = [...grouped.entries()]
    .filter(([, groupItems]) => groupItems.length > 0)
    .map(([key, groupItems]) => ({
      key,
      label: GROUP_LABELS[key],
      items: groupItems,
    }))

  return {
    items: ranked,
    groups,
    activeItemId: bestMatch.id,
  }
}

export function findAppCommandItemById(
  itemId: string | null,
  items: AppCommandItem[],
): AppCommandItem | null {
  if (!itemId) return null
  return items.find((item) => item.id === itemId) ?? null
}
