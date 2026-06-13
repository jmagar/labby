'use client'

import * as React from 'react'
import {
  CheckCircle2,
  Code2,
  FileCode2,
  FlaskConical,
  Loader2,
  Play,
  RefreshCw,
  ShieldCheck,
  XCircle,
} from 'lucide-react'
import { AppHeader } from '@/components/app-header'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  AURORA_CARD_TITLE,
  AURORA_DENSE_META,
  AURORA_DISPLAY_1,
  AURORA_DISPLAY_NUMBER,
  AURORA_MEDIUM_PANEL,
  AURORA_MUTED_LABEL,
  AURORA_PAGE_FRAME,
  AURORA_PAGE_SHELL,
  AURORA_STRONG_PANEL,
} from '@/components/aurora/tokens'
import { snippetsApi } from '@/lib/api/snippets-client'
import type { SnippetInfo } from '@/lib/types/snippets'
import { cn } from '@/lib/utils'

type ActionState =
  | { kind: 'idle' }
  | { kind: 'loading'; label: string }
  | { kind: 'success'; label: string; detail: string }
  | { kind: 'error'; label: string; detail: string }

function inputEntries(snippet: SnippetInfo | null) {
  return Object.entries(snippet?.inputs ?? {})
}

function formatDefault(value: unknown): string {
  if (value === undefined) return 'none'
  if (value === null) return 'null'
  if (typeof value === 'string') return value
  return JSON.stringify(value)
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : 'Unknown snippets error'
}

export function SnippetsPageContent() {
  const [snippets, setSnippets] = React.useState<SnippetInfo[]>([])
  const [selectedName, setSelectedName] = React.useState<string | null>(null)
  const [loading, setLoading] = React.useState(true)
  const [error, setError] = React.useState<string | null>(null)
  const [actionState, setActionState] = React.useState<ActionState>({ kind: 'idle' })

  const reload = React.useCallback(async () => {
    setLoading(true)
    try {
      const next = await snippetsApi.list()
      setSnippets(next)
      setSelectedName((current) => {
        if (current && next.some((snippet) => snippet.name === current)) return current
        return next[0]?.name ?? null
      })
      setError(null)
    } catch (err) {
      setError(errorMessage(err))
    } finally {
      setLoading(false)
    }
  }, [])

  React.useEffect(() => {
    const controller = new AbortController()
    setLoading(true)
    snippetsApi
      .list(controller.signal)
      .then((next) => {
        setSnippets(next)
        setSelectedName(next[0]?.name ?? null)
        setError(null)
      })
      .catch((err) => {
        if (!controller.signal.aborted) setError(errorMessage(err))
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false)
      })
    return () => controller.abort()
  }, [])

  const selected = React.useMemo(
    () => snippets.find((snippet) => snippet.name === selectedName) ?? snippets[0] ?? null,
    [selectedName, snippets],
  )
  const builtinCount = snippets.filter((snippet) => snippet.source === 'builtin').length
  const inputCount = snippets.reduce((sum, snippet) => sum + inputEntries(snippet).length, 0)

  const runAction = async (label: string, fn: () => Promise<unknown>) => {
    setActionState({ kind: 'loading', label })
    try {
      const result = await fn()
      const detail =
        typeof result === 'object' && result !== null
          ? JSON.stringify(result).slice(0, 240)
          : String(result)
      setActionState({ kind: 'success', label, detail })
    } catch (err) {
      setActionState({ kind: 'error', label, detail: errorMessage(err) })
    }
  }

  return (
    <>
      <AppHeader breadcrumbs={[{ label: 'Snippets' }]} />
      <div className={`${AURORA_PAGE_SHELL} flex-1`}>
        <div className={AURORA_PAGE_FRAME}>
          <div className={cn(AURORA_STRONG_PANEL, 'px-6 py-5')}>
            <div className="flex flex-col gap-4 lg:flex-row lg:items-start lg:justify-between">
              <div>
                <p className={AURORA_MUTED_LABEL}>Code Mode workflows</p>
                <h1 className={cn(AURORA_DISPLAY_1, 'mt-2 text-aurora-text-primary')}>Snippets</h1>
                <p className="mt-2 max-w-3xl text-sm leading-6 text-aurora-text-muted">
                  Inspect executable Code Mode snippets, review their typed inputs, and run validation or smoke checks through the shared snippets dispatch layer.
                </p>
              </div>
              <div className="flex flex-wrap gap-2">
                <Button variant="outline" size="sm" onClick={() => void reload()}>
                  <RefreshCw className="size-4" />
                  Refresh
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => void runAction('Test all', () => snippetsApi.testAll())}
                  disabled={snippets.length === 0}
                >
                  <FlaskConical className="size-4" />
                  Test all
                </Button>
              </div>
            </div>
          </div>

          <div className="grid gap-3 sm:grid-cols-3">
            <MetricCard label="Snippets" value={snippets.length} detail="Executable workflows in built-in and user stores." />
            <MetricCard label="Built-in" value={builtinCount} detail="Frontmatter-backed snippets shipped with Labby." />
            <MetricCard label="Inputs" value={inputCount} detail="Typed parameters with defaults and validation." />
          </div>

          {error ? (
            <div className={cn(AURORA_MEDIUM_PANEL, 'border-aurora-error/40 px-5 py-4 text-sm text-aurora-error')}>
              Failed to load snippets: {error}
            </div>
          ) : null}

          <div className="grid min-h-[520px] gap-5 lg:grid-cols-[minmax(280px,380px)_minmax(0,1fr)]">
            <div className={cn(AURORA_STRONG_PANEL, 'overflow-hidden')}>
              <div className="border-b border-aurora-border-strong px-5 py-4">
                <p className={AURORA_MUTED_LABEL}>Library</p>
              </div>
              <div className="divide-y divide-aurora-border-strong/70">
                {loading && snippets.length === 0 ? (
                  Array.from({ length: 5 }, (_, index) => (
                    <div key={index} className="h-24 animate-pulse bg-aurora-control-surface/45" />
                  ))
                ) : snippets.length === 0 ? (
                  <div className="px-5 py-8 text-sm text-aurora-text-muted">No executable snippets found.</div>
                ) : (
                  snippets.map((snippet) => (
                    <button
                      key={`${snippet.source}:${snippet.name}`}
                      type="button"
                      onClick={() => setSelectedName(snippet.name)}
                      className={cn(
                        'flex w-full items-start gap-3 px-5 py-4 text-left transition hover:bg-aurora-hover-bg/60',
                        selected?.name === snippet.name && 'border-l-2 border-aurora-accent-primary bg-aurora-hover-bg/70 shadow-[var(--aurora-active-glow)]',
                      )}
                    >
                      <FileCode2 className="mt-0.5 size-4 shrink-0 text-aurora-accent-primary" />
                      <span className="min-w-0 flex-1">
                        <span className={cn(AURORA_CARD_TITLE, 'block truncate text-aurora-text-primary')}>{snippet.name}</span>
                        <span className="mt-1 line-clamp-2 block text-xs leading-5 text-aurora-text-muted">
                          {snippet.description ?? 'No description provided.'}
                        </span>
                        <span className="mt-3 flex flex-wrap gap-1.5">
                          <Badge variant="secondary">{snippet.source}</Badge>
                          {snippet.shadowed ? <Badge variant="outline">Shadowed</Badge> : null}
                          {inputEntries(snippet).length > 0 ? <Badge variant="outline">{inputEntries(snippet).length} inputs</Badge> : null}
                        </span>
                      </span>
                    </button>
                  ))
                )}
              </div>
            </div>

            <div className={cn(AURORA_STRONG_PANEL, 'flex min-w-0 flex-col overflow-hidden')}>
              {selected ? (
                <>
                  <div className="border-b border-aurora-border-strong px-5 py-4">
                    <div className="flex flex-col gap-3 md:flex-row md:items-start md:justify-between">
                      <div className="min-w-0">
                        <p className={AURORA_MUTED_LABEL}>Selected snippet</p>
                        <h2 className={cn(AURORA_DISPLAY_1, 'mt-2 truncate text-[26px] text-aurora-text-primary')}>{selected.name}</h2>
                        <p className="mt-2 max-w-3xl text-sm leading-6 text-aurora-text-muted">{selected.description}</p>
                      </div>
                      <div className="flex shrink-0 flex-wrap gap-2">
                        <Button
                          variant="outline"
                          size="sm"
                          onClick={() => void runAction('Validate', () => snippetsApi.validate(selected.name))}
                        >
                          <ShieldCheck className="size-4" />
                          Validate
                        </Button>
                        <Button
                          variant="outline"
                          size="sm"
                          onClick={() => void runAction('Test', () => snippetsApi.test(selected.name))}
                        >
                          <FlaskConical className="size-4" />
                          Test
                        </Button>
                        <Button
                          variant="default"
                          size="sm"
                          onClick={() => void runAction('Execute', () => snippetsApi.exec(selected.name))}
                        >
                          <Play className="size-4" />
                          Execute
                        </Button>
                      </div>
                    </div>
                  </div>

                  <div className="grid gap-4 p-5 xl:grid-cols-[minmax(0,1fr)_320px]">
                    <div className={cn(AURORA_MEDIUM_PANEL, 'min-w-0 overflow-hidden p-4')}>
                      <div className="flex items-center gap-2">
                        <Code2 className="size-4 text-aurora-accent-primary" />
                        <p className={AURORA_MUTED_LABEL}>Inputs</p>
                      </div>
                      {inputEntries(selected).length === 0 ? (
                        <p className="mt-4 text-sm text-aurora-text-muted">This snippet does not declare typed inputs.</p>
                      ) : (
                        <div className="mt-4 divide-y divide-aurora-border-strong/60">
                          {inputEntries(selected).map(([name, spec]) => (
                            <div key={name} className="grid gap-2 py-3 md:grid-cols-[160px_minmax(0,1fr)_160px]">
                              <div>
                                <p className="font-mono text-xs font-semibold text-aurora-text-primary">{name}</p>
                                <p className="mt-1 text-[11px] uppercase tracking-[0.14em] text-aurora-text-muted">
                                  {spec.required ? 'Required' : 'Optional'}
                                </p>
                              </div>
                              <p className="text-sm leading-6 text-aurora-text-muted">
                                {spec.description ?? 'No description provided.'}
                              </p>
                              <div className="min-w-0 text-left md:text-right">
                                <Badge variant="outline">{spec.ty}</Badge>
                                <p className="mt-2 truncate font-mono text-xs text-aurora-text-muted">
                                  default {formatDefault(spec.default)}
                                </p>
                              </div>
                            </div>
                          ))}
                        </div>
                      )}
                    </div>

                    <div className={cn(AURORA_MEDIUM_PANEL, 'p-4')}>
                      <p className={AURORA_MUTED_LABEL}>Run receipt</p>
                      {actionState.kind === 'idle' ? (
                        <p className="mt-4 text-sm leading-6 text-aurora-text-muted">
                          Validate, test, or execute the selected snippet. Actions require admin scope and use the `/v1/snippets` surface.
                        </p>
                      ) : actionState.kind === 'loading' ? (
                        <div className="mt-4 flex items-center gap-2 text-sm text-aurora-text-muted">
                          <Loader2 className="size-4 animate-spin text-aurora-accent-primary" />
                          Running {actionState.label.toLowerCase()}...
                        </div>
                      ) : (
                        <div className="mt-4 space-y-3">
                          <div className="flex items-center gap-2 text-sm font-medium text-aurora-text-primary">
                            {actionState.kind === 'success' ? (
                              <CheckCircle2 className="size-4 text-aurora-success" />
                            ) : (
                              <XCircle className="size-4 text-aurora-error" />
                            )}
                            {actionState.label} {actionState.kind === 'success' ? 'completed' : 'failed'}
                          </div>
                          <pre className="max-h-56 overflow-auto rounded-[0.75rem] border border-aurora-border-strong bg-aurora-control-surface p-3 text-xs leading-5 text-aurora-text-muted">
                            {actionState.detail}
                          </pre>
                        </div>
                      )}
                    </div>
                  </div>
                </>
              ) : (
                <div className="flex min-h-[360px] items-center justify-center p-8 text-sm text-aurora-text-muted">
                  Select a snippet to inspect its inputs and run actions.
                </div>
              )}
            </div>
          </div>
        </div>
      </div>
    </>
  )
}

function MetricCard({ label, value, detail }: { label: string; value: number; detail: string }) {
  return (
    <div className={cn(AURORA_MEDIUM_PANEL, 'px-5 py-4')}>
      <p className={AURORA_MUTED_LABEL}>{label}</p>
      <p className={cn(AURORA_DISPLAY_NUMBER, 'mt-2 text-aurora-text-primary')}>{value}</p>
      <p className={cn(AURORA_DENSE_META, 'mt-2 text-aurora-text-muted')}>{detail}</p>
    </div>
  )
}
