import type { SettingsFieldSpec, SettingsState } from '@/lib/api/setup-client'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'

export function AdvancedReadOnlyBlock({
  state,
  fields,
}: {
  state: SettingsState
  fields: SettingsFieldSpec[]
}): React.ReactElement {
  return (
    <Card>
      <CardHeader>
        <CardTitle>Read-only advanced config</CardTitle>
        <CardDescription>Complex and dangerous settings are visible here redacted. Typed editors are separate follow-up work.</CardDescription>
      </CardHeader>
      <CardContent className="space-y-3">
        {fields.map((field) => (
          <div key={field.key} className="rounded-md border p-3">
            <p className="text-sm font-medium">{field.label}</p>
            <p className="text-xs text-muted-foreground">{field.description}</p>
            <pre className="mt-2 max-h-72 overflow-auto rounded-md bg-muted p-3 text-xs">
              {JSON.stringify(state.values[field.key] ?? null, null, 2)}
            </pre>
          </div>
        ))}
      </CardContent>
    </Card>
  )
}
