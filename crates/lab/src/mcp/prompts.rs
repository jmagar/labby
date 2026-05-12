//! MCP prompt templates.
//!
//! Two prompts for v1:
//! - `run-action` — structured action invocation template
//! - `service-discover` — service exploration starting point

use std::collections::HashMap;

use rmcp::model::{
    GetPromptResult, ListPromptsResult, Prompt, PromptArgument, PromptMessage, PromptMessageRole,
};

use crate::registry::{RegisteredService, ToolRegistry};

/// Return all registered prompt templates.
pub fn list_all() -> ListPromptsResult {
    ListPromptsResult::with_all_items(vec![run_action_prompt(), service_discover_prompt()])
}

/// Resolve a prompt by name, interpolating the supplied arguments.
pub fn get(
    registry: &ToolRegistry,
    name: &str,
    args: &HashMap<String, String>,
) -> Option<GetPromptResult> {
    match name {
        "run-action" => Some(render_run_action(registry, args)),
        "service-discover" => Some(render_service_discover(registry, args)),
        _ => None,
    }
}

// ── Prompt definitions ──────────────────────────────────────────────

fn run_action_prompt() -> Prompt {
    Prompt::new(
        "run-action",
        Some("Execute a lab service action with structured parameters".to_string()),
        Some(vec![
            PromptArgument::new("service")
                .with_description("Service name (e.g. radarr, sonarr)")
                .with_required(true),
            PromptArgument::new("action")
                .with_description("Action to perform (e.g. movie.search)")
                .with_required(true),
            PromptArgument::new("params")
                .with_description("JSON parameters for the action")
                .with_required(false),
        ]),
    )
}

fn service_discover_prompt() -> Prompt {
    Prompt::new(
        "service-discover",
        Some("Explore a lab service's capabilities and available actions".to_string()),
        Some(vec![
            PromptArgument::new("service")
                .with_description("Service name to explore")
                .with_required(true),
        ]),
    )
}

// ── Prompt renderers ────────────────────────────────────────────────

fn render_run_action(registry: &ToolRegistry, args: &HashMap<String, String>) -> GetPromptResult {
    let service_name = args.get("service").map_or("unknown", String::as_str);
    let action_name = args.get("action").map_or("help", String::as_str);
    let params = args.get("params").map_or("{}", String::as_str);
    let service = registry.service(service_name);
    let action = service.and_then(|service| {
        service
            .actions
            .iter()
            .find(|candidate| candidate.name == action_name)
    });

    let text = format!(
        "Use the `{service_name}` tool to execute `{action_name}`.\n\
         \n\
         Service context:\n\
         {}\n\
         \n\
         Action context:\n\
         {}\n\
         \n\
         Built-in discovery actions are always available:\n\
         - `help` lists every action with descriptions, params, and destructive flags\n\
         - `schema` returns the action parameter schema; pass `{{\"action\":\"schema\",\"params\":{{\"action\":\"{action_name}\"}}}}`\n\
         \n\
         Parameters to send now: {params}\n\
         \n\
         Use this exact tool payload:\n\
         ```json\n\
         {{\"action\": \"{action_name}\", \"params\": {params}}}\n\
         ```",
        service_summary(service_name, service),
        action_summary(action_name, action)
    );

    GetPromptResult::new(vec![PromptMessage::new_text(PromptMessageRole::User, text)])
        .with_description(format!("Run {service_name}.{action_name}"))
}

fn render_service_discover(
    registry: &ToolRegistry,
    args: &HashMap<String, String>,
) -> GetPromptResult {
    let service_name = args.get("service").map_or("unknown", String::as_str);
    let service = registry.service(service_name);

    let text = format!(
        "Explore the `{service_name}` service.\n\
         \n\
         {}\n\
         \n\
         Inline action catalog:\n\
         {}\n\
         \n\
         Built-in discovery actions are always available:\n\
         - `help` returns the complete action catalog with descriptions and flags\n\
         - `schema` returns parameter details for one action when called with `params.action`\n\
         \n\
         Start by summarizing the service's purpose, highlight destructive actions, and only call `help` or `schema` if you need more detail than the inline catalog provides.",
        service_summary(service_name, service),
        action_catalog(service)
    );

    GetPromptResult::new(vec![PromptMessage::new_text(PromptMessageRole::User, text)])
        .with_description(format!("Discover {service_name}"))
}

fn service_summary(service_name: &str, service: Option<&RegisteredService>) -> String {
    match service {
        Some(service) => format!(
            "- Description: {}\n- Category: {}\n- Status: {}",
            service.description, service.category, service.status
        ),
        None => format!(
            "- `{service_name}` is not in the current registry.\n- Use `help` to discover valid service names first."
        ),
    }
}

fn action_summary(
    action_name: &str,
    action: Option<&lab_apis::core::action::ActionSpec>,
) -> String {
    match action {
        Some(action) => format!(
            "- Description: {}\n- Destructive: {}\n- Returns: {}\n- Params: {}",
            action.description,
            if action.destructive { "yes" } else { "no" },
            action.returns,
            render_params(action.params)
        ),
        None => format!(
            "- `{action_name}` was not found in the current service catalog.\n- Use `schema` for the exact action payload once you confirm the name."
        ),
    }
}

fn action_catalog(service: Option<&RegisteredService>) -> String {
    match service {
        Some(service) if service.actions.is_empty() => {
            "- No actions are currently registered.".to_string()
        }
        Some(service) => service
            .actions
            .iter()
            .map(|action| {
                format!(
                    "- `{}`: {} [{}] params: {}",
                    action.name,
                    action.description,
                    if action.destructive {
                        "destructive"
                    } else {
                        "read-only"
                    },
                    render_params(action.params)
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        None => "- Service not found in the current registry.".to_string(),
    }
}

fn render_params(params: &[lab_apis::core::action::ParamSpec]) -> String {
    if params.is_empty() {
        return "none".to_string();
    }

    params
        .iter()
        .map(|param| {
            format!(
                "{}:{} ({}, {})",
                param.name,
                param.ty,
                if param.required {
                    "required"
                } else {
                    "optional"
                },
                param.description
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::{get, list_all};
    use crate::registry::{RegisteredService, ToolRegistry};
    use lab_apis::core::action::{ActionSpec, ParamSpec};
    use serde_json::Value;
    use std::collections::HashMap;

    const SEARCH_PARAMS: &[ParamSpec] = &[
        ParamSpec {
            name: "query",
            ty: "string",
            required: true,
            description: "Movie title to search for",
        },
        ParamSpec {
            name: "limit",
            ty: "integer",
            required: false,
            description: "Maximum results to return",
        },
    ];

    const ACTIONS: &[ActionSpec] = &[
        ActionSpec {
            name: "movie.search",
            description: "Search Radarr for movies",
            destructive: false,
            params: SEARCH_PARAMS,
            returns: "MovieSearchResult[]",
        },
        ActionSpec {
            name: "movie.delete",
            description: "Delete a movie from Radarr",
            destructive: true,
            params: &[],
            returns: "object",
        },
    ];

    fn registry() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register(RegisteredService {
            name: "radarr",
            description: "Manage movies and Radarr queues",
            category: "media",
            kind: crate::registry::RegisteredServiceKind::BuiltInUpstreamApi,
            status: "available",
            actions: ACTIONS,
            dispatch: |_action, _params| Box::pin(async { Ok(Value::Null) }),
        });
        registry
    }

    #[test]
    fn list_all_exposes_v1_prompts() {
        let prompts = list_all().prompts;
        let names: Vec<&str> = prompts.iter().map(|prompt| prompt.name.as_str()).collect();
        assert_eq!(names, vec!["run-action", "service-discover"]);
    }

    #[test]
    fn run_action_prompt_includes_catalog_context() {
        let mut args = HashMap::new();
        args.insert("service".to_string(), "radarr".to_string());
        args.insert("action".to_string(), "movie.search".to_string());

        let prompt = get(&registry(), "run-action", &args).expect("prompt should exist");
        let text = serde_json::to_string(&prompt.messages).expect("messages serialize");

        assert!(text.contains("Manage movies and Radarr queues"));
        assert!(text.contains("query:string (required, Movie title to search for)"));
        assert!(text.contains("limit:integer (optional, Maximum results to return)"));
        assert!(text.contains("Built-in discovery actions are always available"));
        assert!(text.contains("schema"));
        assert!(text.contains("movie.search"));
    }

    #[test]
    fn service_discover_prompt_inlines_action_catalog() {
        let mut args = HashMap::new();
        args.insert("service".to_string(), "radarr".to_string());

        let prompt = get(&registry(), "service-discover", &args).expect("prompt should exist");
        let text = serde_json::to_string(&prompt.messages).expect("messages serialize");

        assert!(text.contains("Inline action catalog"));
        assert!(text.contains("`movie.search`: Search Radarr for movies [read-only]"));
        assert!(text.contains("`movie.delete`: Delete a movie from Radarr [destructive]"));
        assert!(text.contains("schema"));
        assert!(!text.contains("First, call `radarr` with action `help`"));
    }
}
