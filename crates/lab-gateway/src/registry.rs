//! Injected registry/service seams for in-process peer registration.
//!
//! The upstream pool can expose Labby's built-in, in-process services as
//! gateway upstream peers. Those services are described by Labby's
//! `crate::registry::RegisteredService` / `ToolRegistry`, which carry product
//! vocabulary (`ActionSpec`, dispatch function pointers) that does NOT belong in
//! this runtime crate.
//!
//! Rather than depend on Labby's registry (or invent a parallel
//! `GatewayRegisteredService`), the pool depends only on the two minimal traits
//! defined here. Labby implements them for its concrete registry types and
//! injects the connector; the pool calls the connector with a type-erased
//! service handle that the Labby-side connector downcasts back to its concrete
//! `RegisteredService`.
//!
//! This keeps `lab-gateway` free of `ActionSpec` / `ToolRegistry` / dispatch
//! function pointers and free of any call to Labby's default registry builder.

use std::any::Any;

/// A built-in service that can be registered as an in-process upstream peer.
///
/// The pool only needs the service's stable name (for the synthetic upstream
/// name and structured logging) and whether it exposes any actions (services
/// with no actions are skipped). The concrete payload required to actually
/// stand up the in-process MCP server is opaque to the pool: it is recovered by
/// the injected connector via [`InProcessService::as_any`] downcasting.
pub trait InProcessService: Any + Send + 'static {
    /// Stable service name (e.g. `"radarr"`). Used to derive the synthetic
    /// `in-process:<name>` upstream name and for structured logging.
    fn service_name(&self) -> &'static str;

    /// Whether this service exposes at least one action. Services with no
    /// actions are not registered as in-process peers.
    fn has_actions(&self) -> bool;

    /// Upcast for the connector to recover the concrete registration payload.
    fn as_any(self: Box<Self>) -> Box<dyn Any>;
}

/// A registry that can enumerate the in-process services to register.
///
/// Labby implements this for its `ToolRegistry`, yielding one boxed
/// [`InProcessService`] per registered service.
///
/// `Send + Sync` is required because the pool holds a `&dyn
/// InProcessServiceRegistry` across `.await` points while registering peers
/// concurrently, and the resulting futures must stay `Send` for the axum/MCP
/// surfaces that drive them.
pub trait InProcessServiceRegistry: Send + Sync {
    /// Enumerate the in-process services to register as upstream peers.
    fn in_process_services(&self) -> Vec<Box<dyn InProcessService>>;
}
