//! Authentication, authorization, OAuth, session, and token support for Labby services.

pub mod at_rest;
#[cfg(feature = "http-axum")]
pub mod auth_context;
#[cfg(feature = "http-axum")]
pub mod authelia;
#[cfg(feature = "http-axum")]
pub mod authorize;
pub mod browser_authority;
#[cfg(test)]
mod browser_authority_tests;
#[cfg(feature = "http-axum")]
pub mod cimd;
pub mod config;
mod config_providers;
pub mod depot_delegation;
pub mod error;
pub mod google;
#[cfg(any(test, feature = "http-axum", feature = "upstream-oauth-rmcp"))]
mod google_refresh;
pub mod jwt;
#[cfg(feature = "http-axum")]
pub mod metadata;
#[cfg(feature = "http-axum")]
pub mod middleware;
pub mod oauth_provider;
#[cfg(feature = "http-axum")]
mod oidc;
pub mod project_session;
#[cfg(feature = "http-axum")]
mod provider_http;
pub mod reauth;
#[cfg(feature = "http-axum")]
pub mod reauth_browser;
#[cfg(test)]
mod reauth_tests;
#[cfg(feature = "http-axum")]
mod remote;
pub mod resource_registry;
#[cfg(feature = "http-axum")]
pub mod routes;
#[cfg(feature = "http-axum")]
pub mod session;
pub mod sqlite;
pub mod state;
#[cfg(feature = "http-axum")]
pub mod token;
pub mod trusted_host;
pub mod types;
#[cfg(feature = "upstream-oauth-rmcp")]
pub mod upstream;
pub mod util;
mod verified_identity;

pub use verified_identity::{
    Authenticator, PrincipalLink, VerifiedIdentity, VerifiedIdentityError,
    verified_identity_from_access_claims,
};

#[cfg(feature = "http-axum")]
pub use auth_context::{AuthContext, auth_context, www_authenticate_value};
#[cfg(feature = "http-axum")]
pub use middleware::{
    ActorKeyDeriver, AuthLayer, AuthService, ProductAccessGrantResolutionFuture,
    ProductAccessGrantResolver, ProjectSessionRevalidationError, ProjectSessionRevalidationFuture,
    ProjectSessionRevalidator, RequiredScopes, parse_bearer_token, tokens_equal,
};
pub use types::ProjectSessionBinding;

#[cfg(test)]
pub mod test_support;
