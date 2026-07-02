'use client'

import { NavButtons } from '@/components/setup/WizardShell'

export default function WelcomePage(): React.ReactElement {
  return (
    <div className="space-y-6">
      <section className="space-y-3">
        <h2 className="text-xl font-semibold">Welcome</h2>
        <p>
          This wizard configures <code>~/.labby/.env</code> by walking through
          eight short steps. You can leave at any time — your progress is
          persisted as a draft and resumed on the next visit.
        </p>
        <p className="text-sm text-muted-foreground">
          You can exit and re-run later by typing <code>labby setup</code> in
          your shell, or set <code>LAB_SKIP_SETUP=1</code> to suppress the
          first-run prompt entirely.
        </p>
      </section>
      <NavButtons hideBack nextLabel="Begin" />
    </div>
  )
}
