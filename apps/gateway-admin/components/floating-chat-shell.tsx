'use client'

/**
 * FloatingChatShell — Wires all /chat features into the floating chat popover.
 *
 * Mounted by the popover host after the floating chat is first opened.
 * Consumes the 4 ChatSession contexts provided by ChatSessionProvider.
 *
 * Lifecycle:
 * - Provider starts SSE when floating chat is enabled.
 * - This shell stays mounted after the first open so chat state persists.
 * - Visibility is controlled by the popover host when closed.
 * - The host suppresses floating chat on /chat to avoid a duplicate stream.
 *
 * pageContext:
 * - Reads sendPageContext from config prop
 * - When enabled AND pageContext is non-null: includes pageContext in prompt body
 * - When disabled or null: omits field entirely (zero token cost)
 */

import * as React from 'react'
import { AlertCircle, Plus, SidebarOpen, Zap } from 'lucide-react'
import { toast } from 'sonner'
import { cn } from '@/lib/utils'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'
import { SessionSidebar } from '@/components/chat/session-sidebar'
import { MessageThread } from '@/components/chat/message-thread'
import { ChatInput } from '@/components/chat/chat-input'
import {
  useChatSessionData,
  useChatSessionActions,
  useChatSessionConnection,
  useChatSessionStream,
} from '@/lib/chat/chat-session-provider'
import type { ChatConfig } from '@/components/floating-chat-popover'
import type { ChatInputPayload } from '@/components/chat/chat-input'

export type FloatingChatShellProps = {
  config?: ChatConfig
}

export function FloatingChatShell({
  config,
}: FloatingChatShellProps) {
  // ---- Context consumers ----
  const {
    visibleRuns,
    hiddenRunCount,
    includeHiddenRuns,
    selectedRun,
    selectedRunId,
    providerHealth,
    selectedAgent,
    agents,
    projects,
    pageContext,
  } = useChatSessionData()
  const {
    createSession,
    selectRun,
    sendPrompt: sendPromptAction,
    selectAgent,
    setIncludeHiddenRuns,
    bulkCloseHiddenSessions,
  } = useChatSessionActions()
  const { connectionState } = useChatSessionConnection()
  const { messages } = useChatSessionStream()

  // ---- Local state ----
  const [sessionPanelOpen, setSessionPanelOpen] = React.useState(false)
  const [lastActionError, setLastActionError] = React.useState<string | null>(null)

  const providerReady = Boolean(providerHealth?.ready)
  const visibleError = lastActionError ?? (!providerReady ? providerHealth?.message : null)

  // ---- sendPrompt (reads pageContext from provider + config) ----
  const sendPrompt = React.useCallback(
    async (payload: ChatInputPayload) => {
      setLastActionError(null)
      try {
        await sendPromptAction(
          payload,
          {
            pageContext,
            includePageContext: Boolean(config?.sendPageContext),
          },
        )
      } catch (error) {
        const message = messageFromUnknownError(error, 'Failed to send prompt to ACP session.')
        setLastActionError(message)
        toast.error(message)
      }
    },
    [
      config?.sendPageContext,
      pageContext,
      sendPromptAction,
    ],
  )

  const createRun = React.useCallback(async () => {
    setLastActionError(null)
    try {
      await createSession()
    } catch (error) {
      const message = messageFromUnknownError(error, 'Failed to create ACP session.')
      setLastActionError(message)
      toast.error(message)
    }
  }, [createSession])

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
      {/* ---- Compact header (48px) ---- */}
      <header className="flex h-12 shrink-0 items-center gap-2 border-b border-aurora-border-default bg-aurora-nav-bg px-2.5">
        <TooltipProvider delayDuration={400}>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon"
                aria-label="Toggle sessions"
                onClick={() => setSessionPanelOpen((open) => !open)}
                className={cn(
                  'size-7 rounded text-aurora-text-muted/60 hover:bg-aurora-hover-bg hover:text-aurora-text-primary',
                  sessionPanelOpen && 'bg-aurora-hover-bg text-aurora-text-primary',
                )}
              >
                <SidebarOpen className="size-3.5" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom" className="text-xs">Toggle sessions</TooltipContent>
          </Tooltip>
        </TooltipProvider>

        {selectedRun && (
          <div className="ml-1 flex min-w-0 flex-1 items-center gap-1.5 text-[12px] text-aurora-text-muted">
            <span className="max-w-[160px] truncate text-aurora-text-primary">{selectedRun.title}</span>
          </div>
        )}

        <div className="ml-auto flex items-center gap-1.5">
          <TooltipProvider delayDuration={400}>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon"
                  aria-label="Start new session"
                  onClick={() => void createRun()}
                  disabled={!providerReady}
                  className="size-7 rounded text-aurora-text-muted/60 hover:bg-aurora-hover-bg hover:text-aurora-text-primary"
                >
                  <Plus className="size-3.5" />
                </Button>
              </TooltipTrigger>
              <TooltipContent side="bottom" className="text-xs">New session</TooltipContent>
            </Tooltip>
          </TooltipProvider>

          <div
            className="flex items-center gap-1 rounded-full border border-aurora-border-default bg-aurora-control-surface px-1.5 py-0.5"
            title={providerReady ? 'ACP live' : 'ACP unavailable'}
          >
            <Zap
              className={cn(
                'size-3',
                providerReady ? 'text-aurora-accent-primary/70' : 'text-aurora-text-muted/40',
              )}
            />
            <span className="text-[11px] text-aurora-text-muted">
              {providerReady ? 'ACP' : '—'}
            </span>
          </div>

          {(connectionState === 'connecting' || connectionState === 'error') && (
            <div
              title={connectionState === 'error' ? 'Stream disconnected — reconnecting…' : 'Connecting…'}
              className={cn(
                'flex items-center gap-1 rounded-full border px-1.5 py-0.5 text-[10px]',
                connectionState === 'error'
                  ? 'border-aurora-error/30 bg-aurora-error/12 text-aurora-error'
                  : 'border-aurora-accent-primary/18 bg-aurora-accent-deep/12 text-aurora-text-muted',
              )}
            >
              <span
                className={cn(
                  'size-1.5 rounded-full',
                  connectionState === 'error'
                    ? 'bg-aurora-error'
                    : 'animate-pulse bg-aurora-accent-primary',
                )}
              />
              <span>{connectionState === 'error' ? 'reconnecting' : 'connecting'}</span>
            </div>
          )}

        </div>
      </header>

      {/* ---- Session sidebar + message thread ---- */}
      <div className="flex min-h-0 flex-1">
        {sessionPanelOpen && (
          <SessionSidebar
            className="hidden w-[180px] shrink-0 md:flex"
            projects={projects}
            runs={visibleRuns}
            selectedRunId={selectedRunId}
            selectedProjectId="workspace"
            onSelectRun={(runId) => selectRun(runId)}
            onNewRun={() => void createRun()}
            hiddenRunCount={hiddenRunCount}
            includeHiddenRuns={includeHiddenRuns}
            onToggleIncludeHidden={() => setIncludeHiddenRuns((v) => !v)}
            onBulkCloseHidden={bulkCloseHiddenSessions}
          />
        )}

        <div className="flex min-h-0 min-w-0 flex-1 flex-col">
          {/* Message thread — React.memo'd, only re-renders when messages changes */}
          <MessageThread run={selectedRun} messages={messages} connectionState={connectionState} />

          {visibleError && (
            <div
              role="status"
              className="mx-3 mb-2 flex items-start gap-2 rounded border border-aurora-error/40 bg-aurora-error/10 px-3 py-2 text-[12px] leading-5 text-aurora-error sm:mx-4"
            >
              <AlertCircle className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
              <span className="min-w-0 break-words">{visibleError}</span>
            </div>
          )}

          {/* Chat input */}
          <ChatInput
            onSend={sendPrompt}
            disabled={!providerReady}
            selectedAgent={selectedAgent}
            agents={agents.length > 0 ? agents : [selectedAgent]}
            onSelectAgent={selectAgent}
          />
        </div>

      </div>
    </div>
  )
}

function messageFromUnknownError(error: unknown, fallback: string): string {
  if (error instanceof Error && error.message.trim().length > 0) {
    return error.message
  }
  return fallback
}
