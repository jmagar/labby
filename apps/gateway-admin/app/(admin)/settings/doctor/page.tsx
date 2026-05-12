'use client'

import { useRef, useState } from 'react'
import { KeyRound, LifeBuoy, PlugZap, Search, ShieldCheck } from 'lucide-react'
import { toast } from 'sonner'
// AppHeader is owned by the parent /settings/layout.tsx — do not double-mount.
import { Badge } from '@/components/ui/badge'
import { Switch } from '@/components/ui/switch'
import {
  AURORA_DISPLAY_1,
  AURORA_DISPLAY_NUMBER,
  AURORA_MEDIUM_PANEL,
  AURORA_MUTED_LABEL,
  AURORA_PAGE_FRAME,
  AURORA_PAGE_SHELL,
  AURORA_STRONG_PANEL,
} from '@/components/aurora/tokens'
import { hasMockDataAuthMode, isStandaloneBearerAuthMode } from '@/lib/auth/auth-mode'
import { buildGatewaySettingsSnapshot } from '@/lib/dashboard/admin-insights'
import {
  useGateways,
  useGatewayMutations,
  useGatewayToolSearchConfig,
} from '@/lib/hooks/use-gateways'
import { useBrowserSession } from '@/lib/auth/session'
import { cn, getErrorMessage } from '@/lib/utils'
import { AllowedUsersPanel } from '@/components/allowed-users-panel'

export default function SettingsPage() {
  const session = useBrowserSession()
  const isAdmin = session.status === 'authenticated' && session.isAdmin === true
  const { data: gateways, isLoading, error } = useGateways()
  const {
    data: toolSearchConfig,
    isLoading: isToolSearchLoading,
    error: toolSearchError,
  } = useGatewayToolSearchConfig()
  const { setToolSearchConfig } = useGatewayMutations()
  const isSavingRef = useRef(false)
  const [isToolSearchSaving, setIsToolSearchSaving] = useState(false)
  const snapshot = gateways ? buildGatewaySettingsSnapshot(gateways, {
    hasStandaloneBearerAuth: isStandaloneBearerAuthMode(),
    hasMockData: hasMockDataAuthMode(),
  }) : null
  const canToggleToolSearch = Boolean(toolSearchConfig) && !isToolSearchLoading && !isToolSearchSaving

  async function handleToolSearchToggle(enabled: boolean) {
    if (!toolSearchConfig || isSavingRef.current) return
    isSavingRef.current = true
    setIsToolSearchSaving(true)
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
      setIsToolSearchSaving(false)
    }
  }

  return (
    <div className="flex flex-col gap-4">
      {/* Page header (lab-bg3e.5: this content lives under /settings/doctor
          since the original /settings root now redirects to /settings/core). */}
      <div className={cn(AURORA_STRONG_PANEL, 'px-6 py-5')}>
        <p className={AURORA_MUTED_LABEL}>Doctor</p>
        <h1 className={cn(AURORA_DISPLAY_1, 'mt-2 text-aurora-text-primary')}>Fleet Posture</h1>
        <p className="mt-2 text-sm text-aurora-text-muted">
          Control-plane posture and effective defaults for the server fleet.
        </p>
      </div>

        {/* Stat cards */}
        {isLoading ? (
          <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
            {Array.from({ length: 4 }, (_, index) => (
              <div key={index} className="h-28 animate-pulse rounded-aurora-3 border border-aurora-border-strong bg-aurora-panel-medium" />
            ))}
          </div>
        ) : error || !snapshot ? (
          <div className="rounded-[1rem] border border-aurora-error/30 bg-aurora-error/8 p-4 text-sm text-aurora-error">
            Failed to load settings because the server list is unavailable.
          </div>
        ) : (
          <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
            <div className={cn(AURORA_MEDIUM_PANEL, 'px-5 py-4')}>
              <p className={AURORA_MUTED_LABEL}>Auth Mode</p>
              <p className="mt-2 text-xl font-semibold text-aurora-text-primary">{snapshot.authModeLabel}</p>
              <p className="mt-1 text-sm text-aurora-text-muted">How the web UI authenticates control-plane requests.</p>
            </div>
            <div className={cn(AURORA_MEDIUM_PANEL, 'px-5 py-4')}>
              <p className={AURORA_MUTED_LABEL}>Runtime</p>
              <p className="mt-2 text-xl font-semibold text-aurora-text-primary">{snapshot.runtimeLabel}</p>
              <p className="mt-1 text-sm text-aurora-text-muted">Current environment mode exposed to the admin UI.</p>
            </div>
            <div className={cn(AURORA_MEDIUM_PANEL, 'px-5 py-4')}>
              <p className={AURORA_MUTED_LABEL}>Warnings</p>
              <p className={cn(AURORA_DISPLAY_NUMBER, 'mt-2 text-[22px]', snapshot.warningCount > 0 ? 'text-aurora-warn' : 'text-aurora-text-primary')}>
                {snapshot.warningCount}
              </p>
              <p className="mt-1 text-sm text-aurora-text-muted">Warnings across all configured servers.</p>
            </div>
            <div className={cn(AURORA_MEDIUM_PANEL, 'px-5 py-4')}>
              <p className={AURORA_MUTED_LABEL}>Disconnected</p>
              <p className={cn(AURORA_DISPLAY_NUMBER, 'mt-2 text-[22px]', snapshot.disconnectedGateways > 0 ? 'text-aurora-error' : 'text-aurora-text-primary')}>
                {snapshot.disconnectedGateways}
              </p>
              <p className="mt-1 text-sm text-aurora-text-muted">Servers that currently need operator attention.</p>
            </div>
          </div>
        )}

        {/* Detail grid */}
        <div className="grid gap-5 xl:grid-cols-[1.4fr_1fr]">
          <div className={cn(AURORA_STRONG_PANEL, 'px-6 py-5')}>
            <p className="text-base font-semibold text-aurora-text-primary">Control-plane posture</p>
            <p className="mt-1 text-sm text-aurora-text-muted">
              A read-only summary of the admin surface and the current server fleet.
            </p>

            {isLoading ? (
              <div className="mt-5 space-y-3">
                {Array.from({ length: 4 }, (_, index) => (
                  <div key={index} className="h-20 animate-pulse rounded-[1rem] border border-aurora-border-strong bg-aurora-control-surface" />
                ))}
              </div>
            ) : error || !snapshot ? (
              <div className="mt-5 rounded-[1rem] border border-aurora-error/30 bg-aurora-error/8 p-4 text-sm text-aurora-error">
                Failed to load settings because the server list is unavailable.
              </div>
            ) : (
              <div className="mt-5 grid gap-3 md:grid-cols-2">
                <div className="rounded-[1rem] border border-aurora-border-strong bg-aurora-control-surface p-4">
                  <div className="flex items-center gap-3">
                    <ShieldCheck className="size-5 text-aurora-accent-primary" />
                    <div>
                      <p className="font-medium text-aurora-text-primary">Authentication</p>
                      <p className="text-sm text-aurora-text-muted">
                        UI requests are running in <span className="font-medium text-aurora-text-primary">{snapshot.authModeLabel}</span>.
                      </p>
                    </div>
                  </div>
                </div>
                <div className="rounded-[1rem] border border-aurora-border-strong bg-aurora-control-surface p-4">
                  <div className="flex items-center gap-3">
                    <LifeBuoy className="size-5 text-aurora-accent-primary" />
                    <div>
                      <p className="font-medium text-aurora-text-primary">Preview mode</p>
                      <p className="text-sm text-aurora-text-muted">
                        <span className="font-medium text-aurora-text-primary">{snapshot.runtimeLabel}</span> is active for this build.
                      </p>
                    </div>
                  </div>
                </div>
                <div className="rounded-[1rem] border border-aurora-border-strong bg-aurora-control-surface p-4">
                  <div className="flex items-center gap-3">
                    <PlugZap className="size-5 text-aurora-accent-primary" />
                    <div>
                      <p className="font-medium text-aurora-text-primary">Server reachability</p>
                      <p className="text-sm text-aurora-text-muted">
                        {snapshot.connectedGateways} of {snapshot.totalGateways} servers are connected.
                      </p>
                    </div>
                  </div>
                </div>
                <div className="rounded-[1rem] border border-aurora-border-strong bg-aurora-control-surface p-4">
                  <div className="flex items-center gap-3">
                    <KeyRound className="size-5 text-aurora-accent-primary" />
                    <div>
                      <p className="font-medium text-aurora-text-primary">Protected upstreams</p>
                      <p className="text-sm text-aurora-text-muted">
                        {snapshot.bearerTokenGateways} servers require bearer-token env wiring.
                      </p>
                    </div>
                  </div>
                </div>
              </div>
            )}
          </div>

          <div className={cn(AURORA_STRONG_PANEL, 'px-6 py-5')}>
            <p className="text-base font-semibold text-aurora-text-primary">Effective defaults</p>
            {isLoading ? (
              <div className="mt-4 space-y-3">
                {Array.from({ length: 4 }, (_, index) => (
                  <div key={index} className="h-14 animate-pulse rounded-[1rem] border border-aurora-border-strong bg-aurora-control-surface" />
                ))}
              </div>
            ) : error || !snapshot ? (
              <div className="mt-4 rounded-[1rem] border border-aurora-error/30 bg-aurora-error/8 p-4 text-sm text-aurora-error">
                Effective defaults are unavailable until the server list loads successfully.
              </div>
            ) : (
              <div className="mt-4 space-y-2 text-sm text-aurora-text-muted">
                <div className="rounded-aurora-1 border border-aurora-border-strong bg-aurora-control-surface px-4 py-3">
                  <div className="flex items-start justify-between gap-4">
                    <div className="flex items-start gap-3">
                      <Search className="mt-0.5 size-5 text-aurora-accent-primary" />
                      <div>
                        <p className="font-medium text-aurora-text-primary">Tool search mode</p>
                        <p className="mt-1 text-sm text-aurora-text-muted">
                          Expose server-wide <code className="rounded bg-aurora-panel-strong px-1.5 py-0.5 text-aurora-text-primary">tool_search</code> and <code className="rounded bg-aurora-panel-strong px-1.5 py-0.5 text-aurora-text-primary">tool_invoke</code> instead of listing every upstream tool directly.
                        </p>
                      </div>
                    </div>
                    <Switch
                      aria-label="Tool search mode"
                      checked={toolSearchConfig?.enabled ?? false}
                      disabled={!canToggleToolSearch}
                      onCheckedChange={handleToolSearchToggle}
                    />
                  </div>
                  {toolSearchError ? (
                    <p className="mt-3 text-xs text-aurora-error">Tool search settings are unavailable.</p>
                  ) : (
                    <div className="mt-3 flex flex-wrap gap-2">
                      <Badge variant="secondary" status={toolSearchConfig?.enabled ? 'success' : 'default'}>
                        {toolSearchConfig?.enabled ? 'Enabled' : 'Disabled'}
                      </Badge>
                      <Badge variant="outline">Top K {toolSearchConfig?.top_k_default ?? '-'}</Badge>
                      <Badge variant="outline">Max tools {toolSearchConfig?.max_tools ?? '-'}</Badge>
                      {isToolSearchSaving ? <Badge variant="outline">Saving</Badge> : null}
                    </div>
                  )}
                </div>
                <div className="flex items-center justify-between rounded-aurora-1 border border-aurora-border-strong bg-aurora-control-surface px-4 py-3">
                  <span>Proxy resources enabled</span>
                  <Badge variant="secondary">{snapshot.proxyResourceGateways} servers</Badge>
                </div>
                <div className="flex items-center justify-between rounded-aurora-1 border border-aurora-border-strong bg-aurora-control-surface px-4 py-3">
                  <span>Disconnected servers</span>
                  <Badge variant="secondary" status={snapshot.disconnectedGateways === 0 ? 'default' : 'error'}>
                    {snapshot.disconnectedGateways}
                  </Badge>
                </div>
                <div className="flex items-center justify-between rounded-aurora-1 border border-aurora-border-strong bg-aurora-control-surface px-4 py-3">
                  <span>Warning backlog</span>
                  <Badge variant={snapshot.warningCount === 0 ? 'secondary' : 'outline'} status={snapshot.warningCount > 0 ? 'warn' : 'default'}>
                    {snapshot.warningCount}
                  </Badge>
                </div>
                <div className="rounded-aurora-1 border border-aurora-border-strong border-dashed p-4 text-aurora-text-muted">
                  Tool search is managed here as a server-wide setting. Other global defaults are still surfaced as effective posture until their backend write APIs exist.
                </div>
              </div>
            )}
          </div>
        </div>

      {/* Allowed users (admin only) */}
      {isAdmin ? <AllowedUsersPanel /> : null}
    </div>
  )
}
