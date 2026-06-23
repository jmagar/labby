//! Shared marketplace artifact diff/merge placeholders.
//!
//! `lab-iut1.4` owns diff/patch behavior and `lab-iut1.5/.6` own update merge
//! behavior. This module exists now so those beads can share one hardened git
//! shell-out boundary without adding Rust diff crates.
#![allow(dead_code)]

use std::path::Path;
use std::process::Command;

use crate::dispatch::error::ToolError;

pub(super) async fn git_diff_files(
    _base: &Path,
    _current: &Path,
    display_label: &str,
) -> Result<Option<String>, ToolError> {
    Err(ToolError::Sdk {
        sdk_kind: "not_implemented".to_string(),
        message: format!("git diff helper is not implemented yet for `{display_label}`"),
    })
}

pub(super) async fn git_merge_file(
    _base: &Path,
    _ours: &Path,
    _theirs_content: &str,
) -> Result<String, String> {
    Err("git merge-file helper is not implemented yet".to_string())
}

pub(super) fn git_local_env() -> Command {
    let mut cmd = Command::new("git");
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null");
    cmd
}
