#![allow(clippy::multiple_crate_versions)]

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
