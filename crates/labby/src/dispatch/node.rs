//! Shared dispatch helpers for fleet node operations.
//!
//! This module provides surface-neutral helpers that the dispatch layer uses
//! when communicating with connected nodes. It does NOT own business logic —
//! the fleet WebSocket handler in `api/nodes/fleet.rs` owns the connection
//! lifecycle; this module exposes shared state (sender registry) for
//! cross-surface callers.

pub mod send;
