'use client'

import { useEffect } from 'react'
import { usePathname, useRouter } from 'next/navigation'

import { ProgressBar, WIZARD_STEPS, WizardProvider, useWizard } from '@/components/setup/WizardShell'
import { Button } from '@/components/ui/button'
import { setupApi } from '@/lib/api/setup-client'

export default function SetupLayout({
  children,
}: {
  children: React.ReactNode
}): React.ReactElement {
  const pathname = usePathname() ?? ''
  const router = useRouter()

  // On first mount, query setup.state. Two outcomes:
  // - .env already complete (state.kind === 'ready') and we landed on
  //   /setup with no last_completed_step → redirect to /settings.
  // - last_completed_step > 0 → resume there.
  //
  // Race guards: AbortController.signal.aborted only flips when the
  // layout itself unmounts. If the user clicks a wizard step link the
  // pathname changes but the layout stays mounted, so we additionally
  // re-check window.location.pathname after the await — the user may
  // have navigated mid-fetch and we must not stomp their intent.
  useEffect(() => {
    const isIndexRoute = pathname === '/setup' || pathname === '/setup/'
    if (!isIndexRoute) return
    const controller = new AbortController()

    function stillOnIndex(): boolean {
      if (typeof window === 'undefined') return true
      const p = window.location.pathname
      return p === '/setup' || p === '/setup/'
    }

    setupApi
      .state(controller.signal)
      .then((snapshot) => {
        if (controller.signal.aborted || !stillOnIndex()) return
        if (!snapshot.first_run && snapshot.state.kind === 'ready') {
          router.replace('/settings/')
          return
        }
        const lastCompletedStep = Math.max(0, Math.min(snapshot.last_completed_step, WIZARD_STEPS.length - 1))
        if (lastCompletedStep > 0) {
          const step = WIZARD_STEPS[lastCompletedStep]
          router.replace(`/setup/${step.slug}/`)
          return
        }
          const suffix = window.location.search.includes('mode=plugin') ? '?mode=plugin' : ''
          router.replace(`/setup/welcome/${suffix}`)
      })
      .catch(() => {
        if (controller.signal.aborted || !stillOnIndex()) return
        const suffix = window.location.search.includes('mode=plugin') ? '?mode=plugin' : ''
        router.replace(`/setup/welcome/${suffix}`)
      })
    return () => controller.abort()
  }, [pathname, router])

  return (
    <WizardProvider>
      <div className="mx-auto flex max-w-3xl flex-col gap-6 p-8">
        <SetupHeader />
        <ProgressBar pathname={pathname} />
        <main className="flex-1">{children}</main>
      </div>
    </WizardProvider>
  )
}

function SetupHeader(): React.ReactElement {
  const { mode } = useWizard()
  return (
    <header className="flex items-start justify-between gap-4">
      <div>
        <h1 className="text-2xl font-semibold">Lab Setup</h1>
          <p className="text-sm text-muted-foreground">
            {mode === 'plugin'
              ? 'Configure the services you have installed plugins for.'
              : 'One-time wizard to configure ~/.lab/.env via the browser.'}
          </p>
      </div>
      {mode === 'plugin' ? (
        <Button variant="outline" asChild>
          <a href="/setup/welcome/?mode=full">Show advanced setup</a>
        </Button>
      ) : null}
    </header>
  )
}
