//! Code Mode tool resolution: mapping `<upstream>::<tool>` selectors onto live
//! upstream catalog entries for `execute`/`callTool` and the raw tool proxy.

use std::collections::HashMap;

use crate::dispatch::error::ToolError;
use crate::dispatch::gateway::code_mode::split_upstream_tool;
use crate::dispatch::upstream::pool::tool_has_mcp_app_ui_resource;
use crate::dispatch::upstream::types::{UpstreamRuntimeOwner, UpstreamTool};

use super::GatewayManager;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CallbackToolLookup {
    LegacyAnyExposed,
    DirectMcpApp,
    SiblingOfMcpApp,
}

impl GatewayManager {
    pub(crate) async fn resolve_widget_callback_tool_candidates_scoped(
        &self,
        tool: &str,
        allowed_upstreams: Option<&std::collections::BTreeSet<String>>,
        _owner: Option<&UpstreamRuntimeOwner>,
        _oauth_subject: Option<&str>,
        lookup: CallbackToolLookup,
    ) -> Result<Vec<(String, UpstreamTool)>, ToolError> {
        let cfg = self.config.read().await.clone();
        let Some(pool) = self.current_pool().await else {
            return Ok(Vec::new());
        };

        let mut matches = Vec::new();
        for upstream in cfg.upstream.iter().filter(|upstream| {
            upstream.enabled
                && is_routable(upstream.priority)
                && allowed_upstreams.is_none_or(|allowed| allowed.contains(&upstream.name))
        }) {
            let upstream_tools = pool.healthy_tools_for_upstream(&upstream.name).await;
            let Some(candidate) = upstream_tools
                .iter()
                .find(|candidate| candidate.tool.name.as_ref() == tool)
            else {
                continue;
            };

            let matched = match lookup {
                CallbackToolLookup::LegacyAnyExposed => true,
                CallbackToolLookup::DirectMcpApp => tool_has_mcp_app_ui_resource(candidate),
                CallbackToolLookup::SiblingOfMcpApp => {
                    upstream_tools.iter().any(tool_has_mcp_app_ui_resource)
                }
            };
            if matched {
                matches.push((upstream.name.clone(), candidate.clone()));
            }
        }
        matches.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(matches)
    }

    pub async fn resolve_code_mode_upstream_tool(
        &self,
        upstream: &str,
        tool: &str,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
    ) -> Result<UpstreamTool, ToolError> {
        let cfg = self.config.read().await;
        // The gateway search/execute surface is gated by the single `code_mode.enabled`
        // toggle, which also exposes the tools. `execute` is only reachable when the
        // surface is exposed, so reject when it is off. This is the single-surface
        // (Cloudflare-parity) model: when search + execute are on, callTool resolution works.
        if !cfg.code_mode.enabled {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: "the gateway search/execute surface is not enabled; \
                    set [code_mode] enabled = true in config"
                    .to_string(),
            });
        }
        let upstream_config = cfg
            .upstream
            .iter()
            .find(|candidate| candidate.name == upstream)
            .cloned();
        drop(cfg);

        let priority = upstream_config.as_ref().map(|c| c.priority).unwrap_or(1.0);
        if !is_routable(priority) {
            tracing::warn!(
                surface = "dispatch",
                service = "gateway",
                action = "code_mode.resolve_tool",
                upstream = %upstream,
                tool = %tool,
                priority = priority,
                "skipping tool resolution: upstream priority is non-positive (disabled)"
            );
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: format!("upstream tool `{upstream}::{tool}` was not found"),
            });
        }

        self.ensure_upstream_tool_runtime_ready(upstream, owner, oauth_subject)
            .await?;
        let pool = self.current_pool().await.ok_or_else(|| ToolError::Sdk {
            sdk_kind: "unknown_tool".to_string(),
            message: format!("upstream tool `{upstream}::{tool}` was not found"),
        })?;

        pool.healthy_tools_for_upstream(upstream)
            .await
            .into_iter()
            .find(|candidate| candidate.tool.name.as_ref() == tool)
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: format!("upstream tool `{upstream}::{tool}` was not found"),
            })
    }

    pub async fn resolve_raw_upstream_tool(
        &self,
        tool: &str,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
    ) -> Result<(String, UpstreamTool), ToolError> {
        let selector = ToolExecuteSelector::parse(tool, None)?;
        let cfg = self.config.read().await.clone();
        let priority_by_upstream: HashMap<String, f32> = cfg
            .upstream
            .iter()
            .map(|upstream| (upstream.name.clone(), upstream.priority))
            .collect();

        let Some(pool) = self.current_pool().await else {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: format!("unknown tool `{}`", selector.display_name()),
            });
        };

        if let Some(upstream_name) = selector.upstream.as_deref() {
            let priority = priority_by_upstream
                .get(upstream_name)
                .copied()
                .unwrap_or(1.0);
            if !is_routable(priority) {
                tracing::warn!(
                    surface = "dispatch",
                    service = "gateway",
                    action = "tool_execute.resolve_tool",
                    upstream = %upstream_name,
                    tool = %selector.tool_name,
                    priority = priority,
                    "skipping tool resolution: upstream priority is non-positive (disabled)"
                );
                return Err(ToolError::Sdk {
                    sdk_kind: "unknown_tool".to_string(),
                    message: format!("unknown tool `{}`", selector.display_name()),
                });
            }
            self.ensure_upstream_tool_runtime_ready(upstream_name, owner, oauth_subject)
                .await?;
            return pool
                .healthy_tools_for_upstream(upstream_name)
                .await
                .into_iter()
                .find(|candidate| candidate.tool.name.as_ref() == selector.tool_name)
                .map(|tool| (upstream_name.to_string(), tool))
                .ok_or_else(|| ToolError::Sdk {
                    sdk_kind: "unknown_tool".to_string(),
                    message: format!("unknown tool `{}`", selector.display_name()),
                });
        }

        if let Some((upstream, tool)) = pool.find_tool(&selector.tool_name).await
            && is_routable(priority_by_upstream.get(&upstream).copied().unwrap_or(1.0))
        {
            return Ok((upstream, tool));
        }

        let mut matches = Vec::new();
        for upstream in cfg
            .upstream
            .iter()
            .filter(|upstream| upstream.enabled && is_routable(upstream.priority))
        {
            self.ensure_upstream_tool_runtime_ready(&upstream.name, owner, oauth_subject)
                .await?;
            matches.extend(
                pool.healthy_tools_for_upstream(&upstream.name)
                    .await
                    .into_iter()
                    .filter(|candidate| candidate.tool.name.as_ref() == selector.tool_name)
                    .map(|tool| (upstream.name.clone(), tool)),
            );
        }

        if matches.is_empty() {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: format!("unknown tool `{}`", selector.display_name()),
            });
        }
        if matches.len() > 1 {
            let valid = matches
                .iter()
                .map(|(upstream, tool)| format!("{upstream}::{}", tool.tool.name))
                .collect::<Vec<_>>();
            return Err(ToolError::AmbiguousTool {
                message: format!(
                    "tool `{}` matched multiple upstream tools",
                    selector.tool_name
                ),
                valid,
            });
        }
        Ok(matches.into_iter().next().expect("checked len"))
    }

    pub async fn resolve_raw_upstream_tool_scoped(
        &self,
        tool: &str,
        allowed_upstreams: Option<&std::collections::BTreeSet<String>>,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
    ) -> Result<(String, UpstreamTool), ToolError> {
        if allowed_upstreams.is_none() {
            return self
                .resolve_raw_upstream_tool(tool, owner, oauth_subject)
                .await;
        }

        let selector = ToolExecuteSelector::parse(tool, None)?;
        let allowed = allowed_upstreams.expect("checked some");
        let cfg = self.config.read().await.clone();
        let Some(pool) = self.current_pool().await else {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: format!("unknown tool `{}`", selector.display_name()),
            });
        };

        if let Some(upstream_name) = selector.upstream.as_deref() {
            if !allowed.contains(upstream_name) {
                return Err(ToolError::Sdk {
                    sdk_kind: "unknown_tool".to_string(),
                    message: format!("unknown tool `{}`", selector.display_name()),
                });
            }
            if cfg
                .upstream
                .iter()
                .find(|candidate| candidate.name == upstream_name)
                .is_some_and(|candidate| !is_routable(candidate.priority))
            {
                return Err(ToolError::Sdk {
                    sdk_kind: "unknown_tool".to_string(),
                    message: format!("unknown tool `{}`", selector.display_name()),
                });
            }
            self.ensure_upstream_tool_runtime_ready(upstream_name, owner, oauth_subject)
                .await?;
            return pool
                .healthy_tools_for_upstream(upstream_name)
                .await
                .into_iter()
                .find(|candidate| candidate.tool.name.as_ref() == selector.tool_name)
                .map(|tool| (upstream_name.to_string(), tool))
                .ok_or_else(|| ToolError::Sdk {
                    sdk_kind: "unknown_tool".to_string(),
                    message: format!("unknown tool `{}`", selector.display_name()),
                });
        }

        if let Some((upstream, tool)) = pool
            .find_tool_allowed(&selector.tool_name, Some(allowed))
            .await
            && cfg
                .upstream
                .iter()
                .find(|candidate| candidate.name == upstream)
                .is_none_or(|candidate| is_routable(candidate.priority))
        {
            return Ok((upstream, tool));
        }

        let mut matches = Vec::new();
        for upstream in cfg
            .upstream
            .iter()
            .filter(|upstream| upstream.enabled && allowed.contains(&upstream.name))
            .filter(|upstream| is_routable(upstream.priority))
        {
            self.ensure_upstream_tool_runtime_ready(&upstream.name, owner, oauth_subject)
                .await?;
            matches.extend(
                pool.healthy_tools_for_upstream(&upstream.name)
                    .await
                    .into_iter()
                    .filter(|candidate| candidate.tool.name.as_ref() == selector.tool_name)
                    .map(|tool| (upstream.name.clone(), tool)),
            );
        }

        if matches.is_empty() {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: format!("unknown tool `{}`", selector.display_name()),
            });
        }
        if matches.len() > 1 {
            let valid = matches
                .iter()
                .map(|(upstream, tool)| format!("{upstream}::{}", tool.tool.name))
                .collect::<Vec<_>>();
            return Err(ToolError::AmbiguousTool {
                message: format!(
                    "tool `{}` matched multiple upstream tools",
                    selector.tool_name
                ),
                valid,
            });
        }
        Ok(matches.into_iter().next().expect("checked len"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolExecuteSelector {
    upstream: Option<String>,
    tool_name: String,
}

impl ToolExecuteSelector {
    /// Parse a tool selector of the form `[<upstream>::]<tool>` or a bare tool
    /// name. When an explicit `upstream` hint is provided it takes precedence
    /// over an embedded `<upstream>::` prefix in `name`.
    ///
    /// The `<upstream>::<tool>` splitting is delegated to
    /// [`split_upstream_tool`] (from `code_mode::types`) so the two callers
    /// share one implementation.
    fn parse(name: &str, upstream: Option<&str>) -> Result<Self, ToolError> {
        let explicit_upstream = upstream.map(str::trim).filter(|value| !value.is_empty());
        let trimmed_name = name.trim();
        if trimmed_name.is_empty() {
            return Err(ToolError::Sdk {
                sdk_kind: "invalid_param".to_string(),
                message: "tool name must not be empty".to_string(),
            });
        }

        if let Some(upstream_name) = explicit_upstream {
            let tool_name = trimmed_name
                .strip_prefix(upstream_name)
                .and_then(|rest| rest.strip_prefix("::"))
                .unwrap_or(trimmed_name)
                .trim();
            if tool_name.is_empty() {
                return Err(ToolError::Sdk {
                    sdk_kind: "invalid_param".to_string(),
                    message: "tool name must not be empty".to_string(),
                });
            }
            return Ok(Self {
                upstream: Some(upstream_name.to_string()),
                tool_name: tool_name.to_string(),
            });
        }

        // Use the shared `<upstream>::<tool>` splitter from `code_mode::types`
        // instead of an inline `split_once("..")` so the two implementations
        // stay in sync (e.g. both reject `a::b::c` and empty segments).
        if trimmed_name.contains("::") {
            return match split_upstream_tool(trimmed_name) {
                Some((upstream_name, tool_name)) => Ok(Self {
                    upstream: Some(upstream_name.to_string()),
                    tool_name: tool_name.to_string(),
                }),
                None => Err(ToolError::Sdk {
                    sdk_kind: "invalid_param".to_string(),
                    message: "qualified tool names must use `<upstream>::<tool>`".to_string(),
                }),
            };
        }

        Ok(Self {
            upstream: None,
            tool_name: trimmed_name.to_string(),
        })
    }

    fn display_name(&self) -> String {
        match &self.upstream {
            Some(upstream) => format!("{upstream}::{}", self.tool_name),
            None => self.tool_name.clone(),
        }
    }
}

/// Returns `true` when `priority` makes an upstream eligible for tool
/// resolution.
///
/// A non-positive priority (`<= 0.0`) is the conventional way to disable an
/// upstream without removing it from the config. The named predicate makes the
/// intent explicit at every check site and avoids the subtle risk of a
/// misread `> 0.0` / `<= 0.0` comparison.
#[inline]
fn is_routable(priority: f32) -> bool {
    priority > 0.0
}
