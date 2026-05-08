'use client'

// Core lab settings — operator preferences (bind host/port, log filter,
// log format). Optimistic per-field save: each field commits immediately
// to ~/.lab/.env via setup.draft.set + setup.draft.commit on blur.
//
// On commit failure the input is preserved so the user can retry without
// retyping (GitLab saving-and-feedback pattern).

import { useEffect, useState } from 'react'
import { Loader2, CheckCircle2, AlertTriangle } from 'lucide-react'

import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { setupApi } from '@/lib/api/setup-client'
import { CORE_FIELDS } from '@/lib/setup/coreFields'
import { unmaskValue } from '@/lib/setup/draft'

type SaveStatus = 'idle' | 'saving' | 'saved' | 'error'

interface FieldState {
  value: string
  status: SaveStatus
  error?: string
}

export default function CorePage(): React.ReactElement {
  const [fields, setFields] = useState<Record<string, FieldState>>(() => {
    const init: Record<string, FieldState> = {}
    for (const field of CORE_FIELDS) {
      init[field.key] = { value: '', status: 'idle' }
    }
    return init
  })

  // Hydrate from current draft on mount.
  useEffect(() => {
    const controller = new AbortController()
    setupApi
      .draftGet(controller.signal)
      .then((draft) => {
        if (controller.signal.aborted) return
        setFields((prev) => {
          const next = { ...prev }
          for (const entry of draft.entries) {
            if (next[entry.key]) {
              next[entry.key] = {
                ...next[entry.key]!,
                value: unmaskValue(entry.value),
              }
            }
          }
          return next
        })
      })
      .catch(() => {
        /* render empty if draft.get fails */
      })
    return () => controller.abort()
  }, [])

  async function commitField(key: string, value: string): Promise<void> {
    setFields((prev) => ({
      ...prev,
      [key]: { ...prev[key]!, status: 'saving', error: undefined },
    }))
    try {
      if (value !== '') {
        await setupApi.draftSet([{ key, value }], { force: true })
        await setupApi.draftCommit({ force: true })
      }
      setFields((prev) => ({
        ...prev,
        [key]: { ...prev[key]!, status: 'saved' },
      }))
      setTimeout(() => {
        setFields((prev) => {
          if (prev[key]?.status !== 'saved') return prev
          return { ...prev, [key]: { ...prev[key]!, status: 'idle' } }
        })
      }, 1500)
    } catch (err) {
      setFields((prev) => ({
        ...prev,
        [key]: {
          ...prev[key]!,
          status: 'error',
          error: err instanceof Error ? err.message : 'save failed',
        },
      }))
    }
  }

  return (
    <>
      <h1 className="sr-only">Core settings</h1>
      <Card>
        <CardHeader>
          <CardTitle>Core</CardTitle>
          <CardDescription>
            Operator-level lab process defaults. Saved on blur to{' '}
            <code>~/.lab/.env</code> via the setup dispatch service.
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          {CORE_FIELDS.map((field) => {
            const state = fields[field.key]!
            return (
              <div key={field.key} className="flex flex-col gap-1">
                <Label htmlFor={field.key} className="font-mono text-xs">
                  {field.key}
                </Label>
                <p className="text-sm text-muted-foreground">{field.description}</p>
                <div className="flex items-center gap-2">
                  <Input
                    id={field.key}
                    placeholder={field.example}
                    value={state.value}
                    onChange={(e) =>
                      setFields((prev) => ({
                        ...prev,
                        [field.key]: {
                          ...prev[field.key]!,
                          value: e.target.value,
                          status: 'idle',
                        },
                      }))
                    }
                    onBlur={() => {
                      if (state.value !== '') void commitField(field.key, state.value)
                    }}
                  />
                  <SaveBadge state={state} />
                </div>
                {state.status === 'error' && state.error ? (
                  <p className="text-xs text-destructive">{state.error}</p>
                ) : null}
              </div>
            )
          })}
        </CardContent>
      </Card>
    </>
  )
}

function SaveBadge({ state }: { state: FieldState }): React.ReactElement | null {
  if (state.status === 'saving') {
    return (
      <span className="text-xs text-muted-foreground inline-flex items-center gap-1">
        <Loader2 className="h-3 w-3 animate-spin" /> saving
      </span>
    )
  }
  if (state.status === 'saved') {
    return (
      <span className="text-xs text-emerald-600 inline-flex items-center gap-1">
        <CheckCircle2 className="h-3 w-3" /> saved
      </span>
    )
  }
  if (state.status === 'error') {
    return (
      <span className="text-xs text-destructive inline-flex items-center gap-1">
        <AlertTriangle className="h-3 w-3" /> error
      </span>
    )
  }
  return null
}
