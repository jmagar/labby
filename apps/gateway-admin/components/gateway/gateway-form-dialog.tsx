'use client'

import Image from 'next/image'
import { useEffect, useMemo, useRef, useState } from 'react'
import { Loader2, Play, ShieldCheck, AlertCircle, CheckCircle2, ChevronRight, ShieldOff, KeyRound } from 'lucide-react'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { TextSurface } from '@/components/ui/text-surface'
import { RadioGroup, RadioGroupItem } from '@/components/ui/radio-group'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { FieldGroup, Field, FieldLabel, FieldDescription } from '@/components/ui/field'
import {
  useGatewayMutations,
  useProtectedMcpRoutes,
  useServiceConfig,
  useSupportedServices,
} from '@/lib/hooks/use-gateways'
import type {
  Gateway,
  CreateGatewayInput,
  UpdateGatewayInput,
  TransportType,
  SupportedService,
  ProtectedMcpRouteInput,
} from '@/lib/types/gateway'
import { toast } from 'sonner'
import { cn, getErrorMessage } from '@/lib/utils'
import { defaultGatewayBearerEnvName, validateBearerTokenEnvName } from '@/lib/gateway-env'
import { validateGatewayName } from '@/lib/utils/gateway-name'
import { isAbortError } from '@/lib/api/service-action-client'
import { GatewayApiError } from '@/lib/api/gateway-client-core'
import {
  initialGatewayAuthMode,
  normalizeProtectedPublicPath,
  protectedRouteForGateway,
  protectedRoutePathInputValue,
  type GatewayAuthMode,
} from '@/lib/gateway-protected-route'
import { upstreamOauthApi } from '@/lib/api/upstream-oauth-client'
import { useUpstreamOauthStatus } from '@/lib/hooks/use-upstream-oauth'
import type { OAuthConnectState } from '@/lib/types/upstream-oauth'
import { formatStdioCommandLine, parseStdioCommandLine } from '@/lib/stdio-command'
import { Badge } from '@/components/ui/badge'
import {
  SERVICE_BRANDS,
  SERVICE_BRAND_FALLBACK,
  SERVICE_ENV_PREFIXES,
  SERVICE_LOGOS,
  SERVICE_SVG_FALLBACKS,
  isServiceKey,
} from '@/lib/branding/service-brands'

interface GatewayFormDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  gateway: Gateway | null
  onSave: (input: CreateGatewayInput | UpdateGatewayInput) => Promise<void>
}

type FormMode = 'custom' | 'lab'
type GatewayAuthSource = 'paste' | 'env'

const PROTECTED_MCP_PUBLIC_HOST = process.env.NEXT_PUBLIC_PROTECTED_MCP_HOST || 'mcp.tootie.tv'
const PROTECTED_ROUTE_SCOPES = ['mcp:read', 'mcp:write']

function valuePreview(fieldName: string, preview?: string | null) {
  return preview ?? (fieldName.endsWith('_URL') ? 'http://localhost' : '')
}

function parseEnvText(text: string): { pairs: Record<string, string>; detectedServices: string[] } {
  const pairs: Record<string, string> = {}
  for (const line of text.split('\n')) {
    const trimmed = line.trim()
    if (!trimmed || trimmed.startsWith('#')) continue
    const eqIdx = trimmed.indexOf('=')
    if (eqIdx < 1) continue
    const key = trimmed.slice(0, eqIdx).trim()
    const val = trimmed.slice(eqIdx + 1).trim()
    pairs[key] = val
  }
  const found = new Set<string>()
  for (const key of Object.keys(pairs)) {
    for (const [prefix, serviceKey] of Object.entries(SERVICE_ENV_PREFIXES)) {
      if (key.startsWith(`${prefix}_`)) {
        found.add(serviceKey)
      }
    }
  }
  return { pairs, detectedServices: [...found] }
}

function ServiceIconBox({ serviceKey }: { serviceKey: string }) {
  const [imgError, setImgError] = useState(false)
  const known = isServiceKey(serviceKey) ? serviceKey : null
  const brand = known ? SERVICE_BRANDS[known] : SERVICE_BRAND_FALLBACK
  const logo = !imgError && known ? SERVICE_LOGOS[known] : null
  const svg = known ? SERVICE_SVG_FALLBACKS[known] : undefined

  return (
    <div
      className="flex items-center justify-center w-9 h-9 rounded-lg shrink-0"
      style={{
        background: 'var(--aurora-control-surface)',
        border: `2px solid ${brand}`,
        boxShadow: `0 0 0 1px ${brand}33`,
      }}
    >
      {logo ? (
        <Image src={logo} alt="" className="h-5 w-5 object-contain" height={20} width={20} unoptimized onError={() => setImgError(true)} />
      ) : svg ? (
        <span
          className="w-5 h-5 block"
          style={{ color: brand }}
          // biome-ignore lint/security/noDangerouslySetInnerHtml: trusted static SVG strings
          dangerouslySetInnerHTML={{ __html: svg.replace('fill="white"', `fill="${brand}"`) }}
        />
      ) : (
        <span className="text-xs font-bold" style={{ color: brand }}>{serviceKey[0]?.toUpperCase()}</span>
      )}
    </div>
  )
}

const emptyCustomState = {
  transport: 'http' as TransportType,
  name: '',
  url: '',
  command: '',
  bearerTokenEnv: '',
  proxyResources: true,
  proxyPrompts: true,
}

function serviceFields(serviceMeta: SupportedService | null) {
  return serviceMeta ? [...serviceMeta.required_env, ...serviceMeta.optional_env] : []
}

export function shouldAutoConnectOauth({
  open,
  isEditing,
  transport,
  authMode,
  oauthDiscovered,
  upstream,
}: {
  open: boolean
  isEditing: boolean
  transport: TransportType
  authMode: GatewayAuthMode
  oauthDiscovered: boolean
  upstream?: string
}) {
  return open
    && !isEditing
    && transport === 'http'
    && authMode === 'none'
    && oauthDiscovered
    && !!upstream?.trim()
}

export function oauthConnectButtonLabel(state: OAuthConnectState) {
  if (state.kind === 'probing') return 'Detecting OAuth...'
  if (state.kind === 'authorizing') return 'Waiting...'
  if (state.kind === 'blocked') return 'Click to authorize'
  return 'Connect via OAuth'
}

export function GatewayFormDialog({
  open,
  onOpenChange,
  gateway,
  onSave,
}: GatewayFormDialogProps) {
  const isEditing = !!gateway
  const isLabGateway = gateway?.source === 'in_process'
  const prevOpenRef = useRef(false)
  const abortControllerRef = useRef<AbortController | null>(null)
  const probeInfoRef = useRef<{ registration_strategy: string; scopes?: string[] } | null>(null)
  const currentProbeUrlRef = useRef('')
  const nameAutoRef = useRef(false)
  const skipUrlOauthResetRef = useRef(false)
  const autoOauthAttemptedForRef = useRef<string | null>(null)
  const protectedRouteHydratedForRef = useRef<string | null>(null)
  const protectedRouteTouchedRef = useRef(false)
  const { data: supportedServices } = useSupportedServices()
  const { data: protectedRoutes = [] } = useProtectedMcpRoutes()
  const { testGateway, saveServiceConfig, enableVirtualServer, disableVirtualServer, addProtectedRoute, updateProtectedRoute, removeProtectedRoute } =
    useGatewayMutations()

  const [mode, setMode] = useState<FormMode>('custom')
  const [transport, setTransport] = useState<TransportType>('http')
  const [name, setName] = useState('')
  const [url, setUrl] = useState('')
  const [protectedPublicPath, setProtectedPublicPath] = useState('')
  const [command, setCommand] = useState('')
  const [authMode, setAuthMode] = useState<GatewayAuthMode>('none')
  const [authSource, setAuthSource] = useState<GatewayAuthSource>('paste')
  const [bearerTokenEnv, setBearerTokenEnv] = useState('')
  const [bearerTokenValue, setBearerTokenValue] = useState('')
  const [proxyResources, setProxyResources] = useState(true)
  const [proxyPrompts, setProxyPrompts] = useState(true)
  const [envDrawerOpen, setEnvDrawerOpen] = useState(false)
  const [jsonDrawerOpen, setJsonDrawerOpen] = useState(false)
  const [jsonText, setJsonText] = useState('')
  const [jsonValid, setJsonValid] = useState(false)
  const syncingRef = useRef(false)
  const [envText, setEnvText] = useState('')

  const [selectedService, setSelectedService] = useState('')
  const [serviceValues, setServiceValues] = useState<Record<string, string>>({})
  const [enableServer, setEnableServer] = useState(true)

  const [isSaving, setIsSaving] = useState(false)
  const [isTesting, setIsTesting] = useState(false)
  const [saveError, setSaveError] = useState<string | null>(null)
  const [errors, setErrors] = useState<Record<string, string>>({})
  const [oauthState, setOauthState] = useState<OAuthConnectState>({ kind: 'idle' })
  const [oauthProbed, setOauthProbed] = useState<{ oauth_discovered: boolean; upstream: string; issuer?: string; scopes?: string[]; registration_strategy?: string } | null>(null)
  const [isProbing, setIsProbing] = useState(false)

  const serviceMeta = useMemo(
    () => supportedServices?.find((service) => service.key === selectedService) ?? null,
    [selectedService, supportedServices],
  )
  const serviceEnvFields = useMemo(() => serviceFields(serviceMeta), [serviceMeta])
  const existingProtectedRoute = useMemo(
    () => protectedRouteForGateway(gateway, protectedRoutes, PROTECTED_MCP_PUBLIC_HOST),
    [gateway, protectedRoutes],
  )
  const protectedRoutePathOptions = useMemo(() => {
    const seen = new Set<string>()
    return protectedRoutes
      .filter((route) => route.enabled && route.public_host === PROTECTED_MCP_PUBLIC_HOST)
      .map((route) => route.public_path)
      .filter((path) => {
        if (seen.has(path)) return false
        seen.add(path)
        return true
      })
      .sort((left, right) => left.localeCompare(right))
  }, [protectedRoutes])
  const { data: serviceConfig } = useServiceConfig(mode === 'lab' && selectedService ? selectedService : null)

  const oauthUpstream = oauthState.kind === 'authorizing'
    || oauthState.kind === 'connected'
    || oauthState.kind === 'discovered'
    || oauthState.kind === 'blocked'
    ? (oauthState as { upstream: string }).upstream
    : null
  const { data: oauthStatus } = useUpstreamOauthStatus(
    oauthState.kind === 'authorizing' ? oauthUpstream : null,
    { pollWhilePending: oauthState.kind === 'authorizing' },
  )

  useEffect(() => {
    if (oauthState.kind === 'authorizing' && oauthStatus?.authenticated) {
      const info = probeInfoRef.current
      setOauthState({
        kind: 'connected',
        upstream: oauthState.upstream,
        registration_strategy: info?.registration_strategy ?? 'dynamic',
        scopes: info?.scopes,
      })
    }
  }, [oauthState, oauthStatus?.authenticated])

  // Auto-probe the URL for OAuth support when transport is HTTP and URL looks valid.
  // Resets probed state and authMode when URL changes so stale OAuth option disappears.
  useEffect(() => {
    currentProbeUrlRef.current = url.trim()
    if (transport !== 'http' || !url.trim()) {
      setOauthProbed(null)
      setIsProbing(false)
      if (authMode === 'oauth' && !protectedPublicPath.trim()) setAuthMode('none')
      return
    }
    const requestedUrl = url.trim()
    setOauthProbed(null)
    const ac = new AbortController()
    const timer = setTimeout(() => {
      setIsProbing(true)
      upstreamOauthApi.probe(requestedUrl, ac.signal).then((result) => {
        if (ac.signal.aborted || currentProbeUrlRef.current !== requestedUrl) return
        setOauthProbed(result); setIsProbing(false)
      }).catch((err: unknown) => {
        if (isAbortError(err)) return
        if (ac.signal.aborted || currentProbeUrlRef.current !== requestedUrl) return
        setOauthProbed({ oauth_discovered: false, upstream: '' }); setIsProbing(false)
      })
    }, 600)
    return () => {
      ac.abort()
      setIsProbing(false)
      clearTimeout(timer)
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [url, transport])

  useEffect(() => {
    if (!oauthProbed) return
    if (!shouldAutoConnectOauth({
      open,
      isEditing,
      transport,
      authMode,
      oauthDiscovered: oauthProbed.oauth_discovered,
      upstream: oauthProbed.upstream,
    })) return

    const attemptKey = `${url.trim()}::${oauthProbed.upstream}`
    if (autoOauthAttemptedForRef.current === attemptKey) return
    autoOauthAttemptedForRef.current = attemptKey

    setAuthMode('oauth')
    void runOauthConnect({ authTab: null, auto: true, probeOverride: oauthProbed })
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [authMode, isEditing, oauthProbed, open, transport, url])

  // Auto-fill the name from the URL hostname when the user hasn't typed a name yet.
  useEffect(() => {
    if (isEditing || transport !== 'http' || !url.trim()) return
    try {
      const hostname = new URL(url).hostname.replace(/^www\./, '')
      const slug = hostname.replace(/[^a-z0-9]+/gi, '-').toLowerCase().replace(/^-+|-+$/g, '')
      if (!slug) return
      setName((prev) => {
        if (!prev || nameAutoRef.current) {
          nameAutoRef.current = true
          return slug
        }
        return prev
      })
    } catch {
      // invalid URL, skip
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [url])

  async function runOauthConnect({
    authTab,
    auto,
    probeOverride,
  }: {
    authTab: Window | null
    auto: boolean
    probeOverride?: NonNullable<typeof oauthProbed>
  }) {
    if (!url.trim()) return
    setOauthState({ kind: 'probing' })
    try {
      const requestedUpstream = name.trim() || undefined
      const reusableProbe = probeOverride?.oauth_discovered
        && (!requestedUpstream || probeOverride.upstream === requestedUpstream)
      const probe = reusableProbe
        ? probeOverride
        : await upstreamOauthApi.probe(url.trim(), undefined, requestedUpstream)
      if (!probe.oauth_discovered) {
        authTab?.close()
        setOauthState({ kind: 'error', message: 'This server does not advertise OAuth support' })
        return
      }
      setOauthProbed(probe)
      setOauthState({ kind: 'discovered', upstream: probe.upstream, issuer: probe.issuer, scopes: probe.scopes })
      probeInfoRef.current = { registration_strategy: probe.registration_strategy ?? 'dynamic', scopes: probe.scopes }
      const { authorization_url } = await upstreamOauthApi.start(probe.upstream)

      const targetTab = authTab ?? window.open(authorization_url, '_blank')
      if (!targetTab || targetTab.closed) {
        setOauthState(
          auto
            ? { kind: 'blocked', upstream: probe.upstream, issuer: probe.issuer, scopes: probe.scopes }
            : { kind: 'error', message: 'Authorization tab was closed. Please try again.' },
        )
        return
      }
      if (authTab) {
        authTab.location.href = authorization_url
      }
      setOauthState({ kind: 'authorizing', upstream: probe.upstream })
    } catch (err: unknown) {
      authTab?.close()
      setOauthState({ kind: 'error', message: err instanceof Error ? err.message : 'OAuth connection failed' })
    }
  }

  async function handleOauthConnect() {
    if (!url.trim()) return
    // Open a blank tab synchronously in the click handler so browsers allow it.
    const authTab = window.open('about:blank', '_blank')
    await runOauthConnect({ authTab, auto: false, probeOverride: oauthProbed ?? undefined })
  }

  useEffect(() => {
    const wasOpen = prevOpenRef.current
    prevOpenRef.current = open
    if (!open || wasOpen) return

    setEnvDrawerOpen(false)
    setJsonDrawerOpen(false)

    if (gateway) {
      if (gateway.source === 'in_process') {
        setMode('lab')
        setSelectedService(gateway.id)
        setEnableServer(gateway.enabled ?? true)
      } else {
        setMode('custom')
        setTransport(gateway.transport === 'in_process' ? 'http' : gateway.transport)
        setName(gateway.name)
        const protectedRoute = protectedRouteForGateway(gateway, protectedRoutes, PROTECTED_MCP_PUBLIC_HOST)
        const protectedPath = protectedRoutePathInputValue(protectedRoute)
        const initialAuthMode = initialGatewayAuthMode(gateway, protectedRoute)
        protectedRouteHydratedForRef.current = protectedRoute ? gateway.id : null
        protectedRouteTouchedRef.current = false
        skipUrlOauthResetRef.current = initialAuthMode === 'oauth'
        setUrl(gateway.config.url || '')
        setProtectedPublicPath(protectedPath)
        setCommand(formatStdioCommandLine(gateway.config.command, gateway.config.args))
        setAuthMode(initialAuthMode)
        if (gateway.config.oauth_enabled) {
          setOauthState({ kind: 'connected', upstream: gateway.name, registration_strategy: 'preregistered', scopes: undefined })
          setOauthProbed({ oauth_discovered: true, upstream: gateway.name })
        } else {
          setOauthState({ kind: 'idle' })
          setOauthProbed(null)
        }
        setAuthSource(gateway.config.bearer_token_env ? 'env' : 'paste')
        setBearerTokenEnv(gateway.config.bearer_token_env || '')
        setBearerTokenValue('')
        setProxyResources(gateway.config.proxy_resources ?? true)
        setProxyPrompts(gateway.config.proxy_prompts ?? true)
      }
      } else {
        setMode('custom')
        setTransport(emptyCustomState.transport)
        setName(emptyCustomState.name)
        setUrl(emptyCustomState.url)
        setProtectedPublicPath('')
        protectedRouteHydratedForRef.current = null
        protectedRouteTouchedRef.current = false
        autoOauthAttemptedForRef.current = null
        setCommand(emptyCustomState.command)
        setAuthMode('none')
        setAuthSource('paste')
        setBearerTokenEnv(emptyCustomState.bearerTokenEnv)
        setBearerTokenValue('')
        setProxyResources(emptyCustomState.proxyResources)
        setProxyPrompts(emptyCustomState.proxyPrompts)
        setSelectedService('')
        setServiceValues({})
        setEnableServer(true)
        nameAutoRef.current = false
      }
    setErrors({})
  }, [open, gateway, protectedRoutes])

  useEffect(() => {
    if (!open || !gateway || gateway.source === 'in_process') return
    if (protectedRouteHydratedForRef.current === gateway.id) return
    if (protectedRouteTouchedRef.current) return
    const protectedRoute = existingProtectedRoute
    if (!protectedRoute) return

    protectedRouteHydratedForRef.current = gateway.id
    setProtectedPublicPath(protectedRoutePathInputValue(protectedRoute))
    setAuthMode((current) => (current === 'bearer' ? current : 'oauth'))
  }, [existingProtectedRoute, gateway, open])

  useEffect(() => {
    setServiceValues({})
  }, [selectedService])

  useEffect(() => {
    if (skipUrlOauthResetRef.current) {
      skipUrlOauthResetRef.current = false
      return
    }
    autoOauthAttemptedForRef.current = null
    setOauthState({ kind: 'idle' })
    setOauthProbed(null)
  }, [url])

  // Stdio connections don't support upstream authentication, so clear all OAuth
  // and bearer state whenever the transport is stdio. If a protected public path
  // is also absent we additionally reset the auth mode to 'none'.
  useEffect(() => {
    if (transport !== 'stdio') return
    if (!protectedPublicPath.trim()) {
      setAuthMode('none')
    }
    setOauthState({ kind: 'idle' })
    setOauthProbed(null)
    setBearerTokenEnv('')
    setBearerTokenValue('')
  }, [protectedPublicPath, transport])

  useEffect(() => {
    if (!serviceMeta || !serviceConfig) return

    const nextValues: Record<string, string> = {}
    for (const field of serviceEnvFields) {
      const configField = serviceConfig.fields.find((item) => item.name === field.name)
      nextValues[field.name] = valuePreview(field.name, configField?.value_preview)
    }
    setServiceValues(nextValues)
  }, [serviceConfig, serviceEnvFields, serviceMeta])

  const validateCustom = () => {
    const newErrors: Record<string, string> = {}

    const nameError = validateGatewayName(name.trim())
    if (nameError) {
      newErrors.name = nameError
    }

    if (transport === 'http') {
      if (!url.trim()) {
        newErrors.url = 'URL is required'
      } else {
        try {
          new URL(url)
        } catch {
          newErrors.url = 'Invalid URL format'
        }
      }

    } else {
      try {
        parseStdioCommandLine(command)
      } catch (error) {
        newErrors.command = error instanceof Error ? error.message : 'Invalid command'
      }
    }

    if (protectedPublicPath.trim()) {
      try {
        normalizeProtectedPublicPath(protectedPublicPath)
      } catch (error) {
        newErrors.protectedPublicPath = error instanceof Error
          ? error.message
          : 'Invalid protected route path'
      }
    }

    if (transport === 'http' && authMode === 'oauth' && !protectedPublicPath.trim()) {
      if (oauthState.kind !== 'connected' && !oauthStatus?.authenticated) {
        newErrors.oauth = 'Complete OAuth authorization before saving'
      }
    }

    if (transport === 'http' && authMode === 'bearer') {
      if (authSource === 'env') {
        if (!bearerTokenEnv.trim()) {
          newErrors.bearerTokenEnv = 'Environment variable name is required'
        } else {
          const bearerTokenEnvError = validateBearerTokenEnvName(bearerTokenEnv)
          if (bearerTokenEnvError) {
            newErrors.bearerTokenEnv = bearerTokenEnvError
          }
        }
      } else {
        if (!bearerTokenValue.trim()) {
          newErrors.bearerTokenValue = 'Bearer token is required'
        }

        if (bearerTokenEnv.trim()) {
          const bearerTokenEnvError = validateBearerTokenEnvName(bearerTokenEnv)
          if (bearerTokenEnvError) {
            newErrors.bearerTokenEnv = bearerTokenEnvError
          }
        }
      }
    }

    setErrors(newErrors)
    return Object.keys(newErrors).length === 0
  }

  const validateLab = () => {
    const newErrors: Record<string, string> = {}
    if (!selectedService) {
      newErrors.service = 'Choose a Lab service'
    }
    for (const field of serviceMeta?.required_env ?? []) {
      const configField = serviceConfig?.fields.find((item) => item.name === field.name)
      const keepExistingSecret = field.secret && configField?.present && !serviceValues[field.name]?.trim()
      if (!keepExistingSecret && !serviceValues[field.name]?.trim()) {
        newErrors[field.name] = `${field.name} is required`
      }
    }
    setErrors(newErrors)
    return Object.keys(newErrors).length === 0
  }

  const buildInput = (): CreateGatewayInput => {
    const stdio = transport === 'stdio' ? parseStdioCommandLine(command) : null
    const authEnabled = transport === 'http'
    return {
      name,
      transport,
      config: {
        ...(transport === 'http'
          ? { url }
          : {
              command: stdio?.command,
              args: stdio && stdio.args.length > 0 ? stdio.args : undefined,
            }),
        bearer_token_env: !authEnabled
          ? undefined
          : authMode === 'none' || authMode === 'oauth'
            ? ''
            : authSource === 'env'
              ? bearerTokenEnv
              : bearerTokenEnv || undefined,
        bearer_token_value:
          authEnabled && authMode === 'bearer' && authSource === 'paste'
            ? bearerTokenValue
            : undefined,
        oauth:
          authEnabled && authMode === 'oauth' && oauthState.kind === 'connected' && oauthState.registration_strategy !== 'unknown'
            ? { registration_strategy: oauthState.registration_strategy, scopes: oauthState.scopes }
            : undefined,
        proxy_resources: proxyResources,
        proxy_prompts: proxyPrompts,
      },
    }
  }

  const buildProtectedRouteInput = (publicPath: string): ProtectedMcpRouteInput => ({
    name: name.trim(),
    enabled: true,
    public_host: PROTECTED_MCP_PUBLIC_HOST,
    public_path: publicPath,
    upstream: name.trim(),
    backend_url: '',
    scopes: PROTECTED_ROUTE_SCOPES,
    health_path: null,
  })

  const saveProtectedRoute = async (publicPath: string, signal?: AbortSignal): Promise<void> => {
    const route = buildProtectedRouteInput(publicPath)
    const existingPathRoute = protectedRoutes.find(
      (item) =>
        item.enabled &&
        item.public_host === route.public_host &&
        item.public_path === route.public_path,
    )
    if (
      existingPathRoute &&
      existingPathRoute.name !== route.name &&
      existingPathRoute.name !== existingProtectedRoute?.name
    ) {
      throw new GatewayApiError(
        `Protected route ${route.public_path} is already assigned to ${existingPathRoute.upstream ?? existingPathRoute.name}. Choose a different path or edit that route first.`,
        409,
      )
    }

    if (existingProtectedRoute) {
      await updateProtectedRoute(existingProtectedRoute.name, {
        ...route,
        name: existingProtectedRoute.name,
      }, signal)
      return
    }

    try {
      await addProtectedRoute(route, signal)
    } catch (error) {
      if (error instanceof GatewayApiError && error.status === 409) {
        await updateProtectedRoute(route.name, route, signal)
        return
      }
      throw error
    }
  }

  const removeExistingProtectedRouteIfCleared = async (
    publicPath: string,
    signal?: AbortSignal,
  ): Promise<void> => {
    if (!existingProtectedRoute) return
    // Only remove when the protected-route field was explicitly cleared.
    // If publicPath is non-empty, saveProtectedRoute already handled the
    // update/replace; deleting here would silently discard the just-saved route.
    if (publicPath) return
    await removeProtectedRoute(existingProtectedRoute.name, signal)
  }

  const handleTest = async () => {
    if (isSaving) return
    if (!gateway || gateway.source === 'in_process') {
      toast.info('Save and enable the server first, then test from the detail page.')
      return
    }

    if (!validateCustom()) return

    const controller = new AbortController()
    abortControllerRef.current = controller

    setIsTesting(true)
    try {
      const result = await testGateway(gateway.id)
      if (controller.signal.aborted) return
      if (result.severity === 'warning') {
        toast.warning(result.detail || result.message)
      } else if (result.success) {
        toast.success(`Connection successful: ${result.latency_ms}ms latency`)
      } else {
        toast.error(`Connection failed: ${result.error || result.message}`)
      }
    } catch (error) {
      if (isAbortError(error)) return
      toast.error(getErrorMessage(error, 'Failed to test connection'))
    } finally {
      setIsTesting(false)
    }
  }

  const handleSaveLab = async (): Promise<boolean> => {
    if (!validateLab() || !selectedService) return false

    const values = Object.fromEntries(
      Object.entries(serviceValues).filter(([field, value]) => {
        const configField = serviceConfig?.fields.find((item) => item.name === field)
        if (configField?.secret && configField.present && !value.trim()) {
          return false
        }
        return true
      }),
    )

    await saveServiceConfig(selectedService, values)
    if (enableServer) {
      await enableVirtualServer(selectedService)
    } else {
      await disableVirtualServer(selectedService)
    }
    return true
  }

  const handleSave = async () => {
    if (isTesting) return

    const controller = new AbortController()
    abortControllerRef.current = controller

    setIsSaving(true)
    try {
      if (mode === 'lab') {
        const saved = await handleSaveLab()
        if (controller.signal.aborted) return
        if (!saved) {
          return
        }
        toast.success(isEditing ? 'Lab server updated successfully' : 'Lab server configured successfully')
        onOpenChange(false)
        return
      }

      if (!validateCustom()) return
      setSaveError(null)
      const normalizedProtectedPath = normalizeProtectedPublicPath(protectedPublicPath)
      await onSave(buildInput())
      if (normalizedProtectedPath) {
        await saveProtectedRoute(normalizedProtectedPath, controller.signal)
      } else {
        await removeExistingProtectedRouteIfCleared(normalizedProtectedPath, controller.signal)
      }
      await removeExistingProtectedRouteIfCleared(normalizedProtectedPath, controller.signal)
      if (controller.signal.aborted) return
      toast.success(
        normalizedProtectedPath
          ? reusedProtectedRoute
            ? `Server saved and joined https://${PROTECTED_MCP_PUBLIC_HOST}${normalizedProtectedPath}`
            : `Server saved and protected at https://${PROTECTED_MCP_PUBLIC_HOST}${normalizedProtectedPath}`
          : isEditing
            ? 'Server updated successfully'
            : 'Server created successfully',
      )
      onOpenChange(false)
    } catch (error) {
      if (isAbortError(error)) return
      if (error instanceof GatewayApiError && error.status === 409) {
        setSaveError(error.message)
        return
      }
      toast.error(
        getErrorMessage(
          error,
          mode === 'lab'
            ? 'Failed to save Lab server'
            : isEditing
              ? 'Failed to update server'
              : 'Failed to create server',
        ),
      )
    } finally {
      setIsSaving(false)
    }
  }

  const toggleEnvDrawer = () => {
    const next = !envDrawerOpen
    setEnvDrawerOpen(next)
    if (next) setJsonDrawerOpen(false)
  }

  const toggleJsonDrawer = () => {
    const next = !jsonDrawerOpen
    setJsonDrawerOpen(next)
    if (next) setEnvDrawerOpen(false)
  }

  const applyEnvToForm = () => {
    const { pairs, detectedServices } = parseEnvText(envText)
    const detected = detectedServices[0]
    if (!detected) return
    const prefix = Object.entries(SERVICE_ENV_PREFIXES).find(([, key]) => key === detected)?.[0]
    if (!prefix) return
    setMode('custom')
    setTransport('http')
    setName(detected)
    const urlKey = `${prefix}_URL`
    if (pairs[urlKey]) setUrl(pairs[urlKey])
    setEnvDrawerOpen(false)
  }

  const buildJsonFromForm = (): object | null => {
    const n = name.trim()
    if (!n) return null
    const cfg: Record<string, unknown> = {}
    if (transport === 'http') {
      const u = url.trim()
      if (u) cfg.url = u
    } else {
      const trimmed = command.trim()
      if (trimmed) {
        try {
          const parsed = parseStdioCommandLine(trimmed)
          cfg.command = parsed.command
          if (parsed.args.length > 0) cfg.args = parsed.args
        } catch {
          cfg.command = trimmed
        }
      }
    }
    return { [n]: cfg }
  }

  const onFormChange = () => {
    if (syncingRef.current || !jsonDrawerOpen) return
    syncingRef.current = true
    const json = buildJsonFromForm()
    if (json) {
      setJsonText(JSON.stringify(json, null, 2))
      setJsonValid(true)
    } else {
      setJsonText('')
      setJsonValid(false)
    }
    // Defer reset so it runs AFTER React flushes batched state — otherwise the
    // useEffect watching [name, url, ...] fires with guard already false and loops.
    setTimeout(() => { syncingRef.current = false }, 0)
  }

  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => { onFormChange() }, [name, url, command, transport, jsonDrawerOpen])

  const parseJsonToForm = (text: string) => {
    if (syncingRef.current) return
    try {
      const parsed = JSON.parse(text) as Record<string, unknown>
      const keys = Object.keys(parsed)
      if (keys.length !== 1) {
        setJsonValid(false)
        return
      }
      const gatewayName = keys[0]!
      const cfg = parsed[gatewayName] as Record<string, unknown>
      setJsonValid(true)
      syncingRef.current = true
      setName(gatewayName)
      if (typeof cfg.url === 'string') {
        setTransport('http')
        setUrl(cfg.url)
      } else if (typeof cfg.command === 'string') {
        setTransport('stdio')
        setCommand(formatStdioCommandLine(cfg.command, Array.isArray(cfg.args) ? cfg.args as string[] : []))
      }
      // Defer reset — same reason as onFormChange: the useEffect fires after React
      // flushes the setName/setUrl/setTransport calls; guard must still be true then.
      setTimeout(() => { syncingRef.current = false }, 0)
    } catch {
      setJsonValid(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={(nextOpen) => {
      if (!nextOpen) {
        abortControllerRef.current?.abort()
      }
      onOpenChange(nextOpen)
    }}>
        <DialogContent
          className={cn(
            'overflow-visible transition-[border-radius] duration-[250ms]',
            'sm:max-w-[540px]',
            (envDrawerOpen || jsonDrawerOpen) && 'rounded-r-none',
          )}
        >
        <DialogHeader className="shrink-0">
          <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
            <div className="flex min-w-0 flex-col gap-1">
              <DialogTitle>{isEditing ? 'Edit Server' : 'Add Server'}</DialogTitle>
              <DialogDescription>
                {isEditing
                  ? 'Edit server settings.'
                  : mode === 'lab'
                    ? 'Connect a built-in Lab service.'
                    : 'Connect an upstream MCP server.'}
              </DialogDescription>
            </div>
            <div
              className={cn(
                'flex shrink-0 items-center gap-1.5 sm:mr-8',
                mode === 'custom' ? 'visible' : 'invisible pointer-events-none',
              )}
            >
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={toggleEnvDrawer}
                className={cn(
                  'h-8 rounded-full px-3 text-xs font-medium',
                  envDrawerOpen
                    ? 'border-aurora-accent-primary/36 bg-aurora-accent-primary/12 text-aurora-text-primary'
                    : 'border-aurora-border-strong bg-aurora-control-surface text-aurora-text-primary hover:bg-aurora-hover-bg',
                )}
              >
                ENV
              </Button>
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={toggleJsonDrawer}
                className={cn(
                  'h-8 rounded-full px-3 text-xs font-medium',
                  jsonDrawerOpen
                    ? 'border-aurora-accent-primary/36 bg-aurora-accent-primary/12 text-aurora-text-primary'
                    : 'border-aurora-border-strong bg-aurora-control-surface text-aurora-text-primary hover:bg-aurora-hover-bg',
                )}
              >
                JSON
              </Button>
            </div>
          </div>
        </DialogHeader>

        <div className="flex-1 min-h-0 overflow-y-auto aurora-scrollbar -mx-6 px-6">
        <Tabs
          value={mode}
          onValueChange={(value) => {
            setMode(value as FormMode)
            setEnvDrawerOpen(false)
            setJsonDrawerOpen(false)
          }}
          className="space-y-6"
        >
          <TabsList className="grid w-full grid-cols-2 overflow-hidden">
            <TabsTrigger value="lab" disabled={isEditing && !isLabGateway}>
              Lab Service
            </TabsTrigger>
            <TabsTrigger value="custom" disabled={isEditing && isLabGateway}>
              Custom
            </TabsTrigger>
          </TabsList>

          <TabsContent value="lab" className="space-y-6">
            <FieldGroup>
              <Field>
                <div
                  className="grid grid-cols-3 sm:grid-cols-4 gap-2 overflow-y-auto aurora-scrollbar pr-1"
                  style={{ maxHeight: 320 }}
                >
                  {(supportedServices ?? []).map((svc) => (
                    <button
                      key={svc.key}
                      type="button"
                      onClick={() => setSelectedService(svc.key)}
                      className={cn(
                        'flex flex-col items-center gap-1.5 rounded-aurora-2 border p-2 text-center transition-colors hover:border-primary/60 hover:bg-accent/30',
                        selectedService === svc.key
                          ? 'border-primary bg-primary/10'
                          : 'border-aurora-border-strong bg-aurora-page-bg',
                      )}
                    >
                      <ServiceIconBox serviceKey={svc.key} />
                      <div className="min-w-0 w-full">
                        <p className="text-xs font-medium leading-tight truncate">{svc.display_name}</p>
                        <p className="text-[10px] text-aurora-text-muted truncate">{svc.category}</p>
                      </div>
                    </button>
                  ))}
                </div>
                {errors.service && <p className="text-sm text-destructive">{errors.service}</p>}
              </Field>
            </FieldGroup>

            {serviceMeta && (
              <FieldGroup>
                {serviceEnvFields.map((field) => {
                  const configField = serviceConfig?.fields.find((item) => item.name === field.name)
                  const hasStoredSecret = field.secret && configField?.present

                  return (
                    <Field key={field.name}>
                    <FieldLabel htmlFor={field.name}>{field.name}</FieldLabel>
                    <Input
                      id={field.name}
                      type={field.secret ? 'password' : 'text'}
                      value={serviceValues[field.name] ?? ''}
                      onChange={(event) =>
                        setServiceValues((current) => ({
                          ...current,
                          [field.name]: event.target.value,
                        }))
                      }
                      placeholder={hasStoredSecret ? 'Leave blank to keep current value' : field.example}
                      className={errors[field.name] ? 'border-destructive' : ''}
                    />
                    {errors[field.name] ? (
                      <p className="text-sm text-destructive">{errors[field.name]}</p>
                    ) : (
                      <FieldDescription>
                        {field.description}
                        {hasStoredSecret ? ' Current secret is already configured.' : ''}
                      </FieldDescription>
                    )}
                    </Field>
                  )
                })}
              </FieldGroup>
            )}

            <div className="flex items-center justify-between rounded-lg border p-4">
              <div className="space-y-0.5">
                <Label htmlFor="enable-virtual-server" className="font-medium">
                  Enable server
                </Label>
                <p className="text-sm text-aurora-text-muted">
                  Save canonical service config and expose this Lab service as a visible server.
                </p>
              </div>
              <Switch
                id="enable-virtual-server"
                checked={enableServer}
                onCheckedChange={setEnableServer}
              />
            </div>
          </TabsContent>

          <TabsContent value="custom" className="space-y-6">
            <FieldGroup>
              <Field>
                <FieldLabel htmlFor="name">Name</FieldLabel>
                <Input
                  id="name"
                  value={name}
                  onChange={(event) => { nameAutoRef.current = false; setName(event.target.value) }}
                  placeholder="my-gateway"
                  className={errors.name ? 'border-destructive' : ''}
                />
                {errors.name ? (
                  <p className="text-sm text-destructive">{errors.name}</p>
                ) : (
                  <FieldDescription>
                    Letters, digits, underscores, hyphens — starts with a letter or digit
                  </FieldDescription>
                )}
              </Field>
            </FieldGroup>

            <RadioGroup
              value={transport}
              onValueChange={(value) => setTransport(value as TransportType)}
              className="grid grid-cols-1 sm:grid-cols-2 gap-3"
            >
              <label className="flex items-start gap-3 rounded-aurora-2 border p-4 cursor-pointer" htmlFor="transport-http">
                <RadioGroupItem value="http" id="transport-http" />
                <div className="space-y-0.5">
                  <span className="font-medium text-sm">HTTP</span>
                  <p className="text-sm text-aurora-text-muted">Remote server via HTTP or SSE</p>
                </div>
              </label>
              <label className="flex items-start gap-3 rounded-aurora-2 border p-4 cursor-pointer" htmlFor="transport-stdio">
                <RadioGroupItem value="stdio" id="transport-stdio" />
                <div className="space-y-0.5">
                  <span className="font-medium text-sm">stdio</span>
                  <p className="text-sm text-aurora-text-muted">Local process via stdin/stdout</p>
                </div>
              </label>
            </RadioGroup>

            {transport === 'http' && (
              <FieldGroup>
                <Field>
                  <FieldLabel htmlFor="url">URL</FieldLabel>
                  <div className="relative">
                    <Input
                      id="url"
                      value={url}
                      onChange={(event) => setUrl(event.target.value)}
                      placeholder="http://localhost:3001/mcp"
                      className={`${errors.url ? 'border-destructive' : ''} pr-8`}
                    />
                    {isProbing && (
                      <Loader2 className="absolute right-2.5 top-1/2 -translate-y-1/2 size-4 text-aurora-text-muted animate-spin pointer-events-none" />
                    )}
                    {!isProbing && oauthProbed?.oauth_discovered && (
                      <CheckCircle2 className="absolute right-2.5 top-1/2 -translate-y-1/2 size-4 text-aurora-success pointer-events-none" />
                    )}
                  </div>
                  {errors.url && <p className="text-sm text-destructive">{errors.url}</p>}
                </Field>
              </FieldGroup>
            )}

            {transport === 'stdio' && (
              <FieldGroup>
                <Field>
                  <FieldLabel htmlFor="command">Command line</FieldLabel>
                  <Input
                    id="command"
                    value={command}
                    onChange={(event) => setCommand(event.target.value)}
                    placeholder="npx -y @modelcontextprotocol/server-filesystem /path"
                    className={errors.command ? 'border-destructive' : ''}
                  />
                  {errors.command && <p className="text-sm text-destructive">{errors.command}</p>}
                  <FieldDescription>
                    Enter the full stdio launch command. Quoted arguments with spaces are preserved.
                  </FieldDescription>
                </Field>
              </FieldGroup>
            )}

            <FieldGroup>
              <Field>
                <FieldLabel htmlFor="protected-public-path">Protected route path</FieldLabel>
                <div className="flex overflow-hidden rounded-md border border-aurora-border-strong bg-aurora-control-surface focus-within:ring-2 focus-within:ring-ring">
                  <span className="hidden items-center border-r border-aurora-border-strong px-3 text-sm text-aurora-text-muted sm:flex">
                    https://{PROTECTED_MCP_PUBLIC_HOST}/
                  </span>
                  <Input
                    id="protected-public-path"
                    list="protected-route-path-options"
                    value={protectedPublicPath}
                    onChange={(event) => {
                      protectedRouteTouchedRef.current = true
                      if (!event.target.value.trim() && existingProtectedRoute && authMode === 'oauth') {
                        setAuthMode('none')
                        setOauthState({ kind: 'idle' })
                      }
                      setProtectedPublicPath(event.target.value)
                    }}
                    placeholder="tools"
                    className={cn(
                      'border-0 bg-transparent focus-visible:ring-0 focus-visible:ring-offset-0',
                      errors.protectedPublicPath && 'text-destructive',
                    )}
                  />
                  <datalist id="protected-route-path-options">
                    {protectedRoutePathOptions.map((path) => (
                      <option key={path} value={path.replace(/^\//, '')}>
                        {`https://${PROTECTED_MCP_PUBLIC_HOST}${path}`}
                      </option>
                    ))}
                  </datalist>
                </div>
                {errors.protectedPublicPath ? (
                  <p className="text-sm text-destructive">{errors.protectedPublicPath}</p>
                ) : (
                  <FieldDescription>
                    Optional. When set, Lab publishes this server through Google OAuth at that public path.
                  </FieldDescription>
                )}
              </Field>
            </FieldGroup>

            {transport === 'http' && (
            <FieldGroup>
              <Field className="space-y-4">
                <div className="space-y-1">
                  <FieldLabel>Authentication</FieldLabel>
                  <FieldDescription>
                    Choose how this server should authenticate upstream requests.
                  </FieldDescription>
                </div>

                <Select value={authMode} onValueChange={(value) => setAuthMode(value as GatewayAuthMode)}>
                  <SelectTrigger className="w-full">
                    <SelectValue>
                      <span className="flex items-center gap-2">
                        {authMode === 'none' && <ShieldOff className="size-4 text-aurora-text-muted" />}
                        {authMode === 'bearer' && <KeyRound className="size-4 text-aurora-text-muted" />}
                        {authMode === 'oauth' && <ShieldCheck className="size-4 text-aurora-text-muted" />}
                        {authMode === 'none' ? 'No auth' : authMode === 'bearer' ? 'Bearer token' : 'OAuth (MCP)'}
                        {authMode === 'oauth' && oauthProbed?.oauth_discovered && (
                          <Badge
                            variant="secondary"
                            className="ml-1 border-aurora-border-strong bg-aurora-control-surface text-xs text-aurora-text-primary"
                          >
                            Detected
                          </Badge>
                        )}
                      </span>
                    </SelectValue>
                  </SelectTrigger>
                  <SelectContent style={{ zIndex: 200 }}>
                    <SelectItem value="none">
                      <span className="flex items-center gap-2">
                        <ShieldOff className="size-4 text-aurora-text-muted" />
                        No auth
                      </span>
                    </SelectItem>
                    <SelectItem value="bearer">
                      <span className="flex items-center gap-2">
                        <KeyRound className="size-4 text-aurora-text-muted" />
                        Bearer token
                      </span>
                    </SelectItem>
                    <SelectItem value="oauth">
                      <span className="flex items-center gap-2">
                        <ShieldCheck className="size-4 text-aurora-text-muted" />
                        OAuth (MCP)
                      </span>
                    </SelectItem>
                  </SelectContent>
                </Select>

                {authMode === 'oauth' && (
                  <div className="rounded-lg border p-4 flex flex-col gap-3">
                    {oauthState.kind === 'connected' ? (
                      <div className="flex items-center justify-between gap-2">
                        <div className="flex items-center gap-2 text-sm text-aurora-success font-medium">
                          <ShieldCheck className="size-4" />
                          Connected
                          <Badge variant="outline" className="border-aurora-success/40 text-aurora-success ml-1">Authorized</Badge>
                        </div>
                        <Button
                          type="button"
                          variant="ghost"
                          size="sm"
                          onClick={() => {
                            setOauthState({ kind: 'idle' })
                            probeInfoRef.current = null
                          }}
                        >
                          Re-authorize
                        </Button>
                      </div>
                    ) : (
                      <>
                        <p className="text-sm text-aurora-text-muted">
                          {!url.trim()
                            ? 'Enter a URL above, then connect.'
                            : oauthState.kind === 'authorizing'
                              ? 'Complete authorization in the new tab.'
                              : oauthState.kind === 'blocked'
                                ? 'OAuth detected. Click to authorize; the browser blocked the automatic popup.'
                                : 'Connect this server via OAuth. A popup will open for you to authorize.'}
                        </p>
                        {oauthState.kind === 'error' && (
                          <div className="flex items-start gap-2 text-sm text-destructive">
                            <AlertCircle className="size-4 mt-0.5 shrink-0" />
                            {oauthState.message}
                          </div>
                        )}
                        <Button
                          type="button"
                          size="sm"
                          variant={oauthState.kind === 'blocked' ? 'default' : 'secondary'}
                          onClick={() => void handleOauthConnect()}
                          disabled={!url.trim() || oauthState.kind === 'probing' || oauthState.kind === 'authorizing'}
                          className={cn(
                            oauthState.kind === 'blocked'
                              && 'ring-2 ring-aurora-accent-primary/45 ring-offset-2 ring-offset-aurora-page-bg',
                          )}
                        >
                          {(oauthState.kind === 'probing' || oauthState.kind === 'authorizing') && (
                            <Loader2 className="size-4 mr-2 animate-spin" />
                          )}
                          {oauthConnectButtonLabel(oauthState)}
                        </Button>
                      </>
                    )}
                  </div>
                )}
                {errors.oauth && <p className="text-sm text-destructive">{errors.oauth}</p>}

                {authMode === 'bearer' && (
                  <div className="space-y-4 rounded-aurora-2 border p-4">
                    <RadioGroup value={authSource} onValueChange={(value) => setAuthSource(value as GatewayAuthSource)}>
                      <label className="flex items-start gap-3 rounded-lg border p-3 cursor-pointer" htmlFor="auth-source-paste">
                        <RadioGroupItem value="paste" id="auth-source-paste" />
                        <div className="space-y-1">
                          <span className="font-medium text-sm">Paste token</span>
                          <p className="text-sm text-aurora-text-muted">
                            Paste the secret here and Labby will store it in <code>~/.lab/.env</code> for you.
                          </p>
                        </div>
                      </label>
                      <label className="flex items-start gap-3 rounded-lg border p-3 cursor-pointer" htmlFor="auth-source-env">
                        <RadioGroupItem value="env" id="auth-source-env" />
                        <div className="space-y-1">
                          <span className="font-medium text-sm">Use existing env var</span>
                          <p className="text-sm text-aurora-text-muted">
                            Reference an existing environment variable instead of entering a secret here.
                          </p>
                        </div>
                      </label>
                    </RadioGroup>

                    {authSource === 'paste' ? (
                      <FieldGroup>
                        <Field>
                          <FieldLabel htmlFor="bearer-token-value">Bearer token</FieldLabel>
                          <Input
                            id="bearer-token-value"
                            type="password"
                            autoComplete="new-password"
                            value={bearerTokenValue}
                            onChange={(event) => setBearerTokenValue(event.target.value)}
                            placeholder="ghp_..."
                            className={errors.bearerTokenValue ? 'border-destructive' : ''}
                          />
                          {errors.bearerTokenValue ? (
                            <p className="text-sm text-destructive">{errors.bearerTokenValue}</p>
                          ) : (
                            <FieldDescription>
                              Paste the token only. Labby will add the <code>Bearer</code> prefix automatically if needed.
                            </FieldDescription>
                          )}
                        </Field>
                        <details className="group">
                          <summary className="flex cursor-pointer select-none list-none items-center gap-1 text-sm text-aurora-text-muted [&::-webkit-details-marker]:hidden">
                            <ChevronRight className="size-3 transition-transform group-open:rotate-90" />
                            Advanced
                          </summary>
                          <div className="mt-3">
                            <Field>
                              <FieldLabel htmlFor="bearer-token-env-override">Env var name</FieldLabel>
                              <Input
                                id="bearer-token-env-override"
                                value={bearerTokenEnv}
                                onChange={(event) => setBearerTokenEnv(event.target.value)}
                                placeholder={defaultGatewayBearerEnvName(name || 'gateway')}
                                className={errors.bearerTokenEnv ? 'border-destructive' : ''}
                              />
                              {errors.bearerTokenEnv ? (
                                <p className="text-sm text-destructive">{errors.bearerTokenEnv}</p>
                              ) : (
                                <FieldDescription>
                                  Optional. Leave blank to let Labby generate an env var name automatically.
                                </FieldDescription>
                              )}
                            </Field>
                          </div>
                        </details>
                      </FieldGroup>
                    ) : (
                      <Field>
                        <FieldLabel htmlFor="bearer-token-env">Bearer token env var</FieldLabel>
                        <Input
                          id="bearer-token-env"
                          value={bearerTokenEnv}
                          onChange={(event) => setBearerTokenEnv(event.target.value)}
                          placeholder={defaultGatewayBearerEnvName(name || 'gateway')}
                          className={errors.bearerTokenEnv ? 'border-destructive' : ''}
                        />
                        {errors.bearerTokenEnv ? (
                          <p className="text-sm text-destructive">{errors.bearerTokenEnv}</p>
                        ) : (
                          <FieldDescription>
                            Enter the env var name only. The env var value can be a bare token or a full <code>Bearer ...</code> header.
                          </FieldDescription>
                        )}
                      </Field>
                    )}
                  </div>
                )}
              </Field>
            </FieldGroup>
            )}

            <details className="group rounded-lg border p-4">
              <summary className="flex cursor-pointer select-none list-none items-center gap-2 text-sm font-medium [&::-webkit-details-marker]:hidden">
                <ChevronRight className="size-4 transition-transform group-open:rotate-90" />
                Advanced
              </summary>
              <div className="mt-4 space-y-4">
                <div className="flex items-center justify-between gap-4">
                  <div className="space-y-0.5">
                    <Label htmlFor="proxy-resources" className="font-medium">
                      Proxy Resources
                    </Label>
                    <p className="text-sm text-aurora-text-muted">
                      Forward MCP resource requests to this server
                    </p>
                  </div>
                  <Switch
                    id="proxy-resources"
                    checked={proxyResources}
                    onCheckedChange={setProxyResources}
                  />
                </div>

                <div className="flex items-center justify-between gap-4">
                  <div className="space-y-0.5">
                    <Label htmlFor="proxy-prompts" className="font-medium">
                      Proxy Prompts
                    </Label>
                    <p className="text-sm text-aurora-text-muted">
                      Forward MCP prompt requests to this server
                    </p>
                  </div>
                  <Switch
                    id="proxy-prompts"
                    checked={proxyPrompts}
                    onCheckedChange={setProxyPrompts}
                  />
                </div>
              </div>
            </details>
          </TabsContent>
        </Tabs>
        </div>

        {/* ENV drawer */}
        <div
          className={cn(
            'absolute top-0 bottom-0 bg-aurora-page-bg border-l border-aurora-border-strong rounded-r-lg overflow-hidden transition-[width] duration-[250ms] ease-[cubic-bezier(.4,0,.2,1)] flex flex-col sm:left-full',
            'max-[600px]:fixed max-[600px]:inset-0 max-[600px]:rounded-none max-[600px]:border-l-0 max-[600px]:z-50',
            envDrawerOpen
              ? 'w-[300px] max-[600px]:w-full max-[600px]:h-full'
              : 'w-0',
          )}
          aria-hidden={!envDrawerOpen}
        >
          <div className="flex flex-col gap-3 p-4 flex-1 overflow-y-auto aurora-scrollbar">
            <p className="text-xs text-aurora-text-muted">
              Paste <code>KEY=VALUE</code> lines — Lab detects the service and can pre-fill the form.
            </p>
            <div className="relative">
              <textarea
                className="w-full min-h-[180px] rounded-md border border-aurora-border-strong bg-aurora-page-bg px-3 py-2 text-xs font-mono resize-none focus:outline-none focus:ring-2 focus:ring-ring"
                placeholder={'RADARR_URL=http://localhost:7878\nRADARR_API_KEY=abc123'}
                value={envText}
                onChange={(e) => setEnvText(e.target.value)}
              />
              {(() => {
                if (!envText.trim()) {
                  return <span className="absolute top-2 right-2 text-[10px] text-aurora-text-muted">Waiting</span>
                }
                const { detectedServices } = parseEnvText(envText)
                if (detectedServices.length > 0) {
                  return (
                    <span className="absolute top-2 right-2 text-[10px] text-aurora-success">
                      Valid · {detectedServices.length} service{detectedServices.length > 1 ? 's' : ''}
                    </span>
                  )
                }
                return <span className="absolute top-2 right-2 text-[10px] text-aurora-warn">No known service</span>
              })()}
            </div>
            {envText.trim() && (() => {
              const { detectedServices } = parseEnvText(envText)
              if (detectedServices.length === 0) return null
              return (
                <div className="flex flex-wrap gap-1.5">
                  {detectedServices.map((s) => (
                    <span
                      key={s}
                      className="rounded-full border border-aurora-accent-primary/30 bg-aurora-accent-primary/10 px-2 py-0.5 text-xs text-aurora-accent-primary"
                    >
                      {s}
                    </span>
                  ))}
                </div>
              )
            })()}
          </div>
          <div className="flex gap-2 border-t border-aurora-border-strong p-3">
            <button
              type="button"
              className="flex-1 rounded-md border border-aurora-border-strong px-3 py-1.5 text-xs hover:bg-accent transition-colors"
              onClick={async () => {
                try {
                  const text = await navigator.clipboard.readText()
                  setEnvText(text)
                } catch {
                  // clipboard access denied — user must paste manually
                }
              }}
            >
              Paste
            </button>
            <button
              type="button"
              className="flex-1 rounded-md bg-primary text-primary-foreground px-3 py-1.5 text-xs hover:bg-primary/90 transition-colors disabled:opacity-50"
              disabled={!parseEnvText(envText).detectedServices.length}
              onClick={applyEnvToForm}
            >
              Apply to form
            </button>
          </div>
        </div>

        {/* JSON drawer */}
        <div
          className={cn(
            'absolute top-0 bottom-0 bg-aurora-page-bg border-l border-aurora-border-strong rounded-r-lg overflow-hidden transition-[width] duration-[250ms] ease-[cubic-bezier(.4,0,.2,1)] flex flex-col sm:left-full',
            'max-[600px]:fixed max-[600px]:inset-0 max-[600px]:rounded-none max-[600px]:border-l-0 max-[600px]:z-50',
            jsonDrawerOpen
              ? 'w-[380px] max-[600px]:w-full max-[600px]:h-full'
              : 'w-0',
          )}
          aria-hidden={!jsonDrawerOpen}
        >
          <div className="flex flex-col gap-3 p-4 flex-1 overflow-y-auto aurora-scrollbar">
            <p className="text-xs text-aurora-text-muted">
              Live editor — changes here update the form, and form changes update this JSON automatically.
            </p>
            <div className="min-h-[240px]">
              <TextSurface
                path="gateway-config.json"
                value={jsonText}
                mode="edit"
                language="json"
                onChange={(next) => {
                  setJsonText(next)
                  parseJsonToForm(next)
                }}
                onCopy={() => {
                  void navigator.clipboard.writeText(jsonText)
                }}
              />
            </div>
            {jsonValid && name && (
              <div className="flex flex-wrap gap-1.5">
                <span className="rounded-full border border-aurora-accent-primary/30 bg-aurora-accent-primary/10 px-2 py-0.5 text-xs text-aurora-accent-primary">
                  {name}
                </span>
                <span className="rounded-full bg-aurora-control-surface border border-aurora-border-strong px-2 py-0.5 text-xs text-aurora-text-muted">
                  {transport}
                </span>
              </div>
            )}
          </div>
          <div className="flex gap-2 border-t border-aurora-border-strong p-3">
            <button
              type="button"
              className="flex-1 rounded-md border border-aurora-border-strong px-3 py-1.5 text-xs hover:bg-accent transition-colors"
              onClick={async () => {
                try {
                  const text = await navigator.clipboard.readText()
                  setJsonText(text)
                  parseJsonToForm(text)
                } catch {
                  // clipboard access denied
                }
              }}
            >
              Paste
            </button>
          </div>
        </div>

        {saveError && (
          <div className="shrink-0 flex items-start gap-2 rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
            <AlertCircle className="size-4 mt-0.5 shrink-0" />
            <span>{saveError}</span>
          </div>
        )}
        <DialogFooter className="shrink-0 gap-2 sm:gap-0">
          {isEditing && !isLabGateway && (
            <Button
              type="button"
              variant="outline"
              onClick={handleTest}
              disabled={isTesting || isSaving}
              className="mr-auto"
            >
              {isTesting ? (
                <Loader2 className="size-4 mr-2 animate-spin" />
              ) : (
                <Play className="size-4 mr-2" />
              )}
              Test
            </Button>
          )}
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button onClick={handleSave} disabled={isSaving || isTesting}>
            {isSaving && <Loader2 className="size-4 mr-2 animate-spin" />}
            {mode === 'lab'
              ? isEditing
                ? 'Save Service'
                : 'Configure Service'
              : isEditing
                ? 'Save Changes'
                : 'Add Server'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
