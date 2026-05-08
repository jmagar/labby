import { Badge } from '@/components/ui/badge'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
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
  AURORA_DISPLAY_2,
  AURORA_DISPLAY_NUMBER,
  AURORA_MEDIUM_PANEL,
  AURORA_MUTED_LABEL,
  AURORA_STRONG_PANEL,
} from '@/components/aurora/tokens'
import { denseRows, keyValueBlocks, metricCards } from './demo-data'

export function DataDisplaySection() {
  return (
    <section className={cn(AURORA_STRONG_PANEL, 'overflow-hidden')}>
      <div className="border-b border-aurora-border-strong px-5 py-4">
        <p className={AURORA_MUTED_LABEL}>Data Display</p>
        <h2 className={cn(AURORA_DISPLAY_2, 'mt-2 text-aurora-text-primary')}>
          Metrics, tables, and inspectors
        </h2>
        <p className="mt-2 max-w-2xl text-sm text-aurora-text-muted">
          Keep dense operational information readable while preserving enough contrast for status
          scanning.
        </p>
      </div>

      <div className="space-y-4 px-5 py-5">
        <section className="space-y-3">
          <p className="text-sm font-medium text-aurora-text-primary">Card variants</p>
          <div className="grid gap-3 md:grid-cols-2">
            <Card variant="medium">
              <CardHeader>
                <CardTitle>Medium card</CardTitle>
                <CardDescription>
                  Default emphasis for list items and secondary panels.
                </CardDescription>
              </CardHeader>
              <CardContent>
                <p className="text-sm text-aurora-text-muted">
                  Uses <code className="font-mono text-xs">bg-aurora-panel-medium</code> with the medium shadow tier.
                </p>
              </CardContent>
            </Card>
            <Card variant="strong">
              <CardHeader>
                <CardTitle>Strong card</CardTitle>
                <CardDescription>
                  Elevated surfaces like hero inspectors and pinned detail views.
                </CardDescription>
              </CardHeader>
              <CardContent>
                <p className="text-sm text-aurora-text-muted">
                  Uses <code className="font-mono text-xs">bg-aurora-panel-strong</code> with the strong shadow tier.
                </p>
              </CardContent>
            </Card>
          </div>
        </section>

        <section className="space-y-3">
          <p className="text-sm font-medium text-aurora-text-primary">Metric cards</p>
          <div className="grid gap-3 md:grid-cols-3">
            {metricCards.map((card) => (
              <article key={card.label} className={cn(AURORA_MEDIUM_PANEL, 'px-4 py-4')}>
                <p className={AURORA_MUTED_LABEL}>{card.label}</p>
                <p className={cn(AURORA_DISPLAY_NUMBER, 'mt-3 text-aurora-text-primary')}>
                  {card.value}
                </p>
                <p className="mt-2 text-sm text-aurora-text-muted">{card.detail}</p>
              </article>
            ))}
          </div>
        </section>

        <div className="grid gap-4 xl:grid-cols-[minmax(0,1.1fr)_minmax(0,0.9fr)]">
          <section className={cn(AURORA_MEDIUM_PANEL, 'space-y-4 px-4 py-4')}>
            <p className="text-sm font-medium text-aurora-text-primary">Dense table rows</p>
            <div className="aurora-scrollbar overflow-x-auto">
            <Table className="min-w-0 md:min-w-[560px]">
              <TableHeader>
                <TableRow className="border-aurora-border-strong text-aurora-text-muted">
                  <TableHead>Gateway</TableHead>
                  <TableHead>Transport</TableHead>
                  <TableHead>Tools</TableHead>
                  <TableHead>Health</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {denseRows.map((row) => (
                  <TableRow key={row.gateway} className="border-aurora-border-strong text-aurora-text-primary">
                    <TableCell className="font-medium">{row.gateway}</TableCell>
                    <TableCell className="font-mono text-xs text-aurora-text-muted">{row.transport}</TableCell>
                    <TableCell>{row.tools}</TableCell>
                    <TableCell>
                      <Badge
                        variant="outline"
                        status={
                          row.health === 'Healthy'
                            ? 'success'
                            : row.health === 'Warning'
                              ? 'warn'
                              : row.health === 'Error'
                                ? 'error'
                                : 'default'
                        }
                      >
                        {row.health}
                      </Badge>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
            </div>
          </section>

          <section className={cn(AURORA_MEDIUM_PANEL, 'space-y-4 px-4 py-4')}>
            <p className="text-sm font-medium text-aurora-text-primary">Key/value blocks</p>
            <div className="space-y-3">
              {keyValueBlocks.map((item) => (
                <div
                  key={item.label}
                  className="grid gap-1 rounded-aurora-1 border border-aurora-border-strong bg-aurora-control-surface px-3 py-3 sm:grid-cols-[120px_minmax(0,1fr)] sm:gap-3"
                >
                  <span className="text-sm text-aurora-text-muted">{item.label}</span>
                  <span className="min-w-0 break-words text-sm font-medium text-aurora-text-primary">
                    {item.value}
                  </span>
                </div>
              ))}
            </div>
          </section>
        </div>
      </div>
    </section>
  )
}
