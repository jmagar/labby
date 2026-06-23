// The stdio spawn-guard now lives in the standalone `lab-gateway` crate
// alongside the upstream pool it protects. Re-exported here so the marketplace
// install/params validation paths keep their `dispatch::security::spawn_guard`
// import path unchanged. (SSRF preflight is consumed directly from
// `labby_gateway::security::ssrf` at its call sites.)
pub use labby_gateway::upstream::spawn_guard;
