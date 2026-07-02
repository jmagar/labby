//! Targeted repair for package-runner transient caches.
//!
//! `npx -y` and `uvx` are convenient stdio MCP launchers, but both create
//! mutable transient execution/cache state. If a process is interrupted or the
//! cache was previously unwritable, startup can fail before MCP initialize with
//! stale `_npx` workdirs, stale npm metadata, or uv temp build dirs. This module
//! classifies those signatures and repairs only the transient pieces before a
//! single retry.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use tokio::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CacheRepairOutcome {
    NotApplicable,
    NotNeeded,
    Repaired { summary: String },
    Failed { summary: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeKind {
    Npx,
    Uvx,
}

pub(super) fn is_repaired(outcome: &CacheRepairOutcome) -> bool {
    matches!(outcome, CacheRepairOutcome::Repaired { .. })
}

pub(super) async fn maybe_repair(command: &str, diagnostics: &str) -> CacheRepairOutcome {
    let Some(kind) = runtime_kind(command) else {
        return CacheRepairOutcome::NotApplicable;
    };
    if !diagnostics_indicate_cache_poison(kind, diagnostics) {
        return CacheRepairOutcome::NotNeeded;
    }

    match kind {
        RuntimeKind::Npx => repair_npm_cache().await,
        RuntimeKind::Uvx => repair_uv_cache().await,
    }
}

fn runtime_kind(command: &str) -> Option<RuntimeKind> {
    let binary = Path::new(command)
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or(command);
    match binary {
        "npx" => Some(RuntimeKind::Npx),
        "uvx" => Some(RuntimeKind::Uvx),
        _ => None,
    }
}

fn diagnostics_indicate_cache_poison(kind: RuntimeKind, diagnostics: &str) -> bool {
    let lower = diagnostics.to_ascii_lowercase();
    match kind {
        RuntimeKind::Npx => {
            lower.contains("eacces")
                || lower.contains("permission denied")
                || lower.contains("_npx")
                || lower.contains("_cacache")
                || lower.contains("err_module_not_found")
                || lower.contains("cannot find module")
                || lower.contains("cannot find package")
                || lower.contains("no matching version found")
                || lower.contains("npm error notarget")
        }
        RuntimeKind::Uvx => {
            lower.contains("failed to initialize cache")
                || lower.contains("permission denied")
                || lower.contains(".cache/uv")
                || lower.contains("no module named")
                || lower.contains("module_not_found")
        }
    }
}

async fn repair_npm_cache() -> CacheRepairOutcome {
    let cache = npm_cache_dir();
    if let Err(error) = ensure_writable_dir(&cache) {
        return CacheRepairOutcome::Failed {
            summary: format!("npm cache dir {} is not writable: {error}", cache.display()),
        };
    }

    let mut actions = Vec::new();
    match remove_children(&cache.join("_npx")) {
        Ok(removed) if removed > 0 => actions.push(format!("removed {removed} _npx entries")),
        Ok(_) => {}
        Err(error) => {
            return CacheRepairOutcome::Failed {
                summary: format!("failed to clean npm _npx state: {error}"),
            };
        }
    }

    // npm stores registry packument metadata through the content-addressed cache
    // index. Deleting only the index forces metadata revalidation while leaving
    // content blobs available as orphaned cache entries for `npm cache verify`.
    for relative in [["_cacache", "index-v5"], ["_cacache", "tmp"]] {
        let path = relative
            .iter()
            .fold(cache.clone(), |acc, part| acc.join(part));
        if path.exists() {
            if let Err(error) = std::fs::remove_dir_all(&path) {
                return CacheRepairOutcome::Failed {
                    summary: format!("failed to remove {}: {error}", path.display()),
                };
            }
            actions.push(format!("removed {}", path.display()));
        }
    }

    drop(
        Command::new("npm")
            .arg("cache")
            .arg("verify")
            .output()
            .await,
    );

    CacheRepairOutcome::Repaired {
        summary: if actions.is_empty() {
            format!("verified npm cache at {}", cache.display())
        } else {
            actions.join("; ")
        },
    }
}

async fn repair_uv_cache() -> CacheRepairOutcome {
    let cache = uv_cache_dir();
    if let Err(error) = ensure_writable_dir(&cache) {
        return CacheRepairOutcome::Failed {
            summary: format!("uv cache dir {} is not writable: {error}", cache.display()),
        };
    }

    let mut removed = 0usize;
    for child in ["builds-v0", "environments-v2", "sdists-v9"] {
        match remove_tmp_children(&cache.join(child)) {
            Ok(count) => removed += count,
            Err(error) => {
                return CacheRepairOutcome::Failed {
                    summary: format!("failed to clean uv temp state under {child}: {error}"),
                };
            }
        }
    }

    let prune = Command::new("uv").arg("cache").arg("prune").output().await;
    let prune_ok = prune.as_ref().is_ok_and(|output| output.status.success());
    if let Err(error) = prune {
        tracing::debug!(
            service = "upstream.pool",
            action = "upstream.cache_repair",
            %error,
            "uv cache prune failed after temp cleanup"
        );
    }

    CacheRepairOutcome::Repaired {
        summary: format!(
            "removed {removed} uv temp entries{}",
            if prune_ok { "; pruned uv cache" } else { "" }
        ),
    }
}

fn npm_cache_dir() -> PathBuf {
    std::env::var_os("NPM_CONFIG_CACHE")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".npm")))
        .unwrap_or_else(|| PathBuf::from(".npm"))
}

fn uv_cache_dir() -> PathBuf {
    std::env::var_os("UV_CACHE_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CACHE_HOME").map(|home| PathBuf::from(home).join("uv")))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache/uv")))
        .unwrap_or_else(|| PathBuf::from(".cache/uv"))
}

fn ensure_writable_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    let probe = path.join(".labby-cache-repair-write-test");
    std::fs::write(&probe, b"ok")?;
    drop(std::fs::remove_file(probe));
    Ok(())
}

fn remove_children(path: &Path) -> std::io::Result<usize> {
    if !path.exists() {
        return Ok(0);
    }
    let mut removed = 0;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let entry_path = entry.path();
        let metadata = entry.file_type()?;
        if metadata.is_dir() {
            std::fs::remove_dir_all(&entry_path)?;
        } else {
            std::fs::remove_file(&entry_path)?;
        }
        removed += 1;
    }
    Ok(removed)
}

fn remove_tmp_children(path: &Path) -> std::io::Result<usize> {
    if !path.exists() {
        return Ok(0);
    }
    let mut removed = 0;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(".tmp") && !name.starts_with("tmp") {
            continue;
        }
        let entry_path = entry.path();
        if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(&entry_path)?;
        } else {
            std::fs::remove_file(&entry_path)?;
        }
        removed += 1;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn npx_diagnostics_detect_transient_cache_failures() {
        for diagnostic in [
            "ERR_MODULE_NOT_FOUND: Cannot find package 'zod'",
            "npm error notarget No matching version found for mcp-searxng@1.6.0",
            "EACCES: permission denied, mkdir '/home/labby/.npm/_cacache'",
            "Cannot find module 'fast-glob'",
        ] {
            assert!(
                diagnostics_indicate_cache_poison(RuntimeKind::Npx, diagnostic),
                "{diagnostic}"
            );
        }
    }

    #[test]
    fn uvx_diagnostics_detect_cache_failures() {
        for diagnostic in [
            "Failed to initialize cache at /home/labby/.cache/uv: Permission denied",
            "ModuleNotFoundError: No module named 'tzlocal'",
        ] {
            assert!(
                diagnostics_indicate_cache_poison(RuntimeKind::Uvx, diagnostic),
                "{diagnostic}"
            );
        }
    }

    #[test]
    fn unrelated_crash_does_not_trigger_repair() {
        assert!(!diagnostics_indicate_cache_poison(
            RuntimeKind::Npx,
            "server exited because API_KEY is missing"
        ));
        assert!(!diagnostics_indicate_cache_poison(
            RuntimeKind::Uvx,
            "server exited because API_KEY is missing"
        ));
    }

    #[test]
    fn remove_children_cleans_npx_workdirs_only_inside_target() {
        let temp = tempfile::tempdir().expect("tempdir");
        let npx = temp.path().join("_npx");
        std::fs::create_dir_all(npx.join("abc/node_modules")).expect("mkdir");
        std::fs::write(npx.join("abc/package.json"), "{}").expect("write");
        std::fs::write(npx.join("file"), "x").expect("write");

        let removed = remove_children(&npx).expect("remove");
        assert_eq!(removed, 2);
        assert!(std::fs::read_dir(&npx).expect("read").next().is_none());
    }

    #[test]
    fn remove_tmp_children_keeps_non_tmp_uv_entries() {
        let temp = tempfile::tempdir().expect("tempdir");
        let builds = temp.path().join("builds-v0");
        std::fs::create_dir_all(builds.join(".tmpABC")).expect("tmp");
        std::fs::create_dir_all(builds.join("real-build")).expect("real");

        let removed = remove_tmp_children(&builds).expect("remove");
        assert_eq!(removed, 1);
        assert!(!builds.join(".tmpABC").exists());
        assert!(builds.join("real-build").exists());
    }
}
