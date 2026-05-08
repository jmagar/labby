'use client'

/**
 * FloatingChatFab — Fixed pill FAB (bottom-right) that opens the floating chat popover.
 *
 * - Fixed position, hidden (visibility:hidden) on /chat route — NOT unmounted
 * - Cmd/Ctrl+/ hotkey toggles open/closed
 * - Ambient connection indicator from ChatSessionConnectionContext
 * - Modal stack guard: registers in openModals ref when open
 * - Respects prefers-reduced-motion
 */

import * as React from 'react'
import { MessageSquare } from 'lucide-react'
import { usePathname } from 'next/navigation'
import { cn } from '@/lib/utils'

export type FloatingChatFabProps = {
  /** Whether the popover is currently open */
  open: boolean
  /** Toggle the popover open/closed */
  onToggle: () => void
  /** Connection state from ChatSessionConnectionContext (only populated after first open) */
  connectionState?: 'idle' | 'connecting' | 'open' | 'error'
  /** Whether a session is currently streaming (running) */
  isStreaming?: boolean
  /** Ref to a set of open modal IDs — FAB registers 'floating-chat' when popover opens */
  openModals?: React.RefObject<Set<string>>
  /** Unread count badge — shows dot until notification system exists */
  unreadCount?: number
}

export function FloatingChatFab({
  open,
  onToggle,
  connectionState,
  isStreaming,
  openModals,
  unreadCount = 0,
}: FloatingChatFabProps) {
  const pathname = usePathname()
  const isOnChatPage = pathname === '/chat' || pathname === '/chat/'

  // Register/deregister from modal stack
  React.useEffect(() => {
    if (!openModals) return
    if (open) {
      openModals.current.add('floating-chat')
    } else {
      openModals.current.delete('floating-chat')
    }
    return () => {
      openModals.current.delete('floating-chat')
    }
  }, [open, openModals])

  // Hotkey: Cmd/Ctrl+/ — skip when focused in inputs
  React.useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (!(event.metaKey || event.ctrlKey)) return
      if (event.key !== '/') return
      const target = event.target as Element | null
      if (
        target instanceof HTMLInputElement ||
        target instanceof HTMLTextAreaElement ||
        (target instanceof HTMLElement && target.isContentEditable)
      ) {
        return
      }
      // Don't open if another modal is focused
      if (openModals?.current && openModals.current.size > 0 && !open) return
      event.preventDefault()
      onToggle()
    }

    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [onToggle, open, openModals])

  // Connection indicator classes
  const connectionRingClass =
    connectionState === 'connecting' ? 'ring-2 ring-aurora-accent-primary/40 animate-pulse' : ''

  const isStreamingPulse = isStreaming && connectionState === 'open'

  return (
    <div
      className={cn(
        'fixed bottom-5 right-5 z-40',
        // CSS-hidden (not unmounted) when on /chat route
        isOnChatPage && 'invisible pointer-events-none',
      )}
      aria-hidden={isOnChatPage}
    >
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={open}
        aria-controls="floating-chat-panel"
        aria-label={open ? 'Close chat' : 'Open chat'}
        tabIndex={isOnChatPage ? -1 : undefined}
        className={cn(
          'relative flex items-center gap-2 rounded-full border border-aurora-border-strong',
          'bg-aurora-panel-strong text-aurora-text-muted',
          'hover:text-aurora-text-primary hover:bg-aurora-hover-bg',
          'size-11 justify-center p-0 text-sm font-medium sm:size-auto sm:justify-start sm:px-4 sm:py-2.5',
          'transition-all duration-150',
          'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary',
          // Connection ring indicator (only after first open — when connectionState is set)
          connectionRingClass,
          // Streaming pulse — soft
          isStreamingPulse && 'motion-safe:animate-pulse',
          // Active state
          open && 'border-aurora-accent-primary/40 bg-aurora-panel-strong text-aurora-text-primary',
        )}
      >
        <MessageSquare className="size-4" />
        <span className="hidden sm:inline">Chat</span>

        {/* Unread count badge — dot until notification system */}
        {unreadCount > 0 && (
          <span
            aria-label={`${unreadCount} unread`}
            className="absolute -right-1 -top-1 flex size-4 items-center justify-center rounded-full bg-aurora-accent-primary text-[10px] font-bold text-white"
          >
            {unreadCount > 9 ? '9+' : unreadCount}
          </span>
        )}

        {/* Error indicator dot */}
        {connectionState === 'error' && (
          <span
            aria-hidden="true"
            className="absolute -right-1 -top-1 size-2.5 rounded-full bg-amber-400"
          />
        )}
      </button>
    </div>
  )
}
