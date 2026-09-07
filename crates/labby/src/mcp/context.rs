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

use axum::http::request::Parts;
use labby_auth::auth_context::AuthContext;
use labby_runtime::caller_auth::{CALLER_AUTH_META_KEY, PropagatedCallerAuth};
use rmcp::RoleServer;
use rmcp::service::RequestContext;
use sha2::{Digest, Sha256};

#[cfg(feature = "gateway")]
use crate::dispatch::gateway::code_mode::CodeModeSurface;
#[cfg(feature = "gateway")]
use crate::dispatch::upstream::types::UpstreamRuntimeOwner;
use crate::mcp::server::LabMcpServer;

#[cfg(feature = "gateway")]
pub(crate) use crate::dispatch::oauth_subject::oauth_upstream_subject_for_request;

pub(crate) fn redact_subject_for_logging(subject: &str) -> String {
    let digest = Sha256::digest(subject.as_bytes());
    format!("sub:{}", hex::encode(digest))[..16].to_string()
}

#[cfg(feature = "gateway")]
pub(crate) fn redacted_oauth_subject_label() -> &'static str {
    "[redacted]"
}

impl LabMcpServer {
    #[cfg(feature = "gateway")]
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

    #[cfg(feature = "gateway")]
    pub(crate) fn request_host_provider_token<'a>(
        &self,
        context: &'a RequestContext<RoleServer>,
    ) -> Option<&'a str> {
        let parts = context.extensions.get::<Parts>()?;
        parts
            .extensions
            .get::<labby_auth::trusted_host::DelegatedActorCredential>()
            .map(|credential| credential.0.as_ref())
    }

    #[cfg(feature = "gateway")]
    pub(crate) fn request_host_provider_request_id<'a>(
        &self,
        context: &'a RequestContext<RoleServer>,
    ) -> Option<&'a str> {
        let parts = context.extensions.get::<Parts>()?;
        parts
            .extensions
            .get::<labby_auth::trusted_host::DelegatedActorContext>()
            .map(|delegated| delegated.request_id.as_str())
    }

    #[cfg(feature = "gateway")]
    pub(crate) fn request_runtime_owner(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> UpstreamRuntimeOwner {
        let subject = self.request_subject(context);
        crate::dispatch::gateway::shared::make_mcp_runtime_owner(subject)
    }

    #[cfg(feature = "gateway")]
    pub(crate) async fn oauth_upstream_configs(&self) -> Vec<crate::config::UpstreamConfig> {
        match &self.gateway_manager {
            Some(manager) => manager.oauth_upstream_configs().await,
            None => Vec::new(),
        }
    }

    #[cfg(feature = "gateway")]
    pub(crate) async fn route_scoped_oauth_upstream_configs(
        &self,
    ) -> Vec<crate::config::UpstreamConfig> {
        let mut configs = self.oauth_upstream_configs().await;
        configs.retain(|config| self.route_scope.allows_upstream(&config.name));
        if self.route_scope.team_credential_subject().is_some() {
            let Ok(store) = self.access_runtime.store().await else {
                return Vec::new();
            };
            let mut admitted = Vec::with_capacity(configs.len());
            for config in configs {
                let Some((team_id, binding_id, generation)) =
                    self.route_scope.team_credential_binding(&config.name)
                else {
                    continue;
                };
                let Ok(Some(binding)) = store
                    .get_team_gateway_credential_binding(team_id.to_owned(), config.name.clone())
                    .await
                else {
                    continue;
                };
                if team_credential_binding_matches(Some(&binding), binding_id, generation) {
                    admitted.push(config);
                }
            }
            return admitted;
        }
        configs
    }

    #[cfg(feature = "gateway")]
    pub(crate) fn route_oauth_subject<'a>(
        &self,
        personal: Option<std::borrow::Cow<'a, str>>,
    ) -> Option<std::borrow::Cow<'a, str>> {
        self.route_scope
            .team_credential_subject()
            .map(std::borrow::Cow::Owned)
            .or(personal)
    }

    #[cfg(feature = "gateway")]
    pub(crate) async fn route_team_credential_valid(&self, upstream: &str) -> bool {
        let Some((team_id, binding_id, generation)) =
            self.route_scope.team_credential_binding(upstream)
        else {
            return self.route_scope.team_credential_subject().is_none();
        };
        let Ok(store) = self.access_runtime.store().await else {
            return false;
        };
        let Ok(binding) = store
            .get_team_gateway_credential_binding(team_id.to_owned(), upstream.to_owned())
            .await
        else {
            return false;
        };
        team_credential_binding_matches(binding.as_ref(), binding_id, generation)
    }

    #[cfg(feature = "gateway")]
    pub(crate) async fn route_team_credentials_current(&self) -> bool {
        if self.route_scope.team_credential_subject().is_none() {
            return true;
        }
        let configs = self.oauth_upstream_configs().await;
        for config in configs
            .iter()
            .filter(|config| self.route_scope.allows_upstream(&config.name))
        {
            if !self.route_team_credential_valid(&config.name).await {
                return false;
            }
        }
        true
    }

    #[cfg(feature = "gateway")]
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

#[cfg(feature = "gateway")]
fn team_credential_binding_matches(
    binding: Option<&labby_runtime::gateway_authority::TeamCredentialBinding>,
    binding_id: &str,
    generation: u64,
) -> bool {
    binding.is_some_and(|binding| binding.binding_id == binding_id && binding.usable(generation))
}

/// Return the capability snapshot for the current request.
///
/// Even an empty capability object requires a relay connection: progress and
/// cancellation are request-scoped protocol behavior, not optional client
/// capabilities. Legacy requests without modern metadata are represented by an
/// honest empty capability set rather than falling back to connection history.
pub(crate) fn forwardable_client_capabilities(
    meta: Option<&rmcp::model::RequestMetaObject>,
) -> Option<rmcp::model::ClientCapabilities> {
    Some(
        meta.and_then(rmcp::model::RequestMetaObject::client_capabilities)
            .unwrap_or_default(),
    )
}

/// Whether an upstream may be opened on a dedicated capability-relay
/// connection.
///
/// Most stdio servers are safe to spawn once per downstream capability set.
/// Singleton servers that own a fixed listener or other process-global state
/// can opt into the pooled connection with `MCP_UPSTREAM_RELAY_MODE=pooled`.
/// The operator is then responsible for ensuring the pooled upstream can serve
/// clients whose capabilities are not mirrored into its handshake.
#[cfg(feature = "gateway")]
pub(crate) fn upstream_uses_capability_relay(config: &crate::config::UpstreamConfig) -> bool {
    !config
        .env
        .get("MCP_UPSTREAM_RELAY_MODE")
        .is_some_and(|mode| mode.eq_ignore_ascii_case("pooled"))
}

pub(crate) fn subject_from_extensions(extensions: &rmcp::model::Extensions) -> Option<&str> {
    auth_context_from_extensions(extensions).map(|auth| auth.sub.as_str())
}

pub(crate) fn actor_key_from_extensions(extensions: &rmcp::model::Extensions) -> Option<&str> {
    auth_context_from_extensions(extensions).and_then(|auth| auth.actor_key.as_deref())
}

pub(crate) fn auth_context_from_extensions(
    extensions: &rmcp::model::Extensions,
) -> Option<&AuthContext> {
    let parts = extensions.get::<Parts>()?;
    parts.extensions.get::<AuthContext>()
}

pub(crate) fn verified_identity_from_extensions(
    extensions: &rmcp::model::Extensions,
) -> Option<&labby_auth::VerifiedIdentity> {
    let parts = extensions.get::<Parts>()?;
    parts.extensions.get::<labby_auth::VerifiedIdentity>()
}

pub(crate) fn bound_access_grant_from_extensions(
    extensions: &rmcp::model::Extensions,
) -> Option<&labby_primitives::product_credential::BoundAccessGrant> {
    let parts = extensions.get::<Parts>()?;
    parts
        .extensions
        .get::<labby_primitives::product_credential::BoundAccessGrant>()
}

pub(crate) fn tool_execute_scope_allowed(auth: Option<&AuthContext>) -> bool {
    auth.is_none_or(|auth| {
        auth.scopes
            .iter()
            .any(|scope| matches!(scope.as_str(), "lab" | "lab:admin"))
    })
}

/// Returns `true` when the caller is allowed to read Code Mode resources.
///
/// Code Mode app resources require at least `lab:read`; executable Code Mode
/// calls require the stronger `lab` or `lab:admin`.
/// `None` auth means stdio transport — trusted by design (no per-request AuthContext).
pub(crate) fn code_mode_read_scope_allowed(auth: Option<&AuthContext>) -> bool {
    auth.is_none_or(|auth| {
        auth.scopes
            .iter()
            .any(|scope| matches!(scope.as_str(), "lab:read" | "lab" | "lab:admin"))
    })
}

/// Whether an absent `AuthContext` may be read as "trusted local stdio".
///
/// The stdio trust model treats a missing per-request auth context as a local
/// operator at a terminal. That inference is only sound on a transport that
/// *would* have carried auth for a remote caller. The in-process peer
/// (`mcp/in_process_peer.rs`) is served over `tokio::io::duplex`, which has no
/// HTTP layer, so `auth_context_from_extensions` finds no `Parts` and resolves
/// to `None` for **every** caller — including a remote, non-admin OAuth caller
/// who reached it through Code Mode. On that transport an absent context proves
/// nothing and must not be trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AbsentAuth {
    /// No auth context means a local stdio caller. Applies to transports that
    /// inject one for every authenticated remote caller.
    TrustedLocal,
    /// No auth context proves nothing about the caller. Applies to the
    /// in-process peer, whose transport cannot carry auth at all.
    ///
    /// This is the *fallback* for that transport, not its normal path: the
    /// gateway propagates the real caller's authorization in `_meta`, and
    /// [`CallerAuthorization::Propagated`] carries it. Reaching `Untrusted` means the
    /// propagation was missing, so the request fails closed.
    Untrusted,
}

/// How a request's authorization was established, once transport is accounted
/// for.
///
/// Built by [`resolve_caller_authorization`] so every gate sees the same
/// resolution rather than each re-deriving it.
#[derive(Debug, Clone)]
pub(crate) enum CallerAuthorization<'a> {
    /// The request carried its own `AuthContext`.
    Direct(&'a AuthContext),
    /// No context, but the transport implies a trusted local operator.
    TrustedLocal,
    /// No context, but the gateway propagated the real caller across the
    /// in-process hop.
    Propagated(PropagatedCallerAuth),
    /// No context and nothing to stand in for one.
    Unknown,
}

impl CallerAuthorization<'_> {
    /// Whether this caller satisfies a read-scoped action.
    #[must_use]
    pub(crate) fn can_read(&self) -> bool {
        match self {
            Self::Direct(auth) => auth
                .scopes
                .iter()
                .any(|scope| matches!(scope.as_str(), "lab:read" | "lab" | "lab:admin")),
            Self::TrustedLocal => true,
            Self::Propagated(auth) => {
                auth.trusted_local
                    || auth
                        .scopes
                        .iter()
                        .any(|scope| matches!(scope.as_str(), "lab:read" | "lab" | "lab:admin"))
            }
            Self::Unknown => false,
        }
    }

    /// Whether this caller satisfies an admin-gated action.
    #[must_use]
    pub(crate) fn is_admin(&self) -> bool {
        match self {
            Self::Direct(auth) => auth.scopes.iter().any(|scope| scope == "lab:admin"),
            Self::TrustedLocal => true,
            Self::Propagated(auth) => auth.is_admin(),
            Self::Unknown => false,
        }
    }

    /// Whether this caller is a trusted local operator, which is what the
    /// credential-minting `setup` actions require.
    #[must_use]
    pub(crate) fn is_trusted_local(&self) -> bool {
        match self {
            Self::TrustedLocal => true,
            Self::Propagated(auth) => auth.trusted_local,
            Self::Direct(_) | Self::Unknown => false,
        }
    }
}

/// Read caller authorization propagated across the in-process peer hop.
///
/// Returns `None` unless the request carries the reserved `_meta` key. Callers
/// MUST only consult this on the in-process transport — see
/// `labby_runtime::caller_auth` for why that restriction is the whole basis for
/// trusting it.
pub(crate) fn propagated_caller_auth(
    meta: Option<&rmcp::model::RequestMetaObject>,
) -> Option<PropagatedCallerAuth> {
    let value = meta?.get(CALLER_AUTH_META_KEY)?;
    serde_json::from_value(value.clone()).ok()
}

/// Resolve a request's authorization, accounting for transport and any
/// authorization the gateway propagated across the in-process hop.
///
/// `propagated` is read **only** when the transport cannot carry an
/// `AuthContext` of its own — see `labby_runtime::caller_auth` for why that
/// restriction is what makes trusting it sound.
pub(crate) fn resolve_caller_authorization(
    auth: Option<&AuthContext>,
    absent_auth: AbsentAuth,
    propagated: Option<PropagatedCallerAuth>,
) -> CallerAuthorization<'_> {
    if let Some(auth) = auth {
        // A real context always wins. Propagated facts never override or widen
        // an authorization the caller actually presented.
        return CallerAuthorization::Direct(auth);
    }
    match absent_auth {
        AbsentAuth::TrustedLocal => CallerAuthorization::TrustedLocal,
        AbsentAuth::Untrusted => match propagated {
            Some(propagated) => CallerAuthorization::Propagated(propagated),
            None => CallerAuthorization::Unknown,
        },
    }
}

pub(crate) fn tool_execute_builtin_action_allowed(
    entry: &crate::registry::RegisteredService,
    action: &str,
    caller: &CallerAuthorization<'_>,
) -> bool {
    let bare = action
        .strip_prefix(&format!("{}.", entry.name))
        .unwrap_or(action);
    if entry.name == "setup" && crate::dispatch::setup::LOCAL_ONLY_ACTIONS.contains(&bare) {
        // These mint credentials or ask the host to probe a caller-selected
        // URL, so they are for trusted local stdio only. A remote caller always
        // carries an AuthContext and is refused; a local operator reaching them
        // through Code Mode is honored, because the propagated facts say so.
        return caller.is_trusted_local();
    }
    if !builtin_action_requires_admin(entry, action) {
        return true;
    }
    // INTENTIONAL ASYMMETRY with the HTTP API gate (`api/services/gateway.rs`,
    // which uses `is_some_and` — absent auth = DENIED). Here absent auth is
    // allowed *only* on a transport where it genuinely implies local stdio.
    // Remote MCP-over-HTTP cannot reach here unauthenticated because
    // `cli/serve.rs` refuses to bind a non-loopback address without auth
    // configured, and the `/mcp` route carries the bearer/OAuth layer when auth
    // is configured. The in-process peer is the transport that broke that
    // inference, which is why `absent_auth` is a parameter rather than a
    // constant. Do NOT widen this without proving the new transport injects an
    // AuthContext for every authenticated caller.
    caller.is_admin()
}

pub(crate) fn builtin_action_requires_admin(
    entry: &crate::registry::RegisteredService,
    action: &str,
) -> bool {
    // Catalog-driven metadata is the single source of truth for every
    // registered service. Keeping an allow-list here caused newly registered
    // services (notably Doctor) to silently bypass their admin metadata.
    let service_prefix = format!("{}.", entry.name);
    let bare = action.strip_prefix(&service_prefix).unwrap_or(action);
    if bare == "help" || bare == "schema" {
        return false;
    }
    let lookup = if entry.actions.iter().any(|spec| spec.name == action) {
        action
    } else {
        bare
    };
    entry
        .actions
        .iter()
        .find(|spec| spec.name == lookup)
        .map(|spec| spec.requires_admin)
        // Unknown actions fail closed. Dispatch will still return its normal
        // unknown-action envelope after an administrator reaches it.
        .unwrap_or(true)
}

#[cfg(test)]
mod tests;
