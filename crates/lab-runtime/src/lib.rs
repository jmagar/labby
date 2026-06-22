//! `lab-runtime` — surface-neutral runtime primitives shared by the `lab`
//! binary and the standalone product slices.
//!
//! This crate currently owns the canonical [`error::ToolError`] type used across
//! all surfaces (MCP, API, CLI). It is the home for `From<ServiceError>`
//! conversions whose source error types live in `lab-apis`, because the orphan
//! rule requires those `impl`s to live alongside the local `ToolError` type.
#![forbid(unsafe_code)]

pub mod error;
