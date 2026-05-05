'use client'

import { useEffect, useRef, useState } from 'react'
import { Loader2 } from 'lucide-react'

import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { NavButtons, useWizard } from '@/components/setup/WizardShell'
import { ServiceForm, type ProbeOutcome } from '@/components/setup/ServiceForm'
import { PluginToggle } from '@/components/setup/PluginToggle'
import { setupApi, type ServiceSchema } from '@/lib/api/setup-client'
import { doctorApi } from '@/lib/api/doctor-client'
import { type FieldView } from '@/lib/setup/schemaBuilder'
import { buildServiceFormDefaults, draftEntriesToMap } from '@/lib/setup/draft'

interface ServiceState {
  schema: ServiceSchema
  fields: FieldView[]
  defaultValues: Record<string, string>
}

export default function ConfigurationPage(): React.ReactElement {
  const wizard = useWizard()
  const [services, setServices] = useState<ServiceState[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | undefined>()
  const [activeTab, setActiveTab] = useState<string | undefined>(undefined)
  const [installedPlugins, setInstalledPlugins] = useState<Set<string>>(new Set())

  // In-progress field values per service slug. ServiceForm flushes its
  // current values to this cache on unmount via onUnmount; on remount
  // we prefer the cached values over the draft snapshot so a tab switch
  // doesn't silently discard typed-but-unsaved input.
  const valuesCache = useRef<Map<string, Record<string, string>>>(new Map())

  useEffect(() => {
    if (wizard.selectedServices.length === 0) {
      setLoading(false)
      return
    }
    const controller = new AbortController()
    Promise.all([
      setupApi.schemaGet(wizard.selectedServices, controller.signal),
      setupApi.draftGet(controller.signal),
      setupApi.installedPlugins(controller.signal),
    ])
      .then(([schemaResponse, draft, plugins]) => {
        if (controller.signal.aborted) return
        const draftMap = draftEntriesToMap(draft.entries)
        const next: ServiceState[] = []
        for (const slug of wizard.selectedServices) {
          const schema = schemaResponse.services[slug]
          if (!schema) continue
          const { fields, defaults } = buildServiceFormDefaults(schema.env, draftMap)
          next.push({ schema, fields, defaultValues: defaults })
        }
        valuesCache.current.clear()
        setServices(next)
        setInstalledPlugins(
          new Set(plugins.map((plugin) => plugin.service).filter((service): service is string => service !== null)),
        )
      })
      .catch((err) => {
        if (controller.signal.aborted) return
        setError(err instanceof Error ? err.message : 'configuration load failed')
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false)
      })
    return () => controller.abort()
  }, [wizard.selectedServices])

  async function saveService(values: Record<string, string>): Promise<void> {
    const entries = Object.entries(values).map(([key, value]) => ({ key, value }))
    if (entries.length > 0) {
      await setupApi.draftSet(entries)
    }
  }

  async function probeService(slug: string, signal: AbortSignal): Promise<ProbeOutcome> {
    try {
      const finding = await doctorApi.serviceProbe(slug, undefined, signal)
      return {
        status: finding.severity === 'ok' ? 'ok' : 'fail',
        message: finding.message,
      }
    } catch (err) {
      return {
        status: 'fail',
        message: err instanceof Error ? err.message : 'probe failed',
      }
    }
  }

  return (
    <div className="space-y-6">
      <section className="space-y-3">
        <h2 className="text-xl font-semibold">Service Configuration</h2>
        <p className="text-sm text-muted-foreground">
          Fill out connection details for each service you selected. Use
          the Test connection button to verify each one before continuing.
        </p>
      </section>

      {loading ? (
        <div className="flex items-center gap-2 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" /> loading service schemas
        </div>
      ) : null}

      {error ? (
        <div className="rounded-md border border-destructive/50 bg-destructive/10 p-3 text-sm text-destructive">
          {error}
        </div>
      ) : null}

      {!loading && services.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          No services selected — go back to the previous step or skip ahead
          to Finalize.
        </p>
      ) : null}

      {services.length > 0 ? (
        <Tabs
          value={activeTab ?? services[0]!.schema.name}
          onValueChange={setActiveTab}
          className="w-full"
        >
          <TabsList className="flex w-full flex-wrap">
            {services.map((s) => (
              <TabsTrigger key={s.schema.name} value={s.schema.name}>
                {s.schema.display_name}
              </TabsTrigger>
            ))}
          </TabsList>
          {services.map((s) => {
            const current = (activeTab ?? services[0]!.schema.name) === s.schema.name
            // Radix Tabs mounts every TabsContent by default. Mounting 22
            // ServiceForms simultaneously means 22 useForm + zodResolver
            // builds up front. Only render the active tab's form to keep
            // mount cost O(1) regardless of selected-service count.
            if (!current) return null
            const cached = valuesCache.current.get(s.schema.name)
            return (
              <TabsContent key={s.schema.name} value={s.schema.name} className="pt-4">
                <ServiceForm
                  fields={s.fields}
                  defaultValues={cached ?? s.defaultValues}
                  onSave={saveService}
                  onProbe={(_values, signal) => probeService(s.schema.name, signal)}
                  onUnmount={(values) => valuesCache.current.set(s.schema.name, values)}
                />
                <div className="mt-4 flex justify-end border-t pt-4">
                  <PluginToggle
                    service={s.schema.name}
                    installed={installedPlugins.has(s.schema.name)}
                    onChanged={(installed) => {
                      setInstalledPlugins((prev) => {
                        const next = new Set(prev)
                        if (installed) next.add(s.schema.name)
                        else next.delete(s.schema.name)
                        return next
                      })
                    }}
                  />
                </div>
              </TabsContent>
            )
          })}
        </Tabs>
      ) : null}

      <NavButtons />
    </div>
  )
}
