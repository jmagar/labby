import type { SettingsFieldSpec, SettingsState, SettingsUpdateEntry } from '@/lib/api/setup-client'

const INVALID_FIELD_INPUT = '__lab_invalid_settings_input__'

export interface InvalidFieldInput {
  readonly kind: typeof INVALID_FIELD_INPUT
  readonly raw: string
  readonly message: string
}

function invalidFieldInput(raw: string, message: string): InvalidFieldInput {
  return { kind: INVALID_FIELD_INPUT, raw, message }
}

export function isInvalidFieldInput(value: unknown): value is InvalidFieldInput {
  return typeof value === 'object'
    && value !== null
    && (value as { kind?: unknown }).kind === INVALID_FIELD_INPUT
}

export function fieldsForSection(schemaFields: SettingsFieldSpec[], section: string): SettingsFieldSpec[] {
  return schemaFields
    .filter((field) => field.section === section)
    .sort((a, b) => a.label.localeCompare(b.label))
}

export function editableFields(fields: SettingsFieldSpec[]): SettingsFieldSpec[] {
  return fields.filter((field) => field.write_policy === 'editable' && field.control !== 'read_only')
}

export function valueAsInputString(value: unknown): string {
  if (isInvalidFieldInput(value)) return value.raw
  if (value === null || value === undefined) return ''
  if (Array.isArray(value)) return value.join('\n')
  return String(value)
}

export function parseFieldInput(field: SettingsFieldSpec, raw: string | boolean): unknown {
  if (field.control === 'bool') return Boolean(raw)
  const text = String(raw)
  if (field.control === 'number') {
    if (text.trim() === '') return null
    const parsed = Number(text)
    if (!Number.isFinite(parsed) || !Number.isInteger(parsed)) return invalidFieldInput(text, 'Must be an integer.')
    if (field.min !== null && parsed < field.min) return invalidFieldInput(text, `Must be at least ${field.min}.`)
    if (field.max !== null && parsed > field.max) return invalidFieldInput(text, `Must be at most ${field.max}.`)
    return parsed
  }
  if (field.control === 'string_list') {
    return text
      .split(/\r?\n|,/)
      .map((entry) => entry.trim())
      .filter(Boolean)
  }
  return text
}

export function collectFieldInputErrors(
  fields: SettingsFieldSpec[],
  changedKeys: Set<string>,
  values: Record<string, unknown>,
): Record<string, string> {
  const errors: Record<string, string> = {}
  for (const field of fields) {
    if (!changedKeys.has(field.key)) continue
    const value = values[field.key]
    if (isInvalidFieldInput(value)) errors[field.key] = value.message
  }
  return errors
}

export function buildDirtyEntries(
  fields: SettingsFieldSpec[],
  changedKeys: Set<string>,
  values: Record<string, unknown>,
  initialValues: Record<string, unknown>,
): SettingsUpdateEntry[] {
  return fields
    .filter((field) => changedKeys.has(field.key))
    .map((field) => {
      const value = values[field.key] ?? null
      if (isInvalidFieldInput(value)) {
        throw new Error(`invalid value for ${field.key}: ${value.message}`)
      }
      const previous = initialValues[field.key] ?? null
      const unset = field.backend === 'config_toml'
        && !field.required
        && (value === null || value === '' || (Array.isArray(value) && value.length === 0))
      return unset ? { key: field.key, value: null, previous, unset: true } : { key: field.key, value, previous }
    })
}

export function buildDirtyEntriesByBackend(
  fields: SettingsFieldSpec[],
  changedKeys: Set<string>,
  values: Record<string, unknown>,
  initialValues: Record<string, unknown>,
  sources: SettingsState['sources'] = {},
): { envEntries: SettingsUpdateEntry[]; configEntries: SettingsUpdateEntry[] } {
  const editable = editableFields(fields).filter((field) => {
    return !(field.backend === 'config_toml' && sources[field.key]?.overridden_by_env)
  })
  const backendByKey = new Map(editable.map((field) => [field.key, field.backend]))
  const entries = buildDirtyEntries(editable, changedKeys, values, initialValues)
  return {
    envEntries: entries.filter((entry) => backendByKey.get(entry.key) === 'env'),
    configEntries: entries.filter((entry) => backendByKey.get(entry.key) === 'config_toml'),
  }
}

export function hasEnvOverrideWarning(field: SettingsFieldSpec, state: SettingsState): boolean {
  return Boolean(state.sources[field.key]?.overridden_by_env)
}
