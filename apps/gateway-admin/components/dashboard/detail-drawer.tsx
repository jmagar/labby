'use client'

import type { ReactNode } from 'react'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { AURORA_MUTED_LABEL, AURORA_STAT_PANEL } from '@/components/aurora/tokens'
import { DASH_CONTROL, DASH_INNER, DASH_METRIC_SM } from './ui'
import { cn } from '@/lib/utils'

/** Shared right-side drawer shell for entity drill-downs. */
export function DetailDrawer({
  open,
  onClose,
  icon,
  title,
  subtitle,
  children,
}: {
  open: boolean
  onClose: () => void
  icon: ReactNode
  title: ReactNode
  subtitle: ReactNode
  children: ReactNode
}) {
  return (
    <Sheet open={open} onOpenChange={(next) => { if (!next) onClose() }}>
      <SheetContent
        side="right"
        className="w-full gap-0 overflow-y-auto bg-aurora-page-bg p-0 sm:max-w-2xl"
      >
        <SheetHeader className="border-b border-aurora-border-strong bg-aurora-panel-strong px-6 py-5">
          <div className="flex items-center gap-3">
            <span className={cn('flex size-10 shrink-0 items-center justify-center bg-aurora-accent-primary/15 text-aurora-accent-primary', DASH_INNER)}>
              {icon}
            </span>
            <div className="min-w-0">
              <SheetTitle className="truncate text-aurora-text-primary">{title}</SheetTitle>
              <SheetDescription className="truncate">{subtitle}</SheetDescription>
            </div>
          </div>
        </SheetHeader>
        <div className="flex flex-col gap-6 px-6 py-5">{children}</div>
      </SheetContent>
    </Sheet>
  )
}

export interface DrawerStat {
  label: string
  value: ReactNode
  tone?: 'default' | 'success' | 'error'
}

const STAT_TONE = {
  default: 'text-aurora-text-primary',
  success: 'text-aurora-success',
  error: 'text-aurora-error',
} as const

export function DrawerStatGrid({ items }: { items: DrawerStat[] }) {
  return (
    <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
      {items.map((item) => (
        <div key={item.label} className={cn(AURORA_STAT_PANEL, DASH_CONTROL)}>
          <p className={cn(DASH_METRIC_SM, STAT_TONE[item.tone ?? 'default'])}>{item.value}</p>
          <p className="mt-1 text-xs text-aurora-text-muted">{item.label}</p>
        </div>
      ))}
    </div>
  )
}

export function DrawerSection({
  title,
  action,
  children,
}: {
  title: string
  action?: ReactNode
  children: ReactNode
}) {
  return (
    <section className="flex flex-col gap-3">
      <div className="flex items-center justify-between gap-3">
        <p className={AURORA_MUTED_LABEL}>{title}</p>
        {action}
      </div>
      {children}
    </section>
  )
}

/** A ranked, optionally-clickable usage row (callers, tools-used). */
export function RankRow({
  label,
  value,
  mono = false,
  onClick,
}: {
  label: ReactNode
  value: ReactNode
  mono?: boolean
  onClick?: () => void
}) {
  const content = (
    <>
      <span className={cn('min-w-0 flex-1 truncate text-sm text-aurora-text-primary', mono && 'font-mono text-[13px]')}>
        {label}
      </span>
      <span className="shrink-0 text-sm font-semibold tabular-nums text-aurora-text-muted">{value}</span>
    </>
  )
  if (onClick) {
    return (
      <button
        type="button"
        onClick={onClick}
        className="flex w-full items-center gap-3 rounded-aurora-1 px-2 py-1.5 text-left transition-colors hover:bg-aurora-hover-bg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary/40"
      >
        {content}
      </button>
    )
  }
  return <div className="flex items-center gap-3 px-2 py-1.5">{content}</div>
}
