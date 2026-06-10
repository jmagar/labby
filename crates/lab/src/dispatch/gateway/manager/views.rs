//! Read-only inspection surface: `list`, `get`, `status`, `test`, discovered
//! tool/resource/prompt views, surface gating checks, and client config export.

use crate::config::{LabConfig, UpstreamConfig};
use crate::dispatch::error::ToolError;
use crate::dispatch::gateway::SHARED_GATEWAY_OAUTH_SUBJECT;
use crate::dispatch::gateway::projection::*;
use crate::dispatch::gateway::types::{
    GatewayRuntimeView, GatewayToolExposureRowView, GatewayView, McpClientConfigView,
    McpClientTransportType,
};
use crate::dispatch::gateway::view_models::ServerView;
use crate::dispatch::upstream::pool::in_process_upstream_name;

use super::GatewayManager;
use super::virtual_servers::find_virtual_server;

const WARNING_UNKNOWN_SERVICE: &str = "unknown_service";

fn find_virtual_server_for_service<'a>(
    cfg: &'a LabConfig,
    service: &str,
) -> Option<&'a crate::config::VirtualServerConfig> {
    cfg.virtual_servers
        .iter()
        .find(|server| server.service == service || server.id == service)
}

impl GatewayManager {
    pub async fn list(&self) -> Result<Vec<ServerView>, ToolError> {
        let (cfg_guard, pool) = tokio::join!(self.config.read(), self.runtime.current_pool(),);
        let cfg = cfg_guard.clone();
        drop(cfg_guard);
        let mut views = Vec::with_capacity(cfg.upstream.len() + cfg.virtual_servers.len());
        for upstream in &cfg.upstream {
            views.push(server_view_from_upstream(pool.as_deref(), upstream).await);
        }
        for virtual_server in &cfg.virtual_servers {
            let peer_name = in_process_upstream_name(&virtual_server.service);
            let summary = upstream_summary(pool.as_deref(), &peer_name).await;
            let last_error = operator_visible_upstream_error(match pool.as_deref() {
                Some(pool) => pool.upstream_last_error(&peer_name).await,
                None => None,
            });
            views.push(server_view_from_virtual_server(
                virtual_server,
                summary,
                last_error,
                None,
            ));
        }
        let unknown_service_count = degraded_server_warning_count(&views, WARNING_UNKNOWN_SERVICE);
        if unknown_service_count > 0 {
            tracing::warn!(
                action = "gateway.list",
                unknown_service_count,
                "gateway list returned degraded rows with unknown services"
            );
        }
        Ok(views)
    }

    pub async fn get_server(&self, id: &str) -> Result<ServerView, ToolError> {
        let (cfg_guard, pool) = tokio::join!(self.config.read(), self.runtime.current_pool(),);
        let cfg = cfg_guard.clone();
        drop(cfg_guard);

        if let Some(upstream) = cfg.upstream.iter().find(|upstream| upstream.name == id) {
            return Ok(server_view_from_upstream(pool.as_deref(), upstream).await);
        }

        let virtual_server = find_virtual_server(&cfg, id)?;
        let peer_name = in_process_upstream_name(&virtual_server.service);
        let summary = upstream_summary(pool.as_deref(), &peer_name).await;
        let last_error = operator_visible_upstream_error(match pool.as_deref() {
            Some(pool) => pool.upstream_last_error(&peer_name).await,
            None => None,
        });
        Ok(server_view_from_virtual_server(
            virtual_server,
            summary,
            last_error,
            None,
        ))
    }

    pub async fn get(&self, name: &str) -> Result<GatewayView, ToolError> {
        let cfg = self.config.read().await;
        let code_mode = cfg.code_mode.clone();
        let upstream = cfg
            .upstream
            .iter()
            .find(|u| u.name == name)
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("gateway `{name}` not found"),
            })?
            .clone();
        drop(cfg);

        Ok(GatewayView {
            config: config_view(&upstream, &code_mode),
            runtime: runtime_view(
                self.runtime.current_pool().await.as_deref(),
                &upstream.name,
                None,
            )
            .await,
        })
    }

    pub async fn surface_enabled_for_service(&self, service: &str, surface: &str) -> bool {
        if self.registered_service_meta(service).is_none() {
            return true;
        }

        let cfg = self.config.read().await;
        let Some(virtual_server) = find_virtual_server_for_service(&cfg, service) else {
            return surface != "mcp";
        };

        if !virtual_server.enabled {
            return false;
        }

        match surface {
            "cli" => virtual_server.surfaces.cli,
            "api" => virtual_server.surfaces.api,
            "mcp" => virtual_server.surfaces.mcp,
            "webui" => virtual_server.surfaces.webui,
            _ => false,
        }
    }

    pub async fn allowed_mcp_actions_for_service(&self, service: &str) -> Option<Vec<String>> {
        if self.registered_service_meta(service).is_none() {
            return None;
        }

        let cfg = self.config.read().await;
        let virtual_server = find_virtual_server_for_service(&cfg, service)?;
        if !virtual_server.enabled || !virtual_server.surfaces.mcp {
            return Some(Vec::new());
        }

        if let Some(policy) = &virtual_server.mcp_policy
            && !policy.allowed_actions.is_empty()
        {
            let mut allowed = vec!["help".to_string(), "schema".to_string()];
            allowed.extend(policy.allowed_actions.clone());
            return Some(allowed);
        }

        None
    }

    pub async fn mcp_action_allowed_for_service(&self, service: &str, action: &str) -> bool {
        if self.registered_service_meta(service).is_none() {
            return true;
        }

        if !self.surface_enabled_for_service(service, "mcp").await {
            return false;
        }

        if matches!(action, "help" | "schema") {
            return true;
        }

        let cfg = self.config.read().await;
        let Some(virtual_server) = find_virtual_server_for_service(&cfg, service) else {
            return false;
        };

        match &virtual_server.mcp_policy {
            Some(policy) if !policy.allowed_actions.is_empty() => policy
                .allowed_actions
                .iter()
                .any(|allowed| allowed == action),
            _ => true,
        }
    }

    pub async fn status(&self, name: Option<&str>) -> Result<Vec<GatewayRuntimeView>, ToolError> {
        let upstreams: Vec<UpstreamConfig> = self
            .config
            .read()
            .await
            .upstream
            .iter()
            .filter(|u| name.is_none_or(|needle| needle == u.name))
            .cloned()
            .collect();
        let pool = self.runtime.current_pool().await;
        // P-M8: use the cached prompt-ownership snapshot instead of a live
        // prompts/list fan-out on every status poll (mirrors the resources fix
        // for lab-mzm2 — same pattern, same rationale).
        let prompt_owners = match pool.as_deref() {
            Some(p) => Some(p.cached_prompt_ownership_map().await),
            None => None,
        };
        let mut items = Vec::new();
        for upstream in &upstreams {
            items.push(runtime_view(pool.as_deref(), &upstream.name, prompt_owners.as_ref()).await);
        }
        Ok(items)
    }

    pub async fn test(
        &self,
        spec_or_name: Result<&UpstreamConfig, &str>,
    ) -> Result<GatewayRuntimeView, ToolError> {
        let upstream = match spec_or_name {
            Ok(spec) => spec.clone(),
            Err(name) => {
                let cfg = self.config.read().await;
                cfg.upstream
                    .iter()
                    .find(|u| u.name == name)
                    .cloned()
                    .ok_or_else(|| ToolError::Sdk {
                        sdk_kind: "not_found".to_string(),
                        message: format!("gateway `{name}` not found"),
                    })?
            }
        };

        let request_timeout = self.config.read().await.upstream_request_timeout();
        let pool = self.new_base_pool(request_timeout);
        let registry = self.builtin_service_registry();
        pool.discover_all_for_subject_ephemeral_with_in_process_peers(
            &[upstream.clone()],
            SHARED_GATEWAY_OAUTH_SUBJECT,
            &registry,
        )
        .await;

        let view = runtime_view(Some(&pool), &upstream.name, None).await;
        pool.drain_for_swap("gateway.test.ephemeral").await;
        Ok(view)
    }

    pub async fn client_config(&self, name: &str) -> Result<McpClientConfigView, ToolError> {
        let upstream = self
            .upstream_config(name)
            .await
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("gateway `{name}` not found"),
            })?;

        if let Some(url) = upstream.url.clone() {
            return Ok(McpClientConfigView {
                name: upstream.name,
                r#type: McpClientTransportType::Http,
                url: Some(url),
                command: None,
                args: None,
                env: None,
            });
        }

        let Some(command) = upstream.command.clone() else {
            return Err(ToolError::Sdk {
                sdk_kind: "invalid_config".to_string(),
                message: format!("gateway `{name}` has neither url nor command configured"),
            });
        };

        Ok(McpClientConfigView {
            name: upstream.name,
            r#type: McpClientTransportType::Stdio,
            url: None,
            command: Some(command),
            args: (!upstream.args.is_empty()).then_some(upstream.args),
            env: None,
        })
    }

    pub async fn discovered_tools(
        &self,
        name: &str,
    ) -> Result<Vec<GatewayToolExposureRowView>, ToolError> {
        let Some(pool) = self.runtime.current_pool().await else {
            return Ok(Vec::new());
        };

        Ok(pool
            .tool_exposure_rows(name)
            .await
            .into_iter()
            .map(|row| GatewayToolExposureRowView {
                name: row.name,
                description: row.description,
                exposed: row.exposed,
                matched_by: row.matched_by,
            })
            .collect())
    }

    pub async fn discovered_resources(&self, name: &str) -> Result<Vec<String>, ToolError> {
        let Some(pool) = self.runtime.current_pool().await else {
            return Ok(Vec::new());
        };
        // Serve from the cached resource URI snapshot to avoid a live fan-out
        // RPC burst on every admin inspection call (lab-mzm2).
        let all = pool.cached_upstream_resource_uris().await;
        let mut resources: Vec<String> = all
            .into_iter()
            .filter(|(upstream_name, _)| upstream_name == name)
            .flat_map(|(_, uris)| uris)
            .collect();
        resources.sort();
        Ok(resources)
    }

    pub async fn discovered_prompts(&self, name: &str) -> Result<Vec<String>, ToolError> {
        let Some(pool) = self.runtime.current_pool().await else {
            return Ok(Vec::new());
        };
        // Serve from the cached prompt name snapshot to avoid a live fan-out
        // RPC burst on every admin inspection call (lab-mzm2).
        let all = pool.cached_upstream_prompt_names_by_upstream().await;
        let mut prompts: Vec<String> = all
            .into_iter()
            .filter(|(upstream_name, _)| upstream_name == name)
            .flat_map(|(_, names)| names)
            .collect();
        prompts.sort();
        Ok(prompts)
    }

    pub async fn gateway_servers_doc(&self) -> Result<serde_json::Value, ToolError> {
        let Some(pool) = self.runtime.current_pool().await else {
            return Err(ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: "upstream pool not configured".to_string(),
            });
        };
        Ok(pool.gateway_servers_doc().await)
    }

    pub async fn gateway_server_schema(&self, name: &str) -> Result<serde_json::Value, ToolError> {
        let Some(pool) = self.runtime.current_pool().await else {
            return Err(ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: "upstream pool not configured".to_string(),
            });
        };
        pool.gateway_server_schema(name)
            .await
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("unknown upstream: {name}"),
            })
    }
}
