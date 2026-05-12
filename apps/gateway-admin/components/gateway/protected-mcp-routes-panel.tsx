'use client'

import { useEffect, useMemo, useState } from 'react'
import { CheckCircle2, Loader2, Plus, Radar, Save, ShieldCheck, Trash2, X } from 'lucide-react'
import { toast } from 'sonner'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import {
  useGatewayMutations,
  useProtectedMcpRoutes,
} from '@/lib/hooks/use-gateways'
import { doctorApi, isBlockingDoctorSeverity, type DoctorReport } from '@/lib/api/doctor-client'
import type {
  ProtectedMcpRoute,
  ProtectedMcpRouteInput,
  ProtectedMcpRouteTestResult,
} from '@/lib/types/gateway'
import { cn, getErrorMessage } from '@/lib/utils'

type RouteDraft = {
  name: string
  enabled: boolean
  public_host: string
  public_path: string
  upstream: string
  backend_url: string
  scopes: string
  health_path: string
}

const EMPTY_DRAFT: RouteDraft = {
  name: '',
  enabled: true,
  public_host: '',
  public_path: '/',
  upstream: '',
  backend_url: '',
  scopes: '',
  health_path: '',
}

function draftFromRoute(route: ProtectedMcpRoute): RouteDraft {
  return {
    name: route.name,
    enabled: route.enabled,
    public_host: route.public_host,
    public_path: route.public_path,
    upstream: route.upstream ?? '',
    backend_url: route.backend_url ?? '',
    scopes: route.scopes.join(', '),
    health_path: route.health_path ?? '',
  }
}

function routeFromDraft(draft: RouteDraft): ProtectedMcpRouteInput {
  return {
    name: draft.name.trim(),
    enabled: draft.enabled,
    public_host: draft.public_host.trim(),
    public_path: draft.public_path.trim(),
    upstream: draft.upstream.trim() || null,
    backend_url: draft.backend_url.trim(),
    scopes: draft.scopes
      .split(',')
      .map((scope) => scope.trim())
      .filter(Boolean),
    health_path: draft.health_path.trim() || null,
  }
}

function routeResource(route: ProtectedMcpRoute) {
  return `https://${route.public_host}${route.public_path}`
}

function isRouteComplete(draft: RouteDraft) {
  return Boolean(
      draft.name.trim() &&
      draft.public_host.trim() &&
      draft.public_path.trim() &&
      (draft.backend_url.trim() || draft.upstream.trim()),
  )
}

function isSmokeRouteComplete(draft: RouteDraft) {
  return Boolean(draft.public_host.trim() && draft.public_path.trim())
}

const LAB_RESERVED_PATH_PREFIXES = [
  '/.well-known',
  '/_next',
  '/auth',
  '/authorize',
  '/callback',
  '/dev',
  '/health',
  '/jwks',
  '/mcp',
  '/ready',
  '/register',
  '/settings',
  '/setup',
  '/success',
  '/token',
  '/v1',
]

function normalizedHost(value: string) {
  return value.trim().replace(/\.$/, '').toLowerCase()
}

function normalizedPath(value: string) {
  const trimmed = value.trim()
  if (trimmed.length > 1 && trimmed.endsWith('/')) return trimmed.replace(/\/+$/, '')
  return trimmed
}

function protectedRouteDraftHints(
  draft: RouteDraft,
  routes: ProtectedMcpRoute[],
  editingName: string | null,
) {
  const hints: string[] = []
  const name = draft.name.trim()
  const publicHost = normalizedHost(draft.public_host)
  const publicPath = normalizedPath(draft.public_path)
  const backendUrl = draft.backend_url.trim()

  if (name && routes.some((route) => route.name === name && route.name !== editingName)) {
    hints.push(`A protected route named ${name} already exists.`)
  }

  if (publicHost.includes('/') || publicHost.includes(':') || publicHost.includes('@')) {
    hints.push('Public host should be a bare host without scheme, port, path, or user info.')
  }

  if (publicPath && !publicPath.startsWith('/')) {
    hints.push('Public path must start with /.')
  } else if (publicPath === '/') {
    hints.push('Public path needs a service segment, for example /tools.')
  } else if (publicPath.includes('?') || publicPath.includes('#')) {
    hints.push('Public path should not include query strings or fragments.')
  } else if (publicPath.includes('//') || /(^|\/)\.{1,2}(\/|$)/.test(publicPath) || /%2f|%5c|%2e/i.test(publicPath)) {
    hints.push('Public path contains ambiguous or unsafe path segments.')
  }

  if (publicHost && publicPath && draft.enabled) {
    const duplicate = routes.find(
      (route) =>
        route.name !== editingName &&
        route.enabled &&
        normalizedHost(route.public_host) === publicHost &&
        normalizedPath(route.public_path) === publicPath,
    )
    if (duplicate) {
      hints.push(`Public route ${publicHost}${publicPath} is already used by ${duplicate.name}.`)
    }
  }

  if (typeof window !== 'undefined' && publicHost === window.location.hostname.toLowerCase()) {
    const reserved = LAB_RESERVED_PATH_PREFIXES.find(
      (prefix) => publicPath === prefix || publicPath.startsWith(`${prefix}/`),
    )
    if (reserved) {
      hints.push(`Public path ${publicPath} conflicts with Lab's ${reserved} routes on this host.`)
    }
  }

  if (backendUrl) {
    try {
      const parsed = new URL(backendUrl)
      if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
        hints.push('Backend URL must use http:// or https://.')
      }
      if (parsed.pathname !== '/' || parsed.search || parsed.hash) {
        hints.push('Backend URL should be only an origin; remove any path, query, or fragment.')
      }
    } catch {
      hints.push('Backend URL must be a valid URL.')
    }
  }

  return hints
}

export function ProtectedMcpRoutesPanel() {
  const { data: routes = [], isLoading, error } = useProtectedMcpRoutes()
  const {
    addProtectedRoute,
    updateProtectedRoute,
    removeProtectedRoute,
    testProtectedRoute,
  } = useGatewayMutations()

  const [editingName, setEditingName] = useState<string | null>(null)
  const [draft, setDraft] = useState<RouteDraft>(EMPTY_DRAFT)
  const [pendingAction, setPendingAction] = useState<string | null>(null)
  const [testResult, setTestResult] = useState<ProtectedMcpRouteTestResult | null>(null)
  const [smokeResult, setSmokeResult] = useState<DoctorReport | null>(null)
  const [formError, setFormError] = useState<string | null>(null)

  const sortedRoutes = useMemo(
    () => [...routes].sort((left, right) => left.name.localeCompare(right.name)),
    [routes],
  )
  const isEditing = editingName !== null

  useEffect(() => {
    if (isEditing && !routes.some((route) => route.name === editingName)) {
      setEditingName(null)
      setDraft(EMPTY_DRAFT)
      setTestResult(null)
      setSmokeResult(null)
      setFormError(null)
    }
  }, [editingName, isEditing, routes])

  const updateDraft = <K extends keyof RouteDraft>(key: K, value: RouteDraft[K]) => {
    setDraft((current) => ({ ...current, [key]: value }))
    setFormError(null)
    setTestResult(null)
    setSmokeResult(null)
  }

  const startCreate = () => {
    setEditingName(null)
    setDraft(EMPTY_DRAFT)
    setTestResult(null)
    setSmokeResult(null)
    setFormError(null)
  }

  const startEdit = (route: ProtectedMcpRoute) => {
    setEditingName(route.name)
    setDraft(draftFromRoute(route))
    setTestResult(null)
    setSmokeResult(null)
    setFormError(null)
  }

  const validationHints = useMemo(
    () => protectedRouteDraftHints(draft, routes, editingName),
    [draft, editingName, routes],
  )

  const handleTest = async () => {
    if (!isRouteComplete(draft)) {
      setFormError('Name, public host, public path, and backend URL are required before testing.')
      return
    }

    const controller = new AbortController()
    setPendingAction('test')
    setFormError(null)
    try {
      const result = await testProtectedRoute(routeFromDraft(draft), controller.signal)
      setTestResult(result)
      toast.success('Protected route validated')
    } catch (error) {
      const message = getErrorMessage(error, 'Failed to validate protected route')
      setFormError(message)
      toast.error(message)
    } finally {
      setPendingAction(null)
    }
  }

  const handleSave = async () => {
    if (!isRouteComplete(draft)) {
      setFormError('Name, public host, public path, and backend URL are required.')
      return
    }

    const controller = new AbortController()
    const route = routeFromDraft(draft)
    setPendingAction('save')
    setFormError(null)
    try {
      const saved = isEditing && editingName
        ? await updateProtectedRoute(editingName, route, controller.signal)
        : await addProtectedRoute(route, controller.signal)
      setEditingName(saved.name)
      setDraft(draftFromRoute(saved))
      toast.success(isEditing ? 'Protected route updated' : 'Protected route added')
    } catch (error) {
      const message = getErrorMessage(error, 'Failed to save protected route')
      setFormError(message)
      toast.error(message)
    } finally {
      setPendingAction(null)
    }
  }

  const handleSmoke = async () => {
    if (!isSmokeRouteComplete(draft)) {
      setFormError('Public host and public path are required before running the proxy smoke check.')
      return
    }

    const controller = new AbortController()
    setPendingAction('smoke')
    setFormError(null)
    setSmokeResult(null)
    try {
      const report = await doctorApi.proxyCheck(
        {
          app_url: window.location.origin,
          mcp_url: `https://${draft.public_host.trim()}`,
          route: draft.public_path.trim(),
        },
        controller.signal,
      )
      setSmokeResult(report)
      const blocked = report.findings.some((finding) => isBlockingDoctorSeverity(finding.severity))
      if (blocked) {
        toast.error('Protected route proxy smoke failed')
      } else {
        toast.success('Protected route proxy smoke passed')
      }
    } catch (error) {
      const message = getErrorMessage(error, 'Failed to run protected route proxy smoke')
      setFormError(message)
      toast.error(message)
    } finally {
      setPendingAction(null)
    }
  }

  const handleRemove = async (route: ProtectedMcpRoute) => {
    const controller = new AbortController()
    setPendingAction(`remove:${route.name}`)
    try {
      await removeProtectedRoute(route.name, controller.signal)
      if (editingName === route.name) {
        startCreate()
      }
      toast.success('Protected route removed')
    } catch (error) {
      toast.error(getErrorMessage(error, 'Failed to remove protected route'))
    } finally {
      setPendingAction(null)
    }
  }

  return (
    <div className="rounded-lg border bg-aurora-page-bg p-4" data-protected-routes-panel>
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <ShieldCheck className="size-4 text-aurora-text-muted" />
            <h3 className="text-sm font-semibold text-aurora-text-primary">Protected MCP routes</h3>
          </div>
          <p className="mt-1 text-sm text-aurora-text-muted">
            Publish public MCP route prefixes through Lab OAuth while proxying to private upstream MCP backends.
          </p>
        </div>
        <Button type="button" variant="outline" size="sm" onClick={startCreate}>
          <Plus className="mr-2 size-4" />
          New route
        </Button>
      </div>

      {error ? (
        <div className="mt-4 rounded-lg border border-aurora-error/30 bg-aurora-error/10 px-3 py-2 text-sm text-aurora-error">
          {getErrorMessage(error, 'Failed to load protected routes')}
        </div>
      ) : null}

      <div className="mt-4 grid gap-4 xl:grid-cols-[minmax(22rem,0.7fr)_minmax(0,1fr)]">
        <div className="order-2 overflow-hidden rounded-lg border xl:order-2">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Route</TableHead>
                <TableHead>Backend</TableHead>
                <TableHead className="w-[8rem] text-right">State</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {isLoading ? (
                <TableRow>
                  <TableCell colSpan={3} className="py-8 text-center text-aurora-text-muted">
                    Loading protected routes...
                  </TableCell>
                </TableRow>
              ) : sortedRoutes.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={3} className="py-8 text-center text-aurora-text-muted">
                    No protected routes configured
                  </TableCell>
                </TableRow>
              ) : (
                sortedRoutes.map((route) => (
                  <TableRow
                    key={route.name}
                    className={cn(
                      'cursor-pointer',
                      editingName === route.name && 'bg-aurora-control-surface/30',
                    )}
                    onClick={() => startEdit(route)}
                  >
                    <TableCell className="align-top">
                      <div className="min-w-0">
                        <p className="font-medium text-aurora-text-primary">{route.name}</p>
                        <p className="mt-1 break-all font-mono text-xs text-aurora-text-muted">{routeResource(route)}</p>
                        {route.scopes.length > 0 ? (
                          <div className="mt-2 flex flex-wrap gap-1">
                            {route.scopes.map((scope) => (
                              <Badge key={scope} variant="secondary" className="text-[11px]">{scope}</Badge>
                            ))}
                          </div>
                        ) : null}
                      </div>
                    </TableCell>
                    <TableCell className="align-top">
                      <p className="break-all font-mono text-xs text-aurora-text-primary">
                        {route.upstream ? `upstream:${route.upstream}` : route.backend_url}
                      </p>
                      {route.health_path ? (
                        <p className="mt-1 font-mono text-xs text-aurora-text-muted">health {route.health_path}</p>
                      ) : null}
                    </TableCell>
                    <TableCell className="align-top text-right">
                      <div className="flex justify-end gap-2">
                        <Badge variant={route.enabled ? 'default' : 'secondary'}>
                          {route.enabled ? 'Enabled' : 'Disabled'}
                        </Badge>
                        <Button
                          type="button"
                          variant="outline"
                          size="icon"
                          className="size-8"
                          onClick={(event) => {
                            event.stopPropagation()
                            void handleRemove(route)
                          }}
                          disabled={pendingAction === `remove:${route.name}`}
                          aria-label={`Remove protected route ${route.name}`}
                          title="Remove protected route"
                        >
                          {pendingAction === `remove:${route.name}` ? (
                            <Loader2 className="size-3.5 animate-spin" />
                          ) : (
                            <Trash2 className="size-3.5" />
                          )}
                        </Button>
                      </div>
                    </TableCell>
                  </TableRow>
                ))
              )}
            </TableBody>
          </Table>
        </div>

        <div className="order-1 rounded-lg border bg-aurora-control-surface/10 p-4 xl:order-1">
          <div className="flex items-center justify-between gap-3">
            <div>
              <h4 className="text-sm font-semibold text-aurora-text-primary">
                {isEditing ? `Edit ${editingName}` : 'Add protected route'}
              </h4>
              <p className="mt-1 text-xs text-aurora-text-muted">
                Paths should be public prefixes such as /tools. Set a server upstream or a backend MCP URL.
              </p>
            </div>
            <Switch
              checked={draft.enabled}
              onCheckedChange={(enabled) => updateDraft('enabled', enabled)}
              aria-label="Protected route enabled"
            />
          </div>

          <div className="mt-4 grid gap-3">
            <div className="grid gap-1.5">
              <Label htmlFor="protected-route-name">Name</Label>
              <Input
                id="protected-route-name"
                value={draft.name}
                onChange={(event) => updateDraft('name', event.target.value)}
                placeholder="tools"
              />
            </div>
            <div className="grid gap-1.5">
              <Label htmlFor="protected-route-public-host">Public host</Label>
              <Input
                id="protected-route-public-host"
                value={draft.public_host}
                onChange={(event) => updateDraft('public_host', event.target.value)}
                placeholder="mcp.example.net"
              />
            </div>
            <div className="grid gap-1.5">
              <Label htmlFor="protected-route-public-path">Public path</Label>
              <Input
                id="protected-route-public-path"
                value={draft.public_path}
                onChange={(event) => updateDraft('public_path', event.target.value)}
                placeholder="/tools"
              />
            </div>
            <div className="grid gap-1.5">
              <Label htmlFor="protected-route-upstream">Server upstream</Label>
              <Input
                id="protected-route-upstream"
                value={draft.upstream}
                onChange={(event) => updateDraft('upstream', event.target.value)}
                placeholder="axon"
              />
            </div>
            <div className="grid gap-1.5">
              <Label htmlFor="protected-route-backend-url">Backend URL</Label>
              <Input
                id="protected-route-backend-url"
                value={draft.backend_url}
                onChange={(event) => updateDraft('backend_url', event.target.value)}
                placeholder="http://host:3100/mcp"
              />
            </div>
            <div className="grid gap-3 sm:grid-cols-2">
              <div className="grid gap-1.5">
                <Label htmlFor="protected-route-scopes">Scopes</Label>
                <Input
                  id="protected-route-scopes"
                  value={draft.scopes}
                  onChange={(event) => updateDraft('scopes', event.target.value)}
                  placeholder="mcp:read, mcp:write"
                />
              </div>
              <div className="grid gap-1.5">
                <Label htmlFor="protected-route-health-path">Health path</Label>
                <Input
                  id="protected-route-health-path"
                  value={draft.health_path}
                  onChange={(event) => updateDraft('health_path', event.target.value)}
                  placeholder="/health"
                />
              </div>
            </div>
          </div>

          {formError ? (
            <div className="mt-3 rounded-lg border border-aurora-error/30 bg-aurora-error/10 px-3 py-2 text-sm text-aurora-error">
              {formError}
            </div>
          ) : null}

          {validationHints.length > 0 ? (
            <div className="mt-3 rounded-lg border border-aurora-warning/30 bg-aurora-warning/10 px-3 py-2 text-sm text-aurora-text-primary">
              <ul className="space-y-1">
                {validationHints.map((hint) => (
                  <li key={hint}>{hint}</li>
                ))}
              </ul>
            </div>
          ) : null}

          {testResult ? (
            <div className="mt-3 rounded-lg border border-aurora-success/30 bg-aurora-success/10 px-3 py-2 text-sm">
              <div className="flex items-center gap-2 text-aurora-text-primary">
                <CheckCircle2 className="size-4 text-aurora-success" />
                <span className="font-medium">Route validated</span>
              </div>
              <p className="mt-1 break-all font-mono text-xs text-aurora-text-muted">{testResult.resource}</p>
              <p className="mt-1 break-all font-mono text-xs text-aurora-text-muted">{testResult.metadata_url}</p>
            </div>
          ) : null}

          {smokeResult ? (
            <div className="mt-3 rounded-lg border bg-aurora-page-bg px-3 py-2 text-sm">
              <div className="flex items-center gap-2 text-aurora-text-primary">
                <Radar className="size-4 text-aurora-text-muted" />
                <span className="font-medium">Proxy smoke results</span>
              </div>
              <div className="mt-2 space-y-1">
                {smokeResult.findings.map((finding) => (
                  <div key={`${finding.category ?? 'proxy'}:${finding.message}`} className="flex gap-2 text-xs">
                    <Badge
                      variant={isBlockingDoctorSeverity(finding.severity) ? 'default' : 'secondary'}
                      status={isBlockingDoctorSeverity(finding.severity) ? 'error' : 'default'}
                      className="h-5 shrink-0"
                    >
                      {finding.severity}
                    </Badge>
                    <span className="min-w-0 text-aurora-text-muted">{finding.message}</span>
                  </div>
                ))}
              </div>
            </div>
          ) : null}

          <div className="mt-4 flex flex-wrap justify-end gap-2">
            {isEditing ? (
              <Button type="button" variant="outline" size="sm" onClick={startCreate}>
                <X className="mr-2 size-4" />
                Clear
              </Button>
            ) : null}
            <Button type="button" variant="outline" size="sm" onClick={handleTest} disabled={pendingAction !== null}>
              {pendingAction === 'test' ? (
                <Loader2 className="mr-2 size-4 animate-spin" />
              ) : (
                <ShieldCheck className="mr-2 size-4" />
              )}
              Test
            </Button>
            <Button type="button" variant="outline" size="sm" onClick={handleSmoke} disabled={pendingAction !== null}>
              {pendingAction === 'smoke' ? (
                <Loader2 className="mr-2 size-4 animate-spin" />
              ) : (
                <Radar className="mr-2 size-4" />
              )}
              Smoke
            </Button>
            <Button type="button" size="sm" onClick={handleSave} disabled={pendingAction !== null}>
              {pendingAction === 'save' ? (
                <Loader2 className="mr-2 size-4 animate-spin" />
              ) : (
                <Save className="mr-2 size-4" />
              )}
              {isEditing ? 'Save route' : 'Add route'}
            </Button>
          </div>
        </div>
      </div>
    </div>
  )
}
