import { Cable, Plus } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import {
  AURORA_DISPLAY_2,
  AURORA_MUTED_LABEL,
  AURORA_STRONG_PANEL,
} from '@/components/aurora/tokens'
import { gatewayActionTone } from './gateway-theme'

interface EmptyStateProps {
  title: string
  description: string
  action?: {
    label: string
    onClick: () => void
  }
  icon?: React.ReactNode
  className?: string
}

export function EmptyState({ 
  title, 
  description, 
  action, 
  icon,
  className 
}: EmptyStateProps) {
  return (
    <div className={cn(AURORA_STRONG_PANEL, 'flex flex-col items-center justify-center px-4 py-16 text-center', className)}>
      <div className="flex size-14 items-center justify-center rounded-full border border-aurora-border-strong bg-[linear-gradient(180deg,rgba(18,40,56,0.96),rgba(14,31,44,0.98))] shadow-[0_10px_18px_rgba(0,0,0,0.16),var(--aurora-highlight-medium)]">
        {icon || <Cable className="size-6 text-aurora-accent-strong" />}
      </div>
      <p className={cn(AURORA_MUTED_LABEL, 'mt-5')}>Server fleet</p>
      <h3 className={cn(AURORA_DISPLAY_2, 'mt-2 text-aurora-text-primary')}>{title}</h3>
      <p className="mt-2 max-w-sm text-sm leading-6 text-aurora-text-muted">
        {description}
      </p>
      {action && (
        <Button
          onClick={action.onClick}
          className={cn(gatewayActionTone('accent'), 'mt-6 border px-4 text-aurora-text-primary hover:bg-[var(--aurora-panel-strong-top)] hover:text-aurora-text-primary')}
        >
          <Plus className="mr-2 size-4" />
          {action.label}
        </Button>
      )}
    </div>
  )
}
