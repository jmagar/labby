'use client'

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { cn } from '@/lib/utils'

interface ActionConfirmationDialogProps {
  open: boolean
  title: string
  description: string
  confirmLabel: string
  cancelLabel?: string
  busy?: boolean
  error?: { title: string; detail: string }
  onOpenChange: (open: boolean) => void
  onConfirm: () => void
}

export function ActionConfirmationDialog({
  open,
  title,
  description,
  confirmLabel,
  cancelLabel = 'Cancel',
  busy = false,
  error,
  onOpenChange,
  onConfirm,
}: ActionConfirmationDialogProps) {
  return (
    <AlertDialog open={open} onOpenChange={(nextOpen) => {
      if (busy && !nextOpen) return
      onOpenChange(nextOpen)
    }}>
      <AlertDialogContent className="border-aurora-border-strong bg-aurora-panel-strong text-aurora-text-primary">
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription className="text-aurora-text-muted">
            {description}
          </AlertDialogDescription>
        </AlertDialogHeader>
        {error ? (
          <div role="alert" className="rounded-aurora-1 border border-aurora-error/35 bg-aurora-error/5 p-3">
            <strong className="text-sm text-aurora-error">{error.title}</strong>
            <p className="mt-1 text-xs text-aurora-text-muted">{error.detail}</p>
          </div>
        ) : null}
        <AlertDialogFooter>
          <AlertDialogCancel disabled={busy}>{cancelLabel}</AlertDialogCancel>
          <AlertDialogAction
            disabled={busy}
            onClick={(event) => {
              event.preventDefault()
              onConfirm()
            }}
            className={cn(
              'bg-destructive text-white hover:bg-destructive/90 focus-visible:ring-destructive/30',
              busy && 'cursor-wait opacity-70',
            )}
          >
            {busy ? 'Working...' : confirmLabel}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
