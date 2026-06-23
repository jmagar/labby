//! No external HTTP client — `lab_admin` is a local-only service.
//!
//! This file satisfies the required dispatch service layout contract
//! (every migrated service must have `client.rs`) while making it explicit
//! that `lab_admin` performs local filesystem operations only and has no
//! external service to connect to.
