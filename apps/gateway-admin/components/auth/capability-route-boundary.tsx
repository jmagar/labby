'use client'

import { usePathname } from 'next/navigation'

import { capabilityForPath } from '@/components/console/nav-model'
import { useBrowserSession } from '@/lib/auth/session'

export function CapabilityRouteBoundary({ children }: { children: React.ReactNode }) {
  const pathname = usePathname()
  const session = useBrowserSession()
  if (session.status !== 'authenticated') return children
  const required = capabilityForPath(pathname)
  const allowed = required === null || (required !== undefined && session.authority?.capabilities.includes(required))
  if (allowed) {
    const workspaceKey = session.authority
      ? `${session.authority.generation}:${session.authority.activeOwner.kind}:${session.authority.activeOwner.id}`
      : 'authority-unavailable'
    return <div className="contents" key={workspaceKey}>{children}</div>
  }
  return <main className="mx-auto flex min-h-[60vh] max-w-xl items-center px-6"><section role="alert" className="w-full rounded-xl border border-aurora-border-default bg-aurora-panel-medium p-6"><p className="text-xs font-semibold uppercase tracking-[0.14em] text-aurora-text-muted">Workspace access</p><h1 className="mt-2 text-xl font-semibold text-aurora-text-primary">This page is not available in the selected workspace</h1><p className="mt-2 text-sm text-aurora-text-secondary">Choose another Team, Project, or Personal workspace, or ask an owner to update your access.</p></section></main>
}
