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

  constructor(message: string, status: number, code?: string) {
    super(message)
    this.name = 'SetupApiError'
    this.status = status
    this.code = code
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
    createError: (message, status, code) => new SetupApiError(message, status, code),
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

function mockSettingsSurfaces(): SettingsState['surfaces'] {
  return {
    mcp: {
      transport: 'http',
      host: '127.0.0.1',
      port: 8765,
      stateful: null,
    },
    web: {
      auth_disabled: false,
      assets_dir: null,
    },
    auth: {
      mode: 'bearer',
      public_url: null,
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

export interface SettingsState {
  config_path: string
  restart_required: boolean
  restart_note: string
  services: {
    built_in_upstream_apis_enabled: boolean
    built_in_upstream_api_services: string[]
    bootstrap_services: string[]
  }
  surfaces: {
    mcp: {
      transport: string
      host: string
      port: number
      stateful: boolean | null
    }
    web: {
      auth_disabled: boolean
      assets_dir: string | null
    }
    auth: {
      mode: string
      public_url: string | null
    }
  }
}

export interface SettingsUpdate {
  services?: {
    built_in_upstream_apis_enabled?: boolean
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

  settingsState(signal?: AbortSignal): Promise<SettingsState> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve({
        config_path: '~/.config/lab/config.toml',
        restart_required: false,
        restart_note: 'Changes to built-in upstream API services take effect after restarting labby serve.',
        services: {
          built_in_upstream_apis_enabled: true,
          built_in_upstream_api_services: Object.keys(MOCK_SERVICES),
          bootstrap_services: ['setup', 'doctor', 'extract', 'gateway'],
        },
        surfaces: mockSettingsSurfaces(),
      })
    }
    return setupAction<SettingsState>('settings.state', {}, signal)
  },

  settingsUpdate(patch: SettingsUpdate, signal?: AbortSignal): Promise<SettingsState> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve({
        config_path: '~/.config/lab/config.toml',
        restart_required: true,
        restart_note: 'Changes to built-in upstream API services take effect after restarting labby serve.',
        services: {
          built_in_upstream_apis_enabled:
            patch.services?.built_in_upstream_apis_enabled ?? true,
          built_in_upstream_api_services: Object.keys(MOCK_SERVICES),
          bootstrap_services: ['setup', 'doctor', 'extract', 'gateway'],
        },
        surfaces: mockSettingsSurfaces(),
      })
    }
    return setupAction<SettingsState>(
      'settings.update',
      { ...(patch as Record<string, unknown>), confirm: true },
      signal,
    )
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
