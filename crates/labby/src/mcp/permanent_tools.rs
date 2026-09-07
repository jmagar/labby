//! Permanent product-tool identity/dispatch resolution, and the sole
//! construction site for Labby-owned MCP `Tool` descriptors.
//!
//! Two responsibilities live here deliberately:
//!
//! 1. `PermanentToolRegistry::resolve` maps permanent tool names to dispatch
//!    ids independently of upstream health.
//! 2. The `*_tool` / `*_descriptor` constructors below are the only place a
//!    Labby-owned descriptor is assembled. `handlers_tools::list_tools_impl`
//!    and `peer_contract::visible_tool_descriptors` both consume them, so the
//!    two listing paths cannot drift apart (see
//!    `handlers_tools/tests.rs` descriptor drift tests).
//!
//! Do not construct `Tool` values for Labby-owned tools anywhere else.

use std::sync::{Arc, LazyLock};

use rmcp::model::{MetaObject, Tool, ToolAnnotations};
use serde::Serialize;
use serde_json::Value;

#[cfg(feature = "gateway")]
use crate::mcp::call_tool_codemode::{
    CodeModeUpstreamDescription, code_mode_description_with_suffix,
};
#[cfg(feature = "gateway")]
use crate::mcp::catalog::{
    ADD_SERVER_TOOL_NAME, CODE_MODE_UI_TOOL_NAME, GATEWAY_STATUS_TOOL_NAME, MCP_APP_TOOL_NAME,
    SETTINGS_TOOL_NAME,
};
use crate::mcp::catalog::{CODE_MODE_READ_TOOL_NAME, CODE_MODE_TOOL_NAME, SERVER_LOGS_TOOL_NAME};
use crate::mcp::completion::action_schema;
use crate::mcp::handlers_tools::server_logs_tool_meta;
#[cfg(feature = "gateway")]
use crate::mcp::handlers_tools::{
    add_server_tool_meta, add_server_tool_schema, code_mode_app_text_note,
    code_mode_execute_schema, code_mode_tool_meta, code_mode_trace_output_schema,
    code_mode_ui_description, gateway_status_tool_meta, gateway_status_tool_schema,
    mcp_app_tool_description, mcp_app_tool_meta, mcp_app_tool_schema, settings_tool_meta,
    settings_tool_schema,
};
#[cfg(feature = "skills")]
use crate::mcp::handlers_tools::{skill_library_tool_description, skill_library_tool_meta};
use crate::registry::RegisteredService;

/// Shared `{action, params, instance}` input schema advertised by every
/// builtin service tool. Kept private so callers must go through
/// [`PermanentToolRegistry::builtin_service_tool`]; the single definition site
/// exists for drift prevention, not performance.
fn builtin_action_schema() -> Arc<serde_json::Map<String, Value>> {
    static BUILTIN_ACTION_SCHEMA: LazyLock<Arc<serde_json::Map<String, Value>>> =
        LazyLock::new(|| Arc::new(action_schema()));
    Arc::clone(&BUILTIN_ACTION_SCHEMA)
}

#[cfg(feature = "skills")]
fn skill_action_schema(allowed_actions: Option<&[String]>) -> Arc<serde_json::Map<String, Value>> {
    static MANAGEMENT: LazyLock<Arc<serde_json::Map<String, Value>>> =
        LazyLock::new(|| action_enum_schema(&crate::dispatch::artifacts::ACTIONS));
    let Some(allowed) = allowed_actions else {
        return Arc::clone(&MANAGEMENT);
    };
    let actions = crate::dispatch::artifacts::ACTIONS
        .iter()
        .filter(|action| {
            matches!(action.name, "help" | "schema")
                || allowed.iter().any(|allowed| allowed == action.name)
        })
        .copied()
        .collect::<Vec<_>>();
    action_enum_schema(&actions)
}

#[cfg(feature = "skills")]
fn action_enum_schema(
    actions: &[labby_primitives::action::ActionSpec],
) -> Arc<serde_json::Map<String, Value>> {
    let mut schema = action_schema();
    schema["properties"]["action"]["enum"] = Value::Array(
        actions
            .iter()
            .map(|action| Value::String(action.name.to_owned()))
            .collect(),
    );
    Arc::new(schema)
}

/// Success-envelope output schema shared by every builtin service tool.
///
/// Mirrors `build_success` (mcp/envelope.rs) — the two must change in the same
/// commit. `data` is intentionally unconstrained: one tool serves many
/// actions, so a tool-level schema cannot describe per-action payloads.
///
/// Error envelopes are deliberately NOT described here — see
/// docs/contracts/mcp-tool-output.md §C3.2. The exemption for
/// `isError` results is ecosystem convention, not explicit spec text.
///
/// `additionalProperties` is `true` by decision (SPEC §5.2): closing the
/// envelope would make any future `build_success` field break all builtins'
/// advertised schemas at once, client-side. If `build_success` ever grows a
/// field, this schema changes in the same commit anyway — the open object just
/// means clients do not break first.
fn dispatch_envelope_output_schema() -> Arc<serde_json::Map<String, Value>> {
    static ENVELOPE_OUTPUT_SCHEMA: LazyLock<Arc<serde_json::Map<String, Value>>> = LazyLock::new(
        || match serde_json::json!({
            "type": "object",
            "properties": {
                "ok": { "const": true },
                "service": { "type": "string",
                    "description": "Service tool that answered the call." },
                "action": { "type": "string",
                    "description": "Resolved dotted action, including the built-in `help` and `schema` actions." },
                "data": { "description": "Action-specific payload; shape varies by action." }
            },
            "required": ["ok", "service", "action", "data"],
            "additionalProperties": true
        }) {
            Value::Object(map) => Arc::new(map),
            _ => unreachable!("dispatch envelope output schema must be an object"),
        },
    );
    Arc::clone(&ENVELOPE_OUTPUT_SCHEMA)
}

/// Typed dispatcher key for a permanent product tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PermanentToolId {
    CodeModeRead,
    CodeMode,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum SkillLibraryDescriptorMode<'a> {
    #[default]
    Hidden,
    Management {
        app_visible: bool,
        allowed_actions: Option<&'a [String]>,
    },
}

#[derive(Debug, Clone, Copy)]
struct PermanentToolEntry {
    id: PermanentToolId,
    name: &'static str,
}

/// Registry built with every MCP server composition.
///
/// The registry owns permanent identity and dispatch resolution. Request-time
/// visibility and authorization still decide whether a descriptor is listed or
/// a resolved tool may execute.
const PERMANENT_TOOLS: [PermanentToolEntry; 2] = [
    PermanentToolEntry {
        id: PermanentToolId::CodeModeRead,
        name: CODE_MODE_READ_TOOL_NAME,
    },
    PermanentToolEntry {
        id: PermanentToolId::CodeMode,
        name: CODE_MODE_TOOL_NAME,
    },
];

/// Conservatively reserve every Labby-owned non-upstream Tool identity.
///
/// This is visibility-independent: a hidden product Tool still cannot be
/// impersonated by a regular upstream route.
#[cfg(feature = "gateway")]
pub(crate) fn is_reserved_non_upstream_tool_name(name: &str) -> bool {
    matches!(
        name,
        CODE_MODE_TOOL_NAME
            | CODE_MODE_READ_TOOL_NAME
            | CODE_MODE_UI_TOOL_NAME
            | MCP_APP_TOOL_NAME
            | ADD_SERVER_TOOL_NAME
            | GATEWAY_STATUS_TOOL_NAME
            | SETTINGS_TOOL_NAME
    )
}

#[must_use]
pub(crate) fn code_mode_read_annotations() -> ToolAnnotations {
    ToolAnnotations::new()
        .read_only(true)
        .destructive(false)
        .idempotent(true)
        .open_world(true)
}

#[must_use]
pub(crate) fn code_mode_full_annotations() -> ToolAnnotations {
    ToolAnnotations::new()
        .read_only(false)
        .destructive(true)
        .idempotent(false)
        .open_world(true)
}

/// Advertised safety hints for a registry-backed service tool.
///
/// `destructiveHint` is the least-safe union of the service's action catalog,
/// except for `server_logs`: that admin-only surface can disclose sensitive
/// free-text and is deliberately kept behind the next-hop destructive gate.
/// The other hints are reviewed service-level claims; absence of a destructive
/// action is not enough to infer that a service is read-only.
#[must_use]
fn builtin_service_annotations(service: &RegisteredService) -> ToolAnnotations {
    let derived_destructive = service.actions.iter().any(|action| action.destructive);
    let (read_only, destructive, idempotent, open_world) = match service.name {
        "fs" | "lab_admin" => (true, derived_destructive, true, false),
        // Stash mutates only Labby-owned local state. Its mixed action set is
        // neither read-only nor uniformly idempotent, and delete is destructive.
        "stash" => (false, derived_destructive, false, false),
        "skills" => (true, derived_destructive, true, true),
        "doctor" => (false, derived_destructive, true, true),
        "access" | "agents" | "tasks" | "dev_containers" => {
            (false, derived_destructive, false, false)
        }
        "browser" | "gateway" | "setup" | "snippets" | "artifacts" | "bundles" | "jobs"
        | "sources" | "uploads" => (false, derived_destructive, false, true),
        // `server_logs` is operationally read-only, but advertising it as such
        // would bypass the conservative next-hop gate described above.
        SERVER_LOGS_TOOL_NAME => (false, true, false, false),
        // Registries are extensible in tests and future feature slices. New
        // services get conservative hints until their behavior is audited.
        _ => (false, true, false, true),
    };

    ToolAnnotations::new()
        .read_only(read_only)
        .destructive(destructive)
        .idempotent(idempotent)
        .open_world(open_world)
}

#[cfg(feature = "gateway")]
#[must_use]
fn mcp_app_annotations() -> ToolAnnotations {
    ToolAnnotations::new()
        .read_only(false)
        .destructive(false)
        .idempotent(true)
        .open_world(false)
}

#[cfg(feature = "gateway")]
#[must_use]
fn gateway_status_annotations() -> ToolAnnotations {
    ToolAnnotations::new()
        .read_only(true)
        .destructive(false)
        .idempotent(true)
        .open_world(false)
}

#[cfg(feature = "gateway")]
#[must_use]
fn settings_annotations() -> ToolAnnotations {
    ToolAnnotations::new()
        .read_only(false)
        .destructive(true)
        .idempotent(false)
        .open_world(false)
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct PermanentToolRegistry;

// The one legitimate `Tool::new` site: this registry IS the sole construction
// point the clippy.toml `disallowed-methods` entry directs everyone to.
#[allow(clippy::disallowed_methods)]
impl PermanentToolRegistry {
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self
    }

    #[must_use]
    pub(crate) fn resolve(&self, name: &str) -> Option<PermanentToolId> {
        PERMANENT_TOOLS
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.id)
    }

    /// Descriptor for one builtin service tool.
    ///
    /// Advertises the success-envelope `outputSchema`: every registry
    /// service's success path flows through `format_dispatch_result`, which
    /// always sets the envelope as `structuredContent` (audit on bead
    /// lab-41e7m.1). The `SERVER_LOGS_TOOL_NAME` check is invariant across
    /// callers and lives here; only `admin_apps_visible` differs, because the
    /// live-request path resolves it from request auth while the stored peer
    /// contract resolves it from a captured `PeerCatalogAudience`.
    #[must_use]
    pub(crate) fn builtin_service_tool(
        &self,
        service: &RegisteredService,
        admin_apps_visible: bool,
        skill_library_mode: SkillLibraryDescriptorMode<'_>,
    ) -> Tool {
        #[cfg(feature = "skills")]
        if service.name == "artifacts" {
            return self.skill_library_tool(service, skill_library_mode);
        }
        #[cfg(not(feature = "skills"))]
        let _ = skill_library_mode;
        let tool = Tool::new(service.name, service.description, builtin_action_schema())
            .with_annotations(builtin_service_annotations(service))
            .with_raw_output_schema(dispatch_envelope_output_schema());
        let tool = if service.name == SERVER_LOGS_TOOL_NAME && admin_apps_visible {
            tool.with_meta(server_logs_tool_meta(service.name))
        } else {
            tool
        };
        with_labby_security(tool)
    }

    /// Canonical descriptor for the `artifacts` service with its Artifact
    /// Library presentation binding. The underlying service remains callable
    /// as ordinary text on hosts that do not render MCP Apps.
    #[cfg(feature = "skills")]
    #[must_use]
    pub(crate) fn skill_library_tool(
        &self,
        service: &RegisteredService,
        mode: SkillLibraryDescriptorMode<'_>,
    ) -> Tool {
        debug_assert_eq!(service.name, "artifacts");
        let allowed_actions = match mode {
            SkillLibraryDescriptorMode::Management {
                allowed_actions, ..
            } => allowed_actions,
            SkillLibraryDescriptorMode::Hidden => None,
        };
        let annotations = ToolAnnotations::new()
            .read_only(false)
            .destructive(true)
            .idempotent(false)
            .open_world(true);
        let tool = Tool::new(
            service.name,
            skill_library_tool_description(service.description),
            skill_action_schema(allowed_actions),
        )
        .with_annotations(annotations)
        .with_raw_output_schema(dispatch_envelope_output_schema());
        let tool = match mode {
            SkillLibraryDescriptorMode::Management {
                app_visible: true, ..
            } => tool.with_meta(skill_library_tool_meta(service.name)),
            SkillLibraryDescriptorMode::Hidden
            | SkillLibraryDescriptorMode::Management {
                app_visible: false, ..
            } => tool,
        };
        with_labby_security(tool)
    }

    /// Descriptor for the optional Code Mode MCP App twin of `codemode`.
    #[cfg(feature = "gateway")]
    #[must_use]
    pub(crate) fn code_mode_ui_tool(&self, upstreams: &[CodeModeUpstreamDescription]) -> Tool {
        with_labby_security(
            Tool::new(
                CODE_MODE_UI_TOOL_NAME,
                code_mode_ui_description(upstreams),
                code_mode_execute_schema(),
            )
            .with_annotations(code_mode_full_annotations())
            .with_raw_output_schema(code_mode_trace_output_schema())
            .with_meta(code_mode_tool_meta(CODE_MODE_UI_TOOL_NAME)),
        )
    }

    /// Descriptor for the MCP App control tool.
    ///
    /// Deliberately carries no `outputSchema`: its success payload is
    /// `{"kind": "mcp_app_control", …}` (call_tool.rs), not the dispatch
    /// envelope, and advertising a schema the results do not match is a hard
    /// client-side error in strict SDKs.
    #[cfg(feature = "gateway")]
    #[must_use]
    pub(crate) fn mcp_app_tool(&self, app_visible: bool) -> Tool {
        let tool = Tool::new(
            MCP_APP_TOOL_NAME,
            mcp_app_tool_description(),
            mcp_app_tool_schema(),
        )
        .with_annotations(mcp_app_annotations());
        let tool = if app_visible {
            tool.with_meta(mcp_app_tool_meta(MCP_APP_TOOL_NAME))
        } else {
            tool
        };
        with_labby_security(tool)
    }

    /// Descriptor for the Add Server admin app tool.
    ///
    /// Its synthetic actions (`open`/`test`/`create`) all format through
    /// `format_dispatch_result`, so the success envelope schema is accurate.
    #[cfg(feature = "gateway")]
    #[must_use]
    pub(crate) fn add_server_tool(&self) -> Tool {
        with_labby_security(Tool::new(
            ADD_SERVER_TOOL_NAME,
            "Open a responsive form to test and add a remote or local MCP server to the Labby gateway catalog.",
            add_server_tool_schema(),
        )
        .with_annotations(code_mode_full_annotations())
        .with_raw_output_schema(dispatch_envelope_output_schema())
        .with_meta(add_server_tool_meta(ADD_SERVER_TOOL_NAME)))
    }

    /// Descriptor for the Gateway Status admin app tool.
    ///
    /// Its synthetic actions (`open`/`refresh`) all format through
    /// `format_dispatch_result`, so the success envelope schema is accurate.
    #[cfg(feature = "gateway")]
    #[must_use]
    pub(crate) fn gateway_status_tool(&self) -> Tool {
        with_labby_security(Tool::new(
            GATEWAY_STATUS_TOOL_NAME,
            "Display live connection status, capabilities, and warnings for gateway upstream MCP servers.",
            gateway_status_tool_schema(),
        )
        .with_annotations(gateway_status_annotations())
        .with_raw_output_schema(dispatch_envelope_output_schema())
        .with_meta(gateway_status_tool_meta(GATEWAY_STATUS_TOOL_NAME)))
    }

    #[cfg(feature = "gateway")]
    #[must_use]
    pub(crate) fn settings_tool(&self) -> Tool {
        with_labby_security(Tool::new(
            SETTINGS_TOOL_NAME,
            "Open and manage schema-backed Labby settings, including Code Mode, proxy, surface, and feature controls.",
            settings_tool_schema(),
        )
        .with_annotations(settings_annotations())
        .with_raw_output_schema(dispatch_envelope_output_schema())
        .with_meta(settings_tool_meta(SETTINGS_TOOL_NAME)))
    }

    #[cfg(feature = "gateway")]
    #[must_use]
    pub(crate) fn code_mode_descriptor(&self, upstreams: &[CodeModeUpstreamDescription]) -> Tool {
        debug_assert_eq!(
            PERMANENT_TOOLS
                .iter()
                .find(|entry| entry.name == CODE_MODE_TOOL_NAME)
                .map(|entry| entry.name),
            Some(CODE_MODE_TOOL_NAME),
        );
        // `codemode` is permanently text-only: the MCP App metadata belongs to
        // the optional `codemode_ui` twin so disabling the app surface can never
        // remove the execution entry point. See mcp/CLAUDE.md.
        with_labby_security(
            Tool::new(
                CODE_MODE_TOOL_NAME,
                code_mode_description_with_suffix(upstreams, &code_mode_app_text_note()),
                code_mode_execute_schema(),
            )
            .with_annotations(code_mode_full_annotations())
            .with_raw_output_schema(code_mode_trace_output_schema()),
        )
    }

    #[cfg(feature = "gateway")]
    #[must_use]
    pub(crate) fn code_mode_read_descriptor(
        &self,
        upstreams: &[CodeModeUpstreamDescription],
    ) -> Tool {
        with_labby_security(Tool::new(
            CODE_MODE_READ_TOOL_NAME,
            code_mode_description_with_suffix(
                upstreams,
                "Read-only Code Mode execution. Only upstream tools explicitly annotated readOnly=true are discoverable and callable; artifact writes are disabled. Use codemode for write-capable execution.",
            ),
            code_mode_execute_schema(),
        )
        .with_annotations(code_mode_read_annotations())
        .with_raw_output_schema(code_mode_trace_output_schema()))
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "gateway")]
    use super::is_reserved_non_upstream_tool_name;
    use super::{
        PermanentToolId, PermanentToolRegistry, SkillLibraryDescriptorMode,
        dispatch_envelope_output_schema, with_labby_security,
    };
    #[cfg(feature = "gateway")]
    use crate::mcp::call_tool_codemode::CODE_MODE_DESCRIPTION_MAX_BYTES;
    #[cfg(feature = "gateway")]
    use crate::mcp::catalog::{
        ADD_SERVER_TOOL_NAME, CODE_MODE_UI_TOOL_NAME, GATEWAY_STATUS_TOOL_NAME, MCP_APP_TOOL_NAME,
        SETTINGS_TOOL_NAME,
    };
    use crate::mcp::catalog::{
        CODE_MODE_READ_TOOL_NAME, CODE_MODE_TOOL_NAME, SERVER_LOGS_TOOL_NAME,
    };
    use crate::registry::RegisteredService;
    use rmcp::model::Tool;
    use serde_json::Value;

    fn noop_dispatch(
        _action: String,
        _params: Value,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<Value, crate::dispatch::error::ToolError>> + Send>,
    > {
        Box::pin(async { Ok(serde_json::json!({})) })
    }

    fn service(name: &'static str) -> RegisteredService {
        RegisteredService {
            name,
            description: "Test service",
            category: "test",
            kind: crate::registry::RegisteredServiceKind::BootstrapOperator,
            status: "available",
            actions: &[],
            dispatch: noop_dispatch,
        }
    }

    /// AC-15 drift protection: the runtime envelope schema must match the
    /// published JSON Schema artifact, read as plain data (no validator
    /// dependency) — same pattern as
    /// `crates/labby-runtime/tests/agent_error_schema.rs`.
    #[test]
    fn envelope_output_schema_matches_published_schema() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/contracts/schemas/dispatch-envelope.schema.json");
        let published: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|error| {
                panic!("published schema unreadable at {}: {error}", path.display())
            }))
            .expect("published schema parses");
        let runtime = dispatch_envelope_output_schema();

        assert_eq!(runtime["type"], published["type"]);
        assert_eq!(runtime["required"], published["required"]);
        assert_eq!(
            runtime["additionalProperties"],
            published["additionalProperties"]
        );
        let runtime_props = runtime["properties"].as_object().expect("properties");
        let published_props = published["properties"].as_object().expect("properties");
        assert_eq!(
            runtime_props.keys().collect::<Vec<_>>(),
            published_props.keys().collect::<Vec<_>>(),
            "property sets must match"
        );
        assert_eq!(
            runtime_props["ok"]["const"], published_props["ok"]["const"],
            "`ok` must be const true in both"
        );
        for key in ["service", "action"] {
            assert_eq!(
                runtime_props[key]["type"], published_props[key]["type"],
                "`{key}` core type must match"
            );
        }
    }

    #[test]
    fn builtin_service_tool_advertises_envelope_schema() {
        let registry = PermanentToolRegistry::new();
        let tool = registry.builtin_service_tool(
            &service("gateway-alpha"),
            true,
            SkillLibraryDescriptorMode::Hidden,
        );
        let schema = tool.output_schema.as_ref().expect("outputSchema");
        assert_eq!(schema["properties"]["ok"]["const"], serde_json::json!(true));
        assert!(
            tool.meta.is_some(),
            "every Labby-owned tool carries auth metadata"
        );
        let annotations = tool.annotations.expect("fallback annotations");
        assert_eq!(annotations.read_only_hint, Some(false));
        assert_eq!(annotations.destructive_hint, Some(true));
        assert_eq!(annotations.idempotent_hint, Some(false));
        assert_eq!(annotations.open_world_hint, Some(true));
    }

    #[test]
    fn builtin_service_tool_advertises_security_schemes_extension() {
        let registry = PermanentToolRegistry::new();
        let tool = registry.builtin_service_tool(
            &service("gateway-alpha"),
            false,
            SkillLibraryDescriptorMode::Hidden,
        );
        let expected = serde_json::json!([{"type": "oauth2", "scopes": ["lab:read"]}]);
        let serialized = serde_json::to_value(&tool).expect("Tool descriptor serializes");
        assert_eq!(
            serialized["_meta"]["securitySchemes"], expected,
            "OAI-AUTH-002"
        );
        let round_trip: Tool = serde_json::from_value(serialized).expect("Tool round trips");
        assert_eq!(
            round_trip.meta.expect("auth metadata").0["securitySchemes"],
            expected
        );
    }

    #[test]
    fn every_registry_service_tool_declares_the_required_oauth_scope() {
        let registry = crate::registry::build_default_registry();
        let permanent = PermanentToolRegistry::new();
        let expected = serde_json::json!([{"type": "oauth2", "scopes": ["lab:read"]}]);

        for service in registry.services() {
            assert!(
                !service.actions.is_empty(),
                "registered customer-facing service `{}` must expose an auditable action denominator",
                service.name
            );
            let tool =
                permanent.builtin_service_tool(service, true, SkillLibraryDescriptorMode::Hidden);
            let serialized = serde_json::to_value(tool).expect("Tool descriptor serializes");
            assert_eq!(
                serialized["_meta"]["securitySchemes"], expected,
                "OAI-CLAUSE-001: registered MCP service `{}` lacks the OAuth scope required before dispatch",
                service.name
            );
        }
    }

    #[test]
    fn builtin_service_tool_keeps_security_schemes_in_meta() {
        let tool = PermanentToolRegistry::new().builtin_service_tool(
            &service("gateway-alpha"),
            false,
            SkillLibraryDescriptorMode::Hidden,
        );
        let serialized = serde_json::to_value(tool).expect("Tool descriptor serializes");
        assert!(serialized["_meta"]["securitySchemes"].is_array());
    }

    #[test]
    fn protected_boundary_rebinds_upstream_descriptor_security_policy() {
        // Decode the same wire descriptor an upstream MCP peer supplies. Tests
        // must not use the project-banned rmcp `Tool::new` constructor because
        // it silently leaves contract fields at SDK defaults.
        let upstream: Tool = serde_json::from_value(serde_json::json!({
            "name": "upstream_search",
            "description": "Search an upstream",
            "inputSchema": {"type": "object"},
            "securitySchemes": [{
                "type": "oauth2",
                "scopes": ["upstream:private"]
            }]
        }))
        .unwrap();

        let serialized = serde_json::to_value(with_labby_security(upstream)).unwrap();
        let expected = serde_json::json!([{"type": "oauth2", "scopes": ["lab:read"]}]);
        assert_eq!(serialized["_meta"]["securitySchemes"], expected);
    }

    #[test]
    fn server_logs_meta_is_gated_on_admin_visibility() {
        let registry = PermanentToolRegistry::new();
        let visible = registry.builtin_service_tool(
            &service(SERVER_LOGS_TOOL_NAME),
            true,
            SkillLibraryDescriptorMode::Hidden,
        );
        assert!(visible.meta.is_some(), "admin-visible server_logs has meta");
        let hidden = registry.builtin_service_tool(
            &service(SERVER_LOGS_TOOL_NAME),
            false,
            SkillLibraryDescriptorMode::Hidden,
        );
        assert!(
            hidden
                .meta
                .as_ref()
                .is_some_and(|meta| !meta.0.contains_key("ui")),
            "non-admin server_logs has no app meta"
        );
        // The schema is not audience-dependent.
        assert_eq!(visible.output_schema, hidden.output_schema);
        assert_eq!(visible.annotations, hidden.annotations);
    }

    /// Reviewed hint table: `(service, readOnly, destructive, idempotent, openWorld)`.
    ///
    /// One row per registry-backed service. A service with no row falls through
    /// to the least-safe `_` arm, which
    /// `every_registry_service_has_a_reviewed_hint_row` turns into a CI failure
    /// rather than a silent conservative default.
    const EXPECTED_SERVICE_ANNOTATIONS: &[(&str, bool, bool, bool, bool)] = &[
        ("access", false, false, false, false),
        ("agents", false, false, false, false),
        ("tasks", false, false, false, false),
        ("dev_containers", false, true, false, false),
        ("doctor", false, false, true, true),
        ("artifacts", false, true, false, true),
        ("browser", false, false, false, true),
        ("bundles", false, true, false, true),
        ("fs", true, false, true, false),
        ("gateway", false, true, false, true),
        ("jobs", false, false, false, true),
        ("lab_admin", true, false, true, false),
        ("server_logs", false, true, false, false),
        ("setup", false, true, false, true),
        ("snippets", false, true, false, true),
        ("sources", false, false, false, true),
        ("stash", false, true, false, false),
        ("uploads", false, false, false, true),
    ];

    /// Pinned action sets for services advertising `readOnlyHint: true`.
    ///
    /// `readOnlyHint` claims **every** action is non-mutating. That is strictly
    /// stronger than "declares no destructive action" and therefore cannot be
    /// derived from `ActionSpec` — a mutating-but-non-destructive action would
    /// satisfy the derived check while making the advertised hint a lie (the
    /// `doctor` trap: `system.checks` writes a probe file, which is why `doctor`
    /// is deliberately *not* in this list). Pinning the action set forces a
    /// re-audit whenever one is added.
    const READ_ONLY_SERVICE_ACTIONS: &[(&str, &[&str])] = &[
        ("fs", &["fs.list"]),
        ("lab_admin", &["help", "schema", "onboarding.audit"]),
    ];

    fn expected_annotation_row(name: &str) -> Option<(bool, bool, bool, bool)> {
        EXPECTED_SERVICE_ANNOTATIONS
            .iter()
            .find(|(row, ..)| *row == name)
            .map(|&(_, read_only, destructive, idempotent, open_world)| {
                (read_only, destructive, idempotent, open_world)
            })
    }

    #[test]
    fn every_registry_service_advertises_reviewed_explicit_annotations() {
        let registry = crate::registry::build_docs_registry();
        let permanent = PermanentToolRegistry::new();

        for service in registry.services() {
            let name = service.name;
            let Some((read_only, destructive, idempotent, open_world)) =
                expected_annotation_row(name)
            else {
                panic!(
                    "service `{name}` has no row in EXPECTED_SERVICE_ANNOTATIONS; \
                     it is silently shipping the least-safe fallback hints. Audit \
                     its actions and add a reviewed row."
                );
            };
            let annotations = permanent
                .builtin_service_tool(service, true, SkillLibraryDescriptorMode::Hidden)
                .annotations
                .expect("every Labby-owned service tool must carry annotations");
            assert_eq!(annotations.read_only_hint, Some(read_only), "{name}");
            assert_eq!(annotations.destructive_hint, Some(destructive), "{name}");
            assert_eq!(annotations.idempotent_hint, Some(idempotent), "{name}");
            assert_eq!(annotations.open_world_hint, Some(open_world), "{name}");
            if !matches!(name, SERVER_LOGS_TOOL_NAME | "artifacts") {
                assert_eq!(
                    destructive,
                    service.actions.iter().any(|action| action.destructive),
                    "{name} destructiveHint drifted from ActionSpec"
                );
            }
        }
    }

    #[test]
    fn stash_annotations_reflect_its_local_mixed_mutability_action_set() {
        let registry = crate::registry::build_docs_registry();
        let stash = registry.service("stash").expect("stash service");
        let annotations = PermanentToolRegistry::new()
            .builtin_service_tool(stash, true, SkillLibraryDescriptorMode::Hidden)
            .annotations
            .expect("stash annotations");

        assert_eq!(annotations.read_only_hint, Some(false));
        assert_eq!(annotations.destructive_hint, Some(true));
        assert_eq!(annotations.idempotent_hint, Some(false));
        assert_eq!(annotations.open_world_hint, Some(false));
        assert!(
            stash
                .actions
                .iter()
                .any(|action| action.name == "stash.rename" && !action.destructive)
        );
        assert!(
            stash
                .actions
                .iter()
                .any(|action| action.name == "stash.delete" && action.destructive)
        );
    }

    /// The table→registry direction: a row for a renamed or removed service is
    /// as silent as a missing row. Only meaningful when every service feature is
    /// compiled, which is the authoritative build per the root `CLAUDE.md`.
    #[cfg(all(feature = "gateway", feature = "fs", feature = "lab-admin"))]
    #[test]
    fn every_reviewed_hint_row_names_a_live_service() {
        let registry = crate::registry::build_docs_registry();
        for (name, ..) in EXPECTED_SERVICE_ANNOTATIONS {
            assert!(
                registry.service(name).is_some(),
                "EXPECTED_SERVICE_ANNOTATIONS has a stale row for `{name}`, \
                 which no longer resolves to a registered service"
            );
        }
    }

    #[test]
    fn read_only_services_pin_their_action_sets() {
        let registry = crate::registry::build_docs_registry();

        for (name, pinned) in READ_ONLY_SERVICE_ACTIONS {
            assert_eq!(
                expected_annotation_row(name).map(|(read_only, ..)| read_only),
                Some(true),
                "`{name}` is pinned as read-only here but its hint row disagrees"
            );
            let Some(service) = registry.service(name) else {
                // Feature slices intentionally omit services they do not compile.
                continue;
            };
            let actual: Vec<&str> = service.actions.iter().map(|action| action.name).collect();
            assert_eq!(
                actual, *pinned,
                "`{name}` advertises readOnlyHint: true, so its action set is pinned. \
                 A new action must be audited as non-mutating before it is added here — \
                 `readOnlyHint` is the hint clients act on (Claude Code gates parallel \
                 execution on it, VS Code skips confirmation)."
            );
        }
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn admin_app_tools_advertise_envelope_schema_but_mcp_app_does_not() {
        let registry = PermanentToolRegistry::new();
        assert!(registry.add_server_tool().output_schema.is_some());
        assert!(registry.gateway_status_tool().output_schema.is_some());
        // mcp_app returns `{"kind": "mcp_app_control", …}`, not the dispatch
        // envelope — advertising the envelope schema would be a lie strict
        // clients enforce.
        assert!(registry.mcp_app_tool(true).output_schema.is_none());
        // codemode_ui carries the trace schema, not the envelope schema.
        let ui_schema = registry.code_mode_ui_tool(&[]).output_schema;
        assert!(ui_schema.is_some());
        assert_ne!(ui_schema, registry.add_server_tool().output_schema);

        let cases = [
            (registry.mcp_app_tool(true), false, false, true, false),
            (registry.add_server_tool(), false, true, false, true),
            (registry.gateway_status_tool(), true, false, true, false),
            (registry.code_mode_ui_tool(&[]), false, true, false, true),
        ];
        for (tool, read_only, destructive, idempotent, open_world) in cases {
            let name = tool.name.to_string();
            let annotations = tool.annotations.expect("owned meta tool annotations");
            assert_eq!(annotations.read_only_hint, Some(read_only), "{name}");
            assert_eq!(annotations.destructive_hint, Some(destructive), "{name}");
            assert_eq!(annotations.idempotent_hint, Some(idempotent), "{name}");
            assert_eq!(annotations.open_world_hint, Some(open_world), "{name}");
        }
    }

    #[cfg(feature = "skills")]
    #[test]
    fn skill_library_descriptor_has_versioned_dual_host_binding_and_text_fallback() {
        let permanent = PermanentToolRegistry::new();
        let artifacts = service("artifacts");
        let tool = permanent.skill_library_tool(
            &artifacts,
            SkillLibraryDescriptorMode::Management {
                app_visible: true,
                allowed_actions: None,
            },
        );
        assert_eq!(
            tool,
            permanent.builtin_service_tool(
                &artifacts,
                false,
                SkillLibraryDescriptorMode::Management {
                    app_visible: true,
                    allowed_actions: None,
                },
            ),
            "every service-advertisement path must reuse the canonical Skill Library builder"
        );
        assert_eq!(tool.name.as_ref(), "artifacts");
        assert!(
            tool.description
                .as_deref()
                .is_some_and(|description| description.contains("non-App hosts")),
            "the descriptor must document its text fallback"
        );
        assert_eq!(tool.input_schema["required"], serde_json::json!(["action"]));
        assert_eq!(
            tool.input_schema["properties"]["action"]["enum"]
                .as_array()
                .expect("bounded action enum")
                .len(),
            31
        );
        let annotations = tool.annotations.as_ref().expect("mixed-operation hints");
        assert_eq!(annotations.read_only_hint, Some(false));
        assert_eq!(annotations.destructive_hint, Some(true));
        assert_eq!(annotations.idempotent_hint, Some(false));

        let meta = tool.meta.expect("Skill Library app metadata");
        let resource_uri = meta.0["ui"]["resourceUri"]
            .as_str()
            .expect("MCP Apps resource URI");
        let output_template = meta.0["openai/outputTemplate"]
            .as_str()
            .expect("Skybridge output template");
        assert!(resource_uri.starts_with("ui://lab/skill-library/app?v="));
        assert!(output_template.starts_with("ui://lab/skill-library/app.skybridge?v="));
        assert_eq!(
            meta.0["ui"]["visibility"],
            serde_json::json!(["model", "app"])
        );
        assert_eq!(meta.0["openai/widgetAccessible"], serde_json::json!(true));
        assert_eq!(
            meta.0["securitySchemes"][0]["scopes"],
            serde_json::json!(["lab:read"])
        );

        let hidden_app = permanent.skill_library_tool(
            &artifacts,
            SkillLibraryDescriptorMode::Management {
                app_visible: false,
                allowed_actions: None,
            },
        );
        assert!(
            hidden_app
                .meta
                .as_ref()
                .is_some_and(|meta| !meta.0.contains_key("ui"))
        );
        assert_eq!(hidden_app.input_schema, tool.input_schema);
    }

    /// F9 regression guard — the compensating control for accepting the widened
    /// next-hop reach (design decision "Option A").
    ///
    /// In a labby → labby chain the downstream gateway derives its own
    /// `UpstreamTool.destructive` from the annotations we advertise
    /// (`upstream_destructive_from_annotations`), and that value is a hard gate:
    /// Code Mode, the palette, and widget callbacks all refuse a destructive
    /// tool unless `destructive_permitted(Mcp, caller) == caller.can_execute()`.
    ///
    /// Before annotations existed every Labby tool arrived with none and failed
    /// closed to `destructive: true`, so a non-execute caller could reach none of
    /// them. Advertising hints deliberately opens the subset below. That is
    /// accepted, but it rests on deployment configuration rather than an
    /// invariant, so the set is pinned here: widening it must be a reviewed
    /// change, not a side effect of editing a hint row.
    #[cfg(feature = "gateway")]
    #[test]
    fn labby_owned_annotations_pin_the_next_hop_destructive_gate() {
        use labby_gateway::upstream::pool::upstream_destructive_from_annotations;

        let permanent = PermanentToolRegistry::new();
        let services = crate::registry::build_docs_registry();

        // Reachable by a caller with `can_execute() == false` at hop 2.
        let expected_callable = [
            "access",
            "agents",
            "browser",
            "doctor",
            "fs",
            "jobs",
            "lab_admin",
            "mcp_app",
            "sources",
            "tasks",
            "uploads",
            "gateway_status",
            CODE_MODE_READ_TOOL_NAME,
        ];

        let mut descriptors: Vec<Tool> = services
            .services()
            .iter()
            .map(|service| {
                permanent.builtin_service_tool(service, true, SkillLibraryDescriptorMode::Hidden)
            })
            .collect();
        descriptors.extend([
            permanent.mcp_app_tool(true),
            permanent.add_server_tool(),
            permanent.gateway_status_tool(),
            permanent.code_mode_descriptor(&[]),
            permanent.code_mode_read_descriptor(&[]),
            permanent.code_mode_ui_tool(&[]),
        ]);

        let mut callable: Vec<String> = descriptors
            .iter()
            .filter(|tool| !upstream_destructive_from_annotations(tool.annotations.as_ref()))
            .map(|tool| tool.name.to_string())
            .collect();
        callable.sort();

        let mut expected: Vec<String> = expected_callable
            .iter()
            .filter(|name| {
                // Feature slices omit services they do not compile; the meta
                // tools are all gateway-gated alongside this test.
                services.service(name).is_some()
                    || !EXPECTED_SERVICE_ANNOTATIONS
                        .iter()
                        .any(|(row, ..)| row == *name)
            })
            .map(|name| (*name).to_string())
            .collect();
        expected.sort();

        assert_eq!(
            callable, expected,
            "the set of Labby tools a non-execute caller can reach through a \
             downstream gateway changed. This is an authorization change, not a \
             hint tweak — re-review F9 before updating this list."
        );
    }

    #[test]
    fn codemode_identity_is_registered_permanently() {
        let registry = PermanentToolRegistry::new();
        assert_eq!(
            registry.resolve(CODE_MODE_TOOL_NAME),
            Some(PermanentToolId::CodeMode)
        );
        assert_eq!(
            registry.resolve(CODE_MODE_READ_TOOL_NAME),
            Some(PermanentToolId::CodeModeRead)
        );
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn every_synthetic_tool_name_is_conservatively_reserved() {
        for name in [
            CODE_MODE_TOOL_NAME,
            CODE_MODE_READ_TOOL_NAME,
            CODE_MODE_UI_TOOL_NAME,
            MCP_APP_TOOL_NAME,
            ADD_SERVER_TOOL_NAME,
            GATEWAY_STATUS_TOOL_NAME,
            SETTINGS_TOOL_NAME,
        ] {
            assert!(is_reserved_non_upstream_tool_name(name), "{name}");
        }
        assert!(!is_reserved_non_upstream_tool_name(
            "ordinary-upstream-tool"
        ));
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn codemode_descriptor_is_dynamic_and_final_description_is_bounded() {
        let registry = PermanentToolRegistry::new();
        let descriptor = registry.code_mode_descriptor(&[]);
        let description = descriptor.description.expect("description");
        assert!(description.len() <= CODE_MODE_DESCRIPTION_MAX_BYTES);
        assert!(description.contains("codemode.search"));
        assert!(description.contains("nested upstream MCP Apps"));
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn codemode_read_descriptor_is_truthfully_annotated_and_bounded() {
        let descriptor = PermanentToolRegistry::new().code_mode_read_descriptor(&[]);
        let annotations = descriptor.annotations.expect("annotations");
        assert_eq!(annotations.read_only_hint, Some(true));
        assert_eq!(annotations.destructive_hint, Some(false));
        assert_eq!(annotations.idempotent_hint, Some(true));
        assert_eq!(annotations.open_world_hint, Some(true));
        assert!(
            descriptor.description.expect("description").len() <= CODE_MODE_DESCRIPTION_MAX_BYTES
        );
    }
}
#[derive(Serialize)]
struct OAuthSecurityScheme<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    scopes: &'a [&'a str],
}

/// Bind a descriptor advertised by Labby's protected MCP boundary to Labby's
/// OAuth policy. This applies to both Labby-owned and proxied upstream tools:
/// upstream authentication is terminated by the gateway and must not leak as
/// the public boundary's client-facing policy.
pub(crate) fn with_labby_security(mut tool: Tool) -> Tool {
    const DISCOVERY_SCOPES: &[&str] = &["lab:read"];
    let schemes = serde_json::to_value([OAuthSecurityScheme {
        kind: "oauth2",
        scopes: DISCOVERY_SCOPES,
    }])
    .expect("static OAuth security scheme serializes");
    tool.meta
        .get_or_insert_with(|| MetaObject(serde_json::Map::new()))
        .0
        .insert("securitySchemes".to_string(), schemes);
    tool
}
