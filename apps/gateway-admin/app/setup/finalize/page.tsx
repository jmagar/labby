'use client'

import { useEffect, useState } from 'react'
import { useRouter } from 'next/navigation'
import { CheckCircle2, AlertTriangle, Loader2 } from 'lucide-react'

import { Button } from '@/components/ui/button'
import { NavButtons, useWizard } from '@/components/setup/WizardShell'
import { setupApi, type CommitOutcome } from '@/lib/api/setup-client'
import { doctorApi, type DoctorReport } from '@/lib/api/doctor-client'

export default function FinalizePage(): React.ReactElement {
  const router = useRouter()
  const wizard = useWizard()
  const [audit, setAudit] = useState<DoctorReport | undefined>()
  const [auditError, setAuditError] = useState<string | undefined>()
  const [auditing, setAuditing] = useState(false)
  const [committing, setCommitting] = useState(false)
  const [commitOutcome, setCommitOutcome] = useState<CommitOutcome | undefined>()
  const [commitError, setCommitError] = useState<string | undefined>()
  const [pluginSummary, setPluginSummary] = useState<string>('Installed: none. Uninstalled: none.')

  async function runAudit(): Promise<void> {
    setAuditing(true)
    setAuditError(undefined)
    try {
      setAudit(await doctorApi.auditFull())
    } catch (err) {
      setAuditError(err instanceof Error ? err.message : 'audit.full failed')
    } finally {
      setAuditing(false)
    }
  }

  useEffect(() => {
    void runAudit()
    setupApi.installedPlugins()
      .then((plugins) => {
        const installed = plugins.map((plugin) => plugin.id).join(', ')
        setPluginSummary(`Installed: ${installed || 'none'}. Uninstalled: none.`)
      })
      .catch(() => setPluginSummary('Installed: unavailable. Uninstalled: none.'))
  }, [])

  // Tie the post-commit redirect to a cleanup-able timer. If the user
  // navigates away during the 1.2s grace window, clearTimeout prevents
  // router.replace from firing on a dead component.
  useEffect(() => {
    if (commitOutcome === undefined || commitOutcome.ok === false) return
    const t = setTimeout(() => router.replace('/settings/'), 1200)
    return () => clearTimeout(t)
  }, [commitOutcome, router])

  async function commit(): Promise<void> {
    setCommitting(true)
    setCommitError(undefined)
    try {
      const outcome = await setupApi.finalize()
      setCommitOutcome(outcome)
      // On success, wipe wizard selection (so a new wizard run starts fresh)
      // — actual redirect runs from a useEffect tied to commitOutcome so it
      // can be cleaned up on unmount.
      if (outcome.ok !== false) {
        wizard.clearWizardState()
      }
    } catch (err) {
      setCommitError(err instanceof Error ? err.message : 'finalize failed')
    } finally {
      setCommitting(false)
    }
  }

  const errors = audit?.findings.filter((f) => f.severity === 'error') ?? []
  const allPass = audit !== undefined && errors.length === 0

  return (
    <div className="space-y-6">
      <section className="space-y-3">
        <h2 className="text-xl font-semibold">Finalize</h2>
        <p className="text-sm text-muted-foreground">
          PreFlight Round 2 runs <code>doctor.audit.full</code> against
          every selected service. The Commit button below atomically merges
          your draft into <code>~/.lab/.env</code> and creates a backup.
        </p>
      </section>

      <div className="rounded-md border">
        <header className="flex items-center justify-between border-b px-3 py-2 text-sm">
          <span className="font-medium">Audit results</span>
          {auditing ? (
            <span className="flex items-center gap-1 text-muted-foreground">
              <Loader2 className="h-3 w-3 animate-spin" /> running
            </span>
          ) : null}
        </header>
        {auditError ? (
          <div className="p-3 text-sm text-destructive">{auditError}</div>
        ) : null}
        {audit ? (
          <ul className="divide-y">
            {audit.findings.map((finding, idx) => (
              <li
                key={`${finding.service ?? finding.category ?? 'system'}-${idx}`}
                className="flex items-start gap-3 p-3 text-sm"
              >
                {finding.severity === 'error' ? (
                  <AlertTriangle className="h-4 w-4 mt-0.5 text-destructive" />
                ) : finding.severity === 'warn' ? (
                  <AlertTriangle className="h-4 w-4 mt-0.5 text-amber-500" />
                ) : (
                  <CheckCircle2 className="h-4 w-4 mt-0.5 text-emerald-600" />
                )}
                <div>
                  <p className="font-medium">{finding.service ?? finding.category ?? 'system'}</p>
                  <p className="text-muted-foreground">{finding.message}</p>
                </div>
              </li>
            ))}
          </ul>
        ) : null}
      </div>

      {commitError ? (
        <div className="rounded-md border border-destructive/50 bg-destructive/10 p-3 text-sm text-destructive">
          {commitError}
        </div>
      ) : null}

      {commitOutcome ? (
        <div className="rounded-md border border-emerald-600/50 bg-emerald-50 p-3 text-sm">
          <p className="font-medium text-emerald-700">
            ✓ Wrote {commitOutcome.written} entries to ~/.lab/.env
          </p>
          {commitOutcome.backup_path ? (
            <p className="text-xs text-muted-foreground mt-1">
              Backup: {commitOutcome.backup_path}
            </p>
          ) : null}
          <p className="text-xs text-muted-foreground mt-1">
            Audit: {commitOutcome.audit_pass_count} / {commitOutcome.audit_total_count} passing.
            Redirecting to Settings…
          </p>
          <p className="text-xs text-muted-foreground mt-1">{pluginSummary}</p>
        </div>
      ) : null}

      <div className="flex items-center justify-between">
        <Button
          onClick={commit}
          disabled={!allPass || committing || commitOutcome !== undefined}
        >
          {committing ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : null}
          Commit configuration
        </Button>
        <Button variant="outline" onClick={runAudit} disabled={auditing}>
          Re-run audit
        </Button>
      </div>

      <NavButtons />
    </div>
  )
}
