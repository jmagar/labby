//! Outbound OAuth support for upstream MCP servers.
//!
//! Reusable runtime mechanics for the outbound `authorization_code` + PKCE flow
//! that lab uses when proxying to OAuth-protected upstream MCP servers. This is
//! the SDK-side half of upstream OAuth: per-`(upstream, subject)` token storage,
//! single-flight refresh, encryption-at-rest with AAD binding, and the
//! per-subject `AuthClient` cache. Browser/API route handling, sessions,
//! `AuthContext`, cookies, and admin checks stay with the product binary.
//!
//! Gated behind the `upstream-oauth-rmcp` feature because it depends on the rmcp
//! client/auth transport stack and `oauth2`.

pub mod cache;
pub mod encryption;
pub mod manager;
pub mod refresh;
pub mod runtime;
pub mod store;
pub mod types;
