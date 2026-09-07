use super::projection::service_surfaces;
use super::types::{ActionDoc, ParamDoc, SurfaceAvailability};
use crate::registry::RegisteredService;

/// Product actions reached by a concrete CLI command.
///
/// This is intentionally narrower than the set of actions registered by a
/// CLI-capable service. Keep it aligned with the command-to-dispatch bindings
/// in `crate::cli`; an action is not a CLI surface merely because another
/// action from the same service has a CLI adapter.
const CLI_ACTION_BINDINGS: &[(&str, &str)] = &[
    ("doctor", "audit.full"),
    ("doctor", "auth.check"),
    ("doctor", "oauth.relay.check"),
    ("doctor", "proxy.check"),
    ("doctor", "proxy.preflight"),
    ("doctor", "system.checks"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.add"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.clients.list"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.code_mode.get"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.code_mode.set"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.discover"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.enrich.apply"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.enrich.preview"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.get"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.import"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.import_pending.approve"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.import_pending.list"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.import_pending.reject"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.list"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.loadout.add"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.loadout.get"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.loadout.list_state"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.loadout.patch"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.loadout.remove"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.loadout.stage_patch"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.loadout.stage_remove"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.mcp.cleanup"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.mcp.disable"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.mcp.enable"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.mcp.list"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.mcp.restart"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.oauth.clear"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.oauth.google_revoke"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.oauth.start"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.oauth.status"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.oauth.wait"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.protected_route.add"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.protected_route.get"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.protected_route.list_state"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.protected_route.remove"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.protected_route.stage_add"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.protected_route.stage_remove"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.protected_route.stage_update"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.protected_route.test"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.protected_route.update"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.public_urls.get"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.reload"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.remove"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.skills.list"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.test"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.update"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.usage.calls"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.usage.metrics"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.virtual_server.quarantine.list"),
    #[cfg(feature = "gateway")]
    ("gateway", "gateway.virtual_server.quarantine.restore"),
    ("server_logs", "server_logs.query"),
    ("setup", "check"),
    ("setup", "draft.discard"),
    ("setup", "plugin.install"),
    ("setup", "plugin.uninstall"),
    ("setup", "plugin_connectivity"),
    ("setup", "plugin_export"),
    ("setup", "plugin_hook"),
    ("setup", "plugin_sync"),
    ("setup", "plugins.installed"),
    ("setup", "proxy.configure"),
    ("setup", "repair"),
    ("setup", "services.status"),
    ("setup", "state"),
    #[cfg(feature = "gateway")]
    ("snippets", "snippets.create"),
    #[cfg(feature = "gateway")]
    ("snippets", "snippets.exec"),
    #[cfg(feature = "gateway")]
    ("snippets", "snippets.get"),
    #[cfg(feature = "gateway")]
    ("snippets", "snippets.list"),
    #[cfg(feature = "gateway")]
    ("snippets", "snippets.remove"),
    #[cfg(feature = "gateway")]
    ("snippets", "snippets.test"),
    #[cfg(feature = "gateway")]
    ("snippets", "snippets.validate"),
];

const WEB_ACTION_CLIENT_SOURCES: &[&str] = &[
    include_str!("../../../../apps/gateway-admin/lib/api/doctor-client.ts"),
    include_str!("../../../../apps/gateway-admin/lib/api/gateway-client.ts"),
    include_str!("../../../../apps/gateway-admin/lib/api/metrics-client.ts"),
    include_str!("../../../../apps/gateway-admin/lib/api/server-logs-client.ts"),
    include_str!("../../../../apps/gateway-admin/lib/api/setup-client.ts"),
    include_str!("../../../../apps/gateway-admin/lib/api/snippets-client.ts"),
    include_str!("../../../../apps/gateway-admin/lib/api/skill-library-client.ts"),
    include_str!("../../../../apps/gateway-admin/lib/api/artifact-control-client.ts"),
    include_str!("../../../../apps/gateway-admin/components/skills/artifact-control-plane.tsx"),
    include_str!("../../../../apps/gateway-admin/lib/fs/client.ts"),
];

/// Stash uses dedicated REST routes instead of the generic action endpoint.
/// Keep this list aligned with `apps/gateway-admin/lib/stash/client.ts`; only
/// registered actions that the web client invokes through those routes belong
/// here.
const STASH_WEB_ACTION_BINDINGS: &[(&str, &str)] = &[
    ("stash", "stash.delete"),
    ("stash", "stash.grants.create"),
    ("stash", "stash.grants.list"),
    ("stash", "stash.grants.revoke"),
    ("stash", "stash.list"),
    ("stash", "stash.rename"),
    ("stash", "stash.search"),
    ("stash", "stash.stats"),
];

#[cfg(test)]
const STASH_WEB_CLIENT_SOURCE: &str =
    include_str!("../../../../apps/gateway-admin/lib/stash/client.ts");

#[cfg(test)]
const CLI_DISPATCH_SOURCES: &[&str] = &[
    include_str!("../cli/doctor.rs"),
    include_str!("../cli/gateway/code.rs"),
    include_str!("../cli/gateway/dispatch.rs"),
    include_str!("../cli/gateway/oauth.rs"),
    include_str!("../cli/setup.rs"),
    include_str!("../cli/skills.rs"),
    include_str!("../cli/snippets.rs"),
];

pub(super) fn build_action_catalog(services: &[RegisteredService]) -> Vec<ActionDoc> {
    let mut actions = Vec::new();
    for service in services {
        let surfaces = service_surfaces(service.name);
        let service_actions = canonical_actions_for_service(service);
        if !service_actions.iter().any(|action| action.name == "help") {
            actions.push(builtin_action(
                service.name,
                "help",
                "Show service actions",
                &surfaces,
            ));
        }
        if !service_actions.iter().any(|action| action.name == "schema") {
            actions.push(builtin_action(
                service.name,
                "schema",
                "Show the schema for a specific action",
                &surfaces,
            ));
        }
        for action in service_actions {
            let action_surfaces = action_surfaces(service.name, action.name, &surfaces);
            actions.push(ActionDoc {
                service: service.name.to_string(),
                action: action.name.to_string(),
                description: action.description.to_string(),
                destructive: action.destructive,
                requires_admin: action.requires_admin,
                required_scopes: if action.requires_admin {
                    vec!["lab:admin".to_string()]
                } else {
                    Vec::new()
                },
                params: action
                    .params
                    .iter()
                    .map(|param| ParamDoc {
                        name: param.name.to_string(),
                        ty: param.ty.to_string(),
                        required: param.required,
                        description: param.description.to_string(),
                    })
                    .collect(),
                returns: action.returns.to_string(),
                surface_availability: action_surfaces,
                requires_http_subject: service.name == "fs" && action.name == "fs.preview",
                auth_posture: auth_posture(service.name, action.name, action.requires_admin),
                inventory_scope: "global_inventory_not_active_runtime_exposure".to_string(),
                builtin: false,
            });
        }
    }
    actions.sort_by(|a, b| {
        (a.service.as_str(), a.action.as_str()).cmp(&(b.service.as_str(), b.action.as_str()))
    });
    actions
}

fn canonical_actions_for_service<'a>(
    service: &'a RegisteredService,
) -> &'a [labby_primitives::action::ActionSpec] {
    #[cfg(feature = "fs")]
    if service.name == "fs" {
        return crate::dispatch::fs::catalog::ACTIONS;
    }
    service.actions
}

fn action_surfaces(
    service: &str,
    action: &str,
    service_surfaces: &SurfaceAvailability,
) -> SurfaceAvailability {
    let mut surfaces = service_surfaces.clone();
    surfaces.cli = CLI_ACTION_BINDINGS.contains(&(service, action));
    surfaces.web_ui &= web_action_bound(service, action);
    if service == "fs" && action == "fs.preview" {
        surfaces.mcp = false;
        surfaces.api = true;
        surfaces.web_ui = true;
    }
    surfaces
}

fn web_action_bound(service: &str, action: &str) -> bool {
    STASH_WEB_ACTION_BINDINGS.contains(&(service, action))
        || binding_literal_exists(WEB_ACTION_CLIENT_SOURCES, action)
        || (service == "fs"
            && action == "fs.preview"
            && WEB_ACTION_CLIENT_SOURCES.iter().any(|source| {
                source.contains("'/v1/fs/preview'") || source.contains("\"/v1/fs/preview\"")
            }))
}

fn binding_literal_exists(sources: &[&str], action: &str) -> bool {
    let single_quoted = format!("'{action}'");
    let double_quoted = format!("\"{action}\"");
    sources
        .iter()
        .any(|source| source.contains(&single_quoted) || source.contains(&double_quoted))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn cli_action_bindings_are_unique_registered_actions() {
        let bindings = CLI_ACTION_BINDINGS.iter().copied().collect::<BTreeSet<_>>();
        assert_eq!(bindings.len(), CLI_ACTION_BINDINGS.len());

        let registry = crate::registry::build_docs_registry();
        let actions = build_action_catalog(registry.services());
        let projected = actions
            .iter()
            .filter(|action| action.surface_availability.cli)
            .map(|action| (action.service.as_str(), action.action.as_str()))
            .collect::<BTreeSet<_>>();
        assert_eq!(projected, bindings);
        assert!(
            actions
                .iter()
                .all(|action| { !action.builtin || !action.surface_availability.cli })
        );
    }

    #[cfg(feature = "all")]
    #[test]
    fn all_features_cli_action_denominator_is_exact() {
        assert_eq!(CLI_ACTION_BINDINGS.len(), 76);
    }

    #[test]
    fn cli_catalog_covers_actions_named_by_compiled_dispatch_adapters() {
        let registry = crate::registry::build_docs_registry();
        let actions = build_action_catalog(registry.services());
        let projected = actions
            .iter()
            .filter(|action| action.surface_availability.cli)
            .map(|action| (action.service.as_str(), action.action.as_str()))
            .collect::<BTreeSet<_>>();
        let source_bound = actions
            .iter()
            .filter(|action| !action.builtin)
            .filter(|action| binding_literal_exists(CLI_DISPATCH_SOURCES, &action.action))
            .map(|action| (action.service.as_str(), action.action.as_str()))
            .collect::<BTreeSet<_>>();
        let direct_adapters = BTreeSet::from([
            ("doctor", "audit.full"),
            ("doctor", "auth.check"),
            ("doctor", "oauth.relay.check"),
            ("doctor", "system.checks"),
            ("server_logs", "server_logs.query"),
        ]);
        assert_eq!(
            projected,
            source_bound.union(&direct_adapters).copied().collect()
        );
    }

    #[test]
    fn web_catalog_is_derived_from_concrete_frontend_client_bindings() {
        let registry = crate::registry::build_docs_registry();
        let actions = build_action_catalog(registry.services());
        let projected = actions
            .iter()
            .filter(|action| action.surface_availability.web_ui)
            .map(|action| (action.service.as_str(), action.action.as_str()))
            .collect::<BTreeSet<_>>();
        assert!(
            projected
                .iter()
                .all(|(service, action)| web_action_bound(service, action))
        );
        assert!(!projected.is_empty());
    }

    #[test]
    fn stash_web_catalog_matches_direct_route_client() {
        let registry = crate::registry::build_docs_registry();
        let actions = build_action_catalog(registry.services());
        let projected = actions
            .iter()
            .filter(|action| action.service == "stash" && action.surface_availability.web_ui)
            .map(|action| (action.service.as_str(), action.action.as_str()))
            .collect::<BTreeSet<_>>();
        let expected = STASH_WEB_ACTION_BINDINGS
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();

        assert_eq!(projected, expected);
        for route_fragment in [
            "request(`/?${query}`",
            "request('/stats'",
            "method: 'PATCH'",
            "method: 'DELETE'",
            "/grants?${query}`",
            "/grants`,",
            "/grants/${encodeURIComponent(grantId)}`",
        ] {
            assert!(
                STASH_WEB_CLIENT_SOURCE.contains(route_fragment),
                "stash web client no longer contains direct route binding {route_fragment}"
            );
        }
    }
}

fn auth_posture(service: &str, action: &str, requires_admin: bool) -> String {
    if service == "fs" && action == "fs.preview" {
        "HTTP-only admin/browser session path; intentionally unavailable on MCP".to_string()
    } else if requires_admin {
        "requires lab:admin in addition to the selected transport authentication".to_string()
    } else {
        "uses the selected transport auth and gateway visibility policy".to_string()
    }
}

fn builtin_action(
    service: &str,
    action: &str,
    description: &str,
    surfaces: &SurfaceAvailability,
) -> ActionDoc {
    let mut surfaces = surfaces.clone();
    surfaces.cli = false;
    surfaces.web_ui = false;
    let params = if action == "schema" {
        vec![ParamDoc {
            name: "action".to_string(),
            ty: "string".to_string(),
            required: true,
            description: "Action name to describe".to_string(),
        }]
    } else {
        Vec::new()
    };
    ActionDoc {
        service: service.to_string(),
        action: action.to_string(),
        description: description.to_string(),
        destructive: false,
        requires_admin: false,
        required_scopes: Vec::new(),
        params,
        returns: if action == "schema" {
            "ActionSpec".to_string()
        } else {
            "HelpPayload".to_string()
        },
        surface_availability: surfaces,
        requires_http_subject: false,
        auth_posture: "uses the selected transport auth and gateway visibility policy".to_string(),
        inventory_scope: "global_inventory_not_active_runtime_exposure".to_string(),
        builtin: true,
    }
}
