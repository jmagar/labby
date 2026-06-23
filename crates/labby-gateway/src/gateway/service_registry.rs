//! Service-registry seam for [`GatewayManager`].
//!
//! The manager needs three things from Labby's `ToolRegistry`: the set of
//! registered service names, each service's actions (name/description/
//! destructive), and the `&'static PluginMeta` for a service. It also hands the
//! registry to the upstream pool for in-process peer discovery.
//!
//! Rather than depend on Labby's concrete `ToolRegistry` (which carries
//! `ActionSpec` dispatch function pointers and the default-registry builder),
//! the manager depends only on this trait. Labby implements it for `ToolRegistry`
//! and injects it. The trait is a supertrait of [`InProcessServiceRegistry`] so
//! the same value can be passed to the pool's discovery entry points.

use labby_apis::core::PluginMeta;

use crate::registry::InProcessServiceRegistry;

/// A single action exposed by a registered service, projected to the data the
/// gateway dispatch surface needs (no `ActionSpec` dispatch pointers).
#[derive(Debug, Clone)]
pub struct ServiceActionInfo {
    pub name: &'static str,
    pub description: &'static str,
    pub destructive: bool,
}

/// Read-only view of Labby's service registry the gateway manager depends on.
///
/// `InProcessServiceRegistry` is a supertrait so the same trait object can be
/// passed to `UpstreamPool::discover_all_*_with_in_process_peers`.
pub trait GatewayServiceRegistry: InProcessServiceRegistry {
    /// Stable names of every registered service.
    fn service_names(&self) -> Vec<&'static str>;

    /// Whether a service with this name is registered.
    fn contains_service(&self, name: &str) -> bool;

    /// Actions exposed by a registered service, or `None` if not registered.
    fn service_actions(&self, name: &str) -> Option<Vec<ServiceActionInfo>>;

    /// `PluginMeta` for a registered service, or `None` if it has no metadata.
    fn service_meta(&self, name: &str) -> Option<&'static PluginMeta>;
}

/// An empty service registry: no registered services, no in-process peers.
///
/// Used as the default before a real registry is injected (e.g. a freshly
/// constructed manager in a test that does not exercise service lookups).
#[derive(Debug, Default, Clone, Copy)]
pub struct EmptyServiceRegistry;

impl InProcessServiceRegistry for EmptyServiceRegistry {
    fn in_process_services(&self) -> Vec<Box<dyn crate::registry::InProcessService>> {
        Vec::new()
    }
}

impl GatewayServiceRegistry for EmptyServiceRegistry {
    fn service_names(&self) -> Vec<&'static str> {
        Vec::new()
    }

    fn contains_service(&self, _name: &str) -> bool {
        false
    }

    fn service_actions(&self, _name: &str) -> Option<Vec<ServiceActionInfo>> {
        None
    }

    fn service_meta(&self, _name: &str) -> Option<&'static PluginMeta> {
        None
    }
}
