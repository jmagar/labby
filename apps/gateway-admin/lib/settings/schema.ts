import type { SettingsFieldSpec, SettingsState, SettingsUpdateEntry } from '@/lib/api/setup-client'

export function fieldsForSection(schemaFields: SettingsFieldSpec[], section: string): SettingsFieldSpec[] {
  return schemaFields
    .filter((field) => field.section === section)
    .sort((a, b) => a.label.localeCompare(b.label))
}

export function editableFields(fields: SettingsFieldSpec[]): SettingsFieldSpec[] {
  return fields.filter((field) => field.write_policy === 'editable' && field.control !== 'read_only')
}

export function valueAsInputString(value: unknown): string {
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
    if (!Number.isFinite(parsed) || !Number.isInteger(parsed)) return null
    if (field.min !== null && parsed < field.min) return null
    if (field.max !== null && parsed > field.max) return null
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

export function buildDirtyEntries(
  fields: SettingsFieldSpec[],
  changedKeys: Set<string>,
  values: Record<string, unknown>,
  initialValues: Record<string, unknown>,
): SettingsUpdateEntry[] {
  return fields
    .filter((field) => changedKeys.has(field.key))
    .map((field) => ({
      key: field.key,
      value: values[field.key] ?? null,
      previous: initialValues[field.key] ?? null,
    }))
}

export function buildDirtyEntriesByBackend(
  fields: SettingsFieldSpec[],
  changedKeys: Set<string>,
  values: Record<string, unknown>,
  initialValues: Record<string, unknown>,
): { envEntries: SettingsUpdateEntry[]; configEntries: SettingsUpdateEntry[] } {
  const editable = editableFields(fields)
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
