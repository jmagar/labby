//! Propagating a caller's authorization facts across the in-process peer hop.
//!
//! # The problem this solves
//!
//! Labby serves its own built-in services to Code Mode through an in-process
//! MCP peer over a duplex pipe. `AuthContext` normally reaches a handler inside
//! `http::request::Parts`, which lives in rmcp's *local* extensions — and local
//! extensions do not cross a serialization boundary. So the mini server on the
//! far side of that pipe saw `None` for every caller, whoever they were.
//!
//! Fail-closed (refusing admin actions on that transport) fixed the escalation
//! but denied genuine admins too. This carries the caller's authorization facts
//! across the hop in the request's `_meta`, which *does* serialize, so the mini
//! server evaluates the real caller.
//!
//! # Why trusting `_meta` here is sound
//!
//! `_meta` is ordinarily attacker-influenced and must never be treated as
//! authorization. Two properties make this use safe, and **both** are required:
//!
//! 1. **The value is minted, never forwarded.** The inner request is
//!    constructed fresh by the gateway from the caller's already-verified
//!    `AuthContext`. Nothing a remote client sends is copied into it, so a
//!    caller cannot supply their own.
//! 2. **It is honored on exactly one transport.** Only the in-process peer's
//!    server reads it, and only Labby holds the client end of that pipe. A
//!    remote client cannot address that server at all.
//!
//! If either property stops holding — if this key is ever read on a networked
//! transport, or if an inbound `_meta` is ever forwarded into an inner request —
//! this becomes a trivial privilege escalation. Do not relax either one.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Reserved `_meta` key carrying propagated caller authorization.
///
/// Reverse-domain prefixed so it cannot collide with a protocol-defined key.
pub const CALLER_AUTH_META_KEY: &str = "ai.dinglebear.labby/callerAuth";

/// Reserved `_meta` key carrying the caller-visible upstream namespace scope.
/// Honored only on Labby's private in-process MCP transport, under the same
/// mint-never-forward trust boundary as [`CALLER_AUTH_META_KEY`].
pub const CALLER_UPSTREAM_SCOPE_META_KEY: &str = "ai.dinglebear.labby/callerUpstreamScope";

/// Upstream namespaces the outer caller may observe through an in-process
/// built-in service. `None` means the root route / unrestricted namespace set;
/// `Some(empty)` means no upstreams are visible.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropagatedCallerUpstreamScope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_upstreams: Option<BTreeSet<String>>,
}

impl PropagatedCallerUpstreamScope {
    #[must_use]
    pub fn new(allowed_upstreams: Option<BTreeSet<String>>) -> Self {
        Self { allowed_upstreams }
    }
}

/// A caller's authorization facts, as they cross the in-process hop.
///
/// Deliberately carries the *scopes*, not a decision. The receiving side
/// applies its own gate, so a future action with different scope requirements
/// is evaluated correctly rather than against a stale yes/no.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropagatedCallerAuth {
    /// The caller reached Labby over trusted local stdio and carried no
    /// per-request auth context of their own.
    #[serde(default)]
    pub trusted_local: bool,
    /// OAuth scopes the caller actually presented.
    #[serde(default)]
    pub scopes: Vec<String>,
    /// JWT `sub`, when the caller had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub: Option<String>,
    /// AccessStore-resolved durable principal ID. This is minted only by the
    /// outer Labby process for its private in-process peer; remote `_meta`
    /// remains untrusted and this field is never honored on network routes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_principal_id: Option<String>,
    /// Opaque, host-minted capability for the private in-process hop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_context_token: Option<String>,
}

impl PropagatedCallerAuth {
    /// A local stdio operator: no scopes, but trusted by transport.
    #[must_use]
    pub fn trusted_local() -> Self {
        Self {
            trusted_local: true,
            scopes: Vec::new(),
            sub: None,
            access_principal_id: None,
            private_context_token: None,
        }
    }

    /// A scoped caller, carrying exactly the scopes they presented.
    #[must_use]
    pub fn scoped(scopes: Vec<String>, sub: Option<String>) -> Self {
        Self {
            trusted_local: false,
            scopes,
            sub,
            access_principal_id: None,
            private_context_token: None,
        }
    }

    #[must_use]
    pub fn with_private_context_token(mut self, token: String) -> Self {
        self.private_context_token = Some(token);
        self
    }

    #[must_use]
    pub fn with_access_principal_id(mut self, principal_id: String) -> Self {
        self.access_principal_id = Some(principal_id);
        self
    }

    /// Whether these facts satisfy an admin-gated action.
    #[must_use]
    pub fn is_admin(&self) -> bool {
        self.trusted_local || self.scopes.iter().any(|scope| scope == "lab:admin")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_local_operator_is_admin_and_a_scoped_caller_is_not_by_default() {
        assert!(PropagatedCallerAuth::trusted_local().is_admin());
        assert!(!PropagatedCallerAuth::scoped(vec!["lab".into()], None).is_admin());
        assert!(PropagatedCallerAuth::scoped(vec!["lab:admin".into()], None).is_admin());
    }

    #[test]
    fn scopes_survive_the_round_trip_rather_than_a_decision() {
        // Carrying the scopes, not a boolean, is what lets the receiving side
        // evaluate an action whose requirements differ from the sender's.
        let original = PropagatedCallerAuth::scoped(
            vec!["lab".into(), "lab:read".into()],
            Some("alice".into()),
        );
        let encoded = serde_json::to_value(&original).expect("serializes");
        let decoded: PropagatedCallerAuth = serde_json::from_value(encoded).expect("deserializes");
        assert_eq!(original, decoded);
        assert_eq!(decoded.scopes, vec!["lab", "lab:read"]);
    }

    #[test]
    fn upstream_scope_round_trips_root_and_restricted() {
        let root = PropagatedCallerUpstreamScope::new(None);
        let root_value = serde_json::to_value(&root).expect("serializes");
        let decoded_root: PropagatedCallerUpstreamScope =
            serde_json::from_value(root_value).expect("deserializes");
        assert_eq!(decoded_root, root);

        let restricted = PropagatedCallerUpstreamScope::new(Some(BTreeSet::from([
            "github".to_string(),
            "docs".to_string(),
        ])));
        let value = serde_json::to_value(&restricted).expect("serializes");
        let decoded: PropagatedCallerUpstreamScope =
            serde_json::from_value(value).expect("deserializes");
        assert_eq!(decoded, restricted);
        assert_eq!(decoded.allowed_upstreams.expect("restricted").len(), 2);
    }

    #[test]
    fn an_absent_or_partial_payload_defaults_to_no_privilege() {
        let empty: PropagatedCallerAuth =
            serde_json::from_value(serde_json::json!({})).expect("deserializes");
        assert!(!empty.is_admin());
        assert!(!empty.trusted_local);
        assert!(empty.scopes.is_empty());
    }
}
