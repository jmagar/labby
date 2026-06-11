'use client'

import { Suspense, useMemo, useState } from 'react'
import { useSearchParams } from 'next/navigation'
import Link from 'next/link'
import { ChevronLeft, ChevronRight, Search } from 'lucide-react'
import { AppHeader } from '@/components/app-header'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { Skeleton } from '@/components/ui/skeleton'
import { WindowSelector } from '@/components/dashboard/window-selector'
import { OutcomeDot, SurfaceTag } from '@/components/dashboard/recent-calls'
import { DASH_SURFACE } from '@/components/dashboard/ui'
import { useToolCalls } from '@/lib/hooks/use-usage-drilldown'
import {
  WINDOW_LABELS,
  formatCompactNumber,
  formatDuration,
  formatRelativeTime,
} from '@/lib/dashboard/dashboard-metrics'
import { METRICS_WINDOWS, type CallOutcome, type MetricsWindow } from '@/lib/types/metrics'
import {
  AURORA_DISPLAY_1,
  AURORA_MUTED_LABEL,
  AURORA_PAGE_FRAME,
  AURORA_PAGE_SHELL,
  AURORA_STRONG_PANEL,
} from '@/components/aurora/tokens'
import { cn } from '@/lib/utils'

const PAGE_SIZE = 50
const ALL = 'all'

function isWindow(value: string | null): value is MetricsWindow {
  return value !== null && (METRICS_WINDOWS as readonly string[]).includes(value)
}

function UsageExplorer() {
  const params = useSearchParams()
  const initialWindow = isWindow(params.get('window')) ? (params.get('window') as MetricsWindow) : '24h'

  const [window, setWindow] = useState<MetricsWindow>(initialWindow)
  const [tool, setTool] = useState<string>(params.get('tool') ?? ALL)
  const [agent, setAgent] = useState<string>(params.get('agent') ?? ALL)
  const [ip, setIp] = useState<string>(params.get('ip') ?? ALL)
  const [outcome, setOutcome] = useState<string>(ALL)
  const [search, setSearch] = useState('')
  const [offset, setOffset] = useState(0)

  // Unfiltered fetch → distinct option lists for the dropdowns.
  const { data: all } = useToolCalls({ window, limit: 5000 })
  const toolOptions = useMemo(
    () => [...new Set((all?.calls ?? []).map((c) => c.tool))].sort(),
    [all],
  )
  const agentOptions = useMemo(() => {
    const map = new Map<string, string>()
    for (const c of all?.calls ?? []) map.set(c.agent_id, c.agent_label)
    return [...map.entries()].sort((a, b) => a[1].localeCompare(b[1]))
  }, [all])
  const ipOptions = useMemo(
    () => [...new Set((all?.calls ?? []).map((c) => c.ip))].sort(),
    [all],
  )

  const { data, isLoading, error, mutate } = useToolCalls({
    window,
    tool: tool === ALL ? undefined : tool,
    agent: agent === ALL ? undefined : agent,
    ip: ip === ALL ? undefined : ip,
    outcome: outcome === ALL ? undefined : (outcome as CallOutcome),
    search: search.trim() || undefined,
    limit: PAGE_SIZE,
    offset,
  })

  const resetPaging = () => setOffset(0)
  const filtered = data?.filtered ?? 0
  const showingFrom = filtered === 0 ? 0 : offset + 1
  const showingTo = Math.min(offset + PAGE_SIZE, filtered)

  return (
    <>
      <AppHeader
        breadcrumbs={[{ label: 'Overview', href: '/' }, { label: 'Usage explorer' }]}
        actions={<WindowSelector value={window} onChange={(w) => { setWindow(w); resetPaging() }} />}
      />

      <div className={cn(AURORA_PAGE_FRAME, AURORA_PAGE_SHELL)}>
        <div className={cn(AURORA_STRONG_PANEL, DASH_SURFACE, 'px-6 py-5')}>
          <p className={AURORA_MUTED_LABEL}>Tool-call explorer</p>
          <h1 className={cn(AURORA_DISPLAY_1, 'mt-2 text-aurora-text-primary')}>Usage explorer</h1>
          <p className="mt-2 text-sm text-aurora-text-muted">
            Every dispatched tool call in the window — filter by tool, agent, outcome, or text.{' '}
            {WINDOW_LABELS[window]}.
          </p>
        </div>

        {/* Filters */}
        <div className="flex flex-col gap-3 sm:flex-row sm:flex-wrap sm:items-center">
          <div className="relative flex-1 sm:min-w-[220px]">
            <Search className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-aurora-text-muted" />
            <Input
              value={search}
              onChange={(e) => { setSearch(e.target.value); resetPaging() }}
              placeholder="Search tool, action, agent, error…"
              className="pl-9"
            />
          </div>
          <Select value={tool} onValueChange={(v) => { setTool(v); resetPaging() }}>
            <SelectTrigger className="sm:w-40"><SelectValue placeholder="Tool" /></SelectTrigger>
            <SelectContent>
              <SelectItem value={ALL}>All tools</SelectItem>
              {toolOptions.map((name) => (
                <SelectItem key={name} value={name}>{name}</SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Select value={agent} onValueChange={(v) => { setAgent(v); resetPaging() }}>
            <SelectTrigger className="sm:w-44"><SelectValue placeholder="Agent" /></SelectTrigger>
            <SelectContent>
              <SelectItem value={ALL}>All agents</SelectItem>
              {agentOptions.map(([id, label]) => (
                <SelectItem key={id} value={id}>{label}</SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Select value={ip} onValueChange={(v) => { setIp(v); resetPaging() }}>
            <SelectTrigger className="sm:w-40"><SelectValue placeholder="IP" /></SelectTrigger>
            <SelectContent>
              <SelectItem value={ALL}>All IPs</SelectItem>
              {ipOptions.map((addr) => (
                <SelectItem key={addr} value={addr}>{addr}</SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Select value={outcome} onValueChange={(v) => { setOutcome(v); resetPaging() }}>
            <SelectTrigger className="sm:w-36"><SelectValue placeholder="Outcome" /></SelectTrigger>
            <SelectContent>
              <SelectItem value={ALL}>All outcomes</SelectItem>
              <SelectItem value="ok">Succeeded</SelectItem>
              <SelectItem value="failed">Failed</SelectItem>
            </SelectContent>
          </Select>
        </div>

        {/* Count summary */}
        <div className="flex items-center justify-between gap-3 text-sm text-aurora-text-muted">
          <span>
            {isLoading && !data
              ? 'Loading…'
              : `${formatCompactNumber(filtered)} matching call${filtered === 1 ? '' : 's'}`}
            {data ? ` of ${formatCompactNumber(data.total)} in window` : ''}
          </span>
          {filtered > 0 ? (
            <span className="tabular-nums">{showingFrom}–{showingTo}</span>
          ) : null}
        </div>

        {/* Table */}
        <div className="overflow-hidden rounded-aurora-2 border border-aurora-border-strong bg-aurora-panel-medium">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead className="w-[120px]">Time</TableHead>
                <TableHead>Tool · action</TableHead>
                <TableHead>Agent</TableHead>
                <TableHead className="w-[80px]">Surface</TableHead>
                <TableHead className="w-[110px]">Outcome</TableHead>
                <TableHead className="w-[90px] text-right">Tokens</TableHead>
                <TableHead className="w-[90px] text-right">Latency</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {error && !data ? (
                <TableRow>
                  <TableCell colSpan={7} className="py-8 text-center">
                    <span className="text-sm text-aurora-error">Couldn&apos;t load calls. </span>
                    <button
                      type="button"
                      onClick={() => mutate()}
                      className="text-sm font-medium text-aurora-accent-primary underline-offset-4 hover:underline"
                    >
                      Retry
                    </button>
                  </TableCell>
                </TableRow>
              ) : isLoading && !data ? (
                Array.from({ length: 8 }, (_, i) => (
                  <TableRow key={i}>
                    <TableCell colSpan={7}><Skeleton className="h-5 w-full" /></TableCell>
                  </TableRow>
                ))
              ) : !data || data.calls.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={7} className="py-10 text-center text-sm text-aurora-text-muted">
                    No calls match these filters.
                  </TableCell>
                </TableRow>
              ) : (
                data.calls.map((call) => (
                  <TableRow key={call.id}>
                    <TableCell className="text-aurora-text-muted">{formatRelativeTime(call.ts)}</TableCell>
                    <TableCell>
                      <span className="font-mono text-[13px] text-aurora-text-primary">{call.tool}</span>
                      {call.action ? (
                        <span className="font-mono text-[12px] text-aurora-text-muted">.{call.action}</span>
                      ) : null}
                    </TableCell>
                    <TableCell>
                      <div className="text-aurora-text-primary">{call.agent_label}</div>
                      <div className="font-mono text-[11px] text-aurora-text-muted">{call.ip}</div>
                    </TableCell>
                    <TableCell><SurfaceTag surface={call.surface} /></TableCell>
                    <TableCell>
                      <span className="inline-flex items-center gap-2">
                        <OutcomeDot outcome={call.outcome} />
                        <span className={call.outcome === 'failed' ? 'text-aurora-error' : 'text-aurora-text-muted'}>
                          {call.outcome === 'failed' ? (call.error_kind ?? 'failed') : 'ok'}
                        </span>
                      </span>
                    </TableCell>
                    <TableCell className="text-right tabular-nums text-aurora-text-muted">
                      {formatCompactNumber(call.input_tokens + call.output_tokens)}
                    </TableCell>
                    <TableCell className="text-right tabular-nums text-aurora-text-muted">
                      {formatDuration(call.elapsed_ms)}
                    </TableCell>
                  </TableRow>
                ))
              )}
            </TableBody>
          </Table>
        </div>

        {/* Pagination */}
        {filtered > PAGE_SIZE ? (
          <div className="flex items-center justify-end gap-2">
            <Button
              variant="outline"
              size="sm"
              disabled={offset === 0}
              onClick={() => setOffset((o) => Math.max(0, o - PAGE_SIZE))}
            >
              <ChevronLeft className="size-4" /> Prev
            </Button>
            <Button
              variant="outline"
              size="sm"
              disabled={showingTo >= filtered}
              onClick={() => setOffset((o) => o + PAGE_SIZE)}
            >
              Next <ChevronRight className="size-4" />
            </Button>
          </div>
        ) : null}

        <div>
          <Button variant="ghost" size="sm" asChild>
            <Link href="/">← Back to overview</Link>
          </Button>
        </div>
      </div>
    </>
  )
}

export default function UsageExplorerPage() {
  return (
    <Suspense fallback={null}>
      <UsageExplorer />
    </Suspense>
  )
}
