import type { ReactNode } from 'react'

import { AppHeader } from '@/components/app-header'
import { SettingsRail } from '@/components/settings/SettingsRail'
import { DraftStaleBanner } from '@/components/settings/DraftStaleBanner'

export default function SettingsLayout({
  children,
}: {
  children: ReactNode
}): React.ReactElement {
  return (
    <div className="flex flex-col">
      <AppHeader breadcrumbs={[{ label: 'Settings' }]} />
      <div className="grid min-w-0 gap-4 p-4 sm:p-6 lg:grid-cols-[220px_minmax(0,1fr)] lg:gap-6">
        <aside className="min-w-0 rounded-md border bg-card">
          <SettingsRail />
        </aside>
        <main className="flex min-w-0 flex-col gap-4">
          <DraftStaleBanner />
          {children}
        </main>
      </div>
    </div>
  )
}
