'use client'

import * as React from 'react'
import Link from 'next/link'
import {
  AlertTriangle,
  CheckCircle2,
  Clock3,
  KeyRound,
  LinkIcon,
  ListTree,
  MessageSquare,
  Plug,
  Server,
  Settings,
  Smartphone,
  Store,
  Wrench,
  XCircle,
} from 'lucide-react'
import { AppHeader } from '@/components/app-header'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  AURORA_DISPLAY_1,
  AURORA_DISPLAY_NUMBER,
  AURORA_MEDIUM_PANEL,
  AURORA_MUTED_LABEL,
  AURORA_PAGE_FRAME,
  AURORA_PAGE_SHELL,
  AURORA_STRONG_PANEL,
} from '@/components/aurora/tokens'
import { fetchLogs } from '@/lib/api/logs-client'
import {
  ACTIVITY_SUBSYSTEMS,
  buildActivityItemsFromLogs,
  type ActivityItem,
} from '@/lib/dashboard/admin-insights'
import { useBrowserSession } from '@/lib/auth/session'
import { formatUiDateTime } from '@/lib/format-ui-time'
import { cn } from '@/lib/utils'

const toneStyles = {
  success: 'border-aurora-accent-strong/30 bg-aurora-accent-strong/10 text-aurora-accent-strong',
  info: 'border-aurora-accent-primary/30 bg-aurora-accent-primary/10 text-aurora-accent-primary',
  warning: 'border-aurora-warn/30 bg-aurora-warn/10 text-aurora-warn',
  danger: 'border-aurora-error/30 bg-aurora-error/10 text-aurora-error',
} as const

const kindIcons = {
  artifact: LinkIcon,
  device: Smartphone,
  gateway: Server,
  marketplace: Store,
  oauth: KeyRound,
  prompt: MessageSquare,
  resource: ListTree,
  session: Plug,
  settings: Settings,
  tool: Wrench,
  other: LinkIcon,
} as const

const toneFallbackIcons = {
  success: CheckCircle2,
  info: CheckCircle2,
  warning: AlertTriangle,
  danger: XCircle,
} as const

const ACTIVITY_LIMIT = 50
const POLL_INTERVAL_MS = 10_000

function useActivityFeed() {
  const [items, setItems] = React.useState<ActivityItem[]>([])
  const [loading, setLoading] = React.useState(true)
  const [error, setError] = React.useState<string | null>(null)
  const activeControllerRef = React.useRef<AbortController | null>(null)
  const latestRequestIdRef = React.useRef(0)

  const reload = React.useCallback(async () => {
    activeControllerRef.current?.abort()
    const controller = new AbortController()
    activeControllerRef.current = controller
    const requestId = ++latestRequestIdRef.current
    try {
      const result = await fetchLogs(
        {
          subsystems: [...ACTIVITY_SUBSYSTEMS],
          limit: ACTIVITY_LIMIT,
        },
        { signal: controller.signal },
      )
      if (controller.signal.aborted || requestId !== latestRequestIdRef.current) return
      setItems(buildActivityItemsFromLogs(result.events))
      setError(null)
    } catch (err) {
      if (controller.signal.aborted || requestId !== latestRequestIdRef.current) return
      setError(err instanceof Error ? err.message : 'Failed to load activity')
    } finally {
      if (!controller.signal.aborted && requestId === latestRequestIdRef.current) {
        setLoading(false)
      }
    }
  }, [])

  React.useEffect(() => {
    void reload()
    const interval = window.setInterval(() => {
      void reload()
    }, POLL_INTERVAL_MS)
    return () => {
      activeControllerRef.current?.abort()
      window.clearInterval(interval)
    }
  }, [reload])

  return { items, loading, error }
}

function eventSubject(item: ActivityItem): string | undefined {
  const fields = item.event.fields_json as Record<string, unknown> | undefined
  const subject = fields?.subject
  return typeof subject === 'string' && subject.length > 0 ? subject : undefined
}

export default function ActivityPage() {
  const { items: allItems, loading, error } = useActivityFeed()
  const session = useBrowserSession()
  const viewerSub = session.status === 'authenticated' ? session.user.sub : undefined
  const [mineOnly, setMineOnly] = React.useState(false)

  const items = React.useMemo(() => {
    if (!mineOnly || !viewerSub) return allItems
    return allItems.filter((item) => eventSubject(item) === viewerSub)
  }, [allItems, mineOnly, viewerSub])

  const oauthCount = items.filter((item) => item.kind === 'oauth').length
  const toolCount = items.filter((item) => item.kind === 'tool').length
  const issueCount = items.filter((item) => item.tone === 'warning' || item.tone === 'danger').length

  return (
    <>
      <AppHeader
        breadcrumbs={[{ label: 'Activity' }]}
        actions={(
          <div className="flex items-center gap-2">
            {viewerSub ? (
              <Button
                variant={mineOnly ? 'default' : 'outline'}
                size="sm"
                onClick={() => setMineOnly((value) => !value)}
              >
                {mineOnly ? 'My activity' : 'All users'}
              </Button>
            ) : null}
            <Button variant="outline" size="sm" asChild>
              <Link href="/logs">Open logs</Link>
            </Button>
          </div>
        )}
      />
      <div className={`${AURORA_PAGE_SHELL} flex-1`}>
        <div className={AURORA_PAGE_FRAME}>
          <div className={cn(AURORA_STRONG_PANEL, 'px-6 py-5')}>
            <p className={AURORA_MUTED_LABEL}>Control Plane</p>
            <h1 className={cn(AURORA_DISPLAY_1, 'mt-2 text-aurora-text-primary')}>Activity</h1>
            <p className="mt-2 text-sm text-aurora-text-muted">
              Live feed of server changes, service exposure, devices, chat sessions, artifacts, tools, resources, prompts, marketplace changes, settings, and OAuth flows from the log store.
              Need structured search?{' '}
              <Link href="/logs" className="font-medium text-aurora-accent-primary underline-offset-4 hover:underline">
                Open the log console
              </Link>
              .
            </p>
          </div>

          <div className="grid gap-3 sm:grid-cols-3">
            <div className={cn(AURORA_MEDIUM_PANEL, 'px-5 py-4')}>
              <p className={AURORA_MUTED_LABEL}>Events</p>
              <p className={cn(AURORA_DISPLAY_NUMBER, 'mt-2 text-aurora-text-primary')}>{items.length}</p>
              <p className="mt-1 text-sm text-aurora-text-muted">Most recent {ACTIVITY_LIMIT} user-visible events across the app.</p>
            </div>
            <div className={cn(AURORA_MEDIUM_PANEL, 'px-5 py-4')}>
              <p className={AURORA_MUTED_LABEL}>Tool calls</p>
              <p className={cn(AURORA_DISPLAY_NUMBER, 'mt-2 text-aurora-text-primary')}>{toolCount}</p>
              <p className="mt-1 text-sm text-aurora-text-muted">Tool invocations from MCP clients in this window.</p>
            </div>
            <div className={cn(AURORA_MEDIUM_PANEL, 'px-5 py-4')}>
              <p className={AURORA_MUTED_LABEL}>OAuth events</p>
              <p className={cn(AURORA_DISPLAY_NUMBER, 'mt-2 text-aurora-text-primary')}>{oauthCount}</p>
              <p className="mt-1 text-sm text-aurora-text-muted">Relay + upstream OAuth connects, callbacks, and clears.</p>
            </div>
          </div>

          <div className={cn(AURORA_STRONG_PANEL, 'overflow-hidden')}>
            {loading && items.length === 0 ? (
              <div className="space-y-3 p-6">
                {Array.from({ length: 4 }, (_, index) => (
                  <div key={index} className="h-24 animate-pulse rounded-[1rem] border border-aurora-border-strong bg-aurora-control-surface" />
                ))}
              </div>
            ) : error ? (
              <div className="p-6 text-sm text-aurora-error">
                Failed to load activity: {error}
              </div>
            ) : items.length === 0 ? (
              <div className="p-6 text-sm text-aurora-text-muted">
                {mineOnly && viewerSub
                  ? 'No activity for your account yet. Toggle "All users" to see other subjects, or trigger an MCP request while signed in.'
                  : 'No activity yet. Connect an MCP client or trigger an OAuth flow to populate this feed.'}
              </div>
            ) : (
              <div className="divide-y divide-aurora-border-strong/60">
                {issueCount > 0 ? (
                  <div className="px-6 py-3 text-xs text-aurora-text-muted">
                    {issueCount} event{issueCount === 1 ? '' : 's'} flagged warning or error — tone icons below highlight them.
                  </div>
                ) : null}
                {items.map((item) => {
                  const KindIcon = kindIcons[item.kind] ?? toneFallbackIcons[item.tone]
                  const logsHref = `/logs?request=${encodeURIComponent(item.event.request_id ?? '')}`

                  return (
                    <div
                      key={item.id}
                      className="flex flex-col gap-4 px-6 py-5 lg:flex-row lg:items-start lg:justify-between"
                    >
                      <div className="flex gap-3">
                        <div className={cn('mt-0.5 rounded-full border p-2', toneStyles[item.tone])}>
                          <KindIcon className="size-4" />
                        </div>
                        <div className="space-y-1">
                          <div className="flex flex-wrap items-center gap-2">
                            <p className="font-medium text-aurora-text-primary">{item.title}</p>
                            <Badge variant="outline">{item.kind}</Badge>
                            <Badge variant="outline">{item.event.subsystem}</Badge>
                            {item.event.level !== 'info' ? (
                              <Badge variant="outline">{item.event.level}</Badge>
                            ) : null}
                          </div>
                          <p className="text-sm text-aurora-text-muted">{item.detail}</p>
                          <div className="flex flex-wrap items-center gap-3 text-xs text-aurora-text-muted">
                            <span className="inline-flex items-center gap-1.5">
                              <Clock3 className="size-3.5" />
                              {formatUiDateTime(item.timestamp)}
                            </span>
                            {item.event.session_id ? <span>session {item.event.session_id.slice(0, 8)}</span> : null}
                            {item.event.request_id ? <span>req {item.event.request_id.slice(0, 8)}</span> : null}
                          </div>
                        </div>
                      </div>
                      {item.event.request_id ? (
                        <Button variant="outline" size="sm" asChild>
                          <Link href={logsHref}>View in logs</Link>
                        </Button>
                      ) : null}
                    </div>
                  )
                })}
              </div>
            )}
          </div>
        </div>
      </div>
    </>
  )
}
