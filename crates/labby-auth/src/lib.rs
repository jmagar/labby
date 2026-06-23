pub mod at_rest;
#[cfg(feature = "http-axum")]
pub mod auth_context;
#[cfg(feature = "http-axum")]
pub mod authorize;
pub mod config;
pub mod error;
pub mod google;
pub mod jwt;
#[cfg(feature = "http-axum")]
pub mod metadata;
#[cfg(feature = "http-axum")]
pub mod middleware;
#[cfg(feature = "http-axum")]
pub mod routes;
#[cfg(feature = "http-axum")]
pub mod session;
pub mod sqlite;
pub mod state;
#[cfg(feature = "http-axum")]
pub mod token;
pub mod types;
#[cfg(feature = "upstream-oauth-rmcp")]
pub mod upstream;
pub mod util;

#[cfg(feature = "http-axum")]
pub use auth_context::{AuthContext, auth_context, www_authenticate_value};
#[cfg(feature = "http-axum")]
pub use middleware::{ActorKeyDeriver, AuthLayer, AuthService, parse_bearer_token, tokens_equal};

#[cfg(test)]
pub mod test_support;
