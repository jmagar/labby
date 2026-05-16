/**
 * Types for the /v1/catalog endpoint consumed by the ⌘K palette.
 *
 * These are intentionally separate from `ServiceAction` in `lib/types/gateway.ts`
 * because `CatalogAction` includes `params` and `returns` fields that
 * `ServiceAction` does not have.
 */

export interface CatalogParam {
  /** Parameter name. */
  name: string
  /**
   * Free-form type label: `"string"`, `"integer"`, `"boolean"`, `"object"`,
   * `"array"`, union literals like `"string|null"`, or enum literals.
   */
  ty: string
  /** Whether this parameter must be present for the action to succeed. */
  required: boolean
  /** Human-readable description of the parameter. */
  description: string
  /** When true, render as a password input and never show the value in plaintext. */
  secret?: boolean
}

export interface CatalogAction {
  /** Dotted action name (e.g., `movie.search`). */
  action: string
  /** Short description. */
  description: string
  /** Whether the action mutates state and requires confirmation. */
  destructive: boolean
  /** Declared parameters for this action. Empty when the action takes no params. */
  params: CatalogParam[]
  /** Type-name hint for the return shape, e.g. `"Movie[]"`. Informational only. */
  returns: string
}

export interface CatalogService {
  /** Service identifier (matches the MCP tool name and CLI subcommand). */
  name: string
  /** Short human description. */
  description: string
  /** Category slug (Media, Servarr, Notifications, etc.). */
  category: string
  /** Implementation status: `"available"` or `"stub"`. */
  status: string
  /** List of actions exposed by the service. */
  actions: CatalogAction[]
}

export interface CatalogResponse {
  services: CatalogService[]
}
