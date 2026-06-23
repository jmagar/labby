// The stdio spawn-guard now lives in the standalone `lab-gateway` crate alongside
// the upstream pool it protects. Re-exported here so the gateway-config and
// marketplace-install validation paths keep their `dispatch::security::spawn_guard`
// import path unchanged.
pub use lab_gateway::upstream::spawn_guard;
pub mod ssrf;
