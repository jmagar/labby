'use client'

import useSWR, { mutate } from 'swr'
import { gatewayApi } from '@/lib/api/gateway-client'
import {
  getMockGatewayFallback,
  getMockGatewaysFallback,
  getMockServiceActionsFallback,
  getMockServiceConfigFallback,
  getMockSupportedServicesFallback,
} from '@/lib/api/mock-fallback'
import { setMockGatewayOverride } from '@/lib/api/mock-gateway-overrides'
import { upstreamMcpGateways } from '@/lib/api/gateway-list-model'
import {
  mockGateways,
  mockReloadResult,
  mockTestResult,
} from '@/lib/api/mock-data'
import { previewExposurePolicy as sharedPreviewExposurePolicy } from '@/lib/api/exposure-policy-matcher'
import type {
  Gateway,
  CreateGatewayInput,
  UpdateGatewayInput,
  ExposurePolicy,
  TestGatewayResult,
  ReloadGatewayResult,
  GatewayCleanupResult,
  ExposurePolicyPreview,
  ServiceConfig,
  ServiceAction,
  SupportedService,
  CodeModeConfig,
  CodeModeConfigInput,
  ProtectedMcpRoute,
  ProtectedMcpRouteInput,
  ProtectedMcpRouteTestResult,
  DiscoveredMcpServer,
  GatewayImportResult,
} from '@/lib/types/gateway'
import { useCallback } from 'react'

// Set NEXT_PUBLIC_MOCK_DATA=true to use mock data for development
const USE_MOCK_DATA = process.env.NEXT_PUBLIC_MOCK_DATA === 'true'
const DEFAULT_CODE_MODE_CONFIG: CodeModeConfig = {
  enabled: false,
  timeout_ms: 5000,
  max_tool_calls: 8,
  max_response_bytes: 24 * 1024,
  max_response_tokens: 6000,
}
let mockCodeModeConfig: CodeModeConfig = DEFAULT_CODE_MODE_CONFIG
let mockProtectedRoutes: ProtectedMcpRoute[] = [
  {
    name: 'tools',
    enabled: true,
    public_host: 'mcp.example.net',
    public_path: '/tools',
    upstream: null,
    backend_url: 'http://localhost:3100/mcp',
    scopes: ['mcp:read'],
    health_path: '/health',
  },
]

// Simulate network delay for mock data
const mockDelay = (ms: number = 500) => new Promise(resolve => setTimeout(resolve, ms))

function abortableMockDelay(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(new DOMException('Aborted', 'AbortError'))
      return
    }

    const timer = setTimeout(() => {
      signal?.removeEventListener('abort', onAbort)
      resolve()
    }, ms)

    const onAbort = () => {
      clearTimeout(timer)
      reject(new DOMException('Aborted', 'AbortError'))
    }

    signal?.addEventListener('abort', onAbort, { once: true })
  })
}

// Fetcher functions that handle mock/real data
const fetchGateways = async (): Promise<Gateway[]> => {
  if (USE_MOCK_DATA) {
    await mockDelay()
    return upstreamMcpGateways(getMockGatewaysFallback())
  }

  return upstreamMcpGateways(await gatewayApi.list())
}

const fetchGateway = async (id: string): Promise<Gateway> => {
  if (USE_MOCK_DATA) {
    await mockDelay()
    const gateway = getMockGatewayFallback(id)
    if (!gateway) throw new Error('Gateway not found')
    return gateway
  }
  return gatewayApi.get(id)
}

const fetchExposurePolicy = async (id: string): Promise<ExposurePolicy> => {
  if (USE_MOCK_DATA) {
    await mockDelay()
    const gateway = mockGateways.find(g => g.id === id)
    if (!gateway) throw new Error('Gateway not found')
    return {
      mode: gateway.config.expose_tools ? 'allowlist' : 'expose_all',
      patterns: gateway.config.expose_tools || [],
    }
  }
  return gatewayApi.getExposurePolicy(id)
}

const fetchSupportedServices = async (): Promise<SupportedService[]> => {
  if (USE_MOCK_DATA) {
    await mockDelay()
    return getMockSupportedServicesFallback()
  }
  return gatewayApi.supportedServices()
}

const fetchServiceConfig = async (service: string): Promise<ServiceConfig> => {
  if (USE_MOCK_DATA) {
    await mockDelay()
    return getMockServiceConfigFallback(service)
  }
  return gatewayApi.getServiceConfig(service)
}

const fetchServiceActions = async (service: string): Promise<ServiceAction[]> => {
  if (USE_MOCK_DATA) {
    await mockDelay()
    return getMockServiceActionsFallback(service)
  }
  return gatewayApi.serviceActions(service)
}

const fetchCodeModeConfig = async (): Promise<CodeModeConfig> => {
  if (USE_MOCK_DATA) {
    await mockDelay()
    return mockCodeModeConfig
  }
  return gatewayApi.getCodeModeConfig()
}

const fetchProtectedRoutes = async (): Promise<ProtectedMcpRoute[]> => {
  if (USE_MOCK_DATA) {
    await mockDelay()
    return mockProtectedRoutes
  }
  return gatewayApi.listProtectedRoutes()
}

// SWR Keys
export const GATEWAYS_KEY = '/gateways'
export const gatewayKey = (id: string) => `/gateways/${id}`
export const exposurePolicyKey = (id: string) => `/gateways/${id}/exposure`
export const SUPPORTED_SERVICES_KEY = '/gateway-supported-services'
export const serviceConfigKey = (service: string) => `/gateway-service-config/${service}`
export const serviceActionsKey = (service: string) => `/gateway-service-actions/${service}`
export const CODE_MODE_CONFIG_KEY = '/gateway-code-mode-config'
export const PROTECTED_MCP_ROUTES_KEY = '/gateway-protected-mcp-routes'

async function refreshGatewayCache(id?: string, extraKeys: string[] = []) {
  const keys = [GATEWAYS_KEY, ...(id ? [gatewayKey(id)] : []), ...extraKeys]
  await Promise.all(keys.map((key) => mutate(key)))
}

// Hooks
export function useGateways() {
  return useSWR<Gateway[]>(GATEWAYS_KEY, fetchGateways, {
    revalidateOnFocus: false,
    fallbackData: USE_MOCK_DATA ? getMockGatewaysFallback() : undefined,
    revalidateOnMount: !USE_MOCK_DATA,
  })
}

export function useGateway(id: string | null) {
  const fallbackGateway = USE_MOCK_DATA && id ? getMockGatewayFallback(id) : undefined

  return useSWR<Gateway>(
    id ? gatewayKey(id) : null,
    id ? () => fetchGateway(id) : null,
    {
      revalidateOnFocus: false,
      fallbackData: fallbackGateway,
      revalidateOnMount: !USE_MOCK_DATA || fallbackGateway === undefined,
    }
  )
}

export function useExposurePolicy(id: string | null) {
  return useSWR<ExposurePolicy>(
    id ? exposurePolicyKey(id) : null,
    id ? () => fetchExposurePolicy(id) : null,
    {
      revalidateOnFocus: false,
    }
  )
}

export function useSupportedServices() {
  return useSWR<SupportedService[]>(SUPPORTED_SERVICES_KEY, fetchSupportedServices, {
    revalidateOnFocus: false,
    fallbackData: USE_MOCK_DATA ? getMockSupportedServicesFallback() : undefined,
    revalidateOnMount: !USE_MOCK_DATA,
  })
}

export function useServiceConfig(service: string | null) {
  return useSWR<ServiceConfig>(
    service ? serviceConfigKey(service) : null,
    service ? () => fetchServiceConfig(service) : null,
    {
      revalidateOnFocus: false,
      fallbackData: USE_MOCK_DATA && service ? getMockServiceConfigFallback(service) : undefined,
      revalidateOnMount: !USE_MOCK_DATA,
    }
  )
}

export function useServiceActions(service: string | null) {
  return useSWR<ServiceAction[]>(
    service ? serviceActionsKey(service) : null,
    service ? () => fetchServiceActions(service) : null,
    {
      revalidateOnFocus: false,
      fallbackData: USE_MOCK_DATA && service ? getMockServiceActionsFallback(service) : undefined,
      revalidateOnMount: !USE_MOCK_DATA,
    }
  )
}

export function useGatewayCodeModeConfig() {
  return useSWR<CodeModeConfig>(CODE_MODE_CONFIG_KEY, fetchCodeModeConfig, {
    revalidateOnFocus: false,
    fallbackData: USE_MOCK_DATA ? DEFAULT_CODE_MODE_CONFIG : undefined,
    revalidateOnMount: !USE_MOCK_DATA,
  })
}

export function useProtectedMcpRoutes() {
  return useSWR<ProtectedMcpRoute[]>(PROTECTED_MCP_ROUTES_KEY, fetchProtectedRoutes, {
    revalidateOnFocus: false,
    fallbackData: USE_MOCK_DATA ? mockProtectedRoutes : undefined,
    revalidateOnMount: !USE_MOCK_DATA,
  })
}

// Mutation hooks
export function useGatewayMutations() {
  const createGateway = useCallback(async (input: CreateGatewayInput): Promise<Gateway> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      const newGateway: Gateway = {
        id: `gw-${Date.now()}`,
        name: input.name,
        transport: input.transport,
        config: input.config,
        status: {
          healthy: false,
          connected: false,
          discovered_tool_count: 0,
          exposed_tool_count: 0,
          discovered_resource_count: 0,
          exposed_resource_count: 0,
          discovered_prompt_count: 0,
          exposed_prompt_count: 0,
        },
        discovery: { tools: [], resources: [], prompts: [] },
        warnings: [],
        // created_at / updated_at come from the backend; omit in mock paths.
      }
      await mutate(GATEWAYS_KEY, (current: Gateway[] = []) => [...current, newGateway], false)
      return newGateway
    }
    const gateway = await gatewayApi.create(input)
    await refreshGatewayCache(gateway.id)
    return gateway
  }, [])

  const discoverExternalConfigs = useCallback(async (): Promise<DiscoveredMcpServer[]> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      return [
        {
          name: 'local-files',
          source_client: 'claude-code',
          source_path: '~/.claude/settings.json',
          transport: 'stdio',
          command_preview: 'npx',
          env_key_count: 1,
          already_configured: false,
          tombstoned: false,
        },
      ]
    }
    return gatewayApi.discoverExternalConfigs()
  }, [])

  const importExternalConfigs = useCallback(async (names?: string[]): Promise<GatewayImportResult> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      return {
        imported: (names && names.length > 0 ? names : ['local-files']).map((name) => ({
          config: { name, enabled: false },
        })),
        skipped: [],
        errors: [],
      }
    }
    const result = await gatewayApi.importExternalConfigs(names)
    await refreshGatewayCache()
    return result
  }, [])

  const clearImportTombstone = useCallback(async (server: DiscoveredMcpServer): Promise<void> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      return
    }
    await gatewayApi.clearImportTombstone(server)
  }, [])

  const restoreImportTombstone = useCallback(async (server: DiscoveredMcpServer): Promise<Gateway> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      return {
        id: server.name,
        name: server.name,
        transport: server.transport,
        source: 'custom_gateway',
        configured: true,
        enabled: false,
        config: {},
        status: {
          healthy: false,
          connected: false,
          discovered_tool_count: 0,
          exposed_tool_count: 0,
          discovered_resource_count: 0,
          exposed_resource_count: 0,
          discovered_prompt_count: 0,
          exposed_prompt_count: 0,
        },
        discovery: { tools: [], resources: [], prompts: [] },
        warnings: [],
        // created_at / updated_at come from the backend; omit in mock paths.
      }
    }
    const gateway = await gatewayApi.restoreImportTombstone(server)
    await refreshGatewayCache(gateway.id)
    return gateway
  }, [])

  const updateGateway = useCallback(async (id: string, input: UpdateGatewayInput): Promise<Gateway> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      const gateway = mockGateways.find(g => g.id === id)
      if (!gateway) throw new Error('Gateway not found')
      const updated = {
        ...gateway,
        ...input,
        config: {
          ...gateway.config,
          ...input.config,
        },
        // updated_at comes from the backend; omit in mock paths.
      }
      if (input.config?.proxy_resources !== undefined) {
        setMockGatewayOverride(id, { proxyResources: input.config.proxy_resources })
      }
      await mutate(gatewayKey(id), updated, false)
      await mutate(GATEWAYS_KEY)
      return updated
    }
    const gateway = await gatewayApi.update(id, input)
    await refreshGatewayCache(id)
    return gateway
  }, [])

  const removeGateway = useCallback(async (id: string): Promise<void> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      await mutate(GATEWAYS_KEY, (current: Gateway[] = []) => current.filter(g => g.id !== id), false)
      return
    }
    await gatewayApi.remove(id)
    await refreshGatewayCache()
  }, [])

  const removeVirtualServer = useCallback(async (id: string): Promise<void> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      await mutate(GATEWAYS_KEY, (current: Gateway[] = []) => current.filter(g => g.id !== id), false)
      return
    }
    await gatewayApi.removeVirtualServer(id)
    await refreshGatewayCache()
  }, [])

  const testGateway = useCallback(async (id: string): Promise<TestGatewayResult> => {
    if (USE_MOCK_DATA) {
      await mockDelay(1500) // Longer delay for test
      const gateway = mockGateways.find(g => g.id === id)
      if (!gateway) throw new Error('Gateway not found')
      if (!gateway.status.healthy) {
        return {
          success: false,
          message: 'Connection failed',
          error: gateway.status.last_error,
        }
      }
      return mockTestResult
    }
    return await gatewayApi.test(id)
  }, [])

  const reloadGateway = useCallback(async (id: string): Promise<ReloadGatewayResult> => {
    if (USE_MOCK_DATA) {
      await mockDelay(2000) // Longer delay for reload
      return mockReloadResult
    }
    const result = await gatewayApi.reload(id)
    await refreshGatewayCache(id)
    return result
  }, [])

  const setExposurePolicy = useCallback(async (id: string, policy: ExposurePolicy): Promise<ExposurePolicy> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      setMockGatewayOverride(id, { exposurePolicy: policy })
      const updatedGateway = getMockGatewayFallback(id)
      if (!updatedGateway) {
        throw new Error('Gateway not found')
      }

      await mutate(
        gatewayKey(id),
        updatedGateway,
        false,
      )
      await mutate(
        GATEWAYS_KEY,
        (current: Gateway[] = []) =>
          current.map((gateway) => (gateway.id === id ? updatedGateway : gateway)),
        false,
      )
      await mutate(exposurePolicyKey(id), policy, false)
      return policy
    }
    const result = await gatewayApi.setExposurePolicy(id, policy)
    await refreshGatewayCache(id, [exposurePolicyKey(id)])
    return result
  }, [])

  const previewExposurePolicy = useCallback(async (id: string, patterns: string[], signal?: AbortSignal): Promise<ExposurePolicyPreview> => {
    if (USE_MOCK_DATA) {
      await abortableMockDelay(300, signal)
      const gateway = mockGateways.find(g => g.id === id)
      if (!gateway) throw new Error('Gateway not found')

      // Use the shared pure matcher so mock and real preview paths are
      // semantically identical (lab-2oec.7).
      const toolNames = gateway.discovery.tools.map((t) => t.name)
      return sharedPreviewExposurePolicy(toolNames, patterns)
    }
    return gatewayApi.previewExposurePolicy(id, patterns, signal)
  }, [])

  const saveServiceConfig = useCallback(async (service: string, values: Record<string, string>): Promise<ServiceConfig> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      const fields = Object.entries(values).map(([name, value]) => ({
        name,
        present: value.length > 0,
        secret: name.includes('TOKEN') || name.includes('KEY') || name.includes('PASSWORD'),
        value_preview: name.includes('TOKEN') || name.includes('KEY') || name.includes('PASSWORD') ? null : value,
      }))
      const result = { service, configured: fields.length > 0, fields }
      await mutate(serviceConfigKey(service), result, false)
      return result
    }
    const result = await gatewayApi.setServiceConfig(service, values)
    await refreshGatewayCache(undefined, [serviceConfigKey(service)])
    return result
  }, [])

  const setCodeModeConfig = useCallback(async (input: CodeModeConfigInput): Promise<CodeModeConfig> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      mockCodeModeConfig = {
        ...mockCodeModeConfig,
        ...input,
      }
      await mutate(CODE_MODE_CONFIG_KEY, mockCodeModeConfig, false)
      return mockCodeModeConfig
    }
    const result = await gatewayApi.setCodeModeConfig(input)
    await mutate(CODE_MODE_CONFIG_KEY, result, false)
    await mutate(GATEWAYS_KEY)
    return result
  }, [])

  const addProtectedRoute = useCallback(
    async (route: ProtectedMcpRouteInput, signal?: AbortSignal): Promise<ProtectedMcpRoute> => {
      if (USE_MOCK_DATA) {
        await abortableMockDelay(300, signal)
        if (mockProtectedRoutes.some((item) => item.name === route.name)) {
          throw new Error(`Protected route ${route.name} already exists`)
        }
        mockProtectedRoutes = [...mockProtectedRoutes, route]
        await mutate(PROTECTED_MCP_ROUTES_KEY, mockProtectedRoutes, false)
        return route
      }
      const result = await gatewayApi.addProtectedRoute(route, signal)
      await mutate(PROTECTED_MCP_ROUTES_KEY)
      return result
    },
    [],
  )

  const updateProtectedRoute = useCallback(
    async (
      name: string,
      route: ProtectedMcpRouteInput,
      signal?: AbortSignal,
    ): Promise<ProtectedMcpRoute> => {
      if (USE_MOCK_DATA) {
        await abortableMockDelay(300, signal)
        mockProtectedRoutes = mockProtectedRoutes.map((item) => (item.name === name ? route : item))
        await mutate(PROTECTED_MCP_ROUTES_KEY, mockProtectedRoutes, false)
        return route
      }
      const result = await gatewayApi.updateProtectedRoute(name, route, signal)
      await mutate(PROTECTED_MCP_ROUTES_KEY)
      return result
    },
    [],
  )

  const removeProtectedRoute = useCallback(
    async (name: string, signal?: AbortSignal): Promise<ProtectedMcpRoute> => {
      if (USE_MOCK_DATA) {
        await abortableMockDelay(300, signal)
        const removed = mockProtectedRoutes.find((item) => item.name === name)
        if (!removed) {
          throw new Error(`Protected route ${name} not found`)
        }
        mockProtectedRoutes = mockProtectedRoutes.filter((item) => item.name !== name)
        await mutate(PROTECTED_MCP_ROUTES_KEY, mockProtectedRoutes, false)
        return removed
      }
      const result = await gatewayApi.removeProtectedRoute(name, signal)
      await mutate(PROTECTED_MCP_ROUTES_KEY)
      return result
    },
    [],
  )

  const testProtectedRoute = useCallback(
    async (
      route: ProtectedMcpRouteInput,
      signal?: AbortSignal,
    ): Promise<ProtectedMcpRouteTestResult> => {
      if (USE_MOCK_DATA) {
        await abortableMockDelay(250, signal)
        return {
          ok: true,
          route,
          resource: `https://${route.public_host}${route.public_path}`,
          metadata_url: `https://${route.public_host}/.well-known/oauth-protected-resource${route.public_path}`,
        }
      }
      return gatewayApi.testProtectedRoute(route, signal)
    },
    [],
  )

  const enableVirtualServer = useCallback(async (id: string): Promise<Gateway> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      const gateway = mockGateways.find((item) => item.id === id)
      if (!gateway) throw new Error('Gateway not found')
      const result = { ...gateway, enabled: true }
      await mutate(gatewayKey(id), result, false)
      await mutate(GATEWAYS_KEY)
      return result
    }
    const result = await gatewayApi.enableVirtualServer(id)
    await refreshGatewayCache(id)
    return result
  }, [])

  const disableVirtualServer = useCallback(async (id: string): Promise<Gateway> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      const gateway = mockGateways.find((item) => item.id === id)
      if (!gateway) throw new Error('Gateway not found')
      const result = { ...gateway, enabled: false }
      await mutate(gatewayKey(id), result, false)
      await mutate(GATEWAYS_KEY)
      return result
    }
    const result = await gatewayApi.disableVirtualServer(id)
    await refreshGatewayCache(id)
    return result
  }, [])

  const setVirtualServerSurface = useCallback(
    async (id: string, surface: 'cli' | 'api' | 'mcp' | 'webui', enabled: boolean): Promise<Gateway> => {
      if (USE_MOCK_DATA) {
        await mockDelay()
        const gateway = mockGateways.find((item) => item.id === id)
        if (!gateway) throw new Error('Gateway not found')
        const result = {
          ...gateway,
          surfaces: gateway.surfaces
            ? {
                ...gateway.surfaces,
                [surface]: { ...gateway.surfaces[surface], enabled },
              }
            : gateway.surfaces,
        }
        await mutate(gatewayKey(id), result, false)
        await mutate(GATEWAYS_KEY)
        return result
      }
      const result = await gatewayApi.setVirtualServerSurface(id, surface, enabled)
      await refreshGatewayCache(id)
      return result
    },
    [],
  )

  const enableGateway = useCallback(async (id: string): Promise<Gateway> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      const gateway = mockGateways.find((item) => item.id === id)
      if (!gateway) throw new Error('Gateway not found')
      const result = { ...gateway, enabled: true }
      await mutate(gatewayKey(id), result, false)
      await mutate(GATEWAYS_KEY)
      return result
    }
    const result = await gatewayApi.enableGateway(id)
    await refreshGatewayCache(id)
    return result
  }, [])

  const disableGateway = useCallback(async (id: string): Promise<Gateway> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      const gateway = mockGateways.find((item) => item.id === id)
      if (!gateway) throw new Error('Gateway not found')
      const result = { ...gateway, enabled: false }
      await mutate(gatewayKey(id), result, false)
      await mutate(GATEWAYS_KEY)
      return result
    }
    const result = await gatewayApi.disableGateway(id)
    await refreshGatewayCache(id)
    return result
  }, [])

  const cleanupGateway = useCallback(async (
    id: string,
    aggressive: boolean = false,
    dryRun: boolean = false,
  ): Promise<GatewayCleanupResult> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      return {
        upstream: id,
        aggressive,
        dry_run: dryRun,
        gateway_matched: 0,
        local_matched: 0,
        aggressive_matched: 0,
        gateway_killed: 0,
        local_killed: 0,
        aggressive_killed: 0,
        gateway_matches: [],
        local_matches: [],
        aggressive_matches: [],
      }
    }
    const result = await gatewayApi.cleanupGateway(id, aggressive, dryRun)
    await refreshGatewayCache(id)
    return result
  }, [])

  return {
    createGateway,
    discoverExternalConfigs,
    importExternalConfigs,
    clearImportTombstone,
    restoreImportTombstone,
    updateGateway,
    removeGateway,
    removeVirtualServer,
    testGateway,
    reloadGateway,
    setExposurePolicy,
    previewExposurePolicy,
    saveServiceConfig,
    setCodeModeConfig,
    addProtectedRoute,
    updateProtectedRoute,
    removeProtectedRoute,
    testProtectedRoute,
    enableVirtualServer,
    disableVirtualServer,
    enableGateway,
    disableGateway,
    cleanupGateway,
    setVirtualServerSurface,
  }
}
