import type { ElementType, ReactNode } from 'react'
import { Card, CardContent } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { DASH_INNER, DASH_METRIC, DASH_SURFACE } from './ui'
import { cn } from '@/lib/utils'

/** Icon-container tones — Aurora semantic tokens only, no raw hex. */
const TILE_TONE = {
  default: 'bg-aurora-accent-primary/15 text-aurora-accent-primary',
  success: 'bg-aurora-accent-strong/15 text-aurora-accent-strong',
  warning: 'bg-aurora-warn/15 text-aurora-warn',
  error: 'bg-aurora-error/15 text-aurora-error',
  info: 'bg-aurora-accent-deep/20 text-aurora-accent-strong',
} as const

export type StatTileTone = keyof typeof TILE_TONE

export interface StatTileProps {
  label: string
  value: ReactNode
  icon: ElementType
  tone?: StatTileTone
  loading?: boolean
}

/** Compact metric tile: icon + big value + short label. No description line. */
export function StatTile({
  label,
  value,
  icon: Icon,
  tone = 'default',
  loading = false,
}: StatTileProps) {
  return (
    <Card variant="medium" className={DASH_SURFACE}>
      <CardContent className="p-0">
        <div className="flex items-center gap-3 p-4">
          <div
            className={cn(
              'flex size-10 shrink-0 items-center justify-center',
              DASH_INNER,
              TILE_TONE[tone],
            )}
          >
            <Icon className="size-5" />
          </div>
          <div className="min-w-0">
            {loading ? (
              <>
                <Skeleton className="mb-1 h-7 w-10" />
                <Skeleton className="h-4 w-16" />
              </>
            ) : (
              <>
                <p className={cn(DASH_METRIC, 'text-aurora-text-primary')}>{value}</p>
                <p className="text-sm font-medium leading-tight text-aurora-text-muted">{label}</p>
              </>
            )}
          </div>
        </div>
      </CardContent>
    </Card>
  )
}
