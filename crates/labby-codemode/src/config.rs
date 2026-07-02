//! Shared Code Mode constants.
//!
//! These values are intentionally centralized so host surfaces and the sandbox
//! driver cannot drift on source-size limits, observability labels, or snippet
//! budgets.

/// Tracing `service` field for every Code Mode dispatch event.
pub const SERVICE: &str = "code_mode";

/// Maximum accepted Code Mode source size in bytes.
pub const MAX_SOURCE_BYTES: usize = 20_000;

/// Maximum `codemode.run(...)` snippet resolutions allowed in a single run.
pub(crate) const MAX_SNIPPET_RESOLVES_PER_RUN: usize = 32;

/// Hard ceiling on reserved `__lab_internal::*` pseudo-tool calls per run.
///
/// Internal calls (currently `semantic_rank`) are deliberately exempt from
/// the ordinary `callTool` budget and the call trace, but each one can
/// trigger an embedding-service round trip — without a separate ceiling,
/// sandbox JS could loop them for unbounded load on the shared TEI service.
/// Over-ceiling internal calls settle fail-open with the empty semantic
/// result instead of erroring the run (see `runner_drive.rs`).
pub(crate) const MAX_INTERNAL_CALLS_PER_RUN: usize = 32;

/// Maximum total bytes of resolved snippet source allowed in a single run.
pub(crate) const MAX_SNIPPET_RESOLVED_BYTES_PER_RUN: usize = 256 * 1024;

/// Default per-run `callTool` fan-out budget.
const DEFAULT_MAX_CALLTOOL_PER_RUN: u64 = 512;
/// Hard ceiling on the configurable per-run `callTool` budget.
const MAX_CALLTOOL_PER_RUN_CEILING: u64 = 2048;
/// Default host-side byte ceiling on a single `callTool` result, in MiB.
const DEFAULT_CALLTOOL_RESULT_MAX_MIB: usize = 8;

/// Resolve the per-run `callTool` fan-out budget from the environment.
pub(crate) fn max_calltool_per_run() -> u64 {
    let Some(raw) = crate::util::env_non_empty("LAB_CODE_MODE_MAX_CALLS_PER_RUN") else {
        return DEFAULT_MAX_CALLTOOL_PER_RUN;
    };
    match raw.trim().parse::<u64>() {
        Ok(value) if value > 0 => value.min(MAX_CALLTOOL_PER_RUN_CEILING),
        _ => {
            tracing::warn!(
                surface = "dispatch",
                service = SERVICE,
                action = "codemode",
                value = %raw,
                default = DEFAULT_MAX_CALLTOOL_PER_RUN,
                "ignoring invalid LAB_CODE_MODE_MAX_CALLS_PER_RUN; using default"
            );
            DEFAULT_MAX_CALLTOOL_PER_RUN
        }
    }
}

/// Resolve the per-result byte ceiling from the environment.
pub(crate) fn calltool_result_max_bytes() -> usize {
    let default_bytes = DEFAULT_CALLTOOL_RESULT_MAX_MIB * 1024 * 1024;
    let Some(raw) = crate::util::env_non_empty("LAB_CODE_MODE_CALLTOOL_RESULT_MAX_MIB") else {
        return default_bytes;
    };
    match raw.trim().parse::<usize>() {
        Ok(mib) if mib > 0 => mib.saturating_mul(1024 * 1024),
        _ => {
            tracing::warn!(
                surface = "dispatch",
                service = SERVICE,
                action = "codemode",
                value = %raw,
                default_mib = DEFAULT_CALLTOOL_RESULT_MAX_MIB,
                "ignoring invalid LAB_CODE_MODE_CALLTOOL_RESULT_MAX_MIB; using default"
            );
            default_bytes
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_label_is_canonical() {
        assert_eq!(SERVICE, "code_mode");
    }

    #[test]
    fn max_source_bytes_is_stable() {
        assert_eq!(MAX_SOURCE_BYTES, 20_000);
    }
}
