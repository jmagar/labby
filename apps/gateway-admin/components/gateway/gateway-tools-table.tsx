'use client'

import { Badge } from '@/components/ui/badge'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { cn } from '@/lib/utils'
import {
  AURORA_GATEWAY_ROW,
  AURORA_MEDIUM_PANEL,
  AURORA_MUTED_LABEL,
  AURORA_GATEWAY_SUBTLE_SURFACE,
  AURORA_STRONG_PANEL,
} from '@/components/gateway/gateway-theme'
import type { ToolInventoryRow } from './gateway-list-state'

const TOOL_RENDER_LIMIT = 300

export function GatewayToolsTable({ rows }: { rows: ToolInventoryRow[] }) {
  const renderedRows = rows.slice(0, TOOL_RENDER_LIMIT)

  return (
    <>
      {rows.length > renderedRows.length ? (
        <div className={cn(AURORA_MEDIUM_PANEL, 'mb-3 p-3 text-sm text-aurora-text-muted')}>
          Showing the first {renderedRows.length} of {rows.length} tools. Search or filter to narrow the inventory.
        </div>
      ) : null}
      <div className="space-y-3 md:hidden">
        {renderedRows.map((row) => (
          <article key={`${row.gatewayId}:${row.toolName}`} className={cn(AURORA_MEDIUM_PANEL, 'space-y-3 p-4')}>
            <div className="space-y-1">
              <div className="flex flex-wrap items-center gap-2">
                <p className="text-sm font-semibold text-aurora-text-primary">{row.toolName}</p>
                <Badge
                  variant="outline"
                  className="rounded-full border-aurora-border-strong bg-aurora-control-surface text-[10px] uppercase tracking-[0.16em] text-aurora-text-muted"
                >
                  {row.exposed ? 'Exposed' : 'Hidden'}
                </Badge>
              </div>
              {row.description ? (
                <p className="text-sm text-aurora-text-muted">{row.description}</p>
              ) : null}
            </div>
            <div className="grid gap-2 rounded-aurora-1 border border-aurora-border-strong bg-aurora-control-surface px-3 py-3">
              <div className="flex items-center justify-between gap-3">
                <span className={AURORA_MUTED_LABEL}>Server</span>
                <span className="text-sm font-medium text-aurora-text-primary">{row.gatewayName}</span>
              </div>
              <div className="flex items-center justify-between gap-3">
                <span className={AURORA_MUTED_LABEL}>Transport</span>
                <span className="text-sm text-aurora-text-primary">{row.transport.toUpperCase()}</span>
              </div>
            </div>
          </article>
        ))}
      </div>

      <div className={cn(AURORA_STRONG_PANEL, 'hidden overflow-hidden md:block')}>
        <Table className="table-fixed">
          <TableHeader>
            <TableRow className={cn('border-b border-aurora-border-strong hover:bg-inherit', AURORA_GATEWAY_SUBTLE_SURFACE)}>
              <TableHead className={cn(AURORA_MUTED_LABEL, 'px-6 py-4')}>Tool</TableHead>
              <TableHead className={cn(AURORA_MUTED_LABEL, 'px-4 py-4')}>Server</TableHead>
              <TableHead className={cn(AURORA_MUTED_LABEL, 'px-4 py-4')}>State</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {renderedRows.map((row) => (
              <TableRow
                key={`${row.gatewayId}:${row.toolName}`}
                className={AURORA_GATEWAY_ROW}
              >
                <TableCell className="px-6 py-4 align-top">
                  <div className="min-w-0">
                    <p className="text-sm font-medium text-aurora-text-primary">{row.toolName}</p>
                    {row.description ? (
                      <p className="text-xs text-aurora-text-muted">{row.description}</p>
                    ) : null}
                  </div>
                </TableCell>
                <TableCell className="px-4 py-4 align-top text-sm text-aurora-text-primary">
                  {row.gatewayName}
                </TableCell>
                <TableCell className="px-4 py-4 align-top text-sm text-aurora-text-primary">
                  {row.exposed ? 'Exposed' : 'Hidden'}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>
    </>
  )
}
