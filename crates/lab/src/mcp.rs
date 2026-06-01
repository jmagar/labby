//! MCP transport layer — the translation between `lab-apis` clients and
//! the Model Context Protocol. See `crates/lab/src/mcp/CLAUDE.md` for
//! the full rulebook on dispatch, envelopes, and the shared catalog.

pub mod call_tool;
pub mod call_tool_codemode;
pub mod call_tool_upstream;
pub mod catalog;
pub mod completion;
pub mod context;
pub mod elicitation;
pub mod envelope;
pub mod error;
pub mod handlers_prompts;
pub mod handlers_resources;
pub mod handlers_tools;
pub mod logging;
pub mod meta;
pub mod peers;
pub mod prompts;
pub mod registry;
pub mod resource_proxy;
pub mod resources;
pub mod result_format;
pub mod server;
pub mod services;
pub mod upstream;

#[allow(unused_imports)]
pub use envelope::{ToolEnvelope, ToolError};
#[allow(unused_imports)]
pub use registry::ToolRegistry;
