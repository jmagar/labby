'use client'

import * as React from 'react'
import { Plus, Settings2, SidebarOpen, Zap } from 'lucide-react'
import { cn } from '@/lib/utils'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'
import { SidebarTrigger } from '@/components/ui/sidebar'
import { Separator } from '@/components/ui/separator'
import { SessionSidebar } from './session-sidebar'
import { MessageThread } from './message-thread'
import { ChatInput } from './chat-input'
import { SettingsPanel } from './settings-panel'
import {
  useChatSessionData,
  useChatSessionActions,
  useChatSessionConnection,
  useChatSessionStream,
} from '@/lib/chat/chat-session-provider'

export {
  createSessionForIntent,
  ensurePromptRunId,
  ensurePromptRunIdForProvider,
  integrateCreatedRun,
  providerDisplayName,
  resolveSelectedAgent,
  sendPromptForSelectedProvider,
  sessionCreationOptionsForIntent,
  shouldAutoCreateInitialRun,
} from '@/lib/chat/use-chat-session-controller'

export function ChatShell() {
  const [sessionPanelOpen, setSessionPanelOpen] = React.useState(true)
  const [settingsOpen, setSettingsOpen] = React.useState(false)
  const [isMobileViewport, setIsMobileViewport] = React.useState(false)
  const [systemPrompt, setSystemPrompt] = React.useState('')
  const [temperature, setTemperature] = React.useState(0.7)
  const [maxTokens, setMaxTokens] = React.useState(8192)
  const { runs, selectedRun, selectedRunId, providerHealth, selectedAgent, agents, projects } =
    useChatSessionData()
  const { selectRun, createSession, sendPrompt, selectAgent } = useChatSessionActions()
  const { messages } = useChatSessionStream()
  const { connectionState } = useChatSessionConnection()
  const providerReady = Boolean(providerHealth?.ready)
  const providerUnavailableMessage = providerReady ? null : providerHealth?.message?.trim() || null

  const createRun = React.useCallback(async () => {
    try {
      await createSession({ closeSessionPanel: isMobileViewport })
    } catch {
      // Provider health carries the failure detail.
    }
  }, [createSession, isMobileViewport])

  const handleSendPrompt = React.useCallback(
    async (payload: Parameters<typeof sendPrompt>[0]) => {
      try {
        await sendPrompt(payload)
      } catch {
        // Provider health carries the failure detail.
      }
    },
    [sendPrompt],
  )

  React.useEffect(() => {
    const media = window.matchMedia('(max-width: 767px)')
    const sync = () => {
      setIsMobileViewport(media.matches)
      setSessionPanelOpen((open) => (media.matches ? false : open))
    }
    sync()
    media.addEventListener('change', sync)
    return () => media.removeEventListener('change', sync)
  }, [])

  return (
    <div className="flex h-dvh min-h-0 flex-col overflow-hidden bg-aurora-page-bg">
      <header
        role="banner"
        className="sticky top-0 z-20 flex h-[calc(3rem+env(safe-area-inset-top,0px))] shrink-0 items-center gap-2 border-b border-aurora-border-default bg-aurora-nav-bg px-2.5 pt-[env(safe-area-inset-top,0px)] sm:px-3 md:static md:z-auto md:h-12 md:pt-0"
      >
        <SidebarTrigger
          aria-label="Toggle app sidebar"
          className="-ml-1 text-aurora-text-muted/60 hover:text-aurora-text-primary"
        />
        <Separator orientation="vertical" className="h-4 bg-aurora-border-default" />

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
          <div className="ml-1 flex items-center gap-1.5 text-[12px] text-aurora-text-muted">
            <span className="hidden text-aurora-text-muted/50 sm:block">{projects[0]?.name}</span>
            <span className="hidden text-aurora-text-muted/30 sm:block">/</span>
            <span className="max-w-[180px] truncate text-aurora-text-primary sm:max-w-[300px]">{selectedRun.title}</span>
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
            className="flex items-center gap-1 rounded-full border border-aurora-border-default bg-aurora-control-surface px-1.5 py-0.5 sm:px-2"
            title={providerReady ? 'ACP live' : (providerUnavailableMessage ?? 'ACP unavailable')}
          >
            <Zap className="size-3 text-aurora-accent-primary/70" />
            <span className="hidden text-[11px] text-aurora-text-muted sm:inline">
              {providerReady ? 'ACP live' : 'ACP unavailable'}
            </span>
          </div>

          <TooltipProvider delayDuration={400}>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon"
                  aria-label={settingsOpen ? 'Close settings' : 'Open settings'}
                  onClick={() => setSettingsOpen((open) => !open)}
                  className={cn(
                    'size-7 rounded text-aurora-text-muted/60 hover:bg-aurora-hover-bg hover:text-aurora-text-primary',
                    settingsOpen && 'bg-aurora-hover-bg text-aurora-text-primary',
                  )}
                >
                  <Settings2 className="size-3.5" />
                </Button>
              </TooltipTrigger>
              <TooltipContent side="bottom" className="text-xs">Settings</TooltipContent>
            </Tooltip>
          </TooltipProvider>
        </div>
      </header>

      <div className="flex min-h-0 flex-1">
        {sessionPanelOpen && (
          <>
            {isMobileViewport && (
              <button
                type="button"
                aria-label="Close session drawer"
                className="fixed inset-0 z-30 bg-aurora-page-bg/70 backdrop-blur-[2px] md:hidden"
                onClick={() => setSessionPanelOpen(false)}
              />
            )}
            <div
              className={cn(
                'min-h-0 shrink-0',
                isMobileViewport
                  ? 'absolute inset-y-0 left-0 z-40 w-[min(88vw,320px)] md:hidden'
                  : 'relative z-0 hidden md:block',
              )}
            >
              <SessionSidebar
                className="shadow-[var(--aurora-shadow-strong),var(--aurora-highlight-strong)] md:shadow-none"
                projects={projects}
                runs={runs}
                selectedRunId={selectedRunId}
                selectedProjectId="workspace"
                onSelectRun={selectRun}
                onNewRun={() => void createRun()}
              />
            </div>
          </>
        )}

        <div className="flex min-h-0 min-w-0 flex-1 flex-col">
          {providerUnavailableMessage && (
            <div className="shrink-0 border-b border-aurora-warn/30 bg-aurora-warn/10 px-3 py-2 text-[12px] text-aurora-text-primary sm:px-4">
              <span className="font-semibold">ACP provider unavailable:</span>{' '}
              <span className="text-aurora-text-muted">{providerUnavailableMessage}</span>
            </div>
          )}
          <MessageThread run={selectedRun} messages={messages} connectionState={connectionState} />
          <ChatInput
            onSend={handleSendPrompt}
            disabled={!providerReady}
            disabledReason={providerUnavailableMessage ?? undefined}
            selectedAgent={selectedAgent}
            agents={agents.length > 0 ? agents : [selectedAgent]}
            onSelectAgent={selectAgent}
          />
        </div>

        {settingsOpen && (
          <SettingsPanel
            agent={selectedAgent}
            onClose={() => setSettingsOpen(false)}
            systemPrompt={systemPrompt}
            onSystemPromptChange={setSystemPrompt}
            temperature={temperature}
            onTemperatureChange={setTemperature}
            maxTokens={maxTokens}
            onMaxTokensChange={setMaxTokens}
          />
        )}
      </div>
    </div>
  )
}
