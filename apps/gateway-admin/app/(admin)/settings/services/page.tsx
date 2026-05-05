'use client'

// Service catalog overview — lists every service from setup.schema.get
// with a click-through to its dedicated /settings/services/[slug]/ page.
// The "Configured" indicator reflects whether all required env vars are
// present in the live ~/.lab/.env (via setup.state's missing array).

import { useEffect, useMemo, useState } from 'react'
import Link from 'next/link'
import { ChevronRight, Loader2, CircleAlert, CircleCheck } from 'lucide-react'

import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { PluginToggle } from '@/components/setup/PluginToggle'
import { setupApi, type ServiceSchema, type SetupSnapshot, type ServiceStatus } from '@/lib/api/setup-client'

interface ServiceRow {
  schema: ServiceSchema
  configured: boolean
  pluginInstalled: boolean
}

export default function ServicesIndex(): React.ReactElement {
  const [services, setServices] = useState<ServiceSchema[]>([])
  const [snapshot, setSnapshot] = useState<SetupSnapshot | undefined>()
  const [statuses, setStatuses] = useState<ServiceStatus[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | undefined>()

  useEffect(() => {
    const controller = new AbortController()
    Promise.all([
      setupApi.schemaGet(undefined, controller.signal),
      setupApi.state(controller.signal),
      setupApi.servicesStatus(controller.signal),
    ])
      .then(([schemaResponse, snap, statusResponse]) => {
        if (controller.signal.aborted) return
        setServices(
          Object.values(schemaResponse.services).sort((a, b) =>
            a.display_name.localeCompare(b.display_name),
          ),
        )
        setSnapshot(snap)
        setStatuses(statusResponse.services)
      })
      .catch((err) => {
        if (controller.signal.aborted) return
        setError(err instanceof Error ? err.message : 'load failed')
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false)
      })
    return () => controller.abort()
  }, [])

  const rows = useMemo<ServiceRow[]>(() => {
    const missing = new Set<string>(
      snapshot?.state.kind === 'partially_configured'
        ? snapshot.state.missing ?? []
        : snapshot?.state.kind === 'config_missing'
          ? snapshot.state.envars ?? []
          : [],
    )
    const statusByName = new Map(statuses.map((status) => [status.name, status]))
    return services.map((schema) => ({
      schema,
      configured: statusByName.get(schema.name)?.configured ?? schema.env
        .filter((e) => e.required)
        .every((e) => !missing.has(e.name)),
      pluginInstalled: statusByName.get(schema.name)?.plugin_installed ?? false,
    }))
  }, [services, snapshot, statuses])

  return (
    <Card>
      <CardHeader>
        <CardTitle>Services</CardTitle>
        <CardDescription>
          Configure connection details for every Bootstrap service. Click a
          row to edit its env vars; saves commit immediately to{' '}
          <code>~/.lab/.env</code>.
        </CardDescription>
      </CardHeader>
      <CardContent>
        {loading ? (
          <div className="flex items-center gap-2 text-sm text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" /> loading catalog
          </div>
        ) : null}
        {error ? (
          <p className="text-sm text-destructive">{error}</p>
        ) : null}
        {!loading && !error ? (
          <ul className="divide-y rounded-md border">
            {rows.map(({ schema, configured, pluginInstalled }) => (
              <li key={schema.name}>
                <div className="flex items-center justify-between gap-3 p-3 text-sm hover:bg-accent/40">
                  <Link href={`/settings/services/${schema.name}/`} className="min-w-0 flex-1">
                    <div>
                      <p className="font-medium">{schema.display_name}</p>
                      {schema.description ? (
                        <p className="text-xs text-muted-foreground">
                          {schema.description}
                        </p>
                      ) : null}
                    </div>
                  </Link>
                  <div className="flex items-center gap-2 text-xs">
                    <PluginToggle service={schema.name} installed={pluginInstalled} disabled={!configured} />
                    {configured ? (
                      <span className="inline-flex items-center gap-1 text-emerald-600">
                        <CircleCheck className="h-3 w-3" /> configured
                      </span>
                    ) : (
                      <span className="inline-flex items-center gap-1 text-amber-600">
                        <CircleAlert className="h-3 w-3" /> incomplete
                      </span>
                    )}
                    <ChevronRight className="h-4 w-4 text-muted-foreground" />
                  </div>
                </div>
              </li>
            ))}
          </ul>
        ) : null}
      </CardContent>
    </Card>
  )
}
