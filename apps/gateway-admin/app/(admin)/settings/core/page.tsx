'use client'

import { useEffect, useState } from 'react'
import { Loader2 } from 'lucide-react'

import { SettingsScalarSection } from '@/components/settings/SettingsScalarSection'
import { setupApi, type SettingsSchemaResponse, type SettingsState } from '@/lib/api/setup-client'
import { fieldsForSection } from '@/lib/settings/schema'

export default function CorePage(): React.ReactElement {
  const [schema, setSchema] = useState<SettingsSchemaResponse | undefined>()
  const [settings, setSettings] = useState<SettingsState | undefined>()
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | undefined>()

  useEffect(() => {
    const controller = new AbortController()
    Promise.all([
      setupApi.settingsSchema(controller.signal),
      setupApi.settingsState('core', controller.signal),
    ])
      .then(([schemaResponse, stateResponse]) => {
        if (controller.signal.aborted) return
        setSchema(schemaResponse)
        setSettings(stateResponse)
      })
      .catch((err) => {
        if (!controller.signal.aborted) setError(err instanceof Error ? err.message : 'load failed')
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false)
      })
    return () => controller.abort()
  }, [])

  const fields = schema ? fieldsForSection(schema.fields, 'core') : []

  return (
    <>
      <h1 className="sr-only">Core settings</h1>
      {loading ? (
        <div className="flex items-center gap-2 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" /> loading core settings
        </div>
      ) : null}
      {error ? <p className="text-sm text-destructive">{error}</p> : null}
      {settings ? (
        <SettingsScalarSection
          title="Core"
          description="Env-backed process defaults and low-risk operator paths."
          section="core"
          state={settings}
          fields={fields}
          onSaved={setSettings}
        />
      ) : null}
    </>
  )
}
