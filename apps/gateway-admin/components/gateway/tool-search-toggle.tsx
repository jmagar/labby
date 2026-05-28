'use client'

import { useRef, useState } from 'react'
import { Code2, Search } from 'lucide-react'
import { toast } from 'sonner'

import { AURORA_STRONG_PANEL } from '@/components/aurora/tokens'
import { Badge } from '@/components/ui/badge'
import { Switch } from '@/components/ui/switch'
import {
  useGatewayCodeModeConfig,
  useGatewayMutations,
  useGatewayToolSearchConfig,
} from '@/lib/hooks/use-gateways'
import { cn, getErrorMessage } from '@/lib/utils'

export function ToolSearchTogglePanel() {
  const { data: toolSearchConfig, isLoading, error } = useGatewayToolSearchConfig()
  const {
    data: codeModeConfig,
    isLoading: isCodeModeLoading,
    error: codeModeError,
  } = useGatewayCodeModeConfig()
  const { setCodeModeConfig, setToolSearchConfig } = useGatewayMutations()
  const isSavingRef = useRef(false)
  const isCodeModeSavingRef = useRef(false)
  const [isSaving, setIsSaving] = useState(false)
  const [isCodeModeSaving, setIsCodeModeSaving] = useState(false)
  const canToggle = Boolean(toolSearchConfig) && !isLoading && !isSaving
  const canToggleCodeMode =
    Boolean(codeModeConfig) && Boolean(toolSearchConfig?.enabled) && !isCodeModeLoading && !isCodeModeSaving

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

  async function handleCodeModeToggle(enabled: boolean) {
    if (!codeModeConfig || isCodeModeSavingRef.current || !toolSearchConfig?.enabled) return
    isCodeModeSavingRef.current = true
    setIsCodeModeSaving(true)
    try {
      await setCodeModeConfig({
        enabled,
        timeout_ms: codeModeConfig.timeout_ms,
        max_tool_calls: codeModeConfig.max_tool_calls,
        max_response_bytes: codeModeConfig.max_response_bytes,
        max_response_tokens: codeModeConfig.max_response_tokens,
      })
      toast.success(enabled ? 'Code mode execution enabled.' : 'Code mode execution disabled.')
    } catch (requestError) {
      toast.error(getErrorMessage(requestError, 'Failed to update code mode'))
    } finally {
      isCodeModeSavingRef.current = false
      setIsCodeModeSaving(false)
    }
  }

  return (
    <section
      className={cn(AURORA_STRONG_PANEL, 'flex flex-col divide-y divide-aurora-border-subtle px-5 py-1')}
    >
      <div className="flex flex-col gap-3 py-3 lg:flex-row lg:items-center lg:justify-between">
        <div className="flex items-start gap-3">
          <Search className="mt-0.5 size-5 text-aurora-accent-primary" />
          <div className="min-w-0">
            <p className="text-sm font-semibold text-aurora-text-primary">Tool search mode</p>
            <p className="mt-1 text-sm text-aurora-text-muted">
              Expose server-wide{' '}
              <code className="rounded bg-aurora-panel-strong px-1.5 py-0.5 text-aurora-text-primary">
                search
              </code>{' '}
              and{' '}
              <code className="rounded bg-aurora-panel-strong px-1.5 py-0.5 text-aurora-text-primary">
                execute
              </code>{' '}
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
      </div>
      <div className="flex flex-col gap-3 py-3 lg:flex-row lg:items-center lg:justify-between">
        <div className="flex items-start gap-3">
          <Code2 className="mt-0.5 size-5 text-aurora-accent-secondary" />
          <div className="min-w-0">
            <p className="text-sm font-semibold text-aurora-text-primary">Code mode execution</p>
            <p className="mt-1 text-sm text-aurora-text-muted">
              Allow{' '}
              <code className="rounded bg-aurora-panel-strong px-1.5 py-0.5 text-aurora-text-primary">
                execute
              </code>{' '}
              to run constrained snippets against upstream tools discovered through search.
            </p>
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-3">
          {codeModeError ? (
            <p className="text-xs text-aurora-error">Code mode settings unavailable.</p>
          ) : (
            <div className="flex flex-wrap gap-2">
              <Badge variant="secondary" status={codeModeConfig?.enabled ? 'success' : 'default'}>
                {codeModeConfig?.enabled ? 'Enabled' : 'Disabled'}
              </Badge>
              <Badge variant="outline">{codeModeConfig?.max_tool_calls ?? '-'} calls</Badge>
              <Badge variant="outline">{codeModeConfig?.timeout_ms ?? '-'}ms</Badge>
              {!toolSearchConfig?.enabled ? <Badge variant="outline">Requires tool search</Badge> : null}
              {isCodeModeSaving ? <Badge variant="outline">Saving</Badge> : null}
            </div>
          )}
          <Switch
            aria-label="Code mode execution"
            checked={codeModeConfig?.enabled ?? false}
            disabled={!canToggleCodeMode}
            onCheckedChange={handleCodeModeToggle}
          />
        </div>
      </div>
    </section>
  )
}
