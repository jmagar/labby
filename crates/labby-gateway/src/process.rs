//! Unix process-control helpers used by the upstream stdio transport.
//!
//! Vendored into `lab-gateway` so the upstream pool's process-group cleanup is
//! self-contained and does not reach back into the Labby binary crate. The Labby
//! copy at `crates/lab/src/process/unix.rs` stays for non-gateway callers; the
//! two are intentionally identical pure helpers.

pub mod unix;
