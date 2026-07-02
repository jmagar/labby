#![forbid(unsafe_code)]

//! Client-neutral Code Mode JavaScript execution kernel.
//!
//! This crate owns the Javy/QuickJS sandbox runner, its parent-side
//! broker/driver, the result-shaping helpers, and the snippet engine. It is
//! injected with a tool source via the [`CodeModeHost`] trait, so any host (an
//! MCP proxy pool, a REST client, an in-memory stub) can run JS against its own
//! tools without the kernel learning what backs them.
//!
//! Vocabulary is deliberately host-source-neutral: a tool is an opaque `id`
//! (`<namespace>::<tool>`) plus JSON params; a tool descriptor is the neutral
//! [`ToolDescriptor`]; the visibility filter is the neutral [`ToolScope`].
//! The only exception is the runner-reserved local `state`/`git` provider set:
//! those namespaces are not host upstream tools and are injected only for
//! unscoped admin/trusted-local callers. Scoped runs must not discover or
//! dispatch them.
//!
//! Runtime: Javy/QuickJS via a hardened subprocess (NOT Wasmtime). See
//! `CLAUDE.md` for the sandbox containment invariants.

pub mod error {
    //! Re-export of the shared `ToolError` so kernel modules use one path.
    pub use labby_runtime::error::ToolError;
}

mod artifacts;
mod broker;
mod config;
mod execute;
mod git;
pub mod host;
mod local_provider;
mod normalize;
mod pool;
mod preamble;
mod protocol;
mod runner;
mod runner_drive;
mod runner_exe;
mod runner_io;
mod schema;
mod shape;
pub mod snippet;
mod state;
mod trace;
mod truncate;
/// Live TypeScript signature / `.d.ts` generator for Code Mode tool descriptors.
mod ts_signatures;
mod types;
mod util;
mod wrapper;

#[cfg(test)]
mod tests_ids_schema;
#[cfg(test)]
mod tests_normalize;
#[cfg(test)]
mod tests_ts_signatures;

// ── Public surface ──────────────────────────────────────────────────────────

pub use broker::CodeModeBroker;
pub(crate) use broker::lab_action_unknown_tool_hint;
pub use config::{MAX_SOURCE_BYTES, SERVICE};
/// Single source of truth for the discovery-catalog scope formulas. Hosts
/// recomputing the sandbox's scope-filtered entry set (e.g. a gateway's
/// `semantic_rank`) MUST use these rather than restating the formulas.
pub use execute::{discovery_entry_visible, discovery_render_params};
pub use host::{CodeModeHost, ResolvedSnippet, ToolCallOutcome, ToolsRender};
/// Re-export so hosts can name the config type from one crate path.
pub use labby_runtime::CodeModeConfig;
pub use normalize::normalize_user_code;
pub use pool::{RunnerPool, RunnerSpawn};
/// Synchronous runner-subprocess entrypoint. Re-exported unchanged: the
/// consuming binary wires this into its hidden `internal code-mode-runner`
/// subcommand.
pub use runner::run_code_mode_runner_stdio;
pub use schema::validate_code_mode_params_against_schema;
pub use shape::CodeModeResultShapeMetadata;
pub use trace::code_mode_execute_trace;
pub use types::{
    CodeModeCaller, CodeModeCallerCapabilities, CodeModeCatalogKind, CodeModeExecutedCall,
    CodeModeExecutionError, CodeModeExecutionResponse, CodeModeExecutionSource, CodeModeHistory,
    CodeModeHistoryEntry, CodeModeHistoryKind, CodeModeSnippetInputEntry, CodeModeSourceLookup,
    CodeModeSourceStore, CodeModeSurface, ToolDescriptor, ToolScope, UiLink, destructive_permitted,
    namespaced_tool_id, split_namespaced_id,
};
pub use util::serialized_catalog_size;
