'use client'

import { type CSSProperties, useEffect, useState } from 'react'
import {
  AlertTriangle,
  Braces,
  CheckCircle2,
  Clock3,
  Database,
  History,
  Search,
  Sparkles,
} from 'lucide-react'

import {
  AURORA_CARD_TITLE,
  AURORA_DENSE_META,
  AURORA_MESSAGE_SURFACE,
  AURORA_MUTED_LABEL,
  AURORA_PAGE_SHELL,
  AURORA_STRONG_PANEL,
} from '@/components/aurora/tokens'
import {
  type CodeModeTrace,
  flattenTraceRows,
  parseCodeModeTrace,
  stringifyRedactedParams,
} from '@/lib/code-mode-app/trace'
import { cn } from '@/lib/utils'

const AURORA_DARK_TOKENS = {
  '--aurora-page-bg': '#07131c',
  '--aurora-panel-medium': '#102330',
  '--aurora-panel-strong': '#13293a',
  '--aurora-control-surface': '#0c1a24',
  '--aurora-border-default': '#1d3d4e',
  '--aurora-border-strong': '#24536c',
  '--aurora-text-primary': '#e6f4fb',
  '--aurora-text-muted': '#a7bcc9',
  '--aurora-accent-primary': '#29b6f6',
  '--aurora-accent-strong': '#67cbfa',
  '--aurora-warn': '#c6a36b',
  '--aurora-error': '#c78490',
  '--aurora-success': '#7dd3c7',
  '--aurora-hover-bg': '#17364b',
  '--aurora-active-glow': '0 0 0 1px rgba(41, 182, 246, 0.18), 0 0 16px rgba(41, 182, 246, 0.08)',
  '--aurora-shadow-medium': '0 12px 24px rgba(0, 0, 0, 0.18)',
  '--aurora-shadow-strong': '0 20px 38px rgba(0, 0, 0, 0.26)',
  '--aurora-highlight-medium': 'inset 0 1px 0 rgba(255, 255, 255, 0.035)',
  '--aurora-highlight-strong': 'inset 0 1px 0 rgba(255, 255, 255, 0.05)',
  '--color-aurora-page-bg': 'var(--aurora-page-bg)',
  '--color-aurora-panel-medium': 'var(--aurora-panel-medium)',
  '--color-aurora-panel-strong': 'var(--aurora-panel-strong)',
  '--color-aurora-control-surface': 'var(--aurora-control-surface)',
  '--color-aurora-border-default': 'var(--aurora-border-default)',
  '--color-aurora-border-strong': 'var(--aurora-border-strong)',
  '--color-aurora-text-primary': 'var(--aurora-text-primary)',
  '--color-aurora-text-muted': 'var(--aurora-text-muted)',
  '--color-aurora-accent-primary': 'var(--aurora-accent-primary)',
  '--color-aurora-accent-strong': 'var(--aurora-accent-strong)',
  '--color-aurora-warn': 'var(--aurora-warn)',
  '--color-aurora-error': 'var(--aurora-error)',
  '--color-aurora-success': 'var(--aurora-success)',
  '--color-aurora-hover-bg': 'var(--aurora-hover-bg)',
} as CSSProperties

declare global {
  interface Window {
    __LAB_CODE_MODE_INITIAL_TRACE__?: unknown
    // OpenAI Apps runtime (ChatGPT / Codex) injects this; MCP Apps hosts do not.
    openai?: { toolOutput?: unknown }
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

  // OpenAI Apps runtime (ChatGPT / Codex) bridge. These hosts bind the widget
  // via the tool's `openai/outputTemplate` meta and expose the structured tool
  // result on `window.openai.toolOutput` instead of driving the ExtApps
  // `ontoolresult` path, so hydrate from it directly and track live updates.
  useEffect(() => {
    if (!window.openai) return
    // The openai:set_globals CustomEvent carries changed values on
    // event.detail.globals; prefer that, falling back to the live snapshot.
    const sync = (event?: Event) => {
      const detail = (event as CustomEvent<{ globals?: { toolOutput?: unknown } }> | undefined)?.detail
      const raw = detail?.globals?.toolOutput ?? window.openai?.toolOutput
      const next = parseCodeModeTrace(raw)
      if (next) {
        setTrace(next)
        setBridgeWarning(null)
        setBridgeState('connected')
      } else if (raw != null) {
        // Present but unparseable — surface it like the ExtApps path does
        // instead of silently dropping the host's payload.
        setBridgeWarning('Ignored malformed bridge payload.')
      }
    }
    sync()
    window.addEventListener('openai:set_globals', sync)
    return () => window.removeEventListener('openai:set_globals', sync)
  }, [])

  const rows = flattenTraceRows(trace)
  const warnings = [
    ...(bridgeWarning ? [bridgeWarning] : []),
    ...(trace?.warnings?.map((warning) => warning.message) ?? []),
  ]
  const searchCountLabel =
    trace?.kind === 'code_mode_search_trace' ? formatSearchCount(trace) : undefined

  return (
    <main
      className={cn('h-[100dvh] overflow-hidden bg-aurora-page-bg text-aurora-text-primary', AURORA_PAGE_SHELL)}
      style={AURORA_DARK_TOKENS}
    >
      <section className="aurora-scrollbar mx-auto flex h-full w-full max-w-5xl flex-col gap-3 overflow-y-auto p-3 sm:gap-4 sm:p-5">
        <header className={cn('relative overflow-visible px-3 py-2.5 sm:p-4', AURORA_STRONG_PANEL)}>
          <div className="pointer-events-none absolute inset-x-0 top-0 h-px bg-gradient-to-r from-transparent via-aurora-accent-primary/50 to-transparent" />
          <div className="flex flex-col gap-2.5 sm:flex-row sm:items-start sm:justify-between">
            <div className="flex min-w-0 gap-2.5">
              <div className="flex size-8 shrink-0 items-center justify-center rounded-lg border border-aurora-accent-primary/35 bg-[color-mix(in_srgb,var(--aurora-accent-primary)_14%,transparent)] shadow-[var(--aurora-active-glow)] sm:size-10 sm:rounded-aurora-2">
                <Sparkles className="size-4 text-aurora-accent-strong sm:size-5" strokeWidth={1.65} />
              </div>
              <div className="min-w-0">
                <div className={cn('flex items-center gap-1.5', AURORA_MUTED_LABEL, 'text-[10px] tracking-[0.12em] sm:text-[11px] sm:tracking-[0.18em]')}>
                  <Database className="size-3.5 text-aurora-accent-primary" strokeWidth={1.65} />
                  Code Mode trace
                </div>
                <h1 className="mt-0.5 font-display text-[19px] font-extrabold leading-tight text-aurora-text-primary sm:mt-1 sm:text-[22px]">
                  Call inspector
                </h1>
                <p className={cn('mt-0.5 hidden max-w-2xl text-aurora-text-muted sm:mt-1 sm:block', AURORA_DENSE_META)}>
                  {trace
                    ? 'Live calls, search matches, and recent history from the active MCP app session.'
                    : 'Waiting for an MCP Apps tool result or history snapshot.'}
                </p>
              </div>
            </div>
            <div className="flex flex-wrap items-center gap-1.5 text-xs text-aurora-text-muted">
              <StatusPill tone={bridgeState === 'connected' ? 'success' : 'neutral'}>
                {bridgeState}
              </StatusPill>
              {trace ? <StatusPill tone="info">{trace.kind.replace('code_mode_', '')}</StatusPill> : null}
            </div>
          </div>
        </header>

        {!trace ? <EmptyState /> : null}

        {warnings.length > 0 ? <WarningBanner warnings={warnings} /> : null}

        {trace ? (
          <SummaryStrip
            calls={rows.calls.length}
            matches={searchCountLabel ?? rows.matches.length}
            history={rows.history.length}
          />
        ) : null}

        {rows.calls.length > 0 ? (
          <Panel title="Execute calls" count={rows.calls.length}>
            <div className="grid gap-2">
              {rows.calls.map((call, index) => (
                <CallRow key={`${call.id}-${index}`} call={call} />
              ))}
            </div>
          </Panel>
        ) : null}

        {rows.matches.length > 0 ? (
          <Panel
            title="Search matches"
            count={
              trace?.kind === 'code_mode_search_trace' && trace.truncated
                ? `${rows.matches.length} shown`
                : rows.matches.length
            }
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
                    {entry.ok ? (
                      <span aria-label="success" className="inline-flex" role="img">
                        <CheckCircle2 aria-hidden="true" className="size-4 shrink-0 text-aurora-success" />
                      </span>
                    ) : (
                      <StatusPill tone="error">{entry.error_kind ?? 'error'}</StatusPill>
                    )}
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
    <section className={cn('overflow-hidden', AURORA_STRONG_PANEL)}>
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-aurora-border-default/80 px-4 py-3">
        <div className="min-w-0">
          <h2 className={cn(AURORA_CARD_TITLE, 'text-aurora-text-primary')}>{title}</h2>
        </div>
        <div className="flex items-center gap-2">
          <span className="rounded-md border border-aurora-border-strong bg-aurora-control-surface px-2.5 py-1 text-[11px] font-semibold tabular-nums text-aurora-text-muted shadow-[var(--aurora-highlight-medium)]">
            {count}
          </span>
          {meta ? <StatusPill tone="warning">{meta}</StatusPill> : null}
        </div>
      </div>
      <div className="grid gap-2 p-3">{children}</div>
    </section>
  )
}

function WarningBanner({ warnings }: { warnings: string[] }) {
  return (
    <section className="rounded-aurora-2 border border-aurora-warn/50 bg-[color-mix(in_srgb,var(--aurora-warn)_12%,transparent)] p-3 text-sm text-aurora-warn shadow-[var(--aurora-highlight-medium)]">
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

function SummaryStrip({
  calls,
  matches,
  history,
}: {
  calls: number
  matches: number | string
  history: number
}) {
  return (
    <section className="grid grid-cols-3 gap-1.5 sm:gap-2">
      <SummaryStat icon={Braces} label="Execute calls" value={calls} />
      <SummaryStat icon={Search} label="Catalog matches" value={matches} accent />
      <SummaryStat icon={History} label="History entries" value={history} />
    </section>
  )
}

function SummaryStat({
  icon: Icon,
  label,
  value,
  accent,
}: {
  icon: typeof Braces
  label: string
  value: number | string
  accent?: boolean
}) {
  const compactLabel = label.replace('Execute calls', 'Calls').replace('Catalog matches', 'Matches').replace('History entries', 'History')
  const compactValue =
    typeof value === 'string' ? value.replace(/\bof\b/g, '/').replace(/\s+/g, '') : value

  return (
    <div className={cn('flex min-w-0 items-center justify-between gap-1 px-1.5 py-1 sm:gap-3 sm:p-3', AURORA_MESSAGE_SURFACE)}>
      <div className="flex min-w-0 items-center gap-1.5 sm:gap-2">
        <span
          className={cn(
            'hidden size-8 items-center justify-center rounded-aurora-1 border sm:flex',
            accent
              ? 'border-aurora-accent-primary/35 bg-[color-mix(in_srgb,var(--aurora-accent-primary)_12%,transparent)] text-aurora-accent-strong'
              : 'border-aurora-border-strong bg-aurora-control-surface text-aurora-text-muted',
          )}
        >
          <Icon className="size-4" strokeWidth={1.65} />
        </span>
        <span className={cn(AURORA_MUTED_LABEL, 'truncate text-[8px] tracking-[0.06em] sm:text-[11px] sm:tracking-[0.14em]')}>
          <span className="sm:hidden">{compactLabel}</span>
          <span className="hidden sm:inline">{label}</span>
        </span>
      </div>
      <span className="shrink-0 whitespace-nowrap font-display text-[12px] font-extrabold tabular-nums text-aurora-text-primary sm:text-[18px]">
        <span className="sm:hidden">{compactValue}</span>
        <span className="hidden sm:inline">{value}</span>
      </span>
    </div>
  )
}

function CallRow({ call }: { call: ReturnType<typeof flattenTraceRows>['calls'][number] }) {
  const params = stringifyRedactedParams(call.params)
  return (
    <article
      className={cn(
        'p-2.5 transition-colors hover:border-aurora-accent-primary/60 hover:bg-aurora-hover-bg/45 sm:p-3',
        AURORA_MESSAGE_SURFACE,
      )}
    >
      <div className="flex flex-col gap-1.5">
        <div className="min-w-0">
          <p className="truncate font-mono text-[13px] font-semibold text-aurora-text-primary">
            {call.upstream} / {call.tool}
          </p>
          <div className="mt-1 flex min-w-0 flex-wrap items-center gap-1.5 text-xs text-aurora-text-muted">
            <span className="truncate font-mono text-[11px] sm:text-xs">{call.id}</span>
            {call.ok ? (
              <span aria-label="success" className="inline-flex" role="img">
                <CheckCircle2 aria-hidden="true" className="size-4 shrink-0 text-aurora-success" />
              </span>
            ) : (
              <StatusPill tone="error">{call.error_kind ?? 'error'}</StatusPill>
            )}
            <span className="flex items-center gap-1 whitespace-nowrap">
              <Clock3 className="size-3" />
              {call.elapsed_ms}ms
            </span>
          </div>
        </div>
      </div>
      {params ? (
        <details className="mt-1.5 rounded-md border border-aurora-border-default bg-aurora-panel-strong sm:mt-3 sm:rounded-aurora-2">
          <summary className="cursor-pointer px-2.5 py-1 text-xs font-semibold text-aurora-accent-strong sm:px-3 sm:py-2">
            Redacted params
          </summary>
          <pre className="max-h-52 overflow-auto whitespace-pre-wrap break-words px-2.5 pb-2.5 font-mono text-xs text-aurora-text-primary sm:px-3 sm:pb-3">
            {params}
          </pre>
        </details>
      ) : null}
    </article>
  )
}

function SearchRow({ match }: { match: ReturnType<typeof flattenTraceRows>['matches'][number] }) {
  return (
    <article className={cn('p-2.5 transition-colors hover:border-aurora-accent-primary/60 hover:bg-aurora-hover-bg/45 sm:p-3', AURORA_MESSAGE_SURFACE)}>
      <div className="flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <Search className="size-4 text-aurora-accent-primary" />
            <p className="truncate font-mono text-[13px] font-semibold text-aurora-text-primary">
              {match.upstream} / {match.tool}
            </p>
          </div>
          <p className="mt-1 line-clamp-1 text-xs text-aurora-text-muted sm:line-clamp-2">{match.description}</p>
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
    <section className={cn('flex items-center gap-3 p-5 text-sm text-aurora-text-muted', AURORA_STRONG_PANEL)}>
      <div className="flex size-9 items-center justify-center rounded-aurora-1 border border-aurora-border-strong bg-aurora-control-surface text-aurora-accent-primary">
        <Search className="size-4" strokeWidth={1.65} />
      </div>
      <span>Waiting for an MCP Apps tool result or history snapshot.</span>
    </section>
  )
}

function StatusPill({
  tone,
  children,
}: {
  tone: 'success' | 'error' | 'info' | 'neutral' | 'warning'
  children: React.ReactNode
}) {
  return (
    <span
      className={cn(
        'inline-flex items-center rounded-md border px-2 py-0.5 text-xs font-semibold shadow-[var(--aurora-highlight-medium)]',
        tone === 'success' &&
          'border-aurora-success/50 bg-aurora-success/10 text-aurora-success',
        tone === 'error' && 'border-aurora-error/50 bg-aurora-error/10 text-aurora-error',
        tone === 'info' &&
          'border-aurora-accent-primary/50 bg-aurora-accent-primary/10 text-aurora-accent-primary',
        tone === 'warning' && 'border-aurora-warn/50 bg-aurora-warn/10 text-aurora-warn',
        tone === 'neutral' &&
          'border-aurora-border-strong bg-aurora-control-surface text-aurora-text-muted',
      )}
    >
      {children}
    </span>
  )
}
