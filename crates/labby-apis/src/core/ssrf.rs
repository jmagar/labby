//! Canonical SSRF preflight guards for externally supplied HTTPS URLs.
//!
//! Re-exported from the dependency-free `labby-primitives` leaf crate so that
//! `labby-apis` and `labby-gateway` share the exact same allow/deny policy
//! without depending on each other. See `labby_primitives::ssrf` for the
//! implementation and docs.

pub use labby_primitives::ssrf::{
    PRIVATE_TLD_SUFFIXES, SsrfError, check_host_not_private, check_ip_not_private, is_cgnat,
    parse_validated_https_url, redact_url,
};
