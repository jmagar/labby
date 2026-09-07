#![recursion_limit = "256"]
#![allow(clippy::multiple_crate_versions)]
#![cfg_attr(
    test,
    allow(
        clippy::await_holding_lock,
        clippy::bool_assert_comparison,
        clippy::err_expect,
        clippy::float_cmp,
        clippy::items_after_test_module,
        clippy::iter_on_single_items,
        clippy::manual_string_new,
        clippy::mem_replace_option_with_some,
        clippy::needless_borrows_for_generic_args,
        clippy::needless_raw_string_hashes,
        clippy::panic,
        clippy::single_char_pattern,
        clippy::single_element_loop,
        clippy::zombie_processes,
    )
)]

//! Core Labby runtime, command, API, gateway, setup, and operator surfaces.

#[allow(dead_code)]
mod access;
#[allow(unreachable_pub)]
pub mod api;
pub(crate) mod app_assets;
pub(crate) mod app_catalog;
pub(crate) mod app_manifest;
pub mod catalog;
#[allow(unreachable_pub)]
pub mod cli;
pub(crate) mod composition;
pub mod config;
#[allow(unreachable_pub)]
pub mod dispatch;
#[allow(unreachable_pub)]
pub mod docs;
pub mod durable_state;
mod entrypoint;
#[allow(dead_code)]
pub(crate) mod file_stash;
pub mod installation;
pub(crate) mod integration_identity;
#[cfg(feature = "gateway")]
#[allow(unreachable_pub)]
pub mod live_gateway;
#[allow(dead_code)]
pub mod log_fmt;
#[allow(unreachable_pub)]
pub mod mcp;
pub mod net;
#[allow(unreachable_pub)]
pub mod oauth;
#[allow(unreachable_pub)]
pub mod observability;
pub mod output;
#[allow(unreachable_pub)]
pub mod process;
#[allow(unreachable_pub)]
pub mod proxy;
#[allow(unreachable_pub)]
pub mod registry;
#[cfg(feature = "skills")]
pub(crate) mod skills;
mod stdio_sandbox;
#[cfg(test)]
pub mod test_support;
#[doc(hidden)]
pub mod testkit;
#[cfg(unix)]
pub(crate) mod unix_listener;
#[cfg(feature = "fs")]
pub mod workspace;

pub use entrypoint::run;
