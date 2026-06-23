//! Network reprobe backoff/jitter helpers used by the upstream transport layer.
//!
//! Vendored into `lab-gateway` so the upstream pool's circuit-breaker reprobe
//! scheduling is self-contained. The Labby copy at `crates/lab/src/net/backoff.rs`
//! stays for non-gateway callers; the two are intentionally identical pure
//! helpers.

pub mod backoff;
