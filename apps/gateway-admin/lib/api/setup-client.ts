// TypeScript wrapper over the lab-bg3e.3 setup dispatch service.
//
// Mirrors crates/lab/src/dispatch/setup/catalog.rs. All actions go through
// POST /v1/setup with { action, params } shape (same as MCP).
//
// Each action returns the typed response directly; transport errors throw
// SetupApiError with the stable kind tag from docs/ERRORS.md.

import { setupActionUrl } from './gateway-config.ts'
import { performServiceAction, type ServiceActionError } from './service-action-client.ts'

const USE_MOCK_DATA = process.env.NEXT_PUBLIC_MOCK_DATA === 'true'

export class SetupApiError extends Error implements ServiceActionError {
  status: number
  code?: string
  param?: string

  constructor(message: string, status: number, code?: string, param?: string) {
    super(message)
    this.name = 'SetupApiError'
    this.status = status
    this.code = code
    this.param = param
  }
}

async function setupAction<T>(
  action: string,
  params: Record<string, unknown> = {},
  signal?: AbortSignal,
): Promise<T> {
  return performServiceAction<T, SetupApiError>({
    action,
    params,
    signal,
    serviceLabel: 'Setup',
    url: setupActionUrl(),
    createError: (message, status, code, param) => new SetupApiError(message, status, code, param),
  })
}

// ─── State machine ──────────────────────────────────────────────────────

export type SetupStateKind =
  | 'uninitialized'
  | 'config_missing'
  | 'partially_configured'
  | 'health_checking'
  | 'ready'

export interface SetupState {
  kind: SetupStateKind
  envars?: string[]
  missing?: string[]
  services?: string[]
}

export interface SetupSnapshot {
  first_run: boolean
  env_path: string
  draft_path: string
  last_completed_step: number
  draft_stale: boolean
  has_draft: boolean
  state: SetupState
}

// ─── UiSchema projection ────────────────────────────────────────────────

export type FieldKindKey =
  | 'text'
  | 'secret'
  | 'url'
  | 'bool'
  | 'number'
  | 'file_path'
  | 'enum'

export interface UiValidation {
  required: boolean
  min_length: number | null
  max_length: number | null
  pattern: string | null
}

export interface UiFieldSchema {
  kind: FieldKindKey
  enum_values: string[] | null
  advanced: boolean
  help_url: string | null
  depends_on: string | null
  validation: UiValidation
}

export interface ServiceEnvVar {
  name: string
  description: string
  example: string
  secret: boolean
  required: boolean
  ui?: UiFieldSchema
}

export interface ServiceSchema {
  name: string
  display_name: string
  description: string
  category: string
  supports_multi_instance: boolean
  default_port: number | null
  built_in_upstream_api?: boolean
  env: ServiceEnvVar[]
}

export interface SchemaGetResponse {
  services: Record<string, ServiceSchema>
}

const BASE_VALIDATION: UiValidation = {
  required: false,
  min_length: null,
  max_length: null,
  pattern: null,
}

function textUi(required = false, kind: FieldKindKey = 'text'): UiFieldSchema {
  return {
    kind,
    enum_values: null,
    advanced: false,
    help_url: null,
    depends_on: null,
    validation: {
      ...BASE_VALIDATION,
      required,
    },
  }
}

const MOCK_SERVICES: Record<string, ServiceSchema> = {
  radarr: {
    name: 'radarr',
    display_name: 'Radarr',
    description: 'Movie automation service.',
    category: 'Servarr',
    supports_multi_instance: true,
    default_port: 7878,
    env: [
      { name: 'RADARR_URL', description: 'Base URL for Radarr.', example: 'http://radarr.local:7878', secret: false, required: true, ui: textUi(true, 'url') },
      { name: 'RADARR_API_KEY', description: 'Radarr API key.', example: '••••••••', secret: true, required: true, ui: textUi(true, 'secret') },
    ],
  },
  sonarr: {
    name: 'sonarr',
    display_name: 'Sonarr',
    description: 'Series automation service.',
    category: 'Servarr',
    supports_multi_instance: true,
    default_port: 8989,
    env: [
      { name: 'SONARR_URL', description: 'Base URL for Sonarr.', example: 'http://sonarr.local:8989', secret: false, required: true, ui: textUi(true, 'url') },
      { name: 'SONARR_API_KEY', description: 'Sonarr API key.', example: '••••••••', secret: true, required: true, ui: textUi(true, 'secret') },
    ],
  },
  plex: {
    name: 'plex',
    display_name: 'Plex',
    description: 'Media server status and library access.',
    category: 'Media',
    supports_multi_instance: false,
    default_port: 32400,
    env: [
      { name: 'PLEX_URL', description: 'Base URL for Plex.', example: 'http://plex.local:32400', secret: false, required: true, ui: textUi(true, 'url') },
      { name: 'PLEX_TOKEN', description: 'Plex access token.', example: '••••••••', secret: true, required: true, ui: textUi(true, 'secret') },
    ],
  },
}

const MOCK_DRAFT_ENTRIES: DraftEntry[] = [
  { key: 'LAB_MCP_HTTP_HOST', value: '127.0.0.1' },
  { key: 'LAB_MCP_HTTP_PORT', value: '3101' },
  { key: 'LAB_LOG', value: 'lab=info,lab_apis=warn' },
  { key: 'LAB_LOG_FORMAT', value: 'json' },
  { key: 'RADARR_URL', value: 'http://radarr.local:7878' },
  { key: 'RADARR_API_KEY', value: '********' },
]

function mockSetupSnapshot(): SetupSnapshot {
  return {
    first_run: false,
    env_path: '~/.lab/.env',
    draft_path: '~/.lab/.env.draft',
    last_completed_step: 3,
    draft_stale: false,
    has_draft: true,
    state: {
      kind: 'partially_configured',
      missing: ['SONARR_URL', 'SONARR_API_KEY', 'PLEX_URL', 'PLEX_TOKEN'],
      services: Object.keys(MOCK_SERVICES),
    },
  }
}

// ─── Drafts ─────────────────────────────────────────────────────────────

export interface DraftEntry {
  key: string
  value: string
}

export interface DraftSetOutcome {
  written: number
  skipped: string[]
  backup_path: string | null
}

export interface CommitOutcome {
  written: number
  skipped: string[]
  backup_path: string | null
  audit_pass_count: number
  audit_total_count: number
  // Present when the gate failed; the caller can render the audit body inline.
  ok?: false
  audit?: unknown
}

export interface InstalledPlugin {
  id: string
  service: string | null
}

export interface PluginLifecycleOutcome {
  service: string
  package_id: string
  status: string
  message: string
}

export interface ServiceStatus {
  name: string
  display_name: string
  description: string
  configured: boolean
  plugin_installed: boolean
  plugin_package_id: string | null
  required_env: string[]
}

export interface ServicesStatusResponse {
  services: ServiceStatus[]
  plugins: InstalledPlugin[]
}

export type SettingsBackend = 'env' | 'config_toml'
export type SettingsControl = 'text' | 'url' | 'bool' | 'number' | 'enum' | 'string_list' | 'read_only'
export type SettingsRisk = 'low' | 'restart' | 'security_sensitive' | 'dangerous'
export type SettingsWritePolicy = 'editable' | 'read_only' | 'dangerous_flow_required' | 'secret_write_only_future'
export type SettingsApplyMode = 'immediate' | 'restart' | 'partial' | 'read_only'
export type SettingsSourceKind = 'env' | 'config_toml' | 'default'

export interface SettingsOption {
  value: string
  label: string
}

export interface SettingsFieldSpec {
  key: string
  label: string
  description: string
  section: string
  backend: SettingsBackend
  control: SettingsControl
  risk: SettingsRisk
  write_policy: SettingsWritePolicy
  apply_mode: SettingsApplyMode
  secret: boolean
  required: boolean
  env_override: string | null
  min: number | null
  max: number | null
  options: SettingsOption[]
  example: string | null
}

export interface SettingsSectionSpec {
  id: string
  label: string
  description: string
  advanced: boolean
}

export interface SettingsSchemaResponse {
  schema_version: number
  sections: SettingsSectionSpec[]
  fields: SettingsFieldSpec[]
}

export interface SettingsValueSource {
  source: SettingsSourceKind
  overridden_by_env: string | null
}

export interface SettingsState {
  schema_version: number
  config_path: string
  env_path: string
  section: string
  values: Record<string, unknown>
  sources: Record<string, SettingsValueSource>
}

export interface SettingsUpdateEntry {
  key: string
  value: unknown
  previous?: unknown
  unset?: boolean
}

export interface SettingsMutationOutcome {
  state: SettingsState
  backup_path: string | null
}

export interface EnvSettingSpec {
  service: string
  key: string
  required: boolean
  secret: boolean
  description: string
  example: string
  editable: boolean
}

export interface SettingsUpdate {
  services: {
    built_in_upstream_apis_enabled: boolean
  }
}

export const MOCK_SETTINGS_SCHEMA: SettingsSchemaResponse = {
  schema_version: 1,
  sections: [
    { id: 'core', label: 'Core', description: 'Env-backed process defaults.', advanced: false },
    { id: 'features', label: 'Features', description: 'Runtime feature gates.', advanced: false },
    { id: 'advanced', label: 'Advanced', description: 'Advanced settings.', advanced: true },
  ],
  fields: [
    { key: 'LAB_LOG', label: 'Log filter', description: 'Tracing filter directive.', section: 'core', backend: 'env', control: 'text', risk: 'restart', write_policy: 'editable', apply_mode: 'restart', secret: false, required: false, env_override: null, min: null, max: null, options: [], example: 'labby=info' },
    { key: 'services.built_in_upstream_apis_enabled', label: 'Built-in upstream API services', description: 'Enable bundled external API integrations.', section: 'features', backend: 'config_toml', control: 'bool', risk: 'low', write_policy: 'editable', apply_mode: 'immediate', secret: false, required: false, env_override: null, min: null, max: null, options: [], example: 'true' },
    { key: 'auth', label: 'Auth config', description: 'Redacted auth settings.', section: 'advanced', backend: 'config_toml', control: 'read_only', risk: 'security_sensitive', write_policy: 'secret_write_only_future', apply_mode: 'read_only', secret: true, required: false, env_override: null, min: null, max: null, options: [], example: null },
  ],
}

export const MOCK_ENV_SCHEMA: EnvSettingSpec[] = [
  { service: 'lab', key: 'LAB_LOG', required: false, secret: false, description: 'Tracing filter directive.', example: 'labby=info', editable: true },
  { service: 'setup', key: 'LAB_MCP_HTTP_TOKEN', required: true, secret: true, description: 'Bearer token.', example: '<token>', editable: false },
]

function mockSettingsState(section: string, updates: SettingsUpdateEntry[] = []): SettingsState {
  const values: Record<string, unknown> = {
    LAB_LOG: 'labby=info,lab_apis=warn',
    'services.built_in_upstream_apis_enabled': true,
    auth: { google_client_secret: { has_value: true } },
  }
  for (const update of updates) values[update.key] = update.value
  return {
    schema_version: 1,
    config_path: '~/.config/lab/config.toml',
    env_path: '~/.lab/.env',
    section,
    values,
    sources: {
      LAB_LOG: { source: 'env', overridden_by_env: null },
      'services.built_in_upstream_apis_enabled': { source: 'config_toml', overridden_by_env: null },
      auth: { source: 'config_toml', overridden_by_env: null },
    },
  }
}

// ─── Public API ─────────────────────────────────────────────────────────

export const setupApi = {
  state(signal?: AbortSignal): Promise<SetupSnapshot> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve(structuredClone(mockSetupSnapshot()))
    }
    return setupAction<SetupSnapshot>('state', {}, signal)
  },

  schemaGet(services?: string[], signal?: AbortSignal): Promise<SchemaGetResponse> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      const selected = services?.length ? services : Object.keys(MOCK_SERVICES)
      return Promise.resolve({
        services: Object.fromEntries(
          selected
            .map((service) => [service, MOCK_SERVICES[service]] as const)
            .filter(([, schema]) => schema !== undefined),
        ) as Record<string, ServiceSchema>,
      })
    }
    return setupAction<SchemaGetResponse>('schema.get', services ? { services } : {}, signal)
  },

  settingsSchema(signal?: AbortSignal): Promise<SettingsSchemaResponse> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve(structuredClone(MOCK_SETTINGS_SCHEMA))
    }
    return setupAction<SettingsSchemaResponse>('settings.schema', {}, signal)
  },

  settingsState(section = 'core', signal?: AbortSignal): Promise<SettingsState> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve(mockSettingsState(section))
    }
    return setupAction<SettingsState>('settings.state', { section }, signal)
  },

  settingsConfigUpdate(section: string, entries: SettingsUpdateEntry[], confirm: boolean, signal?: AbortSignal): Promise<SettingsMutationOutcome> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve({ state: mockSettingsState(section, entries), backup_path: '~/.config/lab/config.toml.bak.mock' })
    }
    return setupAction<SettingsMutationOutcome>('settings.config.update', { section, entries, confirm }, signal)
  },

  settingsEnvUpdate(section: string, entries: SettingsUpdateEntry[], confirm: boolean, signal?: AbortSignal): Promise<SettingsState> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve(mockSettingsState(section, entries))
    }
    return setupAction<SettingsState>('settings.env.update', { section, entries, confirm }, signal)
  },

  settingsEnvSchema(signal?: AbortSignal): Promise<EnvSettingSpec[]> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve(structuredClone(MOCK_ENV_SCHEMA))
    }
    return setupAction<EnvSettingSpec[]>('settings.env_schema', {}, signal)
  },

  settingsUpdate(patch: SettingsUpdate, signal?: AbortSignal): Promise<SettingsState> {
    return this.settingsConfigUpdate(
      'features',
      [
        {
          key: 'services.built_in_upstream_apis_enabled',
          value: patch.services.built_in_upstream_apis_enabled,
        },
      ],
      true,
      signal,
    ).then((outcome) => outcome.state)
  },

  draftGet(signal?: AbortSignal): Promise<{ entries: DraftEntry[] }> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve({ entries: structuredClone(MOCK_DRAFT_ENTRIES) })
    }
    return setupAction<{ entries: DraftEntry[] }>('draft.get', {}, signal)
  },

  draftSet(
    entries: DraftEntry[],
    options?: { force?: boolean },
    signal?: AbortSignal,
  ): Promise<DraftSetOutcome> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve({ written: entries.length, skipped: [], backup_path: null })
    }
    return setupAction<DraftSetOutcome>(
      'draft.set',
      { entries, force: options?.force ?? false },
      signal,
    )
  },

  draftCommit(
    options?: { force?: boolean },
    signal?: AbortSignal,
  ): Promise<CommitOutcome> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve({
        written: 4,
        skipped: [],
        backup_path: null,
        audit_pass_count: 3,
        audit_total_count: 3,
      })
    }
    return setupAction<CommitOutcome>(
      'draft.commit',
      { force: options?.force ?? false, confirm: true },
      signal,
    )
  },

  finalize(signal?: AbortSignal): Promise<CommitOutcome> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve({
        written: 4,
        skipped: [],
        backup_path: null,
        audit_pass_count: 3,
        audit_total_count: 3,
      })
    }
    return setupAction<CommitOutcome>('finalize', { confirm: true }, signal)
  },

  installedPlugins(signal?: AbortSignal): Promise<InstalledPlugin[]> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve([{ id: 'lab-radarr@lab', service: 'radarr' }])
    }
    return setupAction<InstalledPlugin[]>('installed_plugins', {}, signal)
  },

  servicesStatus(signal?: AbortSignal): Promise<ServicesStatusResponse> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve({
        plugins: [{ id: 'lab-radarr@lab', service: 'radarr' }],
        services: Object.values(MOCK_SERVICES).map((schema) => ({
          name: schema.name,
          display_name: schema.display_name,
          description: schema.description,
          configured: schema.name === 'radarr',
          plugin_installed: schema.name === 'radarr',
          plugin_package_id: `lab-${schema.name}@lab`,
          required_env: schema.env.filter((env) => env.required).map((env) => env.name),
        })),
      })
    }
    return setupAction<ServicesStatusResponse>('services_status', {}, signal)
  },

  installPlugin(service: string, signal?: AbortSignal): Promise<PluginLifecycleOutcome> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve({
        service,
        package_id: `lab-${service}@lab`,
        status: 'install',
        message: 'mock install complete',
      })
    }
    return setupAction<PluginLifecycleOutcome>('install_plugin', { service, confirm: true }, signal)
  },

  uninstallPlugin(service: string, signal?: AbortSignal): Promise<PluginLifecycleOutcome> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve({
        service,
        package_id: `lab-${service}@lab`,
        status: 'uninstall',
        message: 'mock uninstall complete',
      })
    }
    return setupAction<PluginLifecycleOutcome>('uninstall_plugin', { service, confirm: true }, signal)
  },
}
