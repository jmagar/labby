'use client'

import { useEffect, useState } from 'react'
import { Loader2 } from 'lucide-react'

import { Badge } from '@/components/ui/badge'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import { setupApi, type SettingsState } from '@/lib/api/setup-client'

export default function FeaturesPage(): React.ReactElement {
  const [settings, setSettings] = useState<SettingsState | undefined>()
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
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

  async function updateBuiltIns(enabled: boolean): Promise<void> {
    setSaving(true)
    setError(undefined)
    try {
      const next = await setupApi.settingsUpdate({
        services: { built_in_upstream_apis_enabled: enabled },
      })
      setSettings(next)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'save failed')
    } finally {
      setSaving(false)
    }
  }

  return (
    <>
      <h1 className="sr-only">Feature settings</h1>
      <Card>
        <CardHeader>
          <CardTitle>Features</CardTitle>
          <CardDescription>
            Backed feature controls exposed by the setup service.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          {loading ? (
            <div className="flex items-center gap-2 text-sm text-muted-foreground">
              <Loader2 className="h-4 w-4 animate-spin" /> loading features
            </div>
          ) : null}
          {error ? <p className="text-sm text-destructive">{error}</p> : null}
          {settings ? (
            <div className="flex items-start justify-between gap-4 rounded-md border p-3">
              <div className="space-y-1">
                <div className="flex items-center gap-2">
                  <Label htmlFor="features-built-in-upstreams">Built-in upstream API services</Label>
                  <Badge variant="secondary">config.toml</Badge>
                </div>
                <p className="text-sm text-muted-foreground">
                  Disable every bundled external API integration while keeping bootstrap tools online.
                  Changes are saved to config.toml and take effect after restarting labby serve.
                </p>
                {settings.restart_required ? (
                  <p className="text-sm text-amber-700 dark:text-amber-300">
                    {settings.restart_note}
                  </p>
                ) : null}
                <p className="text-xs text-muted-foreground">
                  {settings.services.built_in_upstream_api_services.join(', ')}
                </p>
              </div>
              <Switch
                id="features-built-in-upstreams"
                checked={settings.services.built_in_upstream_apis_enabled}
                disabled={saving}
                onCheckedChange={(checked) => {
                  void updateBuiltIns(checked)
                }}
                aria-label="Enable built-in upstream API services"
              />
            </div>
          ) : null}
        </CardContent>
      </Card>
    </>
  )
}
