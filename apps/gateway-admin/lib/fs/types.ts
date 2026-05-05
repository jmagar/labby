/**
 * Workspace filesystem browser types.
 *
 * Mirrors the response shapes returned by `GET /v1/fs/list` and the
 * payload shape accepted by the chat-input `onSend` callback.
 */

/** Entry kind surfaced by `fs.list`. */
export type FsEntryKind = 'file' | 'dir' | 'symlink' | 'other'

/** Single entry in a workspace directory listing. */
export interface FsEntry {
  name: string
  /** Workspace-relative path (forward-slash joined). Empty string = root. */
  path: string
  kind: FsEntryKind
  /** Bytes for regular files; omitted otherwise. */
  size?: number
  /** RFC-3339 timestamp. Omitted when metadata is unreadable. */
  modified?: string
  /** `false` when stat failed (e.g. dangling symlink). */
  accessible: boolean
}

/** `GET /v1/fs/list` response shape. */
export interface FsListResponse {
  entries: FsEntry[]
  /** `true` when the server capped the listing at 10,000 entries. */
  truncated: boolean
}

export type WorkspaceAttachmentRef = { kind: 'file'; path: string }

export type LocalAttachmentDraft = {
  kind: 'local'
  id: string
  file: File
  previewUrl: string | null
}

export type SerializableLocalAttachment =
  | {
      kind: 'local'
      id: string
      name: string
      mimeType: string
      size: number
      contentKind: 'text'
      text: string
    }
  | {
      kind: 'local'
      id: string
      name: string
      mimeType: string
      size: number
      contentKind: 'blob'
      base64: string
    }

export type PromptAttachmentRef = WorkspaceAttachmentRef | SerializableLocalAttachment
export type AttachmentRef = WorkspaceAttachmentRef | LocalAttachmentDraft
