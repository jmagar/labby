'use client'

import { ShieldAlert, Wrench, X } from 'lucide-react'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Button } from '@/components/ui/button'
import type { Gateway, GatewayCleanupResult } from '@/lib/types/gateway'

interface CleanupResultPanelProps {
  result: { gateway: Gateway; result: GatewayCleanupResult } | null
  onClose: () => void
}

export function CleanupResultPanel({ result, onClose }: CleanupResultPanelProps) {
  if (!result) return null

  const { gateway, result: cleanup } = result
  const isPreview = cleanup.dry_run === true
  const totalMatched =
    (cleanup.gateway_matched ?? cleanup.gateway_killed) +
    (cleanup.local_matched ?? cleanup.local_killed) +
    (cleanup.aggressive_matched ?? cleanup.aggressive_killed)
  const totalKilled =
    cleanup.gateway_killed + cleanup.local_killed + cleanup.aggressive_killed
  const totalPrimary = isPreview ? totalMatched : totalKilled
  const laneLabel = isPreview ? 'matches' : 'terminated'
  const laneVerb = isPreview ? 'matched' : 'terminated'
  const renderMatches = (
    title: string,
    matches: GatewayCleanupResult['gateway_matches'] | undefined,
  ) => {
    if (!matches || matches.length === 0) return null
    return (
      <div className="space-y-2">
        <h5 className="text-xs font-medium uppercase tracking-wide text-aurora-text-muted">
          {title}
        </h5>
        <div className="space-y-2">
          {matches.map((match) => (
            <div key={match.pattern} className="rounded-lg border px-4 py-3">
              <div className="flex items-center justify-between gap-3">
                <code className="text-xs">{match.pattern}</code>
                <span className="text-xs font-medium tabular-nums">
                  {match.pids.length} pid{match.pids.length === 1 ? '' : 's'}
                </span>
              </div>
              <p className="mt-2 text-xs text-aurora-text-muted break-all">
                {match.pids.join(', ')}
              </p>
            </div>
          ))}
        </div>
      </div>
    )
  }

  return (
    <Sheet open={!!result} onOpenChange={(open) => !open && onClose()}>
      <SheetContent className="sm:max-w-md">
        <SheetHeader>
          <SheetTitle>Cleanup Results</SheetTitle>
          <SheetDescription>
            Runtime cleanup results for {gateway.name}
          </SheetDescription>
        </SheetHeader>

        <div className="mt-6 space-y-6">
          <div
            className={`flex items-start gap-4 rounded-lg border p-4 ${
              cleanup.aggressive
                ? 'border-aurora-warn/20 bg-aurora-warn/5'
                : 'border-aurora-success/20 bg-aurora-success/5'
            }`}
          >
            {cleanup.aggressive ? (
              <ShieldAlert className="size-5 text-aurora-warn mt-0.5" />
            ) : (
              <Wrench className="size-5 text-aurora-success mt-0.5" />
            )}
            <div className="flex-1">
              <p
                className={`font-medium ${
                  cleanup.aggressive ? 'text-aurora-warn' : 'text-aurora-success'
                }`}
              >
                {isPreview
                  ? cleanup.aggressive
                    ? 'Aggressive cleanup preview'
                    : 'Runtime cleanup preview'
                  : cleanup.aggressive
                    ? 'Aggressive cleanup completed'
                    : 'Runtime cleanup completed'}
              </p>
              <p className="text-sm text-aurora-text-muted mt-0.5">
                {totalPrimary} process{totalPrimary === 1 ? '' : 'es'} {laneVerb}.
              </p>
              <p className="mt-2 text-xs text-aurora-text-muted">
                Server-side tracked matches, local leaked session workers, and the aggressive fallback lane are reported separately below.
              </p>
            </div>
          </div>

          <div className="space-y-3">
            <h4 className="text-sm font-medium text-aurora-text-muted">Cleanup breakdown</h4>
            <div className="grid gap-3">
              <div className="flex items-center justify-between rounded-lg border px-4 py-3">
                <span className="text-sm">Server runtime {laneLabel}</span>
                <span className="text-sm font-medium tabular-nums">
                  {isPreview ? (cleanup.gateway_matched ?? cleanup.gateway_killed) : cleanup.gateway_killed}
                </span>
              </div>
              <div className="flex items-center justify-between rounded-lg border px-4 py-3">
                <span className="text-sm">Local client/session {laneLabel}</span>
                <span className="text-sm font-medium tabular-nums">
                  {isPreview ? (cleanup.local_matched ?? cleanup.local_killed) : cleanup.local_killed}
                </span>
              </div>
              {cleanup.aggressive && (
                <div className="flex items-center justify-between rounded-lg border px-4 py-3">
                  <span className="text-sm">Aggressive fallback {laneLabel}</span>
                  <span className="text-sm font-medium tabular-nums">
                    {isPreview
                      ? (cleanup.aggressive_matched ?? cleanup.aggressive_killed)
                      : cleanup.aggressive_killed}
                  </span>
                </div>
              )}
            </div>
          </div>

          <div className="space-y-4">
            {renderMatches('Server runtime patterns', cleanup.gateway_matches)}
            {renderMatches('Local client/session patterns', cleanup.local_matches)}
            {cleanup.aggressive && renderMatches('Aggressive fallback patterns', cleanup.aggressive_matches)}
          </div>
        </div>

        <div className="mt-8">
          <Button variant="outline" onClick={onClose} className="w-full">
            <X className="size-4 mr-2" />
            Close
          </Button>
        </div>
      </SheetContent>
    </Sheet>
  )
}
