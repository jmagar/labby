// The stdio spawn-guard and SSRF preflight guards now live in the standalone
// `lab-gateway` crate alongside the upstream pool they protect. Re-exported here
// so the gateway-config and marketplace-install validation paths keep their
// `dispatch::security::{spawn_guard, ssrf}` import paths unchanged.
pub use lab_gateway::security::ssrf;
pub use lab_gateway::upstream::spawn_guard;
