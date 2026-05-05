'use client'

import { useEffect, useState } from 'react'
import { Loader2 } from 'lucide-react'

import { Checkbox } from '@/components/ui/checkbox'
import { NavButtons, useWizard } from '@/components/setup/WizardShell'
import { setupApi, type ServiceSchema } from '@/lib/api/setup-client'

export default function ServiceSelectionPage(): React.ReactElement {
  const wizard = useWizard()
  const [services, setServices] = useState<ServiceSchema[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | undefined>()

  useEffect(() => {
    const controller = new AbortController()
    setupApi
      .schemaGet(undefined, controller.signal)
      .then(async (response) => {
        if (controller.signal.aborted) return
        const installed = wizard.mode === 'plugin'
          ? new Set((await setupApi.installedPlugins(controller.signal))
            .map((plugin) => plugin.service)
            .filter((service): service is string => service !== null))
          : undefined
        const sorted = Object.values(response.services).sort((a, b) =>
          a.name.localeCompare(b.name),
        ).filter((service) => !installed || installed.has(service.name))
        setServices(sorted)
      })
      .catch((err) => {
        if (controller.signal.aborted) return
        setError(err instanceof Error ? err.message : 'schema.get failed')
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false)
      })
    return () => controller.abort()
  }, [wizard.mode])

  function toggle(service: string): void {
    wizard.setSelectedServices((prev) =>
      prev.includes(service) ? prev.filter((s) => s !== service) : [...prev, service],
    )
  }

  return (
    <div className="space-y-6">
      <section className="space-y-3">
        <h2 className="text-xl font-semibold">Service Selection</h2>
        <p className="text-sm text-muted-foreground">
          Choose which services you want to configure now. You can always
          add more later from the Settings rail.
          {wizard.mode === 'plugin' ? ' Plugin mode only shows services with installed plugins.' : null}
        </p>
      </section>

      {loading ? (
        <div className="flex items-center gap-2 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" /> loading service catalog
        </div>
      ) : null}

      {error ? (
        <div className="rounded-md border border-destructive/50 bg-destructive/10 p-3 text-sm text-destructive">
          {error}
        </div>
      ) : null}

      {!loading && !error ? (
        <ul className="grid grid-cols-2 gap-2 sm:grid-cols-3">
          {services.map((s) => (
            <li key={s.name} className="rounded-md border p-3 text-sm">
              <label className="flex cursor-pointer items-start gap-2">
                <Checkbox
                  checked={wizard.selectedServices.includes(s.name)}
                  onCheckedChange={() => toggle(s.name)}
                  aria-label={`Select ${s.display_name}`}
                />
                <span>
                  <span className="font-medium">{s.display_name}</span>
                  {s.description ? (
                    <span className="block text-xs text-muted-foreground">
                      {s.description}
                    </span>
                  ) : null}
                </span>
              </label>
            </li>
          ))}
        </ul>
      ) : null}

      <NavButtons nextDisabled={loading} />
    </div>
  )
}
