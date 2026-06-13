'use client'

import { useEffect, useState } from 'react'
import { Loader2 } from 'lucide-react'

import { AdvancedReadOnlyBlock } from '@/components/settings/AdvancedReadOnlyBlock'
import { SettingsScalarSection } from '@/components/settings/SettingsScalarSection'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { setupApi, type EnvSettingSpec, type SettingsSchemaResponse, type SettingsState } from '@/lib/api/setup-client'
import { fieldsForSection } from '@/lib/settings/schema'

export default function AdvancedPage(): React.ReactElement {
  const [schema, setSchema] = useState<SettingsSchemaResponse | undefined>()
  const [settings, setSettings] = useState<SettingsState | undefined>()
  const [envSchema, setEnvSchema] = useState<EnvSettingSpec[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | undefined>()

  useEffect(() => {
    const controller = new AbortController()
    Promise.all([
      setupApi.settingsSchema(controller.signal),
      setupApi.settingsState('advanced', controller.signal),
      setupApi.settingsEnvSchema(controller.signal),
    ])
      .then(([schemaResponse, stateResponse, envResponse]) => {
        if (controller.signal.aborted) return
        setSchema(schemaResponse)
        setSettings(stateResponse)
        setEnvSchema(envResponse)
      })
      .catch((err) => {
        if (!controller.signal.aborted) setError(err instanceof Error ? err.message : 'load failed')
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false)
      })
    return () => controller.abort()
  }, [])

  const fields = schema ? fieldsForSection(schema.fields, 'advanced') : []
  const readonlyFields = fields.filter((field) => field.write_policy !== 'editable')
  const scalarFields = fields.filter((field) => field.write_policy === 'editable')

  return (
    <>
      <h1 className="sr-only">Advanced settings</h1>
      {loading ? (
        <div className="flex items-center gap-2 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" /> loading advanced settings
        </div>
      ) : null}
      {error ? <p className="text-sm text-destructive">{error}</p> : null}
      {settings ? (
        <div className="space-y-4">
          <SettingsScalarSection
            title="Advanced Scalars"
            description="Low-risk advanced scalar limits and paths."
            section="advanced"
            state={settings}
            fields={scalarFields}
            onSaved={setSettings}
          />
          <AdvancedReadOnlyBlock state={settings} fields={readonlyFields} />
          <EnvInventoryTable entries={envSchema} />
        </div>
      ) : null}
    </>
  )
}

function EnvInventoryTable({ entries }: { entries: EnvSettingSpec[] }): React.ReactElement {
  const [query, setQuery] = useState('')
  const filtered = entries.filter((entry) =>
    `${entry.key} ${entry.service} ${entry.description}`.toLowerCase().includes(query.toLowerCase()),
  )
  return (
    <Card>
      <CardHeader>
        <CardTitle>Environment Inventory</CardTitle>
        <CardDescription>Known env keys from generated docs and service metadata. Only low-risk core env keys are editable in this epic.</CardDescription>
      </CardHeader>
      <CardContent className="space-y-3">
        <Input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Filter env keys" />
        <ul className="max-h-[520px] divide-y overflow-auto rounded-md border">
          {filtered.map((entry) => (
            <li key={entry.key} className="grid gap-1 p-3 text-sm md:grid-cols-[240px_1fr_auto]">
              <p className="font-mono text-xs">{entry.key}</p>
              <p className="text-muted-foreground">{entry.description}</p>
              <p className="text-xs text-muted-foreground">{entry.service}{entry.secret ? ' secret' : ''}{entry.editable ? ' editable' : ''}</p>
            </li>
          ))}
        </ul>
      </CardContent>
    </Card>
  )
}
