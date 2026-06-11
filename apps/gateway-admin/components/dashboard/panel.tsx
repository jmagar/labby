import type { ReactNode } from 'react'
import { AURORA_MEDIUM_PANEL, AURORA_MUTED_LABEL } from '@/components/aurora/tokens'
import { DASH_SURFACE } from './ui'
import { cn } from '@/lib/utils'

/** Shared squared panel shell for dashboard insight/analysis cards. */
export function DashboardPanel({
  title,
  icon,
  meta,
  action,
  className,
  children,
}: {
  title: string
  icon?: ReactNode
  meta?: ReactNode
  action?: ReactNode
  className?: string
  children: ReactNode
}) {
  return (
    <div className={cn(AURORA_MEDIUM_PANEL, DASH_SURFACE, 'flex flex-col gap-3 p-5', className)}>
      <div className="flex items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-2">
          {icon ? <span className="shrink-0 text-aurora-accent-primary">{icon}</span> : null}
          <p className={AURORA_MUTED_LABEL}>{title}</p>
        </div>
        {meta ? <span className="shrink-0 text-xs text-aurora-text-muted">{meta}</span> : action}
      </div>
      {children}
    </div>
  )
}
