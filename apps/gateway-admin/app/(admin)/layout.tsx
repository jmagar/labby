import { AuthBootstrap } from '@/components/auth/auth-bootstrap'
import { ConsoleShell } from '@/components/console/console-shell'
import { AppCommandPalette } from '@/components/app-command-palette'
import { Toaster } from '@/components/ui/sonner'
import { CapabilityRouteBoundary } from '@/components/auth/capability-route-boundary'

export default function AdminLayout({
  children,
}: {
  children: React.ReactNode
}) {
  return (
    <AuthBootstrap>
      <ConsoleShell>
        <CapabilityRouteBoundary>{children}</CapabilityRouteBoundary>
      </ConsoleShell>
      <AppCommandPalette />
      <Toaster />
    </AuthBootstrap>
  )
}
