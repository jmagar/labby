'use client'

import Image from 'next/image'
import * as React from 'react'
import { Send, Paperclip, Wrench, ChevronDown, X, FileText } from 'lucide-react'
import { cn } from '@/lib/utils'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'
import type { ACPAgent, ACPModelOption } from './types'
import {
  createLocalAttachmentDraft,
  fileToSerializableAttachment,
  revokeLocalAttachmentPreview,
  validateLocalFiles,
} from '@/lib/chat/local-attachments'
import type { AttachmentRef, LocalAttachmentDraft, PromptAttachmentRef } from '@/lib/fs/types'
import { isInlineImageMime, previewWorkspaceFile } from '@/lib/fs/client'
import { WorkspacePicker } from './workspace-picker'

/** Payload emitted by the chat input on submit. */
export interface ChatInputPayload {
  text: string
  attachments: PromptAttachmentRef[]
}

interface ChatInputProps {
  onSend: (payload: ChatInputPayload) => void | Promise<void>
  disabled?: boolean
  disabledReason?: string
  draftText?: string
  onDraftTextChange?: (value: string) => void
  attachmentsResetToken?: number
  selectedAgent: ACPAgent | null
  agents: ACPAgent[]
  onSelectAgent: (agentId: string) => void
  selectedModel: ACPModelOption | null
  modelOptions: ACPModelOption[]
  onSelectModel: (modelId: string) => void
}

export const CHAT_INPUT_MAX_HEIGHT_PX = 200
const CHAT_INPUT_MIN_HEIGHT_PX = 44

export function resizeChatPromptTextarea(el: HTMLTextAreaElement) {
  el.style.height = 'auto'
  const nextHeight = Math.min(el.scrollHeight, CHAT_INPUT_MAX_HEIGHT_PX)
  el.style.height = `${nextHeight}px`
  el.style.overflowY = el.scrollHeight > CHAT_INPUT_MAX_HEIGHT_PX ? 'auto' : 'hidden'
}

export function ChatInput({
  onSend,
  disabled = false,
  disabledReason,
  draftText,
  onDraftTextChange,
  attachmentsResetToken,
  selectedAgent,
  agents,
  onSelectAgent,
  selectedModel,
  modelOptions = [],
  onSelectModel = () => {},
}: ChatInputProps) {
  const [uncontrolledValue, setUncontrolledValue] = React.useState('')
  const [sending, setSending] = React.useState(false)
  const [agentPickerOpen, setAgentPickerOpen] = React.useState(false)
  const [modelPickerOpen, setModelPickerOpen] = React.useState(false)
  const [attachments, setAttachments] = React.useState<AttachmentRef[]>([])
  const attachmentsRef = React.useRef<AttachmentRef[]>(attachments)
  const localFileInputRef = React.useRef<HTMLInputElement>(null)
  const [attachmentError, setAttachmentError] = React.useState<string | null>(null)
  const [workspacePickerOpen, setWorkspacePickerOpen] = React.useState(false)
  // Synchronous send-lock: state updates batch across a render tick, so a fast
  // Enter+Click can fire handleSend twice before `sending` flips. The ref
  // engages on the same tick, blocking the second caller. `sending` state is
  // retained for UI (button disabled, textarea opacity).
  const sendingRef = React.useRef(false)
  const textareaRef = React.useRef<HTMLTextAreaElement>(null)
  const pickerRef = React.useRef<HTMLDivElement>(null)
  const triggerRef = React.useRef<HTMLButtonElement>(null)
  const optionRefs = React.useRef<Array<HTMLButtonElement | null>>([])
  const modelPickerRef = React.useRef<HTMLDivElement>(null)
  const modelTriggerRef = React.useRef<HTMLButtonElement>(null)
  const modelOptionRefs = React.useRef<Array<HTMLButtonElement | null>>([])
  const [activeAgentIndex, setActiveAgentIndex] = React.useState(0)
  const [activeModelIndex, setActiveModelIndex] = React.useState(0)
  const pickerId = React.useId()
  const modelPickerId = React.useId()

  const value = draftText ?? uncontrolledValue
  const setValue = React.useCallback(
    (nextValue: string) => {
      if (draftText === undefined) {
        setUncontrolledValue(nextValue)
      }
      onDraftTextChange?.(nextValue)
    },
    [draftText, onDraftTextChange],
  )

  optionRefs.current.length = agents.length
  modelOptionRefs.current.length = modelOptions.length

  const hasContent = value.trim().length > 0 || attachments.length > 0
  const localAttachments = attachments.filter(
    (attachment): attachment is LocalAttachmentDraft => attachment.kind === 'local',
  )

  const handleSend = async () => {
    const trimmed = value.trim()
    if (!hasContent || disabled || sendingRef.current) return
    // Snapshot the attachments at send time so cleanup uses the same list
    // we just shipped, regardless of state churn during the network call.
    const sendingAttachments = attachments
    // Acquire the lock synchronously BEFORE any async work so a re-entrant
    // call within the same tick observes the lock and bails. The release
    // path lives inside `finally` so any synchronous throw between this line
    // and the first await still clears the lock.
    sendingRef.current = true
    setSending(true)
    let serializedAttachments: PromptAttachmentRef[]
    try {
      serializedAttachments = await Promise.all(
        sendingAttachments.map((attachment) =>
          attachment.kind === 'local' ? fileToSerializableAttachment(attachment) : attachment,
        ),
      )
    } catch {
      setAttachmentError('Could not read one or more attached files.')
      sendingRef.current = false
      setSending(false)
      return
    }

    try {
      await onSend({ text: trimmed, attachments: serializedAttachments })
      // Success path: clear the input and chip list now that the prompt
      // shipped. Revoke object URLs for any local previews to free memory.
      sendingAttachments.forEach((attachment) => {
        if (attachment.kind === 'local') revokeLocalAttachmentPreview(attachment)
      })
      setAttachmentError(null)
      setValue('')
      setAttachments([])
      if (textareaRef.current) {
        textareaRef.current.style.height = 'auto'
        textareaRef.current.style.overflowY = 'hidden'
        textareaRef.current.scrollTop = 0
      }
    } catch (error) {
      // Failure path: surface the error inline and KEEP the chips so the
      // user can retry without re-attaching. Previous behavior threw the
      // rejection out of an event handler, which crashed the chat page when
      // it bubbled to the React tree.
      const message = error instanceof Error ? error.message : 'Failed to send prompt.'
      setAttachmentError(message)
    } finally {
      sendingRef.current = false
      setSending(false)
    }
  }

  const handleLocalFiles = (fileList: FileList | null) => {
    if (!fileList) return
    const incoming = Array.from(fileList)
    const { accepted, errors } = validateLocalFiles(
      incoming,
      localAttachments.map((attachment) => attachment.file),
    )
    setAttachmentError(errors[0] ?? null)
    setAttachments((prev) => [...prev, ...accepted.map(createLocalAttachmentDraft)])
    if (localFileInputRef.current) {
      localFileInputRef.current.value = ''
    }
  }

  const handleAttach = (attachment: AttachmentRef) => {
    setAttachments((prev) => {
      // Dedupe by path so double-adding the same file is a no-op.
      if (
        attachment.kind === 'file'
        && prev.some((a) => a.kind === 'file' && a.path === attachment.path)
      ) return prev
      return [...prev, attachment]
    })
  }

  const removeAttachment = (ref: AttachmentRef) => {
    if (ref.kind === 'local') revokeLocalAttachmentPreview(ref)
    setAttachments((prev) =>
      prev.filter((attachment) => {
        if (attachment.kind === 'local' && ref.kind === 'local') return attachment.id !== ref.id
        if (attachment.kind === 'file' && ref.kind === 'file') return attachment.path !== ref.path
        return true
      }),
    )
  }

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.nativeEvent.isComposing) return
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      void handleSend()
    }
  }

  const handleInput = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    setValue(e.target.value)
    resizeChatPromptTextarea(e.target)
  }

  React.useEffect(() => {
    if (!textareaRef.current) return
    textareaRef.current.style.height = 'auto'
    textareaRef.current.style.height = `${Math.min(textareaRef.current.scrollHeight, 200)}px`
  }, [value])

  React.useEffect(() => {
    setAttachments([])
  }, [attachmentsResetToken])

  React.useEffect(() => {
    if (!agentPickerOpen) return

    const selectedIndex = Math.max(agents.findIndex((agent) => agent.id === selectedAgent?.id), 0)
    setActiveAgentIndex(selectedIndex)
    const frame = window.requestAnimationFrame(() => {
      optionRefs.current[selectedIndex]?.focus()
    })

    const handlePointerDown = (event: MouseEvent) => {
      if (!pickerRef.current?.contains(event.target as Node)) {
        setAgentPickerOpen(false)
        triggerRef.current?.focus()
      }
    }

    document.addEventListener('mousedown', handlePointerDown)
    return () => {
      window.cancelAnimationFrame(frame)
      document.removeEventListener('mousedown', handlePointerDown)
    }
  }, [agentPickerOpen, agents, selectedAgent?.id])

  React.useEffect(() => {
    if (!modelPickerOpen) return

    const selectedIndex = Math.max(modelOptions.findIndex((model) => model.id === selectedModel?.id), 0)
    setActiveModelIndex(selectedIndex)
    const frame = window.requestAnimationFrame(() => {
      modelOptionRefs.current[selectedIndex]?.focus()
    })

    const handlePointerDown = (event: MouseEvent) => {
      if (!modelPickerRef.current?.contains(event.target as Node)) {
        setModelPickerOpen(false)
        modelTriggerRef.current?.focus()
      }
    }

    document.addEventListener('mousedown', handlePointerDown)
    return () => {
      window.cancelAnimationFrame(frame)
      document.removeEventListener('mousedown', handlePointerDown)
    }
  }, [modelPickerOpen, modelOptions, selectedModel?.id])

  const selectAgent = (agentId: string) => {
    onSelectAgent(agentId)
    setAgentPickerOpen(false)
    triggerRef.current?.focus()
  }

  const handleAgentTriggerKeyDown = (event: React.KeyboardEvent<HTMLButtonElement>) => {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp' || event.key === 'Enter' || event.key === ' ') {
      event.preventDefault()
      setAgentPickerOpen(true)
    }
  }

  const handleAgentListKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'Escape') {
      event.preventDefault()
      setAgentPickerOpen(false)
      triggerRef.current?.focus()
      return
    }

    if (event.key === 'Tab') {
      setAgentPickerOpen(false)
      return
    }

    if (agents.length === 0) return

    const moveTo = (nextIndex: number) => {
      setActiveAgentIndex(nextIndex)
      optionRefs.current[nextIndex]?.focus()
    }

    if (event.key === 'ArrowDown') {
      event.preventDefault()
      moveTo((activeAgentIndex + 1) % agents.length)
    } else if (event.key === 'ArrowUp') {
      event.preventDefault()
      moveTo((activeAgentIndex - 1 + agents.length) % agents.length)
    } else if (event.key === 'Home') {
      event.preventDefault()
      moveTo(0)
    } else if (event.key === 'End') {
      event.preventDefault()
      moveTo(agents.length - 1)
    } else if ((event.key === 'Enter' || event.key === ' ') && agents[activeAgentIndex]) {
      event.preventDefault()
      selectAgent(agents[activeAgentIndex].id)
    }
  }

  React.useEffect(() => {
    attachmentsRef.current = attachments
  }, [attachments])

  React.useEffect(() => {
    return () => {
      attachmentsRef.current.forEach((attachment) => {
        if (attachment.kind === 'local') revokeLocalAttachmentPreview(attachment)
      })
    }
  }, [])

  const selectModel = (modelId: string) => {
    onSelectModel(modelId)
    setModelPickerOpen(false)
    modelTriggerRef.current?.focus()
  }

  const handleModelTriggerKeyDown = (event: React.KeyboardEvent<HTMLButtonElement>) => {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp' || event.key === 'Enter' || event.key === ' ') {
      event.preventDefault()
      if (modelOptions.length > 1) setModelPickerOpen(true)
    }
  }

  const handleModelListKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'Escape') {
      event.preventDefault()
      setModelPickerOpen(false)
      modelTriggerRef.current?.focus()
      return
    }

    if (event.key === 'Tab') {
      setModelPickerOpen(false)
      return
    }

    if (modelOptions.length === 0) return

    const moveTo = (nextIndex: number) => {
      setActiveModelIndex(nextIndex)
      modelOptionRefs.current[nextIndex]?.focus()
    }

    if (event.key === 'ArrowDown') {
      event.preventDefault()
      moveTo((activeModelIndex + 1) % modelOptions.length)
    } else if (event.key === 'ArrowUp') {
      event.preventDefault()
      moveTo((activeModelIndex - 1 + modelOptions.length) % modelOptions.length)
    } else if (event.key === 'Home') {
      event.preventDefault()
      moveTo(0)
    } else if (event.key === 'End') {
      event.preventDefault()
      moveTo(modelOptions.length - 1)
    } else if ((event.key === 'Enter' || event.key === ' ') && modelOptions[activeModelIndex]) {
      event.preventDefault()
      selectModel(modelOptions[activeModelIndex].id)
    }
  }

  return (
    <div className="shrink-0 border-t border-aurora-border-default bg-aurora-nav-bg px-3 py-2 sm:px-4 sm:py-3">
      <div
        className={cn(
          'relative flex flex-col gap-0 rounded-aurora-2 border border-aurora-border-strong',
          'bg-aurora-control-surface shadow-[var(--aurora-shadow-medium),var(--aurora-highlight-medium)]',
          'transition-shadow focus-within:shadow-[var(--aurora-shadow-medium),var(--aurora-active-glow)]',
          'focus-within:border-aurora-accent-primary/40',
        )}
      >
        {attachments.length > 0 && (
          <ul
            aria-label="Attached files"
            className="flex flex-wrap gap-1.5 border-b border-aurora-border-default px-3 pt-2 pb-1.5 sm:px-4"
          >
            {attachments.map((attachment) => (
              <li key={attachment.kind === 'local' ? attachment.id : attachment.path}>
                <AttachmentChip
                  attachment={attachment}
                  onRemove={() => removeAttachment(attachment)}
                />
              </li>
            ))}
          </ul>
        )}

        {attachmentError && (
          <p role="alert" className="border-b border-aurora-error/30 px-3 py-1.5 text-[11px] text-aurora-error sm:px-4">
            {attachmentError}
          </p>
        )}

        <textarea
          ref={textareaRef}
          name="chat-message"
          value={value}
          onChange={handleInput}
          onKeyDown={handleKeyDown}
          disabled={disabled || sending}
          aria-label="Message"
          placeholder={disabled ? (disabledReason ?? 'ACP provider unavailable…') : 'Message the assistant… (Shift+Enter for newline)'}
          rows={1}
          className={cn(
            'w-full resize-none bg-transparent px-3 pt-2.5 pb-1.5 text-[13px] leading-[1.55] sm:px-4 sm:pt-3 sm:pb-2',
            'text-aurora-text-primary placeholder:text-aurora-text-muted/50',
            'outline-none disabled:opacity-50',
          )}
          style={{ minHeight: `${CHAT_INPUT_MIN_HEIGHT_PX}px`, maxHeight: `${CHAT_INPUT_MAX_HEIGHT_PX}px` }}
        />

        <div className="flex items-center gap-2 px-2.5 pb-2 sm:gap-2.5 sm:px-3">
          <TooltipProvider delayDuration={400}>
            <div className="flex items-center gap-1">
              <DropdownMenu>
                <Tooltip>
                  <TooltipTrigger asChild>
                    <DropdownMenuTrigger asChild>
                      <Button
                        variant="ghost"
                        size="icon"
                        aria-label="Attach file"
                        disabled={disabled || sending}
                        className="size-7 rounded text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary"
                      >
                        <Paperclip className="size-3.5" />
                      </Button>
                    </DropdownMenuTrigger>
                  </TooltipTrigger>
                  <TooltipContent side="top" className="text-xs">Attach file</TooltipContent>
                </Tooltip>
                <DropdownMenuContent align="start" sideOffset={6} className="min-w-44">
                  <DropdownMenuItem
                    onSelect={(event) => {
                      event.preventDefault()
                      localFileInputRef.current?.click()
                    }}
                  >
                    <Paperclip className="size-3.5" />
                    <span>From device</span>
                  </DropdownMenuItem>
                  <DropdownMenuItem
                    onSelect={(event) => {
                      event.preventDefault()
                      setWorkspacePickerOpen(true)
                    }}
                  >
                    <FileText className="size-3.5" />
                    <span>From workspace</span>
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>

              <Tooltip>
                <TooltipTrigger asChild>
                  <Button variant="ghost" size="icon" aria-label="Tools" disabled className="size-7 rounded text-aurora-text-muted/50 hover:bg-aurora-hover-bg hover:text-aurora-text-muted">
                    <Wrench className="size-3.5" />
                  </Button>
                </TooltipTrigger>
                <TooltipContent side="top" className="text-xs">Tools</TooltipContent>
              </Tooltip>
            </div>
          </TooltipProvider>

          <div className="ml-auto flex min-w-0 items-center gap-1.5">
            {modelOptions.length > 0 && (
              <div ref={modelPickerRef} className="relative min-w-0">
                <button
                  ref={modelTriggerRef}
                  type="button"
                  onClick={() => modelOptions.length > 1 && setModelPickerOpen((open) => !open)}
                  onKeyDown={handleModelTriggerKeyDown}
                  aria-label={selectedModel ? `Selected model: ${selectedModel.name}` : 'Select model'}
                  aria-haspopup="listbox"
                  aria-expanded={modelPickerOpen}
                  aria-controls={modelPickerId}
                  disabled={disabled || sending || modelOptions.length <= 1}
                  className={cn(
                    'flex items-center gap-1.5 rounded-full border border-aurora-border-default',
                    'max-w-[8.5rem] bg-aurora-panel-medium px-2.5 py-1 text-[11px] font-medium text-aurora-text-muted sm:max-w-[12rem]',
                    'transition-colors hover:border-aurora-border-strong hover:text-aurora-text-primary',
                    (disabled || sending || modelOptions.length <= 1) && 'opacity-80 hover:border-aurora-border-default',
                  )}
                >
                  <span className="truncate">{selectedModel?.name ?? modelOptions[0]?.name ?? 'Model'}</span>
                  {modelOptions.length > 1 && <ChevronDown className="size-3 shrink-0" />}
                </button>

                {modelPickerOpen && (
                  <div
                    id={modelPickerId}
                    role="listbox"
                    aria-label="Model picker"
                    aria-activedescendant={modelOptions[activeModelIndex] ? `${modelPickerId}-${modelOptions[activeModelIndex].id}` : undefined}
                    onKeyDown={handleModelListKeyDown}
                    className={cn(
                      'absolute bottom-full right-0 z-50 mb-1.5 min-w-[180px] overflow-hidden',
                      'rounded-aurora-2 border border-aurora-border-strong bg-aurora-panel-strong',
                      'shadow-[var(--aurora-shadow-strong),var(--aurora-highlight-strong)]',
                    )}
                  >
                    {modelOptions.map((model, index) => (
                      <button
                        key={model.id}
                        id={`${modelPickerId}-${model.id}`}
                        ref={(node) => {
                          modelOptionRefs.current[index] = node
                        }}
                        type="button"
                        role="option"
                        aria-selected={selectedModel?.id === model.id}
                        tabIndex={index === activeModelIndex ? 0 : -1}
                        onFocus={() => setActiveModelIndex(index)}
                        onClick={() => selectModel(model.id)}
                        className={cn(
                          'flex w-full flex-col gap-0.5 px-3 py-2 text-left transition-colors hover:bg-aurora-hover-bg',
                          selectedModel?.id === model.id && 'bg-aurora-panel-medium',
                        )}
                      >
                        <span className="text-[13px] font-medium text-aurora-text-primary">{model.name}</span>
                        {model.description && <span className="text-[11px] text-aurora-text-muted">{model.description}</span>}
                      </button>
                    ))}
                  </div>
                )}
              </div>
            )}

            <div ref={pickerRef} className="relative min-w-0">
            <button
              ref={triggerRef}
              type="button"
              onClick={() => setAgentPickerOpen((open) => !open)}
              onKeyDown={handleAgentTriggerKeyDown}
              aria-label={selectedAgent ? `Selected agent: ${selectedAgent.name}` : 'Select agent'}
              aria-haspopup="listbox"
              aria-expanded={agentPickerOpen}
              aria-controls={pickerId}
              className={cn(
                'flex items-center gap-1.5 rounded-full border border-aurora-border-default',
                'max-w-[8.5rem] bg-aurora-panel-medium px-2.5 py-1 text-[11px] font-medium text-aurora-text-muted sm:max-w-[12rem]',
                'transition-colors hover:border-aurora-border-strong hover:text-aurora-text-primary',
              )}
            >
              <span className="truncate">{selectedAgent?.name ?? 'Select agent'}</span>
              <ChevronDown className="size-3 shrink-0" />
            </button>

            {agentPickerOpen && (
              <div
                id={pickerId}
                role="listbox"
                aria-label="Agent picker"
                aria-activedescendant={agents[activeAgentIndex] ? `${pickerId}-${agents[activeAgentIndex].id}` : undefined}
                onKeyDown={handleAgentListKeyDown}
                className={cn(
                  'absolute bottom-full right-0 z-50 mb-1.5 min-w-[200px] overflow-hidden',
                  'rounded-aurora-2 border border-aurora-border-strong bg-aurora-panel-strong',
                  'shadow-[var(--aurora-shadow-strong),var(--aurora-highlight-strong)]',
                )}
              >
                {agents.map((agent, index) => (
                  <button
                    key={agent.id}
                    id={`${pickerId}-${agent.id}`}
                    ref={(node) => {
                      optionRefs.current[index] = node
                    }}
                    type="button"
                    role="option"
                    aria-selected={selectedAgent?.id === agent.id}
                    tabIndex={index === activeAgentIndex ? 0 : -1}
                    onFocus={() => setActiveAgentIndex(index)}
                    onClick={() => selectAgent(agent.id)}
                    className={cn(
                      'flex w-full flex-col gap-0.5 px-3 py-2 text-left transition-colors hover:bg-aurora-hover-bg',
                      selectedAgent?.id === agent.id && 'bg-aurora-panel-medium',
                    )}
                  >
                    <span className="text-[13px] font-medium text-aurora-text-primary">{agent.name}</span>
                    <span className="text-[11px] text-aurora-text-muted">{agent.description}</span>
                  </button>
                ))}
              </div>
            )}
            </div>

            <Button
              onClick={() => void handleSend()}
              disabled={!hasContent || disabled || sending}
              size="icon"
              aria-label="Send message"
              className={cn(
                'size-8 shrink-0 rounded-aurora-1 transition-all',
                hasContent && !disabled && !sending
                  ? 'bg-aurora-accent-primary text-aurora-page-bg hover:bg-aurora-accent-strong'
                  : 'bg-aurora-border-default text-aurora-text-muted/40',
              )}
            >
              <Send className="size-3.5" />
            </Button>
          </div>
        </div>
        <input
          ref={localFileInputRef}
          type="file"
          multiple
          accept="text/*,application/json,application/pdf,image/png,image/jpeg,image/gif,image/webp"
          className="sr-only"
          aria-label="Local file picker"
          onChange={(event) => handleLocalFiles(event.currentTarget.files)}
        />
      </div>

      <p className="mt-1.5 px-1 text-center text-[10px] text-aurora-text-muted/40 sm:text-[11px]">
        Assistant may make mistakes. Verify important information.
      </p>

      <WorkspacePicker
        open={workspacePickerOpen}
        onOpenChange={setWorkspacePickerOpen}
        onSelect={handleAttach}
      />
    </div>
  )
}

/**
 * Single attachment chip. For image attachments, fetches
 * `/v1/fs/preview` once and renders the returned bytes via
 * `URL.createObjectURL(blob)` — the blob URL is revoked on unmount. The
 * bytes are backend-approved (deny-list + 2 MiB cap + MIME whitelist),
 * which is why blob-URL usage is acceptable here — unlike the banned
 * pattern of blob URLs over user-supplied `File` objects.
 */
function AttachmentChip({
  attachment,
  onRemove,
}: {
  attachment: AttachmentRef
  onRemove: () => void
}) {
  const label = attachment.kind === 'local' ? attachment.file.name : attachment.path
  // Bundle the URL with the path it was fetched for. Rendering gates on
  // forPath === attachment.path, so a revoked-but-not-yet-replaced URL from
  // a prior path never lands in the DOM during a swap.
  const [thumb, setThumb] = React.useState<{ url: string; forPath: string } | null>(null)

  React.useEffect(() => {
    if (attachment.kind === 'local') return undefined

    const controller = new AbortController()
    let objectUrl: string | null = null
    let disposed = false

    previewWorkspaceFile(attachment.path, { signal: controller.signal })
      .then(({ blob, contentType }) => {
        if (disposed || controller.signal.aborted) return
        if (!isInlineImageMime(contentType)) return
        const url = URL.createObjectURL(blob)
        if (disposed || controller.signal.aborted) {
          URL.revokeObjectURL(url)
          return
        }
        objectUrl = url
        setThumb({ url, forPath: attachment.path })
      })
      .catch(() => {})

    return () => {
      disposed = true
      controller.abort()
      if (objectUrl) URL.revokeObjectURL(objectUrl)
    }
  }, [attachment])

  return (
    <span
      className={cn(
        'inline-flex items-center gap-1.5 rounded-full border border-aurora-border-default',
        'bg-aurora-panel-medium px-2 py-0.5 text-[11px] text-aurora-text-primary',
      )}
    >
      {attachment.kind === 'local' && attachment.previewUrl ? (
        <Image
          src={attachment.previewUrl}
          alt=""
          className="size-4 rounded-[2px] object-cover"
          height={16}
          width={16}
          unoptimized
        />
      ) : thumb && attachment.kind === 'file' && thumb.forPath === attachment.path ? (
        <Image
          src={thumb.url}
          alt=""
          className="size-4 rounded-[2px] object-cover"
          height={16}
          width={16}
          unoptimized
        />
      ) : (
        <FileText className="size-3 text-aurora-text-muted" />
      )}
      <span className="max-w-[18rem] truncate" title={label}>{label}</span>
      <button
        type="button"
        onClick={onRemove}
        aria-label={`Remove ${label}`}
        className="text-aurora-text-muted hover:text-aurora-text-primary"
      >
        <X className="size-3" />
      </button>
    </span>
  )
}
