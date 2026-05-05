'use client'

import * as React from 'react'
import { Bot, Check, ChevronDown, Copy, ListChecks, Pencil, RotateCcw, UserRound } from 'lucide-react'
import {
  Streamdown,
  defaultUrlTransform,
  type AllowElement,
  type UrlTransform,
} from 'streamdown'
import { cn } from '@/lib/utils'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import {
  ChainOfThought,
  ChainOfThoughtContent,
  ChainOfThoughtHeader,
} from '@/components/ai/chain-of-thought'
import { Reasoning, ReasoningContent, ReasoningTrigger } from '@/components/ai/reasoning'
import { formatUiDateTime, formatUiTime } from '@/lib/format-ui-time'
import { ToolCallDisplay } from './tool-call-display'
import { GroupedToolCallDisplay, groupConsecutiveToolCalls } from './grouped-tool-call-display'
import { getToolCategory } from './tool-call-presentation'
import type { ACPMessage } from './types'

type CopyState = 'idle' | 'copied' | 'failed'

function CopyButton({ text }: { text: string }) {
  const [copyState, setCopyState] = React.useState<CopyState>('idle')

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(text)
      setCopyState('copied')
      window.setTimeout(() => setCopyState('idle'), 2000)
    } catch {
      setCopyState('failed')
      window.setTimeout(() => setCopyState('idle'), 2000)
    }
  }

  const label =
    copyState === 'copied'
      ? 'Copied message'
      : copyState === 'failed'
        ? 'Copy failed'
        : 'Copy message'

  return (
    <Button
      variant="ghost"
      size="icon"
      onClick={handleCopy}
      aria-label={label}
      className="size-7 shrink-0 rounded text-aurora-text-muted/70 hover:bg-aurora-hover-bg hover:text-aurora-text-primary"
    >
      {copyState === 'copied' ? <Check className="size-3.5 text-aurora-success" /> : <Copy className="size-3.5" />}
      <span className="sr-only">{label}</span>
    </Button>
  )
}

export function getMessageCopyText(message: Pick<ACPMessage, 'text'>) {
  return message.text
}

export type MessageActionAvailabilityInput = {
  canRetry?: boolean
  canEdit?: boolean
}

export type MessageActionAvailability = {
  copy: boolean
  retry: boolean
  edit: boolean
}

export function getMessageActionAvailability(
  message: Pick<ACPMessage, 'role' | 'text' | 'isStreaming'>,
  input: MessageActionAvailabilityInput = {},
): MessageActionAvailability {
  const hasText = message.text.trim().length > 0
  const isUser = message.role === 'user'
  const isStable = !message.isStreaming

  return {
    copy: hasText,
    retry: hasText && isUser && isStable && Boolean(input.canRetry),
    edit: hasText && isUser && isStable && Boolean(input.canEdit),
  }
}

export type MessageBubbleActionState = {
  selected?: boolean
  canRetry?: boolean
  canEdit?: boolean
}

export type MessageBubbleActionHandlers = {
  onSelect?: (messageId: string) => void
  onRetry?: (message: ACPMessage) => void
  onEdit?: (message: ACPMessage) => void
}

export type MessageTimestampLabels = {
  visible: string
  detail: string
}

function hasValidDate(value: Date) {
  return !Number.isNaN(value.getTime())
}

export function shouldRenderMessageTimestamp(message: Pick<ACPMessage, 'createdAt'>) {
  return hasValidDate(message.createdAt)
}

export function getMessageTimestampLabels(
  message: Pick<ACPMessage, 'createdAt'>,
): MessageTimestampLabels {
  return {
    visible: formatUiTime(message.createdAt),
    detail: formatUiDateTime(message.createdAt),
  }
}

function StreamingCursor() {
  return (
    <span
      aria-hidden="true"
      className="ml-0.5 inline-block h-3.5 w-0.5 animate-pulse rounded-sm bg-aurora-accent-primary align-middle"
    />
  )
}

const SAFE_MARKDOWN_IMAGE_ELEMENTS = ['img'] as const
const NO_REHYPE_PLUGINS: never[] = []
const DISABLED_LINK_SAFETY = { enabled: false } as const
const MESSAGE_CONTENT_CLASS =
  'relative max-w-full overflow-hidden rounded-aurora-2 px-4 py-3'
const ASSISTANT_MESSAGE_CONTENT_CLASS =
  'border border-aurora-border-default bg-aurora-panel-medium shadow-[var(--aurora-shadow-medium),var(--aurora-highlight-medium)]'
const USER_MESSAGE_CONTENT_CLASS =
  'border border-aurora-border-strong bg-aurora-panel-strong shadow-[var(--aurora-shadow-medium),var(--aurora-highlight-medium)]'

function isAllowedMarkdownUrl(url: string) {
  const trimmed = url.trim()
  if (trimmed.startsWith('//')) return false

  const scheme = trimmed.match(/^([a-z][a-z0-9+.-]*):/i)?.[1]?.toLowerCase()
  return !scheme || scheme === 'http' || scheme === 'https' || scheme === 'mailto'
}

const safeMarkdownUrlTransform: UrlTransform = (url, key, node) => {
  const transformed = defaultUrlTransform(url, key, node)
  if (!transformed) return transformed

  return isAllowedMarkdownUrl(transformed) ? transformed : null
}

const allowSafeMarkdownElement: AllowElement = (element) => {
  if (element.tagName === 'img') return false

  if (element.tagName === 'a') {
    const href = element.properties?.href
    return typeof href === 'string' && isAllowedMarkdownUrl(href)
  }

  return true
}

function AssistantMarkdown({
  text,
  isStreaming,
}: {
  text: string
  isStreaming: boolean
}) {
  return (
    <div className="min-w-0 max-w-full overflow-hidden pr-8 text-[13px] leading-[1.55] text-aurora-text-primary [overflow-wrap:anywhere] [&_a]:break-words [&_code]:break-words [&_li]:min-w-0 [&_ol]:min-w-0 [&_pre]:max-w-full [&_pre]:overflow-x-auto [&_pre]:[overflow-wrap:normal] [&_pre]:whitespace-pre [&_table]:block [&_table]:max-w-full [&_table]:overflow-x-auto [&_ul]:min-w-0">
      <Streamdown
        mode={isStreaming ? 'streaming' : 'static'}
        skipHtml
        rehypePlugins={NO_REHYPE_PLUGINS}
        disallowedElements={SAFE_MARKDOWN_IMAGE_ELEMENTS}
        allowElement={allowSafeMarkdownElement}
        urlTransform={safeMarkdownUrlTransform}
        controls={false}
        linkSafety={DISABLED_LINK_SAFETY}
        lineNumbers={false}
      >
        {text}
      </Streamdown>
      {isStreaming ? <StreamingCursor /> : null}
    </div>
  )
}

function AgentActionsPanel({
  open,
  onOpenChange,
  toolCalls,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  toolCalls: ACPMessage['toolCalls']
}) {
  return (
    <div className="w-full overflow-hidden rounded-aurora-3 border border-aurora-border-default bg-aurora-panel-medium shadow-[var(--aurora-shadow-strong),var(--aurora-highlight-strong)]">
      <Collapsible open={open} onOpenChange={onOpenChange}>
        <CollapsibleTrigger
          className="flex w-full items-center gap-2 px-4 py-3 text-[13px] font-medium text-aurora-text-muted transition-colors hover:text-aurora-text-primary"
        >
          <ListChecks className="size-4" />
          <span className="flex-1 text-left">Agent Actions</span>
          <span className="rounded-full border border-aurora-border-default bg-aurora-control-surface px-2 py-0.5 text-[10px] font-semibold text-aurora-text-muted">
            {toolCalls.length}
          </span>
          <ChevronDown className={cn('size-4 transition-transform', open ? 'rotate-180' : 'rotate-0')} />
        </CollapsibleTrigger>
        <CollapsibleContent className="px-4 pb-3">
          <div className="space-y-1">
            {groupConsecutiveToolCalls(toolCalls, getToolCategory).map((entry) =>
              entry.type === 'group' ? (
                <GroupedToolCallDisplay
                  key={`group-${entry.toolCalls[0]!.id}`}
                  category={entry.category}
                  toolCalls={entry.toolCalls}
                />
              ) : (
                <ToolCallDisplay key={entry.toolCall.id} toolCall={entry.toolCall} />
              ),
            )}
          </div>
        </CollapsibleContent>
      </Collapsible>
    </div>
  )
}

function MessageTimestamp({
  message,
  selected,
}: {
  message: ACPMessage
  selected?: boolean
}) {
  if (!shouldRenderMessageTimestamp(message)) return null

  const labels = getMessageTimestampLabels(message)

  return (
    <div
      data-message-timestamp
      aria-label={`Message sent at ${labels.detail}`}
      title={labels.detail}
      className={cn(
        'min-h-4 text-[11px] leading-4 text-aurora-text-muted/60 transition-opacity duration-150',
        'opacity-0 group-hover/bubble:opacity-100 group-focus-within/bubble:opacity-100',
        selected && 'opacity-100',
      )}
    >
      {labels.visible}
    </div>
  )
}

export function WorkingAssistantBubble({ label = 'Codex is working' }: { label?: string }) {
  return (
    <div className="group/bubble flex min-w-0 gap-3">
      <div className="mt-1 flex size-6 shrink-0 items-center justify-center rounded-full border border-aurora-accent-primary/30 bg-aurora-accent-deep/18">
        <Bot className="size-3 text-aurora-accent-primary" />
      </div>

      <div className="flex min-w-0 max-w-[calc(100%-2.25rem)] flex-col gap-2.5 sm:max-w-[80%]">
        <div
          role="status"
          aria-label={label}
          className="relative max-w-full overflow-hidden rounded-aurora-2 border border-aurora-border-default bg-aurora-panel-medium px-4 py-3 shadow-[var(--aurora-shadow-medium),var(--aurora-highlight-medium)]"
        >
          <span
            aria-hidden="true"
            className="absolute inset-y-0 left-0 w-[2px] rounded-l-aurora-2 bg-aurora-accent-primary/40"
          />

          <div className="flex min-w-0 items-center gap-2 pr-1 text-[13px] leading-[1.55] text-aurora-text-primary">
            <span className="font-medium">{label}</span>
            <span className="inline-flex shrink-0 items-center gap-1" aria-hidden="true">
              <span className="size-1.5 animate-pulse rounded-full bg-aurora-accent-primary motion-reduce:animate-none" />
              <span className="size-1.5 animate-pulse rounded-full bg-aurora-accent-primary/70 delay-150 motion-reduce:animate-none" />
              <span className="size-1.5 animate-pulse rounded-full bg-aurora-accent-primary/40 delay-300 motion-reduce:animate-none" />
            </span>
          </div>

          <div className="mt-3 space-y-1.5" aria-hidden="true">
            <div className="h-2 rounded-full bg-aurora-accent-primary/15" />
            <div className="h-2 rounded-full bg-aurora-accent-primary/10" />
            <div className="h-2 w-7/12 rounded-full bg-aurora-accent-primary/10" />
          </div>
        </div>
      </div>
    </div>
  )
}

function MessageActionToolbar({
  message,
  availability,
  selected,
  onRetry,
  onEdit,
}: {
  message: ACPMessage
  availability: MessageActionAvailability
  selected: boolean
  onRetry?: (message: ACPMessage) => void
  onEdit?: (message: ACPMessage) => void
}) {
  if (!availability.copy && !availability.retry && !availability.edit) {
    return null
  }

  return (
    <div
      aria-label="Message actions"
      role="group"
      data-selected={selected ? 'true' : 'false'}
      className={cn(
        'flex w-full justify-end gap-1 pr-1 transition-opacity',
        selected
          ? 'opacity-100'
          : 'opacity-0 group-hover/bubble:opacity-100 group-focus-within/bubble:opacity-100',
      )}
    >
      {availability.copy ? <CopyButton text={getMessageCopyText(message)} /> : null}
      {availability.retry ? (
        <Button
          variant="ghost"
          size="icon"
          aria-label="Retry message"
          className="size-7 rounded text-aurora-text-muted/70 hover:bg-aurora-hover-bg hover:text-aurora-text-primary"
          onClick={() => onRetry?.(message)}
        >
          <RotateCcw className="size-3.5" />
        </Button>
      ) : null}
      {availability.edit ? (
        <Button
          variant="ghost"
          size="icon"
          aria-label="Edit message"
          className="size-7 rounded text-aurora-text-muted/70 hover:bg-aurora-hover-bg hover:text-aurora-text-primary"
          onClick={() => onEdit?.(message)}
        >
          <Pencil className="size-3.5" />
        </Button>
      ) : null}
    </div>
  )
}

function MessageBubbleComponent({
  message,
  actionState = {},
  actionHandlers = {},
}: {
  message: ACPMessage
  actionState?: MessageBubbleActionState
  actionHandlers?: MessageBubbleActionHandlers
}) {
  const isUser = message.role === 'user'
  const isStreaming = Boolean(message.isStreaming)
  const [reasoningOpen, setReasoningOpen] = React.useState(isStreaming)
  const [chainOpen, setChainOpen] = React.useState(isStreaming)
  const [actionsOpen, setActionsOpen] = React.useState(isStreaming)

  React.useEffect(() => {
    setReasoningOpen(isStreaming)
    setChainOpen(isStreaming)
    setActionsOpen(isStreaming || message.toolCalls.length > 0)
  }, [isStreaming, message.toolCalls.length])

  const hasReasoning = !isUser && message.thoughts.length > 0
  const hasActions = !isUser && message.toolCalls.length > 0
  const messageContentClass = cn(
    MESSAGE_CONTENT_CLASS,
    isUser ? USER_MESSAGE_CONTENT_CLASS : ASSISTANT_MESSAGE_CONTENT_CLASS,
  )

  return (
    <div
      data-message-id={message.id}
      className={cn('group/bubble flex min-w-0 gap-3', isUser && 'flex-row-reverse')}
      onClick={(event) => {
        const target = event.target as HTMLElement
        if (target.closest('button,a,input,textarea,select,[role="button"]')) return
        actionHandlers.onSelect?.(message.id)
      }}
    >
      <div
        className={cn(
          'mt-1 flex size-6 shrink-0 items-center justify-center rounded-full border',
          isUser
            ? 'border-aurora-border-strong bg-aurora-panel-strong'
            : 'border-aurora-accent-primary/30 bg-aurora-accent-deep/18',
        )}
      >
        {isUser ? (
          <UserRound className="size-3 text-aurora-text-muted" />
        ) : (
          <Bot className="size-3 text-aurora-accent-primary" />
        )}
      </div>

      <div className={cn('flex min-w-0 max-w-[calc(100%-2.25rem)] flex-col gap-2.5 sm:max-w-[80%]', isUser && 'items-end')}>
        {hasReasoning && (
          <div className="w-full overflow-hidden rounded-aurora-3 border border-aurora-border-default bg-aurora-panel-medium shadow-[var(--aurora-shadow-strong),var(--aurora-highlight-strong)]">
            <ChainOfThought
              open={chainOpen}
              onOpenChange={setChainOpen}
              className="px-4 py-3"
            >
              <ChainOfThoughtHeader className="font-medium text-aurora-text-muted">
                Reasoning Summary
              </ChainOfThoughtHeader>
              <ChainOfThoughtContent className="pt-1">
                <div className="rounded-aurora-2 border border-aurora-border-default/70 bg-aurora-control-surface px-3 py-3">
                  <Reasoning
                    isStreaming={isStreaming}
                    open={reasoningOpen}
                    onOpenChange={setReasoningOpen}
                    className="mb-0"
                  >
                    <ReasoningTrigger
                      className="text-aurora-text-muted"
                      getThinkingMessage={(isStreaming, duration) => {
                        if (isStreaming || duration === 0) return <span className="animate-pulse">Reasoning…</span>
                        if (duration === undefined) return <span>Reasoning</span>
                        return <span>Reasoned for {duration} seconds</span>
                      }}
                    />
                    <ReasoningContent className="mt-3 space-y-3 text-aurora-text-primary">
                      {message.thoughts.join('\n\n')}
                    </ReasoningContent>
                  </Reasoning>
                </div>
              </ChainOfThoughtContent>
            </ChainOfThought>
          </div>
        )}

        {hasActions && (
          <AgentActionsPanel
            open={actionsOpen}
            onOpenChange={setActionsOpen}
            toolCalls={message.toolCalls}
          />
        )}

        {message.text && (
          <div className={messageContentClass}>
            {!isUser && (
              <span
                aria-hidden="true"
                className="absolute inset-y-0 left-0 w-[2px] rounded-l-aurora-2 bg-aurora-accent-primary/40"
              />
            )}
            {isUser ? (
              <p className="min-w-0 whitespace-pre-wrap text-[13px] leading-[1.55] text-aurora-text-primary [overflow-wrap:anywhere]">
                {message.text}
                {message.isStreaming ? <StreamingCursor /> : null}
              </p>
            ) : (
              <AssistantMarkdown text={message.text} isStreaming={isStreaming} />
            )}
          </div>
        )}
        <MessageTimestamp message={message} selected={Boolean(actionState.selected)} />
        <MessageActionToolbar
          message={message}
          availability={getMessageActionAvailability(message, actionState)}
          selected={Boolean(actionState.selected)}
          onRetry={actionHandlers.onRetry}
          onEdit={actionHandlers.onEdit}
        />
      </div>
    </div>
  )
}

export function areMessageBubblePropsEqual(
  previous: Readonly<{
    message: ACPMessage
    actionState?: MessageBubbleActionState
    actionHandlers?: MessageBubbleActionHandlers
  }>,
  next: Readonly<{
    message: ACPMessage
    actionState?: MessageBubbleActionState
    actionHandlers?: MessageBubbleActionHandlers
  }>,
) {
  const prev = previous.message
  const current = next.message

  return (
    prev.id === current.id &&
    prev.role === current.role &&
    prev.text === current.text &&
    Object.is(prev.createdAt.getTime(), current.createdAt.getTime()) &&
    prev.isStreaming === current.isStreaming &&
    prev.version === current.version &&
    prev.thoughts.length === current.thoughts.length &&
    prev.toolCalls.length === current.toolCalls.length &&
    previous.actionState?.selected === next.actionState?.selected &&
    previous.actionState?.canRetry === next.actionState?.canRetry &&
    previous.actionState?.canEdit === next.actionState?.canEdit &&
    previous.actionHandlers?.onSelect === next.actionHandlers?.onSelect &&
    previous.actionHandlers?.onRetry === next.actionHandlers?.onRetry &&
    previous.actionHandlers?.onEdit === next.actionHandlers?.onEdit
  )
}

export const MessageBubble = React.memo(MessageBubbleComponent, areMessageBubblePropsEqual)
