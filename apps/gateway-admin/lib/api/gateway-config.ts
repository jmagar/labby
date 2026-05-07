export function normalizeGatewayApiBase(baseUrl?: string): string {
  const value = baseUrl || process.env.NEXT_PUBLIC_API_URL || '/v1'
  return value.endsWith('/') ? value.slice(0, -1) : value
}

export function gatewayActionUrl(baseUrl?: string): string {
  return `${normalizeGatewayApiBase(baseUrl)}/gateway`
}

export function extractActionUrl(baseUrl?: string): string {
  return `${normalizeGatewayApiBase(baseUrl)}/extract`
}

export function setupActionUrl(baseUrl?: string): string {
  return `${normalizeGatewayApiBase(baseUrl)}/setup`
}

export function doctorActionUrl(baseUrl?: string): string {
  return `${normalizeGatewayApiBase(baseUrl)}/doctor`
}

export function marketplaceActionUrl(baseUrl?: string): string {
  return `${normalizeGatewayApiBase(baseUrl)}/marketplace`
}

export function beadsActionUrl(baseUrl?: string): string {
  return `${normalizeGatewayApiBase(baseUrl)}/beads`
}

export function nodesUrl(baseUrl?: string): string {
  return `${normalizeGatewayApiBase(baseUrl)}/nodes`
}

export function nodeDetailUrl(nodeId: string, baseUrl?: string): string {
  return `${nodesUrl(baseUrl)}/${encodeURIComponent(nodeId)}`
}

export function nodeLogsSearchUrl(baseUrl?: string): string {
  return `${nodesUrl(baseUrl)}/logs/search`
}

export function gatewayDetailHref(id: string): string {
  return `/gateway/?id=${encodeURIComponent(id)}`
}
