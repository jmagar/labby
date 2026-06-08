'use client'

import { useEffect, useState } from 'react'
import { AlertTriangle, CheckCircle2, Clock3, Database, Search, XCircle } from 'lucide-react'

import {
  type CodeModeTrace,
  flattenTraceRows,
  parseCodeModeTrace,
  stringifyRedactedParams,
} from '@/lib/code-mode-app/trace'
import { cn } from '@/lib/utils'

declare global {
  interface Window {
    __LAB_CODE_MODE_INITIAL_TRACE__?: unknown
    ExtApps?: {
      App?: new (
        appInfo: { name: string; version: string },
        capabilities?: Record<string, unknown>,
        options?: Record<string, unknown>,
      ) => {
        ontoolresult?: (result: { structuredContent?: unknown; structured_content?: unknown }) => void
        connect: () => Promise<unknown>
        close?: () => Promise<unknown> | void
      }
    }
  }
}

interface CodeModeInspectorProps {
  initialTrace?: unknown
}

export function CodeModeInspector({ initialTrace }: CodeModeInspectorProps) {
  const [trace, setTrace] = useState<CodeModeTrace | null>(() => parseCodeModeTrace(initialTrace))
  const [bridgeWarning, setBridgeWarning] = useState<string | null>(null)
  const [bridgeState, setBridgeState] = useState<'connecting' | 'connected' | 'fallback'>('fallback')

  useEffect(() => {
    const injected = parseCodeModeTrace(window.__LAB_CODE_MODE_INITIAL_TRACE__)
    if (injected) {
      setTrace(injected)
      setBridgeWarning(null)
    } else if (window.__LAB_CODE_MODE_INITIAL_TRACE__ !== undefined) {
      setBridgeWarning('Ignored malformed initial trace payload.')
    }

    const App = window.ExtApps?.App
    if (!App) return

    const app = new App(
      { name: 'Lab Code Mode Inspector', version: '0.1.0' },
      {},
      { autoResize: true },
    )
    app.ontoolresult = (result) => {
      const payload = result.structuredContent ?? result.structured_content
      const next = parseCodeModeTrace(payload)
      if (next) {
        setTrace(next)
        setBridgeWarning(null)
      } else {
        setBridgeWarning('Ignored malformed bridge payload.')
      }
    }
    setBridgeState('connecting')
    app
      .connect()
      .then(() => setBridgeState('connected'))
      .catch(() => setBridgeState('fallback'))

    return () => {
      void app.close?.()
    }
  }, [])

  const rows = flattenTraceRows(trace)
  const warnings = [
    ...(bridgeWarning ? [bridgeWarning] : []),
    ...(trace?.warnings?.map((warning) => warning.message) ?? []),
  ]
  const searchCountLabel =
    trace?.kind === 'code_mode_search_trace' ? formatSearchCount(trace) : undefined

  return (
    <main className="min-h-screen bg-aurora-page-bg text-aurora-text-primary">
      <section className="mx-auto flex w-full max-w-5xl flex-col gap-4 p-4 sm:p-5">
        <header className="flex flex-col gap-3 border-b border-aurora-border-default pb-4 sm:flex-row sm:items-center sm:justify-between">
          <div className="min-w-0">
            <div className="flex items-center gap-2 text-xs font-semibold uppercase text-aurora-text-muted">
              <Database className="size-4 text-aurora-accent-primary" />
              Code Mode trace
            </div>
            <h1 className="mt-1 text-xl font-semibold text-aurora-text-primary">
              Call inspector
            </h1>
          </div>
          <div className="flex flex-wrap items-center gap-2 text-xs text-aurora-text-muted">
            <StatusPill tone={bridgeState === 'connected' ? 'success' : 'neutral'}>
              {bridgeState}
            </StatusPill>
            {trace ? <StatusPill tone="info">{trace.kind.replace('code_mode_', '')}</StatusPill> : null}
          </div>
        </header>

        {!trace ? <EmptyState /> : null}

        {warnings.length > 0 ? <WarningBanner warnings={warnings} /> : null}

        {rows.calls.length > 0 ? (
          <Panel title="Broker-observed execute calls" count={rows.calls.length}>
            <div className="grid gap-2">
              {rows.calls.map((call, index) => (
                <CallRow key={`${call.id}-${index}`} call={call} />
              ))}
            </div>
          </Panel>
        ) : null}

        {rows.matches.length > 0 ? (
          <Panel
            title="Catalog-inferred search matches"
            count={searchCountLabel ?? rows.matches.length}
            meta={trace?.kind === 'code_mode_search_trace' && trace.truncated ? 'truncated' : undefined}
          >
            <div className="grid gap-2">
              {rows.matches.map((match) => (
                <SearchRow key={match.id} match={match} />
              ))}
            </div>
          </Panel>
        ) : null}

        {rows.history.length > 0 ? (
          <Panel title="Recent history" count={rows.history.length}>
            <div className="grid gap-2">
              {rows.history.map((entry) => (
                <div
                  key={entry.seq}
                  className="grid gap-1 rounded-md border border-aurora-border-default bg-aurora-control-surface p-3"
                >
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <span className="text-sm font-medium text-aurora-text-primary">
                      #{entry.seq} {entry.kind}
                    </span>
                    <StatusPill tone={entry.ok ? 'success' : 'error'}>
                      {entry.ok ? 'ok' : entry.error_kind ?? 'error'}
                    </StatusPill>
                  </div>
                  <p className="text-xs text-aurora-text-muted">
                    {entry.elapsed_ms}ms
                    {entry.match_count !== undefined ? ` / ${entry.match_count} matches` : ''}
                  </p>
                </div>
              ))}
            </div>
          </Panel>
        ) : null}
      </section>
    </main>
  )
}

function Panel({
  title,
  count,
  meta,
  children,
}: {
  title: string
  count: number | string
  meta?: string
  children: React.ReactNode
}) {
  return (
    <section className="rounded-md border border-aurora-border-default bg-aurora-panel-medium shadow-aurora-medium">
      <div className="flex items-center justify-between gap-3 border-b border-aurora-border-default px-4 py-3">
        <h2 className="text-sm font-semibold text-aurora-text-primary">{title}</h2>
        <span className="rounded border border-aurora-border-default bg-aurora-control-surface px-2 py-0.5 text-xs tabular-nums text-aurora-text-muted">
          {count}
        </span>
        {meta ? (
          <span className="rounded border border-aurora-warn/50 bg-aurora-warn/10 px-2 py-0.5 text-xs text-aurora-warn">
            {meta}
          </span>
        ) : null}
      </div>
      <div className="p-3">{children}</div>
    </section>
  )
}

function WarningBanner({ warnings }: { warnings: string[] }) {
  return (
    <section className="rounded-md border border-aurora-warn/50 bg-aurora-warn/10 p-3 text-sm text-aurora-warn">
      <div className="flex gap-2">
        <AlertTriangle className="mt-0.5 size-4 shrink-0" />
        <div className="grid gap-1">
          {warnings.map((warning, index) => (
            <p key={`${warning}-${index}`}>{warning}</p>
          ))}
        </div>
      </div>
    </section>
  )
}

function formatSearchCount(trace: Extract<CodeModeTrace, { kind: 'code_mode_search_trace' }>) {
  const displayed = trace.displayed_count ?? trace.matches.length
  if (trace.match_count !== displayed || trace.truncated) return `${displayed} of ${trace.match_count}`
  return displayed
}

function CallRow({ call }: { call: ReturnType<typeof flattenTraceRows>['calls'][number] }) {
  const params = stringifyRedactedParams(call.params)
  return (
    <article className="rounded-md border border-aurora-border-default bg-aurora-control-surface p-3">
      <div className="flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            {call.ok ? (
              <CheckCircle2 className="size-4 text-aurora-success" />
            ) : (
              <XCircle className="size-4 text-aurora-error" />
            )}
            <p className="truncate text-sm font-medium text-aurora-text-primary">
              {call.upstream} / {call.tool}
            </p>
          </div>
          <p className="mt-1 truncate font-mono text-xs text-aurora-text-muted">{call.id}</p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <StatusPill tone={call.ok ? 'success' : 'error'}>
            {call.ok ? 'ok' : call.error_kind ?? 'error'}
          </StatusPill>
          <span className="flex items-center gap-1 text-xs text-aurora-text-muted">
            <Clock3 className="size-3" />
            {call.elapsed_ms}ms
          </span>
        </div>
      </div>
      {params ? (
        <details className="mt-3 rounded border border-aurora-border-default bg-aurora-panel-strong">
          <summary className="cursor-pointer px-3 py-2 text-xs font-medium text-aurora-text-muted">
            Redacted params
          </summary>
          <pre className="max-h-52 overflow-auto whitespace-pre-wrap break-words px-3 pb-3 font-mono text-xs text-aurora-text-primary">
            {params}
          </pre>
        </details>
      ) : null}
    </article>
  )
}

function SearchRow({ match }: { match: ReturnType<typeof flattenTraceRows>['matches'][number] }) {
  return (
    <article className="rounded-md border border-aurora-border-default bg-aurora-control-surface p-3">
      <div className="flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <Search className="size-4 text-aurora-accent-primary" />
            <p className="truncate text-sm font-medium text-aurora-text-primary">
              {match.upstream} / {match.tool}
            </p>
          </div>
          <p className="mt-1 line-clamp-2 text-xs text-aurora-text-muted">{match.description}</p>
        </div>
        <div className="flex flex-wrap gap-2">
          {match.has_schema ? <StatusPill tone="info">schema</StatusPill> : null}
          {match.has_output_schema ? <StatusPill tone="neutral">output</StatusPill> : null}
        </div>
      </div>
    </article>
  )
}

function EmptyState() {
  return (
    <section className="rounded-md border border-aurora-border-default bg-aurora-panel-medium p-5 text-sm text-aurora-text-muted">
      Waiting for an MCP Apps tool result or history snapshot.
    </section>
  )
}

function StatusPill({
  tone,
  children,
}: {
  tone: 'success' | 'error' | 'info' | 'neutral'
  children: React.ReactNode
}) {
  return (
    <span
      className={cn(
        'inline-flex items-center rounded border px-2 py-0.5 text-xs font-medium',
        tone === 'success' &&
          'border-aurora-success/50 bg-aurora-success/10 text-aurora-success',
        tone === 'error' && 'border-aurora-error/50 bg-aurora-error/10 text-aurora-error',
        tone === 'info' &&
          'border-aurora-accent-primary/50 bg-aurora-accent-primary/10 text-aurora-accent-primary',
        tone === 'neutral' &&
          'border-aurora-border-default bg-aurora-control-surface text-aurora-text-muted',
      )}
    >
      {children}
    </span>
  )
}
