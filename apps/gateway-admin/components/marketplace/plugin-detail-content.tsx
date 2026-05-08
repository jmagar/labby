'use client'

import Link from 'next/link'
import Image from 'next/image'
import { useEffect, useMemo, useState } from 'react'
import { ArrowLeft, Download, GitFork, RefreshCw, Trash2 } from 'lucide-react'
import { AppHeader } from '@/components/app-header'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { CherryPickDialog } from './cherry-pick-dialog'
import { ConfirmDialog, type ConfirmState } from './confirm-dialog'
import { PluginFilesPanel } from './plugin-files-panel'
import { PluginInfoPanel } from './plugin-info-panel'
import { getArtifacts } from '@/lib/api/marketplace-client'
import { isAbortError } from '@/lib/api/service-action-client'
import { useMarketplaceMutations, useMarketplaces, usePlugins } from '@/lib/hooks/use-marketplace'
import type { Artifact } from '@/lib/types/marketplace'
import { cn } from '@/lib/utils'

type DetailTab = 'info' | 'files'

function PluginAvatar({ ghUser, name, size = 44 }: { ghUser?: string; name: string; size?: number }) {
  const [imageFailed, setImageFailed] = useState(false)
  const initials = name.replace(/-/g, ' ').split(' ').filter(Boolean).map((word) => word[0]).join('').toUpperCase().slice(0, 2)
  const style = { width: size, height: size }

  if (!ghUser || imageFailed) {
    return (
      <div
        className="rounded-aurora-1 flex h-full w-full items-center justify-center bg-[linear-gradient(135deg,color-mix(in_srgb,var(--aurora-panel-medium)_88%,transparent),color-mix(in_srgb,var(--aurora-accent-primary)_10%,transparent))] font-display font-black text-aurora-text-muted"
        style={style}
      >
        {initials}
      </div>
    )
  }

  return (
    <div
      className="rounded-aurora-1 flex-shrink-0 overflow-hidden border border-[color-mix(in_srgb,var(--aurora-border-strong)_40%,transparent)] bg-aurora-panel-medium"
      style={style}
    >
      <Image
        src={`https://github.com/${ghUser}.png?size=96`}
        alt={ghUser}
        width={size}
        height={size}
        unoptimized
        className="h-full w-full object-cover"
        onError={() => setImageFailed(true)}
      />
    </div>
  )
}

function EmptyState({ title, sub }: { title: string; sub: string }) {
  return (
    <div className="flex min-h-[320px] flex-col items-center justify-center gap-3 rounded-aurora-3 border border-aurora-border-default bg-aurora-panel-medium p-8 text-center shadow-aurora-small">
      <span className="font-display text-[22px] font-bold tracking-[-0.02em] text-aurora-text-primary">{title}</span>
      <span className="max-w-[560px] text-[13px] text-aurora-text-muted">{sub}</span>
      <Link
        href="/marketplace"
        className="mt-2 inline-flex items-center gap-2 rounded-lg border border-aurora-border-default bg-aurora-control-surface px-3 py-2 text-[13px] font-semibold text-aurora-text-primary transition-colors duration-150 hover:bg-aurora-hover-bg"
      >
        <ArrowLeft className="size-4" />
        Back to marketplace
      </Link>
    </div>
  )
}

export function PluginDetailContent({ pluginId }: { pluginId: string }) {
  const { data: marketplaces = [], error: marketplacesError } = useMarketplaces()
  const { data: plugins = [], error: pluginsError } = usePlugins()
  const { install, uninstall } = useMarketplaceMutations()

  const [tab, setTab] = useState<DetailTab>('info')
  const [artifacts, setArtifacts] = useState<Artifact[]>([])
  const [confirm, setConfirm] = useState<ConfirmState | null>(null)
  const [cherryPickOpen, setCherryPickOpen] = useState(false)

  const plugin = useMemo(() => plugins.find((candidate) => candidate.id === pluginId) ?? null, [pluginId, plugins])
  const marketplace = useMemo(
    () => marketplaces.find((candidate) => candidate.id === plugin?.marketplaceId),
    [marketplaces, plugin?.marketplaceId],
  )
  const installedIds = useMemo(
    () => new Set(plugins.filter((candidate) => candidate.installed).map((candidate) => candidate.id)),
    [plugins],
  )

  useEffect(() => {
    if (!plugin) return
    const controller = new AbortController()
    setArtifacts([])
    getArtifacts(plugin.id, controller.signal)
      .then(setArtifacts)
      .catch((error) => {
        if (isAbortError(error)) return
        setArtifacts([])
      })
    return () => controller.abort()
  }, [plugin])

  if (pluginsError || marketplacesError) {
    const error = pluginsError ?? marketplacesError
    const message = error instanceof Error ? error.message : 'Failed to load marketplace data.'
    return <EmptyState title="Plugin details unavailable" sub={message} />
  }

  if (!plugin) {
    return <EmptyState title={pluginId ? 'Plugin not found' : 'Plugin not selected'} sub={pluginId ? `No marketplace plugin exists for "${pluginId}".` : 'Open a plugin from the marketplace list to inspect files and edit artifacts.'} />
  }

  const isInstalled = installedIds.has(plugin.id)
  const marketplaceOwner = marketplace?.githubOwner ?? marketplace?.ghUser

  return (
    <>
      <AppHeader
        breadcrumbs={[
          { label: 'Labby', href: '/' },
          { label: 'Marketplace', href: '/marketplace' },
          { label: plugin.name },
        ]}
        actions={
          <Link
            href="/marketplace"
            className="inline-flex size-9 items-center justify-center rounded-lg border border-aurora-border-strong bg-transparent text-aurora-text-muted transition-all duration-150 hover:bg-aurora-hover-bg hover:text-aurora-text-primary"
            aria-label="Back to marketplace"
          >
            <ArrowLeft className="size-[14px]" />
          </Link>
        }
      />

      <div className="flex flex-1 flex-col gap-4 px-6 pb-6">
        <section className="overflow-hidden rounded-aurora-3 border border-aurora-border-strong bg-aurora-panel-strong shadow-aurora-strong">
          <div className="flex items-center gap-4 border-b border-aurora-border-default bg-aurora-panel-strong px-5 py-[14px]">
            <PluginAvatar ghUser={marketplaceOwner} name={plugin.name} size={44} />
            <div className="min-w-0 flex-1">
              <div className="mb-[5px] text-[10px] font-bold uppercase tracking-[0.16em] leading-none text-aurora-text-muted">
                {marketplace?.name ?? plugin.marketplaceId}
              </div>
              <h1 className="font-display text-[22px] font-bold leading-[1.12] text-aurora-text-primary">
                {plugin.name}
              </h1>
              <div className="mt-[6px] flex flex-wrap items-center gap-[6px]">
                <span className="rounded-full border border-aurora-border-default bg-aurora-control-surface px-[10px] py-[3px] text-[11px] font-semibold text-aurora-text-muted">
                  v{plugin.version}
                </span>
                {plugin.tags.slice(0, 3).map((tag) => (
                  <span
                    key={tag}
                    className="rounded-full border border-aurora-border-default bg-aurora-control-surface px-[9px] py-[3px] text-[10px] font-bold uppercase tracking-[0.14em] text-aurora-text-muted"
                  >
                    {tag}
                  </span>
                ))}
              </div>
            </div>
            <div className="flex flex-shrink-0 items-center gap-2">
              {/* Cherry-pick button — always visible when plugin data is loaded */}
              <Tooltip>
                <TooltipTrigger asChild>
                  <button
                    onClick={() => setCherryPickOpen(true)}
                    className="inline-flex items-center gap-1.5 rounded-lg border border-aurora-border-strong bg-aurora-control-surface px-[14px] py-1.5 text-[13px] font-semibold text-aurora-text-muted transition-all duration-150 hover:bg-aurora-hover-bg hover:text-aurora-text-primary"
                    aria-label={`Cherry-pick components from ${plugin.name}`}
                  >
                    <GitFork className="size-[14px]" />
                    Cherry-pick
                  </button>
                </TooltipTrigger>
                <TooltipContent>Install individual components to specific devices</TooltipContent>
              </Tooltip>
              {isInstalled ? (
                <>
                  {plugin.hasUpdate && (
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <button
                          onClick={() => void install(plugin.id, plugin.name)}
                          className="inline-flex size-9 items-center justify-center rounded-lg border border-[color-mix(in_srgb,var(--aurora-warn)_25%,transparent)] bg-[color-mix(in_srgb,var(--aurora-warn)_7%,transparent)] text-aurora-warn transition-all duration-150 hover:bg-[color-mix(in_srgb,var(--aurora-warn)_14%,transparent)]"
                          aria-label={`Update ${plugin.name}`}
                        >
                          <RefreshCw className="size-[14px]" />
                        </button>
                      </TooltipTrigger>
                      <TooltipContent>{`Update ${plugin.name}`}</TooltipContent>
                    </Tooltip>
                  )}
                  <button
                    onClick={() => setConfirm({
                      title: `Remove ${plugin.name}?`,
                      description: 'This runs `claude plugin uninstall` and removes the plugin from your Claude configuration.',
                      confirmLabel: 'Remove',
                      destructive: true,
                      onConfirm: async () => {
                        await uninstall(plugin.id, plugin.name)
                        setConfirm(null)
                      },
                    })}
                    className="inline-flex items-center gap-1.5 rounded-lg border border-[color-mix(in_srgb,var(--aurora-error)_30%,transparent)] bg-transparent px-[14px] py-1.5 text-[13px] font-semibold text-aurora-error transition-all duration-150 hover:bg-[color-mix(in_srgb,var(--aurora-error)_8%,transparent)]"
                  >
                    <Trash2 className="size-[14px]" />
                    Remove
                  </button>
                </>
              ) : (
                <Tooltip>
                  <TooltipTrigger asChild>
                    <button
                      onClick={() => void install(plugin.id, plugin.name)}
                      className="inline-flex size-9 items-center justify-center rounded-lg bg-aurora-accent-primary text-aurora-page-bg transition-all duration-150 hover:bg-aurora-accent-strong"
                      aria-label={`Install ${plugin.name}`}
                    >
                      <Download className="size-[14px]" />
                    </button>
                  </TooltipTrigger>
                  <TooltipContent>{`Install ${plugin.name}`}</TooltipContent>
                </Tooltip>
              )}
            </div>
          </div>

          <div className="flex items-center gap-0 border-b border-aurora-border-default bg-aurora-nav-bg px-5">
            {(['info', 'files'] as const).map((tabId) => (
              <button
                key={tabId}
                onClick={() => setTab(tabId)}
                className={cn(
                  'mb-[-1px] border-b-2 border-l-0 border-r-0 border-t-0 bg-transparent px-[14px] pb-2 pt-[9px] text-[12px] font-semibold capitalize transition-[color,border-color] duration-150',
                  tab === tabId
                    ? 'border-aurora-accent-primary text-aurora-accent-primary'
                    : 'border-transparent text-aurora-text-muted hover:text-aurora-text-primary',
                )}
              >
                {tabId}
              </button>
            ))}
          </div>

          {tab === 'info' ? (
            <PluginInfoPanel plugin={plugin} artifacts={artifacts} />
          ) : (
            <PluginFilesPanel pluginId={plugin.id} artifacts={artifacts} />
          )}
        </section>
      </div>

      <ConfirmDialog state={confirm} onOpenChange={(open) => { if (!open) setConfirm(null) }} />

      <CherryPickDialog
        pluginId={plugin.id}
        pluginName={plugin.name}
        open={cherryPickOpen}
        onClose={() => setCherryPickOpen(false)}
        components={plugin.components?.map((c) => ({ type: c.kind, name: c.name, path: c.path }))}
      />
    </>
  )
}
