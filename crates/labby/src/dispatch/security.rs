// The stdio spawn-guard lives in `labby-runtime` so marketplace and gateway
// surfaces share one allowlist without a product-to-product dependency.
pub use labby_runtime::security::spawn_guard;
