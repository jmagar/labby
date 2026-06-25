//! Gateway runtime: `GatewayManager`, catalog/dispatch, OAuth lifecycle,
//! protected-route projection, virtual-server projection, and the Code Mode
//! host binding.
//!
//! This tree was extracted from Labby's `crate::dispatch::gateway`. It is
//! surface-neutral: it depends on the `lab-runtime` config/error contracts, the
//! `labby-codemode` execution kernel, the `labby-auth` upstream OAuth runtime, and
//! this crate's `upstream` pool. The host (`lab`) injects the small seams it
//! cannot own — config persistence ([`config_store::GatewayConfigStore`]) and
//! the service registry ([`service_registry::GatewayServiceRegistry`]).

mod catalog;
mod client;
pub mod code_mode;
pub mod config;
mod config_mutation;
pub mod config_store;
pub mod discovery;
mod dispatch;
mod enrichment;
pub mod manager;
pub mod oauth;
mod oauth_lifecycle;
mod params;
mod projection;
pub mod protected_routes;
mod runtime;
mod service_catalog;
pub mod service_registry;
pub mod shared;
pub mod types;
pub mod view_models;
mod virtual_servers;

pub use catalog::ACTIONS;
pub use client::{current_gateway_manager, install_gateway_manager, require_gateway_manager};
pub use config_store::GatewayConfigStore;
pub use dispatch::{dispatch, dispatch_with_manager};
pub use service_registry::{GatewayServiceRegistry, ServiceActionInfo};
pub use shared::SHARED_GATEWAY_OAUTH_SUBJECT;
