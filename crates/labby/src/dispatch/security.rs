// The stdio spawn-guard lives in `labby-gateway` (gateway-owned; the
// `marketplace` feature depends on `gateway` for exactly this) so marketplace
// and gateway surfaces share one allowlist. `labby-gateway` is an optional
// dependency behind the `gateway` feature, so this re-export must be too.
#[cfg(feature = "gateway")]
pub use labby_gateway::security::spawn_guard;
