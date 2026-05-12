'use client'

// Extract panel — runs `extract.scan` (read-only) and lets the operator
// "Apply to draft" any subset of discovered credentials. Each Apply is a
// batched setup.draft.set scoped to the selected service; commit is
// driven from the per-service settings page.

import { useState } from 'react'
import { Loader2, RefreshCw, ArrowRight, CheckCircle2 } from 'lucide-react'

import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Checkbox } from '@/components/ui/checkbox'
import { extractApi, type ExtractCredential, type ExtractReport } from '@/lib/api/extract-client'
import { setupApi } from '@/lib/api/setup-client'

export default function ExtractPanel(): React.ReactElement {
  const [report, setReport] = useState<ExtractReport | undefined>()
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | undefined>()
  const [selected, setSelected] = useState<Set<number>>(new Set())
  const [applying, setApplying] = useState(false)
  const [appliedCount, setAppliedCount] = useState<number | undefined>()

  async function rescan(): Promise<void> {
    setLoading(true)
    setError(undefined)
    setAppliedCount(undefined)
    try {
      const result = await extractApi.scan()
      setReport(result)
      setSelected(new Set(result.creds.map((_, i) => i)))
    } catch (err) {
      setError(err instanceof Error ? err.message : 'extract.scan failed')
    } finally {
      setLoading(false)
    }
  }

  function toggle(idx: number): void {
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(idx)) next.delete(idx)
      else next.add(idx)
      return next
    })
  }

  async function applyToDraft(): Promise<void> {
    if (!report) return
    const entries: { key: string; value: string }[] = []
    for (const idx of selected) {
      const cred = report.creds[idx]
      if (!cred) continue
      const upper = cred.service.toUpperCase()
      if (cred.url) entries.push({ key: `${upper}_URL`, value: cred.url })
      // The redacted scan response sets secret_present but does not return
      // the value, so we cannot batch the secrets here. The operator must
      // re-enter them in the per-service settings page.
    }
    if (entries.length === 0) {
      setAppliedCount(0)
      return
    }
    setApplying(true)
    try {
      const result = await setupApi.draftSet(entries, { force: true })
      setAppliedCount(result.written)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'draft.set failed')
    } finally {
      setApplying(false)
    }
  }

  const previewEntries = report
    ? [...selected]
        .map((idx) => report.creds[idx])
        .filter((cred): cred is ExtractCredential => Boolean(cred?.url))
        .map((cred) => ({ key: `${cred.service.toUpperCase()}_URL`, value: cred.url! }))
    : []

  return (
    <>
      <h1 className="sr-only">Extract settings</h1>
      <Card>
        <CardHeader className="flex flex-row items-start justify-between space-y-0">
          <div>
            <CardTitle>Extract</CardTitle>
            <CardDescription>
              Scan local + SSH hosts for existing service credentials and apply
              the discovered URLs to your draft. Secret values are redacted in
              transit; re-enter them in each service&apos;s settings page.
            </CardDescription>
          </div>
          <Button variant="outline" size="sm" onClick={rescan} disabled={loading}>
            <RefreshCw className={`mr-2 h-3 w-3 ${loading ? 'animate-spin' : ''}`} />
            {report ? 'Re-scan' : 'Scan'}
          </Button>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          {loading ? (
            <div className="flex items-center gap-2 text-sm text-muted-foreground">
              <Loader2 className="h-4 w-4 animate-spin" /> running extract.scan
            </div>
          ) : null}
          {error ? <p className="text-sm text-destructive">{error}</p> : null}

        {report && report.creds.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            No credentials discovered.
          </p>
        ) : null}

        {report && report.creds.length > 0 ? (
          <>
            <ul className="divide-y rounded-md border">
              {report.creds.map((cred, idx) => (
                <li key={`${cred.service}-${idx}`} className="flex items-start gap-3 p-3 text-sm">
                  <Checkbox
                    checked={selected.has(idx)}
                    onCheckedChange={() => toggle(idx)}
                    aria-label={`Toggle ${cred.service}`}
                  />
                  <div className="flex-1">
                    <p className="font-medium">{cred.service}</p>
                    {cred.url ? (
                      <p className="text-xs text-muted-foreground">URL: {cred.url}</p>
                    ) : null}
                    <p className="text-xs text-muted-foreground">
                      {cred.secret_present ? 'Secret present (redacted)' : 'No secret'}
                      {cred.source_host ? ` — host: ${cred.source_host}` : ''}
                    </p>
                  </div>
                </li>
              ))}
            </ul>
            <div className="rounded-md border p-3">
              <p className="text-sm font-medium">Draft preview</p>
              {previewEntries.length > 0 ? (
                <ul className="mt-2 space-y-1 text-xs text-muted-foreground">
                  {previewEntries.map((entry) => (
                    <li key={entry.key} className="font-mono">
                      {entry.key} = {entry.value}
                    </li>
                  ))}
                </ul>
              ) : (
                <p className="mt-2 text-xs text-muted-foreground">
                  Selected credentials do not include writable URL values.
                </p>
              )}
              <p className="mt-2 text-xs text-muted-foreground">
                Redacted secrets are not written by extract; enter them on each service page.
              </p>
            </div>
            <div className="flex items-center gap-2">
              <Button onClick={applyToDraft} disabled={applying || selected.size === 0}>
                {applying ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : null}
                <ArrowRight className="mr-2 h-4 w-4" />
                Apply {selected.size} to draft
              </Button>
              {appliedCount !== undefined ? (
                <span className="text-xs text-emerald-600 inline-flex items-center gap-1">
                  <CheckCircle2 className="h-3 w-3" /> {appliedCount} entries written
                </span>
              ) : null}
            </div>
          </>
        ) : null}

        {report?.warnings && report.warnings.length > 0 ? (
          <ul className="text-xs text-amber-600 list-disc pl-5">
            {report.warnings.map((w, i) => (
              <li key={i}>
                {w.service ? `${w.service}: ` : ''}
                {w.message}
              </li>
            ))}
          </ul>
        ) : null}
      </CardContent>
      </Card>
    </>
  )
}

// Re-export the credential type for type clarity in this file (suppresses
// unused-import lints if extract-client.ts changes).
export type { ExtractCredential }
