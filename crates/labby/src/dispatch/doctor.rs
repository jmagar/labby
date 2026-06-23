//! Shared dispatch layer for the `doctor` service.
//!
//! Doctor is a Bootstrap utility: no external service URL, no feature gate.
//! `system.checks` reads local state; `service.probe` and `audit.full` use
//! pre-built `ServiceClients`.

mod catalog;
mod client;
mod dispatch;
pub mod gateway;
mod params;
pub mod proxy;
pub mod service;
mod system;
mod types;

pub use catalog::ACTIONS;
pub use dispatch::{dispatch, dispatch_with_clients};
pub use system::{run_auth_checks, run_system_checks};
pub use types::{Finding, Report, Severity};
