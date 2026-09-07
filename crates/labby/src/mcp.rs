//! MCP transport layer — the translation between `labby-apis` clients and
//! the Model Context Protocol. See `crates/labby/src/mcp/CLAUDE.md` for
//! the full rulebook on dispatch, envelopes, and the shared catalog.

pub(crate) mod agent_error;
#[cfg(feature = "gateway")]
pub(crate) mod bound_access;
#[cfg(feature = "gateway")]
pub mod bridge;
pub mod call_tool;
#[cfg(feature = "gateway")]
pub mod call_tool_codemode;
#[cfg(feature = "gateway")]
pub mod call_tool_upstream;
pub mod catalog;
pub(crate) mod catalog_churn;
pub(crate) mod catalog_coalesce;
pub(crate) mod catalog_notifications;
pub mod completion;
pub mod context;
pub mod elicitation;
pub mod envelope;
pub mod error;
pub(crate) mod file_stash;
pub mod handlers_prompts;
pub mod handlers_resources;
pub mod handlers_tools;
#[cfg(feature = "gateway")]
pub mod in_process_peer;
pub mod logging;
pub mod meta;
pub(crate) mod pagination;
pub(crate) mod peer_contract;
pub mod peers;
pub(crate) mod permanent_tools;
#[cfg(feature = "gateway")]
pub(crate) mod prompt_execution;
pub mod prompts;
pub(crate) mod provenance;
pub mod registry;
pub(crate) mod resource_errors;
#[cfg(feature = "gateway")]
pub(crate) mod resource_execution;
#[cfg(feature = "gateway")]
pub mod resource_proxy;
pub mod resources;
pub mod result_format;
pub(crate) mod route_scope;
pub(crate) mod runtime;
pub mod server;
pub mod services;
#[cfg(feature = "skills")]
pub mod skills;
#[cfg(feature = "gateway")]
pub(crate) mod tool_execution;
#[cfg(feature = "gateway")]
pub mod upstream;

#[allow(unused_imports)]
pub use envelope::{ToolEnvelope, ToolError};
#[allow(unused_imports)]
pub use registry::ToolRegistry;
