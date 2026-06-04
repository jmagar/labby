'use client'

import Image from 'next/image'
import { useState, useEffect, useCallback, useRef } from 'react'
import { Package, ExternalLink, RefreshCw, RotateCcw } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { AppHeader } from '@/components/app-header'
import { ServerFilters } from './server-filters'
import { getRegistryConfig } from '@/lib/api/mcpregistry-client'
import { fetchRegistryServers, registryServersKey } from '@/lib/hooks/use-registry'
import type { RegistryServersKey } from '@/lib/hooks/use-registry'
import { safeHref } from '@/lib/utils/safe-href'
import { githubAvatarFromRepoUrl } from '@/lib/github-avatar'
import useSWR from 'swr'
import useSWRInfinite from 'swr/infinite'
import { cn } from '@/lib/utils'
import {
  AURORA_GATEWAY_ROW,
  AURORA_GATEWAY_DISABLED_ROW,
  AURORA_MEDIUM_PANEL,
  AURORA_PAGE_FRAME,
  AURORA_PAGE_SHELL,
} from '@/components/gateway/gateway-theme'
import { LAB_REGISTRY_META_KEY, REGISTRY_META_KEY } from '@/lib/types/registry'
import { RegistryStatusBadge } from './registry-status-badge'
import type { ServerResponse, ServerListResponse } from '@/lib/types/registry'

interface RegistryListContentProps {
  onSelectServer?: (response: ServerResponse) => void
}

const DESCRIPTION_CHAR_LIMIT = 2000

function truncateDescription(desc: string): { text: string; truncated: boolean } {
  if (desc.length <= DESCRIPTION_CHAR_LIMIT) return { text: desc, truncated: false }
  return { text: desc.slice(0, DESCRIPTION_CHAR_LIMIT), truncated: true }
}

export function RegistryListContent({ onSelectServer }: RegistryListContentProps) {
  const [search, setSearch] = useState('')
  const [debouncedSearch, setDebouncedSearch] = useState('')
  const [version, setVersion] = useState('')
  const [debouncedVersion, setDebouncedVersion] = useState('')
  const [updatedSince, setUpdatedSince] = useState('')
  const [debouncedUpdatedSince, setDebouncedUpdatedSince] = useState('')
  const [tag, setTag] = useState('')
  const [debouncedTag, setDebouncedTag] = useState('')
  const [featuredOnly, setFeaturedOnly] = useState(false)
  const [hiddenOnly, setHiddenOnly] = useState(false)
  const [reviewedOnly, setReviewedOnly] = useState(false)
  const [recommendedOnly, setRecommendedOnly] = useState(false)
  const [expandedDescriptions, setExpandedDescriptions] = useState<Set<string>>(new Set())
  const sentinelRef = useRef<HTMLDivElement>(null)
  const pagingRef = useRef(false)

  // Debounce filter fields before updating the SWR key to avoid a fetch on every keystroke
  useEffect(() => {
    const timer = setTimeout(() => {
      setDebouncedSearch(search)
      setDebouncedVersion(version)
      setDebouncedUpdatedSince(updatedSince)
      setDebouncedTag(tag)
    }, 300)
    return () => clearTimeout(timer)
  }, [search, version, updatedSince, tag])

  const getKey = useCallback(
    (pageIndex: number, previousData: ServerListResponse | null): RegistryServersKey | null => {
      if (previousData && !previousData.metadata.nextCursor) return null
      const cursor = pageIndex === 0 ? null : (previousData?.metadata.nextCursor ?? null)
      return registryServersKey(
        debouncedSearch,
        cursor,
        debouncedVersion,
        debouncedUpdatedSince,
        featuredOnly || undefined,
        reviewedOnly || undefined,
        recommendedOnly || undefined,
        hiddenOnly || undefined,
        debouncedTag || undefined,
      )
    },
    [debouncedSearch, debouncedVersion, debouncedUpdatedSince, featuredOnly, reviewedOnly, recommendedOnly, hiddenOnly, debouncedTag],
  )

  const { data: pages, isLoading, isValidating, error, mutate, setSize } = useSWRInfinite<ServerListResponse>(
    getKey,
    (key: RegistryServersKey) => fetchRegistryServers(key),
    { revalidateOnFocus: false, revalidateFirstPage: false },
  )

  const allServers = pages?.flatMap((page) => page.servers) ?? []
  const visibleServers = allServers

  const lastPage = pages?.[pages.length - 1]
  const hasMore = Boolean(lastPage?.metadata.nextCursor)
  const totalLoaded = visibleServers.length

  // Sentinel observer — loads next page when the bottom of the list scrolls into view
  useEffect(() => {
    const el = sentinelRef.current
    if (!el) return
    const observer = new IntersectionObserver(
      ([entry]) => {
        if (entry.isIntersecting && hasMore && !isValidating && !error) {
          if (pagingRef.current) return
          pagingRef.current = true
          setSize(s => s + 1)
        }
      },
      { rootMargin: '300px' },
    )
    observer.observe(el)
    return () => observer.disconnect()
  }, [error, hasMore, isValidating, setSize])

  useEffect(() => {
    if (!isValidating && !error) {
      pagingRef.current = false
    }
  }, [error, isValidating])

  const toggleDescription = (name: string) => {
    setExpandedDescriptions((prev) => {
      const next = new Set(prev)
      if (next.has(name)) next.delete(name)
      else next.add(name)
      return next
    })
  }

  const { data: config } = useSWR('/registry/config', () => getRegistryConfig(), {
    revalidateOnFocus: false,
    revalidateIfStale: false,
  })

  return (
    <div className={AURORA_PAGE_SHELL}>
      <AppHeader
        breadcrumbs={[{ label: 'MCP Registry' }]}
        actions={
          <>
            {config?.url && (
              <code
                title={config.url}
                className="hidden rounded-md border border-aurora-border-strong/60 bg-aurora-control-surface px-2 py-1 font-mono text-xs text-aurora-text-muted md:inline-block"
              >
                {config.url}
              </code>
            )}
            <Button
              variant="outline"
              size="sm"
              onClick={() => void mutate()}
              disabled={isValidating}
              className="gap-1.5"
            >
              <RefreshCw className={cn('size-4', isValidating && 'animate-spin')} />
              Refresh
            </Button>
          </>
        }
      />

      <div className={cn(AURORA_PAGE_FRAME, 'space-y-4')}>
        <ServerFilters
          search={search}
          onSearchChange={setSearch}
          version={version}
          onVersionChange={setVersion}
          updatedSince={updatedSince}
          onUpdatedSinceChange={setUpdatedSince}
          hiddenOnly={hiddenOnly}
          onHiddenOnlyChange={setHiddenOnly}
          tag={tag}
          onTagChange={setTag}
          totalLoaded={totalLoaded}
          hasMore={hasMore}
          isLoading={isLoading}
        />

        <div className="flex flex-wrap gap-2">
          <Button type="button" variant={featuredOnly ? 'default' : 'outline'} size="sm" onClick={() => setFeaturedOnly((v) => !v)}>
            Featured
          </Button>
          <Button type="button" variant={reviewedOnly ? 'default' : 'outline'} size="sm" onClick={() => setReviewedOnly((v) => !v)}>
            Reviewed
          </Button>
          <Button type="button" variant={recommendedOnly ? 'default' : 'outline'} size="sm" onClick={() => setRecommendedOnly((v) => !v)}>
            Recommended
          </Button>
          <Button type="button" variant={hiddenOnly ? 'default' : 'outline'} size="sm" onClick={() => setHiddenOnly((v) => !v)}>
            Hidden
          </Button>
        </div>

        {/* Error state */}
        {!isLoading && error && (
          <div className={cn(AURORA_MEDIUM_PANEL, 'space-y-3 p-6 text-center')}>
            <p className="text-sm text-aurora-error">
              {error instanceof Error ? error.message : 'Failed to load registry'}
            </p>
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                pagingRef.current = false
                void mutate()
              }}
              className="gap-1.5"
            >
              <RotateCcw className="size-4" />
              Retry
            </Button>
          </div>
        )}

        {/* Initial loading skeletons */}
        {isLoading && allServers.length === 0 && (
          <div className="space-y-2">
            {Array.from({ length: 6 }, (_, i) => (
              <div
                key={i}
                className="h-20 animate-pulse rounded-lg border border-aurora-border-strong/40 bg-aurora-control-surface/70"
              />
            ))}
          </div>
        )}

        {/* Empty state */}
        {!isLoading && !error && visibleServers.length === 0 && pages && (
          <div className={cn(AURORA_MEDIUM_PANEL, 'p-10 text-center text-sm text-aurora-text-muted')}>
            No servers found{debouncedSearch ? ` for "${debouncedSearch}"` : ''}.
          </div>
        )}

        {/* Server list */}
        {visibleServers.length > 0 && (
          <div className="overflow-hidden rounded-lg border border-aurora-border-strong">
            {visibleServers.map((response) => {
              const server = response.server
              const { remotes, icons } = server
              const isHTTP = remotes.some(r => r.type === 'streamable-http' || r.type === 'sse')
              const ghAvatar = githubAvatarFromRepoUrl(server.repository?.url)
              const fallbackIcon = icons[0] ?? null
              const fallbackIconHref = safeHref(fallbackIcon?.src)
              const avatarSrc = ghAvatar ?? fallbackIconHref ?? null
              const displayName = server.title ?? server.name
              const { text: descText, truncated } = truncateDescription(server.description)
              const isExpanded = expandedDescriptions.has(server.name)
              const repoHref = safeHref(server.repository?.url)
              const status = response._meta?.[REGISTRY_META_KEY]?.status ?? 'active'
              const labMeta = response._meta?.[LAB_REGISTRY_META_KEY]
              const isDeleted = status === 'deleted'

              return (
                <div
                  key={server.name}
                  className={cn(
                    isDeleted ? cn(AURORA_GATEWAY_DISABLED_ROW, 'opacity-60') : AURORA_GATEWAY_ROW,
                    'cursor-pointer px-5 py-4',
                  )}
                  onClick={() => onSelectServer?.(response)}
                  role="button"
                  tabIndex={0}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter' || e.key === ' ') onSelectServer?.(response)
                  }}
                >
                  <div className="flex items-start gap-4">
                    <div className="flex size-10 shrink-0 items-center justify-center overflow-hidden rounded-lg border border-aurora-border-strong/60 bg-aurora-control-surface">
                      {avatarSrc ? (
                        <>
                          <Image
                            src={avatarSrc}
                            alt=""
                            className="size-full object-cover"
                            height={40}
                            width={40}
                            unoptimized
                            referrerPolicy="no-referrer"
                            onError={(e) => {
                              const img = e.currentTarget
                              if (ghAvatar && fallbackIconHref && img.dataset.fallbackApplied !== 'true') {
                                img.dataset.fallbackApplied = 'true'
                                img.src = fallbackIconHref
                                return
                              }
                              img.style.display = 'none'
                              ;(img.nextElementSibling as HTMLElement | null)?.removeAttribute('style')
                            }}
                          />
                          <Package className="size-5 text-aurora-text-muted" style={{ display: 'none' }} />
                        </>
                      ) : (
                        <Package className="size-5 text-aurora-text-muted" />
                      )}
                    </div>

                    <div className="min-w-0 flex-1">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="truncate font-semibold text-aurora-text-primary">
                          {displayName}
                        </span>
                        <span className="font-mono text-xs text-aurora-text-muted">
                          {server.name}
                        </span>
                        <span className="text-xs text-aurora-text-muted">v{server.version}</span>
                        <span
                          className={cn(
                            'rounded-full px-2 py-0.5 text-xs font-medium',
                            isHTTP
                              ? 'bg-aurora-accent-strong/15 text-aurora-accent-strong'
                              : 'bg-aurora-border-strong/40 text-aurora-text-muted',
                          )}
                        >
                          {isHTTP ? 'HTTP' : 'stdio only'}
                        </span>
                        <RegistryStatusBadge status={status} />
                        {labMeta?.curation?.featured && (
                          <span className="rounded-full border border-aurora-accent-primary/30 bg-aurora-accent-primary/10 px-2 py-0.5 text-xs font-medium text-aurora-accent-primary">
                            Featured
                          </span>
                        )}
                        {labMeta?.trust?.reviewed && (
                          <span className="rounded-full border border-aurora-success/30 bg-aurora-success/10 px-2 py-0.5 text-xs font-medium text-aurora-success">
                            Reviewed
                          </span>
                        )}
                        {labMeta?.ux?.recommended_for_homelab && (
                          <span className="rounded-full border border-aurora-warn/30 bg-aurora-warn/10 px-2 py-0.5 text-xs font-medium text-aurora-warn">
                            Recommended
                          </span>
                        )}
                      </div>

                      <p className="mt-1 text-sm text-aurora-text-muted">
                        {isExpanded ? server.description : descText}
                        {truncated && !isExpanded && '…'}
                      </p>
                      {truncated && (
                        <button
                          className="mt-0.5 text-xs text-aurora-accent-strong hover:underline"
                          onClick={(e) => {
                            e.stopPropagation()
                            toggleDescription(server.name)
                          }}
                          type="button"
                        >
                          {isExpanded ? 'Show less' : 'Show more'}
                        </button>
                      )}

                      {repoHref && (
                        <a
                          href={repoHref}
                          target="_blank"
                          rel="noopener noreferrer"
                          onClick={(e) => e.stopPropagation()}
                          className="mt-1 inline-flex items-center gap-1 text-xs text-aurora-text-muted hover:text-aurora-text-primary"
                        >
                          <ExternalLink className="size-3" />
                          Repository
                        </a>
                      )}
                    </div>
                  </div>
                </div>
              )
            })}
          </div>
        )}

        {/* Sentinel + inline loading indicator */}
        <div ref={sentinelRef} className="flex h-8 items-center justify-center">
          {isValidating && visibleServers.length > 0 && (
            <p className="text-xs text-aurora-text-muted">Loading more…</p>
          )}
          {!hasMore && visibleServers.length > 0 && !isValidating && (
            <p className="text-xs text-aurora-text-muted">
              {totalLoaded} server{totalLoaded === 1 ? '' : 's'} loaded
            </p>
          )}
        </div>
      </div>
    </div>
  )
}
