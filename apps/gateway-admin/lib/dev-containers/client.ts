import { performServiceAction, type ServiceActionError } from '@/lib/api/service-action-client'
import { assertGatewayAuthorityCurrent, captureGatewayAuthority } from '@/lib/api/gateway-request'
import { getSessionAuthority } from '@/lib/auth/session-store'

export type DevContainer = { instance_id: string; owner_kind: 'installation' | 'team' | 'project' | 'personal'; owner_id: string; desired_state: 'running' | 'stopped' | 'deleted'; observed_state: string }

export class DevContainerError extends Error implements ServiceActionError {
  constructor(message: string, public status: number, public code?: string) { super(message); this.name = 'DevContainerError' }
}

async function action<T>(name: string, params: object, signal?: AbortSignal): Promise<T> {
  const request = captureGatewayAuthority(signal)
  try {
    const value = await performServiceAction<T, DevContainerError>({ action: name, params, signal: request.signal, serviceLabel: 'Dev Containers', url: '/v1/dev-containers', createError: (message, status, code) => new DevContainerError(message, status, code) })
    assertGatewayAuthorityCurrent(request.generation)
    return value
  } finally { request.finish() }
}

export async function listDevContainers(signal?: AbortSignal) { return (await action<{ instances: DevContainer[] }>('dev_containers.list', {}, signal)).instances }
export async function createDevContainer(instanceId: string, templateId: string) {
  const owner = getSessionAuthority()?.activeOwner
  if (!owner) throw new DOMException('Authority is unavailable', 'InvalidStateError')
  return action('dev_containers.create', { instance_id: instanceId, template_id: templateId, owner_kind: owner.kind, owner_id: owner.id })
}
export function operateDevContainer(instanceId: string, operation: 'start' | 'stop' | 'destroy' | 'reconcile') { return action(`dev_containers.${operation}`, { instance_id: instanceId }) }
