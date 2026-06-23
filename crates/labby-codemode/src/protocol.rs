//! Code Mode runner stdio protocol types, shared runner state, and tuning consts.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cell::RefCell;
use std::io::{self, BufReader, BufWriter};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum CodeModeRunnerInput {
    Start {
        code: String,
        /// Auto-generated `var codemode = {...}` proxy JS (see
        /// `code_mode_preamble::generate_js_proxy`). Injected into the sandbox
        /// after `callTool` is defined so the user code can call
        /// `codemode.<namespace>.<tool>(params)`.
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
    SnippetResolved {
        seq: u64,
        code: String,
        input: Value,
    },
    ToolError {
        seq: u64,
        kind: String,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum CodeModeRunnerOutput {
    ToolCall {
        seq: u64,
        id: String,
        params: Value,
    },
    /// The sandbox called `writeArtifact(path, content, options?)`. The host
    /// validates `path`, writes `content` under the per-run artifact root, and
    /// settles the matching promise with a receipt (or a structured error).
    /// `#[serde(default)]` on `content_type` keeps the field optional so a
    /// caller that omits `options.contentType` deserializes to `None` (the host
    /// then defaults it to `text/plain`).
    ArtifactWrite {
        seq: u64,
        path: String,
        content: String,
        #[serde(default)]
        content_type: Option<String>,
    },
    SnippetResolve {
        seq: u64,
        name: String,
        #[serde(default)]
        input: Value,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub(crate) enum CodeModeRunnerResult {
    #[default]
    Undefined,
    Json(Value),
}

impl CodeModeRunnerResult {
    #[must_use]
    pub(crate) fn from_response_result(result: Option<Value>) -> Self {
        match result {
            Some(value) => Self::Json(value),
            None => Self::Undefined,
        }
    }

    #[must_use]
    pub(crate) fn into_response_result(self) -> Option<Value> {
        match self {
            Self::Undefined => None,
            Self::Json(value) => Some(value),
        }
    }
}

pub(crate) struct CodeModeRunnerState {
    pub(crate) reader: BufReader<io::Stdin>,
    pub(crate) writer: BufWriter<io::Stdout>,
    pub(crate) next_seq: u64,
}

thread_local! {
    pub(crate) static RUNNER_STATE: RefCell<Option<CodeModeRunnerState>> = const { RefCell::new(None) };
}

// Javy interprets this as the native stack size in bytes. The runtime
// `codemode.*` proxy preamble (one method per tool, ~140+ across the
// catalog) plus await/Promise machinery needs ample headroom; 256 KiB avoids
// operand-stack overflow on a single callTool.
pub(crate) const CODE_MODE_STACK_SIZE_LIMIT: usize = 256 * 1024;
