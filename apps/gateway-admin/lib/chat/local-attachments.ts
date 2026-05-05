import type { LocalAttachmentDraft, SerializableLocalAttachment } from '@/lib/fs/types'

export const MAX_LOCAL_ATTACHMENTS = 5
export const MAX_LOCAL_ATTACHMENT_BYTES = 48 * 1024
export const MAX_LOCAL_ATTACHMENT_LABEL = '48 KiB'

const ALLOWED_EXACT_TYPES = new Set([
  'application/json',
  'application/pdf',
  'image/png',
  'image/jpeg',
  'image/gif',
  'image/webp',
])

export function isSupportedLocalAttachmentType(mimeType: string): boolean {
  const normalized = mimeType.trim().toLowerCase()
  return normalized.startsWith('text/') || ALLOWED_EXACT_TYPES.has(normalized)
}

export function localAttachmentId(file: File): string {
  return `local-${file.name}-${file.size}-${file.lastModified}`
}

export function createLocalAttachmentDraft(file: File): LocalAttachmentDraft {
  const mimeType = file.type.trim().toLowerCase()
  const previewUrl = mimeType.startsWith('image/') ? URL.createObjectURL(file) : null
  return {
    kind: 'local',
    id: localAttachmentId(file),
    file,
    previewUrl,
  }
}

export function revokeLocalAttachmentPreview(attachment: LocalAttachmentDraft): void {
  if (attachment.previewUrl) {
    URL.revokeObjectURL(attachment.previewUrl)
  }
}

export function validateLocalFiles(
  incoming: File[],
  existingLocalAttachments: readonly File[],
): { accepted: File[]; errors: string[] } {
  const errors: string[] = []
  const remainingSlots = Math.max(0, MAX_LOCAL_ATTACHMENTS - existingLocalAttachments.length)

  if (incoming.length > remainingSlots) {
    errors.push(`You can attach up to ${MAX_LOCAL_ATTACHMENTS} files.`)
  }

  const accepted: File[] = []
  for (const candidate of incoming) {
    const mimeType = candidate.type || 'application/octet-stream'
    if (!isSupportedLocalAttachmentType(mimeType)) {
      errors.push(`${candidate.name} has unsupported type ${mimeType}.`)
      continue
    }

    if (candidate.size > MAX_LOCAL_ATTACHMENT_BYTES) {
      errors.push(`${candidate.name} is larger than ${MAX_LOCAL_ATTACHMENT_LABEL}.`)
      continue
    }

    if (accepted.length < remainingSlots) {
      accepted.push(candidate)
    }
  }

  return { accepted, errors }
}

function arrayBufferToBase64(buffer: ArrayBuffer): string {
  let binary = ''
  const bytes = new Uint8Array(buffer)
  const chunkSize = 0x8000
  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + chunkSize))
  }
  return btoa(binary)
}

export async function fileToSerializableAttachment(
  attachment: LocalAttachmentDraft,
): Promise<SerializableLocalAttachment> {
  const mimeType = (attachment.file.type || 'application/octet-stream').trim().toLowerCase()
  const base = {
    kind: 'local' as const,
    id: attachment.id,
    name: attachment.file.name,
    mimeType,
    size: attachment.file.size,
  }

  if (mimeType.startsWith('text/') || mimeType === 'application/json') {
    return {
      ...base,
      contentKind: 'text',
      text: await attachment.file.text(),
    }
  }

  return {
    ...base,
    contentKind: 'blob',
    base64: arrayBufferToBase64(await attachment.file.arrayBuffer()),
  }
}
