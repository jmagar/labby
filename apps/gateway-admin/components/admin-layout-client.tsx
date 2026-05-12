'use client'

/**
 * AdminLayoutClient — client-side wrapper for the admin layout.
 *
 * Hosts ChatSessionProvider, FloatingChatFab, and FloatingChatPopover.
 * Keeps the layout.tsx file a server component (no 'use client' in the layout).
 */

import * as React from 'react'
import { usePathname } from 'next/navigation'
import { ChatSessionProvider } from '@/lib/chat/chat-session-provider'
import { FloatingChatFab } from '@/components/floating-chat-fab'
import { FloatingChatPopover } from '@/components/floating-chat-popover'
import { FloatingChatShell } from '@/components/floating-chat-shell'
import { PageContextSync } from '@/components/page-context-sync'
import {
  readPersistedState,
  patchPersistedState,
  DEFAULT_CONFIG,
  type ChatConfig,
} from '@/components/floating-chat-popover'

export type ChatLayoutState = {
  open: boolean
  config: ChatConfig
}

export function resolvePersistedChatLayoutState(): ChatLayoutState {
  try {
    const persisted = readPersistedState()
    return {
      open: Boolean(persisted.open),
      config: persisted.config ?? DEFAULT_CONFIG,
    }
  } catch {
    return {
      open: false,
      config: DEFAULT_CONFIG,
    }
  }
}

export function shouldAutoBootstrapChat(_open: boolean, _isOnChatPage: boolean) {
  return false
}

export function AdminLayoutClient({
  children,
}: {
  children: React.ReactNode
}) {
  const [open, setOpen] = React.useState(false)
  const [config, setConfig] = React.useState<ChatConfig>(DEFAULT_CONFIG)
  const [persistedStateLoaded, setPersistedStateLoaded] = React.useState(false)

  React.useEffect(() => {
    const persisted = resolvePersistedChatLayoutState()
    setOpen(persisted.open)
    setConfig(persisted.config)
    setPersistedStateLoaded(true)
  }, [])

  // openModals ref — shared between FAB and CommandPalette
  const openModals = React.useRef<Set<string>>(new Set())

  const pathname = usePathname()
  const isOnChatPage = pathname === '/chat' || pathname === '/chat/'

  // Only auto-bootstrap a session when the chat surface is actually visible.
  // Without this gate the provider would mint an empty session on every admin
  // page load, leaving orphan sessions + SSE streams on the backend.
  const autoBootstrap = shouldAutoBootstrapChat(open, isOnChatPage)

  const [isMobileViewport, setIsMobileViewport] = React.useState(false)

  React.useEffect(() => {
    const media = window.matchMedia('(max-width: 767px)')
    const sync = () => setIsMobileViewport(media.matches)
    sync()
    media.addEventListener('change', sync)
    return () => media.removeEventListener('change', sync)
  }, [])

  React.useEffect(() => {
    if (!persistedStateLoaded) return
    patchPersistedState({ open })
  }, [open, persistedStateLoaded])

  const handleToggle = React.useCallback(() => {
    setOpen((prev) => !prev)
  }, [])

  const handleClose = React.useCallback(() => {
    setOpen(false)
  }, [])

  return (
    <ChatSessionProvider isMobileViewport={isMobileViewport} autoBootstrap={autoBootstrap}>
      <PageContextSync />
      <div className={!isOnChatPage ? 'pb-20 md:pb-0' : undefined}>
        {children}
      </div>
      {!isOnChatPage && (
        <>
          <FloatingChatFab
            open={open}
            onToggle={handleToggle}
            openModals={openModals}
          />
          <FloatingChatPopover
            open={open}
            onClose={handleClose}
            config={config}
            onConfigChange={setConfig}
          >
            <FloatingChatShell config={config} />
          </FloatingChatPopover>
        </>
      )}
    </ChatSessionProvider>
  )
}
