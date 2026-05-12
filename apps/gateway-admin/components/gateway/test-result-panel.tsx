'use client'

import { CheckCircle2, XCircle, Clock, Wrench, FileText, MessageSquare, X } from 'lucide-react'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Button } from '@/components/ui/button'
import type { Gateway, TestGatewayResult } from '@/lib/types/gateway'

interface TestResultPanelProps {
  result: { gateway: Gateway; result: TestGatewayResult } | null
  onClose: () => void
}

export function TestResultPanel({ result, onClose }: TestResultPanelProps) {
  if (!result) return null

  const { gateway, result: testResult } = result
  const severity = testResult.severity ?? (testResult.success ? 'success' : 'failure')
  const isSuccess = severity === 'success'
  const isWarning = severity === 'warning'

  return (
    <Sheet open={!!result} onOpenChange={(open) => !open && onClose()}>
      <SheetContent className="sm:max-w-md">
        <SheetHeader>
          <SheetTitle>Connection Test Results</SheetTitle>
          <SheetDescription>
            Test results for {gateway.name}
          </SheetDescription>
        </SheetHeader>

        <div className="mt-6 space-y-6">
          {/* Status */}
          <div className={`flex items-start gap-4 rounded-lg border p-4 ${
            isSuccess
              ? 'border-aurora-success/20 bg-aurora-success/5'
              : isWarning
                ? 'border-aurora-warn/20 bg-aurora-warn/5'
                : 'border-aurora-error/20 bg-aurora-error/5'
          }`}>
            {isSuccess ? (
              <CheckCircle2 className="size-5 text-aurora-success mt-0.5" />
            ) : isWarning ? (
              <Clock className="size-5 text-aurora-warn mt-0.5" />
            ) : (
              <XCircle className="size-5 text-aurora-error mt-0.5" />
            )}
            <div className="flex-1">
              <p className={`font-medium ${
                isSuccess
                  ? 'text-aurora-success'
                  : isWarning
                    ? 'text-aurora-warn'
                    : 'text-aurora-error'
              }`}>
                {isSuccess
                  ? 'Connection Successful'
                  : isWarning
                    ? 'Connection Successful with Warnings'
                    : 'Connection Failed'}
              </p>
              <p className="text-sm text-aurora-text-muted mt-0.5">
                {testResult.message}
              </p>
              {testResult.detail && (
                <p className="text-sm text-aurora-warn mt-2 font-mono bg-aurora-warn/10 rounded px-2 py-1">
                  {testResult.detail}
                </p>
              )}
              {testResult.error && (
                <p className="text-sm text-aurora-error mt-2 font-mono bg-aurora-error/10 rounded px-2 py-1">
                  {testResult.error}
                </p>
              )}
              {(isWarning || !testResult.success) && (
                <p className="mt-2 text-xs text-aurora-text-muted">
                  {isWarning
                    ? 'The server connected, but at least one optional MCP primitive could not be discovered. The note above is the exact operator-facing guidance returned by the server backend.'
                    : 'Check the server transport, auth source, and any required stdio environment variables. The probe message above is the exact last failure returned by the server backend.'}
                </p>
              )}
            </div>
          </div>

          {/* Metrics */}
          {(testResult.success ||
            testResult.discovered_tools !== undefined ||
            testResult.discovered_resources !== undefined ||
            testResult.discovered_prompts !== undefined) && (
            <div className="space-y-3">
              <h4 className="text-sm font-medium text-aurora-text-muted">
                {isSuccess ? 'Connection Details' : 'Probe Details'}
              </h4>
              
              <div className="grid gap-3">
                {testResult.latency_ms !== undefined && (
                  <div className="flex items-center justify-between rounded-lg border px-4 py-3">
                    <div className="flex items-center gap-2 text-sm">
                      <Clock className="size-4 text-aurora-text-muted" />
                      <span>Latency</span>
                    </div>
                    <span className="text-sm font-medium tabular-nums">
                      {testResult.latency_ms}ms
                    </span>
                  </div>
                )}

                {testResult.discovered_tools !== undefined && (
                  <div className="flex items-center justify-between rounded-lg border px-4 py-3">
                    <div className="flex items-center gap-2 text-sm">
                      <Wrench className="size-4 text-aurora-text-muted" />
                      <span>Discovered Tools</span>
                    </div>
                    <span className="text-sm font-medium tabular-nums">
                      {testResult.discovered_tools}
                    </span>
                  </div>
                )}

                {testResult.discovered_resources !== undefined && (
                  <div className="flex items-center justify-between rounded-lg border px-4 py-3">
                    <div className="flex items-center gap-2 text-sm">
                      <FileText className="size-4 text-aurora-text-muted" />
                      <span>Discovered Resources</span>
                    </div>
                    <span className="text-sm font-medium tabular-nums">
                      {testResult.discovered_resources}
                    </span>
                  </div>
                )}

                {testResult.discovered_prompts !== undefined && (
                  <div className="flex items-center justify-between rounded-lg border px-4 py-3">
                    <div className="flex items-center gap-2 text-sm">
                      <MessageSquare className="size-4 text-aurora-text-muted" />
                      <span>Discovered Prompts</span>
                    </div>
                    <span className="text-sm font-medium tabular-nums">
                      {testResult.discovered_prompts}
                    </span>
                  </div>
                )}
              </div>
            </div>
          )}
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
