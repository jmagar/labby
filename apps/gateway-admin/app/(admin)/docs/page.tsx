'use client'

import Link from 'next/link'
import {
  ArrowRight,
  BookOpenText,
  CircleHelp,
  ExternalLink,
  Layers3,
  PlugZap,
  Wrench,
} from 'lucide-react'

import { AppHeader } from '@/components/app-header'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import {
  AURORA_DISPLAY_1,
  AURORA_DISPLAY_2,
  AURORA_MUTED_LABEL,
  AURORA_PAGE_FRAME,
  AURORA_PAGE_SHELL,
} from '@/components/aurora/tokens'
import { buildGatewayDocsSnapshot } from '@/lib/dashboard/admin-insights'
import { useGateways, useSupportedServices } from '@/lib/hooks/use-gateways'
import { cn } from '@/lib/utils'

const quickStartSteps = [
  {
    title: 'Add a server',
    detail:
      'Start in Servers to connect an HTTP MCP endpoint or promote a Lab service into its own server.',
    href: '/gateways',
  },
  {
    title: 'Check health and discovery',
    detail:
      'Use Overview and Activity to confirm reachability, discovered tools, and current warnings.',
    href: '/',
  },
  {
    title: 'Constrain exposure',
    detail:
      'Open a server detail page to narrow downstream tool exposure with explicit allowlist patterns.',
    href: '/gateways',
  },
] as const

const conceptCards = [
  {
    icon: PlugZap,
    title: 'Server',
    detail:
      'A managed upstream MCP connection. Servers can be HTTP endpoints, local stdio processes, or Lab-backed services surfaced through the control plane.',
  },
  {
    icon: Layers3,
    title: 'Exposure policy',
    detail:
      'Controls which discovered tools are re-published downstream. Exact names and prefix wildcards are both supported.',
  },
  {
    icon: Wrench,
    title: 'Discovery',
    detail:
      'Each probe refreshes the visible tool, resource, and prompt catalogs so operators can see what a server will expose before clients rely on it.',
  },
] as const

const LINK_CLASS =
  'text-aurora-accent-primary transition-colors hover:text-aurora-accent-strong focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary/34 focus-visible:ring-offset-2 focus-visible:ring-offset-aurora-page-bg rounded-sm'

export default function DocsPage() {
  const {
    data: gateways = [],
    isLoading: gatewaysLoading,
    error: gatewaysError,
  } = useGateways()
  const {
    data: services = [],
    isLoading: servicesLoading,
    error: servicesError,
  } = useSupportedServices()
  const snapshot = buildGatewayDocsSnapshot(gateways, services.length)

  return (
    <div className={AURORA_PAGE_SHELL}>
      <AppHeader breadcrumbs={[{ label: 'Documentation' }]} />
      <div className={AURORA_PAGE_FRAME}>
        {/* Hero — compact, operator-first */}
        <Card variant="strong" className="gap-4 p-6">
          <div className="flex flex-col gap-5 lg:flex-row lg:items-start lg:justify-between">
            <div className="max-w-[72ch] space-y-3">
              <div className="flex size-10 items-center justify-center rounded-full border border-aurora-border-default bg-aurora-control-surface text-aurora-accent-primary">
                <BookOpenText className="size-5" />
              </div>
              <div className="space-y-2">
                <h1 className={AURORA_DISPLAY_1}>Operator guide</h1>
                <p className="text-[14px] leading-[1.55] text-aurora-text-muted">
                  The admin console manages MCP servers, shows what each upstream
                  exposes, and lets you tighten downstream tool publication before
                  client traffic depends on it.
                </p>
              </div>
              <div className="flex flex-wrap gap-2 pt-1">
                <Badge variant="outline">
                  {snapshot.totalGateways} servers tracked
                </Badge>
                <Badge variant="outline" status="success">
                  {snapshot.connectedGateways} connected
                </Badge>
                <Badge variant="outline">
                  {snapshot.exposedTools} tools exposed
                </Badge>
                <Badge variant="outline">
                  {snapshot.supportedServices} Lab services available
                </Badge>
              </div>
            </div>
            <div className="flex flex-wrap gap-2">
              <Button asChild>
                <Link href="/gateways">
                  Open servers
                  <ArrowRight className="ml-2 size-4" />
                </Link>
              </Button>
              <Button variant="outline" asChild>
                <a
                  href="https://modelcontextprotocol.io"
                  target="_blank"
                  rel="noopener noreferrer"
                >
                  MCP specification
                  <ExternalLink className="ml-2 size-4" />
                </a>
              </Button>
            </div>
          </div>
        </Card>

        {/* Stat row — medium panels, compact density */}
        <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
          <Card variant="medium" className="gap-1.5 p-4">
            <p className={AURORA_MUTED_LABEL}>Warnings</p>
            <p
              className={cn(
                AURORA_DISPLAY_2,
                'text-[24px] leading-[1.1] text-aurora-warn tabular-nums',
              )}
            >
              {snapshot.warningCount}
            </p>
            <p className="text-[13px] leading-[1.5] text-aurora-text-muted">
              Current warning backlog across all servers.
            </p>
          </Card>
          <Card variant="medium" className="gap-1.5 p-4">
            <p className={AURORA_MUTED_LABEL}>HTTP Servers</p>
            <p
              className={cn(
                AURORA_DISPLAY_2,
                'text-[24px] leading-[1.1] text-aurora-text-primary tabular-nums',
              )}
            >
              {snapshot.httpGateways}
            </p>
            <p className="text-[13px] leading-[1.5] text-aurora-text-muted">
              Network-connected MCP upstreams.
            </p>
          </Card>
          <Card variant="medium" className="gap-1.5 p-4">
            <p className={AURORA_MUTED_LABEL}>stdio Servers</p>
            <p
              className={cn(
                AURORA_DISPLAY_2,
                'text-[24px] leading-[1.1] text-aurora-text-primary tabular-nums',
              )}
            >
              {snapshot.stdioGateways}
            </p>
            <p className="text-[13px] leading-[1.5] text-aurora-text-muted">
              Local process-based MCP upstreams.
            </p>
          </Card>
          <Card variant="medium" className="gap-1.5 p-4">
            <p className={AURORA_MUTED_LABEL}>Lab Services</p>
            <p
              className={cn(
                AURORA_DISPLAY_2,
                'text-[24px] leading-[1.1] text-aurora-text-primary tabular-nums',
              )}
            >
              {snapshot.supportedServices}
            </p>
            <p className="text-[13px] leading-[1.5] text-aurora-text-muted">
              Built-in services that can become servers.
            </p>
          </Card>
        </div>

        <div className="grid gap-5 xl:grid-cols-[1.1fr_0.9fr]">
          {/* Quick start — primary content, strong panel */}
          <Card variant="strong" className="gap-4 p-6">
            <div className="flex items-start gap-3">
              <CircleHelp className="mt-0.5 size-5 text-aurora-accent-primary" />
              <div className="space-y-1">
                <h2
                  className={cn(
                    AURORA_DISPLAY_2,
                    'text-[18px] leading-[1.25] text-aurora-text-primary',
                  )}
                >
                  Quick start
                </h2>
                <p className="text-[13px] leading-[1.5] text-aurora-text-muted">
                  The shortest path to a working, constrained server setup.
                </p>
              </div>
            </div>
            <ol className="space-y-3">
              {quickStartSteps.map((step, index) => (
                <li
                  key={step.title}
                  className="flex gap-3 rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface p-4"
                >
                  <div className="flex size-7 shrink-0 items-center justify-center rounded-full border border-aurora-accent-primary/28 bg-aurora-panel-medium text-[13px] font-semibold text-aurora-accent-primary tabular-nums">
                    {index + 1}
                  </div>
                  <div className="min-w-0 flex-1 space-y-1">
                    <div className="flex items-center justify-between gap-3">
                      <h3 className="text-[14px] font-medium text-aurora-text-primary">
                        {step.title}
                      </h3>
                      <Button variant="ghost" size="sm" asChild>
                        <Link href={step.href}>Open</Link>
                      </Button>
                    </div>
                    <p className="text-[13px] leading-[1.55] text-aurora-text-muted">
                      {step.detail}
                    </p>
                  </div>
                </li>
              ))}
            </ol>
          </Card>

          {/* Key concepts — nav/inspector style, medium panel */}
          <Card variant="medium" className="gap-4 p-6">
            <h2
              className={cn(
                AURORA_DISPLAY_2,
                'text-[18px] leading-[1.25] text-aurora-text-primary',
              )}
            >
              Key concepts
            </h2>
            <div className="space-y-3">
              {conceptCards.map((card) => {
                const Icon = card.icon
                return (
                  <div
                    key={card.title}
                    className="rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface p-4"
                  >
                    <div className="flex items-center gap-3">
                      <div className="flex size-8 items-center justify-center rounded-lg border border-aurora-border-default bg-aurora-panel-medium text-aurora-accent-primary">
                        <Icon className="size-4" />
                      </div>
                      <h3 className="text-[14px] font-medium text-aurora-text-primary">
                        {card.title}
                      </h3>
                    </div>
                    <p className="mt-2 text-[13px] leading-[1.55] text-aurora-text-muted">
                      {card.detail}
                    </p>
                  </div>
                )
              })}
            </div>
          </Card>
        </div>

        {/* Environment notes */}
        <Card variant="strong" className="gap-4 p-6">
          <h2
            className={cn(
              AURORA_DISPLAY_2,
              'text-[18px] leading-[1.25] text-aurora-text-primary',
            )}
          >
            Current environment notes
          </h2>
          {gatewaysLoading || servicesLoading ? (
            <p className="text-[14px] leading-[1.55] text-aurora-text-muted">
              Loading server and service metadata…
            </p>
          ) : gatewaysError || servicesError ? (
            <Alert variant="error">
              <AlertTitle>Environment guidance unavailable</AlertTitle>
              <AlertDescription>
                Unable to build environment-aware guidance because the current
                server metadata could not be loaded.
              </AlertDescription>
            </Alert>
          ) : (
            <div className="grid gap-3 md:grid-cols-2">
              <div className="rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface p-4">
                <p className="text-[14px] font-medium text-aurora-text-primary">
                  Server posture
                </p>
                <p className="mt-1 text-[13px] leading-[1.55] text-aurora-text-muted">
                  {snapshot.connectedGateways} of {snapshot.totalGateways} servers
                  are connected right now, with {snapshot.warningCount} warning
                  {snapshot.warningCount === 1 ? '' : 's'} still visible in the fleet.
                </p>
              </div>
              <div className="rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface p-4">
                <p className="text-[14px] font-medium text-aurora-text-primary">
                  Lab-backed onboarding
                </p>
                <p className="mt-1 text-[13px] leading-[1.55] text-aurora-text-muted">
                  The add-server flow currently exposes {snapshot.supportedServices}{' '}
                  supported Lab services for quick setup without hand-writing
                  upstream process details.
                </p>
              </div>
            </div>
          )}
          {!gatewaysLoading &&
            !servicesLoading &&
            !gatewaysError &&
            !servicesError &&
            snapshot.warningCount > 0 && (
              <Alert variant="warn">
                <AlertTitle>Warnings require attention</AlertTitle>
                <AlertDescription>
                  {snapshot.warningCount} warning
                  {snapshot.warningCount === 1 ? '' : 's'} still open across
                  tracked servers. Review them in{' '}
                  <Link href="/activity" className={LINK_CLASS}>
                    Activity
                  </Link>{' '}
                  before extending exposure.
                </AlertDescription>
              </Alert>
            )}
        </Card>
      </div>
    </div>
  )
}
