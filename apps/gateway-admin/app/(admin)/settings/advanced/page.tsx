'use client'

import { useEffect, useState } from 'react'
import { Loader2 } from 'lucide-react'

import { Badge } from '@/components/ui/badge'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { setupApi, type SettingsState } from '@/lib/api/setup-client'

export default function AdvancedPage(): React.ReactElement {
  const [settings, setSettings] = useState<SettingsState | undefined>()
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | undefined>()

  useEffect(() => {
    const controller = new AbortController()
    setupApi
      .settingsState(controller.signal)
      .then((next) => {
        if (!controller.signal.aborted) setSettings(next)
      })
      .catch((err) => {
        if (!controller.signal.aborted) setError(err instanceof Error ? err.message : 'load failed')
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false)
      })
    return () => controller.abort()
  }, [])

  return (
    <>
      <h1 className="sr-only">Advanced settings</h1>
      <Card>
        <CardHeader>
          <CardTitle>Advanced</CardTitle>
          <CardDescription>Redacted effective settings and file locations.</CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          {loading ? (
            <div className="flex items-center gap-2 text-sm text-muted-foreground">
              <Loader2 className="h-4 w-4 animate-spin" /> loading config
            </div>
          ) : null}
          {error ? <p className="text-sm text-destructive">{error}</p> : null}
          {settings ? (
            <>
              <div className="rounded-md border p-3">
                <div className="flex items-center justify-between gap-3">
                  <div>
                    <p className="text-sm font-medium">config.toml</p>
                    <p className="font-mono text-xs text-muted-foreground">{settings.config_path}</p>
                  </div>
                  <Badge variant="secondary">read-only</Badge>
                </div>
              </div>
              <pre className="overflow-auto rounded-md border bg-muted p-3 text-xs">
{JSON.stringify(
  {
    services: {
      built_in_upstream_apis_enabled:
        settings.services.built_in_upstream_apis_enabled,
      built_in_upstream_api_services:
        settings.services.built_in_upstream_api_services,
      bootstrap_services: settings.services.bootstrap_services,
    },
    surfaces: settings.surfaces,
  },
  null,
  2,
)}
              </pre>
            </>
          ) : null}
        </CardContent>
      </Card>
    </>
  )
}
