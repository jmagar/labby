//! Code Mode runner stdio protocol types, shared runner state, and tuning consts.

use std::cell::RefCell;
use std::io::{self, BufReader, BufWriter};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CodeModeRunnerInput {
    Start {
        code: String,
        /// Auto-generated `var codemode = {...}` proxy JS (see
        /// `code_mode_preamble::generate_js_proxy`). Injected into the sandbox
        /// after `callTool` is defined so the user code can call
        /// `codemode.<upstream>.<tool>(params)`.
        ///
        /// `#[serde(default)]` keeps the search path and older Start messages
        /// (which carry only `code`) forward-compatible — they deserialize to
        /// an empty proxy, leaving `codemode` undefined exactly as before.
        #[serde(default)]
        proxy: String,
    },
    ToolResult {
        seq: u64,
        result: Value,
    },
    ToolError {
        seq: u64,
        kind: String,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CodeModeRunnerOutput {
    ToolCall {
        seq: u64,
        id: String,
        params: Value,
    },
    /// Runner completed successfully. `result` is the serialized return value of
    /// the async function (`Undefined` when the function returns undefined).
    /// `logs` carries captured console output (Boa path) or redirected stderr (Javy path).
    Done {
        // #[serde(default)] makes this variant forward-compatible: old runner binaries
        // that emit {"type":"done"} without these fields deserialize to Undefined/[] instead
        // of failing with a missing-field error.
        #[serde(default)]
        result: CodeModeRunnerResult,
        #[serde(default)]
        logs: Vec<String>,
    },
    Error {
        kind: String,
        message: String,
    },
}

impl CodeModeRunnerOutput {
    #[must_use]
    #[cfg(test)]
    pub(in crate::dispatch::gateway::code_mode) fn result_for_response(&self) -> Option<Value> {
        match self {
            Self::Done { result, .. } => result.clone().into_response_result(),
            Self::ToolCall { .. } | Self::Error { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum CodeModeRunnerResult {
    #[default]
    Undefined,
    Json(Value),
}

impl CodeModeRunnerResult {
    #[must_use]
    pub(in crate::dispatch::gateway::code_mode) fn from_response_result(
        result: Option<Value>,
    ) -> Self {
        match result {
            Some(value) => Self::Json(value),
            None => Self::Undefined,
        }
    }

    #[must_use]
    pub(in crate::dispatch::gateway::code_mode) fn into_response_result(self) -> Option<Value> {
        match self {
            Self::Undefined => None,
            Self::Json(value) => Some(value),
        }
    }
}

pub(in crate::dispatch::gateway::code_mode) struct CodeModeRunnerState {
    pub(in crate::dispatch::gateway::code_mode) reader: BufReader<io::Stdin>,
    pub(in crate::dispatch::gateway::code_mode) writer: BufWriter<io::Stdout>,
    pub(in crate::dispatch::gateway::code_mode) next_seq: u64,
}

thread_local! {
    pub(in crate::dispatch::gateway::code_mode) static RUNNER_STATE: RefCell<Option<CodeModeRunnerState>> = const { RefCell::new(None) };
}

// Javy interprets this as the native stack size in bytes. The runtime
// `codemode.*` proxy preamble (one method per upstream tool, ~140+ across the
// gateway) plus await/Promise machinery needs ample headroom; 256 KiB avoids
// operand-stack overflow on a single callTool.
pub(in crate::dispatch::gateway::code_mode) const CODE_MODE_STACK_SIZE_LIMIT: usize = 256 * 1024;

/// Wall-clock budget for a `search` filter run in the Javy runner. Search does
/// pure computation over the catalog (no tool calls), so this is shorter than
/// the configurable `execute` timeout.
pub(in crate::dispatch::gateway::code_mode) const CODE_MODE_SEARCH_TIMEOUT: Duration =
    Duration::from_secs(15);
