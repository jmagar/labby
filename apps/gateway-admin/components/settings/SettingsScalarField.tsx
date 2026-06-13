'use client'

import type { SettingsFieldSpec, SettingsState } from '@/lib/api/setup-client'
import { Badge } from '@/components/ui/badge'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { hasEnvOverrideWarning, parseFieldInput, valueAsInputString } from '@/lib/settings/schema'

export function SettingsScalarField({
  field,
  value,
  state,
  error,
  onChange,
}: {
  field: SettingsFieldSpec
  value: unknown
  state: SettingsState
  error?: string
  onChange: (key: string, value: unknown) => void
}): React.ReactElement {
  const id = `settings-${field.key.replaceAll('.', '-')}`
  const errorId = `${id}-error`
  const inputValue = valueAsInputString(value)
  const source = state.sources[field.key]
  const envOverride = source?.overridden_by_env
  const isEnvShadowedConfig = field.backend === 'config_toml' && Boolean(envOverride)
  const disabled = field.write_policy !== 'editable' || isEnvShadowedConfig
  const sourceLabel = source?.source ?? 'default'
  const backendLabel = field.backend === 'env' ? '.env' : 'config.toml'
  const describedBy = error ? errorId : undefined
  const controlProps = {
    id,
    disabled,
    'aria-invalid': Boolean(error),
    'aria-describedby': describedBy,
  }

  function renderControl(): React.ReactNode {
    switch (field.control) {
      case 'bool':
        return <Switch {...controlProps} checked={Boolean(value)} onCheckedChange={(checked) => onChange(field.key, checked)} />
      case 'enum':
        return (
          <Select value={inputValue} disabled={disabled} onValueChange={(next) => onChange(field.key, next)}>
            <SelectTrigger {...controlProps}>
              <SelectValue placeholder={field.example ?? 'Select'} />
            </SelectTrigger>
            <SelectContent>
              {field.options.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )
      case 'string_list':
        return <Textarea {...controlProps} value={inputValue} className="min-h-24 font-mono text-xs" onChange={(event) => onChange(field.key, parseFieldInput(field, event.target.value))} />
      case 'read_only':
        return <pre className="max-h-64 overflow-auto rounded-md bg-muted p-3 text-xs">{JSON.stringify(value ?? null, null, 2)}</pre>
      default:
        return <Input {...controlProps} type={field.control === 'number' ? 'number' : 'text'} value={inputValue} onChange={(event) => onChange(field.key, parseFieldInput(field, event.target.value))} />
    }
  }

  return (
    <div className="grid gap-2 rounded-md border p-3">
      <div className="flex items-start justify-between gap-3">
        <div>
          <Label htmlFor={id}>{field.label}</Label>
          <p className="mt-1 text-xs text-muted-foreground">{field.description}</p>
          <p className="mt-1 font-mono text-[11px] text-muted-foreground">{field.key}</p>
        </div>
        <span className="rounded bg-muted px-1.5 py-0.5 text-[10px] uppercase text-muted-foreground">
          {field.apply_mode}
        </span>
      </div>
      <div className="flex flex-wrap gap-1.5">
        <Badge variant="secondary">{backendLabel}</Badge>
        <Badge variant="outline">source: {sourceLabel}</Badge>
        <Badge variant="outline">risk: {field.risk}</Badge>
        {field.write_policy !== 'editable' ? <Badge variant="outline" status="warn">{field.write_policy}</Badge> : null}
        {field.env_override ? <Badge variant="outline">env: {field.env_override}</Badge> : null}
      </div>
      {hasEnvOverrideWarning(field, state) ? (
        <p className="text-xs text-amber-600">{envOverride} currently overrides this config.toml value. Edit the env var or remove the override first.</p>
      ) : null}
      {renderControl()}
      {error ? <p id={errorId} className="text-xs text-destructive">{error}</p> : null}
    </div>
  )
}
