import { gatewayRequestInit } from './gateway-request.ts'
import {
  assertDevPreviewCanRunAction,
  devPreviewActionUrl,
} from '@/lib/dev/preview-mode'

export interface ServiceActionError extends Error {
  status: number
  code?: string
  param?: string
}

interface ActionErrorBody {
  kind?: string
  code?: string
  message?: string
  param?: string
}

type ActionErrorFactory<TError extends ServiceActionError> = (
  message: string,
  status: number,
  code?: string,
  param?: string,
) => TError

export function isAbortError(error: unknown): boolean {
  return error instanceof DOMException
    ? error.name === 'AbortError'
    : error instanceof Error && error.name === 'AbortError'
}

export type SafeFanoutResult<TItem, TValue> =
  | { ok: true; item: TItem; value: TValue }
  | { ok: false; item: TItem; error: unknown }

export async function safeFanout<TItem, TValue>(
  items: readonly TItem[],
  load: (item: TItem) => Promise<TValue>,
): Promise<Array<SafeFanoutResult<TItem, TValue>>> {
  return Promise.all(
    items.map((item) =>
      load(item).then(
        (value) => ({ ok: true as const, item, value }),
        (error: unknown) => ({ ok: false as const, item, error }),
      ),
    ),
  )
}

async function parseActionResponse<T, TError extends ServiceActionError>(
  response: Response,
  createError: ActionErrorFactory<TError>,
): Promise<T> {
  if (!response.ok) {
    const error: ActionErrorBody = await (response.json() as Promise<ActionErrorBody>).catch(() => ({ message: 'An error occurred' } satisfies ActionErrorBody))
    throw createError(
      error.message || 'An error occurred',
      response.status,
      error.kind || error.code,
      error.param,
    )
  }

  return response.json()
}

export async function performServiceAction<T, TError extends ServiceActionError>({
  action,
  params,
  signal,
  serviceLabel,
  url,
  createError,
  source,
}: {
  action: string
  params: object
  signal?: AbortSignal
  serviceLabel: string
  url: string
  createError: ActionErrorFactory<TError>
  source?: string
}): Promise<T> {
  assertDevPreviewCanRunAction(action)

  let response: Response
  try {
    const init = gatewayRequestInit(action, params, undefined, signal)
    if (source) {
      init.headers = { ...(init.headers as Record<string, string>), 'X-Lab-Source': source }
    }
    response = await fetch(devPreviewActionUrl(url), init)
  } catch (error) {
    if (isAbortError(error)) {
      throw error
    }
    const message = error instanceof Error ? error.message : 'unknown network error'
    throw createError(
      `${serviceLabel} backend action \`${action}\` failed before a response was received: ${message}`,
      502,
      'backend_unreachable',
    )
  }

  return parseActionResponse<T, TError>(response, createError)
}
