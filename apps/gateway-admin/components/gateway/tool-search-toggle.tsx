'use client'

import { useRef, useState } from 'react'
import { Code2, Search } from 'lucide-react'
import { mutate } from 'swr'
import { toast } from 'sonner'

import { AURORA_STRONG_PANEL } from '@/components/aurora/tokens'
import { Badge } from '@/components/ui/badge'
import { Switch } from '@/components/ui/switch'
import {
  useGatewayCodeModeConfig,
  useGatewayMutations,
  useGatewayToolSearchConfig,
  CODE_MODE_CONFIG_KEY,
  TOOL_SEARCH_CONFIG_KEY,
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
  // Tool search can be toggled whenever it has data and is not already saving.
  const canToggle = Boolean(toolSearchConfig) && !isLoading && !isSaving
  // Code mode can be toggled independently — mutual exclusion is enforced server-side.
  const canToggleCodeMode =
    Boolean(codeModeConfig) && !isCodeModeLoading && !isCodeModeSaving

  async function handleToggle(enabled: boolean) {
    if (!toolSearchConfig || isSavingRef.current) return
    isSavingRef.current = true
    setIsSaving(true)
    try {
      // Mutual exclusion: if disabling tool search while code_mode is active,
      // cascade-disable code_mode first before disabling tool_search.
      if (!enabled && codeModeConfig?.enabled) {
        await setCodeModeConfig({
          enabled: false,
          timeout_ms: codeModeConfig.timeout_ms,
          max_tool_calls: codeModeConfig.max_tool_calls,
          max_response_bytes: codeModeConfig.max_response_bytes,
          max_response_tokens: codeModeConfig.max_response_tokens,
        })
        await mutate(CODE_MODE_CONFIG_KEY)
      }
      await setToolSearchConfig({
        enabled,
        top_k_default: toolSearchConfig.top_k_default,
        max_tools: toolSearchConfig.max_tools,
      })
      // Cross-revalidate so code_mode badge reflects server-side state.
      await mutate(CODE_MODE_CONFIG_KEY)
      toast.success(enabled ? 'Tool search mode enabled.' : 'Tool search mode disabled.')
    } catch (requestError) {
      toast.error(getErrorMessage(requestError, 'Failed to update tool search mode'))
      // Re-fetch both configs so UI reflects actual server state after a partial failure.
      await Promise.allSettled([mutate(TOOL_SEARCH_CONFIG_KEY), mutate(CODE_MODE_CONFIG_KEY)])
    } finally {
      isSavingRef.current = false
      setIsSaving(false)
    }
  }

  async function handleCodeModeToggle(enabled: boolean) {
    if (!codeModeConfig || isCodeModeSavingRef.current) return
    // Mutual exclusion: enabling code_mode while tool_search is active must
    // disable tool_search first (server enforces, but cascade proactively).
    if (enabled && toolSearchConfig?.enabled) {
      toast.error(
        'Tool Search mode and Code Mode are mutually exclusive. Disable tool search first.',
      )
      return
    }
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
      // Cross-revalidate so tool_search badge reflects server-side state.
      await mutate(TOOL_SEARCH_CONFIG_KEY)
      toast.success(enabled ? 'Code mode enabled.' : 'Code mode disabled.')
    } catch (requestError) {
      toast.error(getErrorMessage(requestError, 'Failed to update code mode'))
      // Re-fetch both configs so UI reflects actual server state after a partial failure.
      await Promise.allSettled([mutate(CODE_MODE_CONFIG_KEY), mutate(TOOL_SEARCH_CONFIG_KEY)])
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
              instead of listing every upstream tool directly. Mutually exclusive with Code mode.
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
            <p className="text-sm font-semibold text-aurora-text-primary">Code mode</p>
            <p className="mt-1 text-sm text-aurora-text-muted">
              Expose a single{' '}
              <code className="rounded bg-aurora-panel-strong px-1.5 py-0.5 text-aurora-text-primary">
                code
              </code>{' '}
              tool — discovery via typed preamble, execution via constrained JS sandbox.
              Mutually exclusive with Tool search mode.
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
              {toolSearchConfig?.enabled ? (
                <Badge variant="outline">Disabled (tool search active)</Badge>
              ) : null}
              {isCodeModeSaving ? <Badge variant="outline">Saving</Badge> : null}
            </div>
          )}
          <Switch
            aria-label="Code mode"
            checked={codeModeConfig?.enabled ?? false}
            disabled={!canToggleCodeMode || Boolean(toolSearchConfig?.enabled)}
            onCheckedChange={handleCodeModeToggle}
          />
        </div>
      </div>
    </section>
  )
}
