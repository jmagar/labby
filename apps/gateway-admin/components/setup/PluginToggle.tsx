'use client'

import { useEffect, useState } from 'react'
import { Loader2 } from 'lucide-react'

import { Switch } from '@/components/ui/switch'
import { setupApi } from '@/lib/api/setup-client'

export function PluginToggle({
  service,
  installed,
  disabled = false,
  onChanged,
}: {
  service: string
  installed: boolean
  disabled?: boolean
  onChanged?: (installed: boolean) => void
}): React.ReactElement {
  const [checked, setChecked] = useState(installed)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | undefined>()

  useEffect(() => {
    setChecked(installed)
  }, [installed])

  async function toggle(next: boolean): Promise<void> {
    const prev = checked
    setChecked(next)
    setBusy(true)
    setError(undefined)
    try {
      if (next) {
        await setupApi.installPlugin(service)
      } else {
        await setupApi.uninstallPlugin(service)
      }
      onChanged?.(next)
    } catch (err) {
      setChecked(prev)
      setError(err instanceof Error ? err.message : 'plugin action failed')
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="flex min-w-[10rem] flex-col items-end gap-1 text-xs">
      <label className="flex items-center gap-2">
        {busy ? <Loader2 className="h-3 w-3 animate-spin text-muted-foreground" /> : null}
        <span className="text-muted-foreground">Claude Code</span>
        <Switch
          checked={checked}
          disabled={disabled || busy}
          onCheckedChange={(value) => void toggle(value)}
          aria-label={`${checked ? 'Disable' : 'Enable'} ${service} Claude Code plugin`}
        />
      </label>
      {error ? <span className="max-w-[16rem] text-right text-destructive">{error}</span> : null}
    </div>
  )
}
