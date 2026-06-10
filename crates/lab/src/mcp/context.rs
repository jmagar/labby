//! Request-context, auth-subject, and scope/admin gate helpers.
//!
//! Extracted from `server.rs` (bead `lab-kvji.24.1.1`). Holds:
//! - inherent `impl LabMcpServer` request-context methods (Rust permits
//!   multiple inherent impl blocks for one struct across files; the trait
//!   impl stays single-file in `server.rs`),
//! - free auth-extraction helpers,
//! - the scope/admin gate fns (widened to `pub(crate)` per Revision 2 so
//!   `call_tool*`/resource helpers can call them — visibility change only,
//!   no logic change).

use std::borrow::Cow;

use axum::http::request::Parts;
use rmcp::RoleServer;
use rmcp::service::RequestContext;
use sha2::{Digest, Sha256};

use crate::dispatch::gateway::SHARED_GATEWAY_OAUTH_SUBJECT;
use crate::dispatch::gateway::code_mode::CodeModeSurface;
use crate::dispatch::upstream::types::UpstreamRuntimeOwner;
use crate::mcp::server::LabMcpServer;

pub(crate) fn redact_subject_for_logging(subject: &str) -> String {
    let digest = Sha256::digest(subject.as_bytes());
    format!("sub:{}", hex::encode(digest))[..16].to_string()
}

impl LabMcpServer {
    pub(crate) fn code_mode_surface(&self) -> CodeModeSurface {
        CodeModeSurface::Mcp
    }

    pub(crate) fn request_subject<'a>(
        &self,
        context: &'a RequestContext<RoleServer>,
    ) -> Option<&'a str> {
        subject_from_extensions(&context.extensions)
    }

    pub(crate) fn request_subject_log_tag(&self, context: &RequestContext<RoleServer>) -> String {
        self.request_subject(context)
            .map(redact_subject_for_logging)
            .unwrap_or_default()
    }

    pub(crate) fn request_actor_key<'a>(
        &self,
        context: &'a RequestContext<RoleServer>,
    ) -> Option<&'a str> {
        actor_key_from_extensions(&context.extensions)
    }

    pub(crate) fn request_runtime_owner(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> UpstreamRuntimeOwner {
        let subject = self.request_subject(context).map(ToOwned::to_owned);
        let raw = subject
            .as_ref()
            .map(|subject| format!("mcp:{subject}"))
            .unwrap_or_else(|| "mcp:anonymous".to_string());
        UpstreamRuntimeOwner {
            surface: "mcp".to_string(),
            subject,
            request_id: None,
            session_id: None,
            client_name: None,
            raw: Some(raw),
        }
    }

    pub(crate) async fn oauth_upstream_configs(&self) -> Vec<crate::config::UpstreamConfig> {
        match &self.gateway_manager {
            Some(manager) => manager.oauth_upstream_configs().await,
            None => Vec::new(),
        }
    }

    pub(crate) async fn oauth_upstream_config(
        &self,
        upstream_name: &str,
    ) -> Option<crate::config::UpstreamConfig> {
        match &self.gateway_manager {
            Some(manager) => manager.oauth_upstream_config(upstream_name).await,
            None => None,
        }
    }
}

pub(crate) fn subject_from_extensions(extensions: &rmcp::model::Extensions) -> Option<&str> {
    auth_context_from_extensions(extensions).map(|auth| auth.sub.as_str())
}

pub(crate) fn actor_key_from_extensions(extensions: &rmcp::model::Extensions) -> Option<&str> {
    auth_context_from_extensions(extensions).and_then(|auth| auth.actor_key.as_deref())
}

pub(crate) fn auth_context_from_extensions(
    extensions: &rmcp::model::Extensions,
) -> Option<&crate::api::oauth::AuthContext> {
    let parts = extensions.get::<Parts>()?;
    parts.extensions.get::<crate::api::oauth::AuthContext>()
}

pub(crate) fn oauth_upstream_subject_for_request<'a>(
    auth: Option<&crate::api::oauth::AuthContext>,
    request_subject: Option<&'a str>,
) -> Option<Cow<'a, str>> {
    match auth {
        None => Some(Cow::Borrowed(SHARED_GATEWAY_OAUTH_SUBJECT)),
        Some(ctx) if ctx.scopes.iter().any(|scope| scope == "lab:admin") => {
            Some(Cow::Borrowed(SHARED_GATEWAY_OAUTH_SUBJECT))
        }
        Some(_) => request_subject.map(Cow::Borrowed),
    }
}

pub(crate) fn tool_execute_scope_allowed(auth: Option<&crate::api::oauth::AuthContext>) -> bool {
    auth.is_none_or(|auth| {
        auth.scopes
            .iter()
            .any(|scope| matches!(scope.as_str(), "lab" | "lab:admin"))
    })
}

/// Returns `true` when the caller is allowed to invoke the code_mode tool.
///
/// code_mode requires at least `lab:read`; tool_execute requires the stronger `lab` or `lab:admin`.
/// `None` auth means stdio transport — trusted by design (no per-request AuthContext).
pub(crate) fn code_mode_search_scope_allowed(
    auth: Option<&crate::api::oauth::AuthContext>,
) -> bool {
    auth.is_none_or(|auth| {
        auth.scopes
            .iter()
            .any(|scope| matches!(scope.as_str(), "lab:read" | "lab" | "lab:admin"))
    })
}

pub(crate) fn tool_execute_builtin_action_allowed(
    entry: &crate::registry::RegisteredService,
    action: &str,
    auth: Option<&crate::api::oauth::AuthContext>,
) -> bool {
    if !builtin_action_requires_admin(entry, action) {
        return true;
    }
    auth.is_none_or(|auth| auth.scopes.iter().any(|scope| scope == "lab:admin"))
}

pub(crate) fn builtin_action_requires_admin(
    entry: &crate::registry::RegisteredService,
    action: &str,
) -> bool {
    // Gateway and setup use catalog-driven requires_admin / destructive metadata
    // as the single source of truth (A-H2 / S5 fix: no bespoke match arms).
    if entry.name == "gateway" {
        // The universal built-ins are never admin-gated, whether the caller
        // passes them bare (`help`) or service-prefixed (`gateway.help`).  The
        // catalog stores them bare, so strip any `gateway.` prefix before the
        // discovery check.
        let bare = action.strip_prefix("gateway.").unwrap_or(action);
        if bare == "help" || bare == "schema" {
            return false;
        }
        return entry
            .actions
            .iter()
            .find(|spec| spec.name == action)
            .map(|spec| spec.requires_admin)
            // Unknown actions default to admin-required (fail-safe).
            .unwrap_or(true);
    }
    entry.name == "setup"
        && entry
            .actions
            .iter()
            .any(|spec| spec.name == action && spec.destructive)
}

#[cfg(test)]
mod tests;
