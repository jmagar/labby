import type { Gateway, ServiceAction, ServiceConfig, SupportedService } from '../types/gateway.ts'

export function upstreamMcpGateways(gateways: Gateway[]): Gateway[] {
  return gateways.filter((gateway) => gateway.source !== 'in_process' && gateway.transport !== 'in_process')
}

export function synthesizeLabGateway(
  service: SupportedService,
  config: ServiceConfig | undefined,
  actions: ServiceAction[] | undefined,
): Gateway {
  const urlField = config?.fields.find((field) => field.name.endsWith('_URL'))?.value_preview ?? undefined
  const tools = (actions ?? []).map((action) => ({
    name: action.name,
    description: action.description,
    exposed: false,
    matched_by: null,
  }))

  return {
    id: service.key,
    name: service.key,
    transport: 'in_process',
    source: 'in_process',
    configured: config?.configured ?? false,
    enabled: false,
    surfaces: {
      cli: { enabled: false, connected: false },
      api: { enabled: false, connected: false },
      mcp: { enabled: false, connected: false },
      webui: { enabled: false, connected: false },
    },
    config: {
      ...(urlField ? { url: urlField } : {}),
      proxy_resources: false,
    },
    status: {
      healthy: false,
      connected: false,
      discovered_tool_count: tools.length,
      exposed_tool_count: 0,
      discovered_resource_count: 0,
      exposed_resource_count: 0,
      discovered_prompt_count: 0,
      exposed_prompt_count: 0,
    },
    discovery: {
      tools,
      resources: [],
      prompts: [],
    },
    warnings: [],
    // created_at / updated_at are not available for synthesized lab-service views.
  }
}

function gatewaySortWeight(gateway: Gateway): number {
  if (gateway.source === 'in_process' && gateway.enabled === false) {
    return 2
  }
  return 1
}

export function mergeGatewayListWithSupportedServices(
  gateways: Gateway[],
  supportedServices: SupportedService[],
  serviceConfigs: Map<string, ServiceConfig>,
  serviceActions: Map<string, ServiceAction[]>,
): Gateway[] {
  const existingIds = new Set(gateways.map((gateway) => gateway.id))
  const synthetic = supportedServices
    .filter((service) => !existingIds.has(service.key))
    .map((service) =>
      synthesizeLabGateway(service, serviceConfigs.get(service.key), serviceActions.get(service.key)),
    )

  return [...gateways, ...synthetic].sort((left, right) => {
    const weightDiff = gatewaySortWeight(left) - gatewaySortWeight(right)
    if (weightDiff !== 0) {
      return weightDiff
    }
    return left.name.localeCompare(right.name)
  })
}
