use super::projection::service_surfaces;
use super::types::{ActionDoc, ParamDoc, SurfaceAvailability};
use crate::registry::RegisteredService;

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
                auth_posture: auth_posture(service.name, action.name),
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
) -> &'a [labby_apis::core::action::ActionSpec] {
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
    if service == "fs" && action == "fs.preview" {
        surfaces.cli = false;
        surfaces.mcp = false;
        surfaces.api = true;
        surfaces.web_ui = true;
    }
    surfaces
}

fn auth_posture(service: &str, action: &str) -> String {
    if service == "fs" && action == "fs.preview" {
        "HTTP-only admin/browser session path; intentionally unavailable on MCP".to_string()
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
        params,
        returns: if action == "schema" {
            "ActionSpec".to_string()
        } else {
            "HelpPayload".to_string()
        },
        surface_availability: surfaces.clone(),
        requires_http_subject: false,
        auth_posture: "uses the selected transport auth and gateway visibility policy".to_string(),
        inventory_scope: "global_inventory_not_active_runtime_exposure".to_string(),
        builtin: true,
    }
}
