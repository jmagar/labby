import { CircleAlert, RefreshCw } from 'lucide-react'
import { Button } from '@/components/ui/button'

/** Inline error state with optional retry — for failed data loads. */
export function ErrorNotice({
  message,
  onRetry,
}: {
  message: string
  onRetry?: () => void
}) {
  return (
    <div className="flex flex-col items-start gap-3 rounded-aurora-3 border border-aurora-error/30 bg-aurora-error/8 p-4 sm:flex-row sm:items-center sm:justify-between">
      <div className="flex items-start gap-3">
        <CircleAlert className="size-5 shrink-0 text-aurora-error" />
        <p className="text-sm text-aurora-text-primary">{message}</p>
      </div>
      {onRetry ? (
        <Button variant="outline" size="sm" className="shrink-0" onClick={onRetry}>
          <RefreshCw className="mr-1.5 size-3.5" />
          Retry
        </Button>
      ) : null}
    </div>
  )
}
