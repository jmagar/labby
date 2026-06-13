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
        clippy::zombie_processes
    )
)]

pub mod acp;
#[allow(unreachable_pub)]
pub mod api;
pub mod catalog;
#[allow(unreachable_pub)]
pub mod cli;
pub mod config;
#[allow(unreachable_pub)]
pub mod dispatch;
#[allow(unreachable_pub)]
pub mod docs;
#[cfg(test)]
#[allow(dead_code)]
pub mod log_fmt;
#[allow(unreachable_pub)]
pub mod mcp;
pub mod net;
#[allow(unreachable_pub)]
pub mod node;
#[allow(unreachable_pub)]
pub mod oauth;
#[allow(unreachable_pub)]
pub mod observability;
pub mod output;
#[allow(unreachable_pub)]
pub mod process;
#[allow(unreachable_pub)]
pub mod registry;
#[cfg(test)]
pub mod test_support;
pub(crate) mod tool_names;
#[cfg(feature = "fs")]
pub mod workspace;
