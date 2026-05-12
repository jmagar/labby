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
import type { Gateway } from '@/lib/types/gateway'

interface DisableGatewayDialogProps {
  gateway: Gateway | null
  onOpenChange: (open: boolean) => void
  onConfirm: (gateway: Gateway) => Promise<void> | void
}

export function DisableGatewayDialog({
  gateway,
  onOpenChange,
  onConfirm,
}: DisableGatewayDialogProps) {
  return (
    <AlertDialog open={Boolean(gateway)} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Disable server?</AlertDialogTitle>
          <AlertDialogDescription>
            {gateway ? (
              <>
                This keeps <span className="font-medium">{gateway.name}</span> configured, but
                removes it from the active catalog. Connected clients should no longer have access
                to its tools, resources, or prompts until you re-enable it. Runtime cleanup will
                also be requested automatically.
              </>
            ) : (
              'This server will be removed from the active catalog until you re-enable it, and runtime cleanup will be requested automatically.'
            )}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction
            className="bg-amber-600 text-white hover:bg-amber-500"
            onClick={() => {
              if (gateway) {
                void onConfirm(gateway)
              }
            }}
          >
            Disable server
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
