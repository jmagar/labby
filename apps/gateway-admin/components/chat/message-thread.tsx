'use client'

import * as React from 'react'
import { Loader2, MessageSquare, ShieldQuestion } from 'lucide-react'
import { cn } from '@/lib/utils'
import { AURORA_CARD_TITLE, AURORA_MUTED_LABEL } from '@/components/aurora/tokens'
import { ScrollArea } from '@/components/ui/scroll-area'
import { MessageBubble } from './message-bubble'
import type { ACPMessage, ACPRun } from './types'
import type { SessionEventConnectionState } from '@/lib/chat/use-session-events'

interface MessageThreadProps {
  run: ACPRun | null
  messages: ACPMessage[]
  connectionState?: SessionEventConnectionState
  canRetryMessages?: boolean
  canEditMessages?: boolean
  onRetryMessage?: (message: ACPMessage) => void
  onEditMessage?: (message: ACPMessage) => void
}

function EmptyState() {
  return (
    <div className="flex flex-1 items-center justify-center px-4 py-8 sm:px-6 sm:py-10">
      <div className="w-full max-w-sm rounded-aurora-2 border border-aurora-border-strong bg-aurora-panel-medium p-5 text-center shadow-[var(--aurora-shadow-medium),var(--aurora-highlight-medium)]">
        <div className="mx-auto flex size-12 items-center justify-center rounded-aurora-2 border border-aurora-border-default bg-aurora-panel-strong">
          <MessageSquare className="size-5 text-aurora-text-muted/50" />
        </div>
        <div className="mt-4 space-y-1.5">
          <p className={cn(AURORA_MUTED_LABEL)}>Conversation</p>
          <p className={cn(AURORA_CARD_TITLE, 'text-aurora-text-primary')}>No session selected</p>
          <p className="text-[13px] leading-[1.55] text-aurora-text-muted">
            Open the sessions drawer or start a new run to begin a chat.
          </p>
        </div>
      </div>
    </div>
  )
}

function SessionStatusNotice({ run, connectionState }: { run: ACPRun; connectionState?: SessionEventConnectionState }) {
  if (run.status !== 'running' && run.status !== 'waiting_for_permission') {
    return null
  }
  // Wait until SSE is open — avoids a false "still running" notice during the
  // initial load window before events have been replayed from the server.
  if (connectionState !== 'open') {
    return null
  }

  const waitingForPermission = run.status === 'waiting_for_permission'
  const Icon = waitingForPermission ? ShieldQuestion : Loader2

  return (
    <div className="rounded-aurora-2 border border-aurora-accent-primary/18 bg-aurora-accent-deep/12 px-3 py-2.5 shadow-[var(--aurora-highlight-medium)]">
      <div className="flex items-center gap-2">
        <Icon
          className={cn(
            'size-3.5 shrink-0 text-aurora-accent-primary',
            !waitingForPermission && 'animate-spin',
          )}
        />
        <span className="text-[12px] font-medium text-aurora-text-primary">
          {waitingForPermission ? 'Waiting for permission' : 'Running'}
        </span>
        <span className="ml-auto text-[11px] text-aurora-text-muted/60">
          {waitingForPermission
            ? 'Approve or reject in the activity panel'
            : 'Response in progress…'}
        </span>
      </div>
    </div>
  )
}

export function MessageThread({
  run,
  messages,
  connectionState,
  canRetryMessages = false,
  canEditMessages = false,
  onRetryMessage,
  onEditMessage,
}: MessageThreadProps) {
  const bottomRef = React.useRef<HTMLDivElement>(null)
  const threadRef = React.useRef<HTMLDivElement>(null)
  const [selectedMessageId, setSelectedMessageId] = React.useState<string | null>(null)

  React.useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' })
  }, [messages])

  React.useEffect(() => {
    if (!selectedMessageId) return

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setSelectedMessageId(null)
    }

    const onPointerDown = (event: PointerEvent) => {
      const target = event.target
      if (target instanceof Node && !threadRef.current?.contains(target)) {
        setSelectedMessageId(null)
      }
    }

    document.addEventListener('keydown', onKeyDown)
    document.addEventListener('pointerdown', onPointerDown)
    return () => {
      document.removeEventListener('keydown', onKeyDown)
      document.removeEventListener('pointerdown', onPointerDown)
    }
  }, [selectedMessageId])

  if (!run) {
    return <EmptyState />
  }

  return (
    <ScrollArea className="min-h-0 min-w-0 flex-1 overflow-hidden">
      <div
        ref={threadRef}
        className="mx-auto flex w-full max-w-[860px] min-w-0 flex-col gap-4 px-3 py-4 sm:gap-5 sm:px-6 sm:py-6"
      >
        <SessionStatusNotice run={run} connectionState={connectionState} />
        {messages.map((message) => (
          <MessageBubble
            key={message.id}
            message={message}
            actionState={{
              selected: selectedMessageId === message.id,
              canRetry: canRetryMessages,
              canEdit: canEditMessages,
            }}
            actionHandlers={{
              onSelect: setSelectedMessageId,
              onDismiss: () => setSelectedMessageId(null),
              onRetry: onRetryMessage,
              onEdit: onEditMessage,
            }}
          />
        ))}
        <div ref={bottomRef} />
      </div>
    </ScrollArea>
  )
}
