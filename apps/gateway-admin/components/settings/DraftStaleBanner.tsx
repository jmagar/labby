'use client'

// Polls setup.state on mount + on window.focus (debounced 1s trailing
// edge + AbortController dedup) and renders a non-blocking warning
// banner when `draft_stale: true`.

import { useEffect, useState } from 'react'
import { AlertTriangle } from 'lucide-react'

import { Button } from '@/components/ui/button'
import { setupApi, type SetupSnapshot } from '@/lib/api/setup-client'

type Status = 'unknown' | 'fresh' | 'stale' | 'unavailable'

export function DraftStaleBanner(): React.ReactElement | null {
  const [status, setStatus] = useState<Status>('unknown')
  const [snapshot, setSnapshot] = useState<SetupSnapshot | null>(null)
  const [discarding, setDiscarding] = useState(false)
  const [discardError, setDiscardError] = useState<string | null>(null)

  async function refresh(signal?: AbortSignal): Promise<void> {
    const snapshot = await setupApi.state(signal)
    setSnapshot(snapshot)
    setStatus(snapshot.draft_stale ? 'stale' : 'fresh')
  }

  useEffect(() => {
    let cancelled = false
    let inFlight: AbortController | null = null
    let debounceTimer: ReturnType<typeof setTimeout> | null = null

    async function check(): Promise<void> {
      inFlight?.abort()
      const controller = new AbortController()
      inFlight = controller
      try {
        const snapshot = await setupApi.state(controller.signal)
        if (cancelled || controller.signal.aborted) return
        setSnapshot(snapshot)
        setStatus(snapshot.draft_stale ? 'stale' : 'fresh')
      } catch (err) {
        if (cancelled || controller.signal.aborted) return
        // AbortError is expected churn (a newer check superseded this one)
        // and is silent. Anything else means the gateway is unreachable
        // or returning errors — surface that as 'unavailable' so users
        // know draft-stale detection is offline rather than silently
        // assuming everything is fine.
        if (err instanceof Error && err.name === 'AbortError') return
        console.warn('DraftStaleBanner: setup.state failed', err)
        setStatus('unavailable')
      }
    }

    function schedule(): void {
      if (debounceTimer) clearTimeout(debounceTimer)
      debounceTimer = setTimeout(() => {
        void check()
      }, 1000)
    }

    function onVisibility(): void {
      if (document.visibilityState === 'visible') schedule()
    }

    void check()
    window.addEventListener('focus', schedule)
    // visibilitychange covers tab-switch on browsers where 'focus' doesn't
    // fire when switching between tabs in the same window (Chrome on
    // mobile, some multi-tab desktop workflows).
    document.addEventListener('visibilitychange', onVisibility)
    return () => {
      cancelled = true
      window.removeEventListener('focus', schedule)
      document.removeEventListener('visibilitychange', onVisibility)
      if (debounceTimer) clearTimeout(debounceTimer)
      inFlight?.abort()
    }
  }, [])

  async function discardDraft(): Promise<void> {
    setDiscarding(true)
    setDiscardError(null)
    try {
      await setupApi.draftDiscard()
      await refresh()
    } catch (err) {
      setDiscardError(err instanceof Error ? err.message : 'Could not discard the draft.')
    } finally {
      setDiscarding(false)
    }
  }

  if (status === 'unknown' || status === 'fresh') return null
  if (status === 'unavailable') {
    return (
      <div className="flex items-start gap-2 rounded-md border border-aurora-border-default bg-aurora-panel-muted p-3 text-sm text-aurora-text-muted">
        <AlertTriangle className="h-4 w-4 mt-0.5 flex-shrink-0" />
        <div>
          <p className="font-medium text-aurora-text-primary">Draft state check unavailable.</p>
          <p>
            Could not reach the lab gateway. Concurrent-edit detection is
            offline — saving here may overwrite changes from another session
            without warning.
          </p>
        </div>
      </div>
    )
  }
  const draftSummary = staleDraftSummary(snapshot)
  return (
    <div className="flex items-start gap-3 rounded-md border border-aurora-warn/30 bg-aurora-warn/10 p-3 text-sm text-aurora-text-primary">
      <AlertTriangle className="h-4 w-4 mt-0.5 flex-shrink-0" />
      <div className="min-w-0 flex-1">
        <p className="font-medium">Old draft detected.</p>
        <p>
          A saved setup draft{draftSummary} is older than <code>~/.lab/.env</code>. Discard it if
          you do not need those draft values.
        </p>
        {discardError ? <p className="mt-2 text-xs">{discardError}</p> : null}
      </div>
      <Button
        type="button"
        size="sm"
        variant="outline"
        onClick={() => void discardDraft()}
        disabled={discarding}
        className="shrink-0 border-aurora-warn/40 bg-transparent text-aurora-text-primary hover:bg-aurora-warn/15"
      >
        {discarding ? 'Discarding...' : 'Discard draft'}
      </Button>
    </div>
  )
}

function staleDraftSummary(snapshot: SetupSnapshot | null): string {
  if (!snapshot) return ''
  const pieces: string[] = []
  if (snapshot.draft_entry_count > 0) {
    pieces.push(
      `${snapshot.draft_entry_count} value${snapshot.draft_entry_count === 1 ? '' : 's'}`,
    )
  }
  const date = formatUnixDate(snapshot.draft_mtime_unix_seconds)
  if (date) pieces.push(`from ${date}`)
  return pieces.length > 0 ? ` with ${pieces.join(' ')}` : ''
}

function formatUnixDate(seconds: number | null): string | null {
  if (typeof seconds !== 'number' || !Number.isFinite(seconds) || seconds <= 0) return null
  return new Intl.DateTimeFormat(undefined, {
    month: 'short',
    day: 'numeric',
    year: 'numeric',
  }).format(new Date(seconds * 1000))
}
