'use client'

import { useRef, useState } from 'react'
import { Search } from 'lucide-react'
import { toast } from 'sonner'

import { AURORA_STRONG_PANEL } from '@/components/aurora/tokens'
import { Badge } from '@/components/ui/badge'
import { Switch } from '@/components/ui/switch'
import { useGatewayMutations, useGatewayToolSearchConfig } from '@/lib/hooks/use-gateways'
import { cn, getErrorMessage } from '@/lib/utils'

export function ToolSearchTogglePanel() {
  const { data: toolSearchConfig, isLoading, error } = useGatewayToolSearchConfig()
  const { setToolSearchConfig } = useGatewayMutations()
  const isSavingRef = useRef(false)
  const [isSaving, setIsSaving] = useState(false)
  const canToggle = Boolean(toolSearchConfig) && !isLoading && !isSaving

  async function handleToggle(enabled: boolean) {
    if (!toolSearchConfig || isSavingRef.current) return
    isSavingRef.current = true
    setIsSaving(true)
    try {
      await setToolSearchConfig({
        enabled,
        top_k_default: toolSearchConfig.top_k_default,
        max_tools: toolSearchConfig.max_tools,
      })
      toast.success(enabled ? 'Server tool search enabled.' : 'Server tool search disabled.')
    } catch (requestError) {
      toast.error(getErrorMessage(requestError, 'Failed to update server tool search'))
    } finally {
      isSavingRef.current = false
      setIsSaving(false)
    }
  }

  return (
    <section
      className={cn(
        AURORA_STRONG_PANEL,
        'flex flex-col gap-3 px-5 py-4 lg:flex-row lg:items-center lg:justify-between',
      )}
    >
      <div className="flex items-start gap-3">
        <Search className="mt-0.5 size-5 text-aurora-accent-primary" />
        <div className="min-w-0">
          <p className="text-sm font-semibold text-aurora-text-primary">Tool search mode</p>
          <p className="mt-1 text-sm text-aurora-text-muted">
            Expose server-wide{' '}
            <code className="rounded bg-aurora-panel-strong px-1.5 py-0.5 text-aurora-text-primary">tool_search</code>{' '}
            and{' '}
            <code className="rounded bg-aurora-panel-strong px-1.5 py-0.5 text-aurora-text-primary">tool_execute</code>{' '}
            instead of listing every upstream tool directly.
          </p>
        </div>
      </div>
      <div className="flex flex-wrap items-center gap-3">
        {error ? (
          <p className="text-xs text-aurora-error">Tool search settings unavailable.</p>
        ) : (
          <div className="flex flex-wrap gap-2">
            <Badge variant="secondary" status={toolSearchConfig?.enabled ? 'success' : 'default'}>
              {toolSearchConfig?.enabled ? 'Enabled' : 'Disabled'}
            </Badge>
            <Badge variant="outline">Top K {toolSearchConfig?.top_k_default ?? '-'}</Badge>
            <Badge variant="outline">Max tools {toolSearchConfig?.max_tools ?? '-'}</Badge>
            {isSaving ? <Badge variant="outline">Saving</Badge> : null}
          </div>
        )}
        <Switch
          aria-label="Tool search mode"
          checked={toolSearchConfig?.enabled ?? false}
          disabled={!canToggle}
          onCheckedChange={handleToggle}
        />
      </div>
    </section>
  )
}
