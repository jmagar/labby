//! Local release build + sha256 hashing + disk preflight.

use labby_apis::deploy::DeployError;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crate::config::ArtifactRole;

/// Artifact produced by a successful local build.
#[derive(Debug, Clone)]
pub struct BuildOutcome {
    pub path: PathBuf,
    pub sha256: String,
    pub size_bytes: u64,
    pub target_triple: String,
    /// The artifact role this outcome was built for.
    pub role: ArtifactRole,
}

/// Describes a specific artifact to build or reuse.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ArtifactProfile {
    pub role: ArtifactRole,
    /// Target triple, e.g. `"x86_64-unknown-linux-gnu"`.
    pub target_triple: String,
    /// Binary name, e.g. `"labby"`.
    pub bin: String,
    /// Cargo feature list, e.g. `["all"]` or `["node-runtime"]`.
    pub cargo_features: Vec<String>,
    /// Cargo profile name, e.g. `"controller-deploy"` or `"node-deploy"`.
    pub cargo_profile: String,
    /// Maximum build time in seconds. `None` defaults to 1800 (30 min).
    pub build_timeout_secs: Option<u64>,
}

impl ArtifactProfile {
    /// Build profile for the controller role (full-featured, fast-compile profile).
    pub fn controller() -> Self {
        Self {
            role: ArtifactRole::Controller,
            target_triple: detect_host_triple(),
            bin: "labby".to_string(),
            cargo_features: vec!["all".to_string()],
            cargo_profile: "controller-deploy".to_string(),
            build_timeout_secs: None,
        }
    }

    /// Build profile for the node role (full-featured for now, fast-compile profile).
    pub fn node() -> Self {
        Self {
            role: ArtifactRole::Node,
            target_triple: detect_host_triple(),
            bin: "labby".to_string(),
            cargo_features: vec!["all".to_string()],
            cargo_profile: "node-deploy".to_string(),
            build_timeout_secs: None,
        }
    }
}

/// Path where cargo places the binary for an `ArtifactProfile`.
///
/// - **Host triple**: `target/<cargo_profile>/<bin>`
/// - **Cross-compilation**: `target/<triple>/<cargo_profile>/<bin>`
/// - Windows targets get `.exe` appended.
pub fn expected_artifact_path_for_profile(profile: &ArtifactProfile) -> PathBuf {
    let name = if profile.target_triple.contains("windows") {
        format!("{}.exe", profile.bin)
    } else {
        profile.bin.clone()
    };
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("target"))
        .unwrap_or_else(|| PathBuf::from("target"));
    if profile.target_triple == detect_host_triple() {
        workspace.join(&profile.cargo_profile).join(&name)
    } else {
        workspace
            .join(&profile.target_triple)
            .join(&profile.cargo_profile)
            .join(&name)
    }
}

/// Build or reuse a role/profile-specific artifact.
pub async fn build_artifact(profile: &ArtifactProfile) -> Result<BuildOutcome, DeployError> {
    let build_started = Instant::now();
    let required_free_bytes = 1_500_000_000u64;
    let cargo_profile = &profile.cargo_profile;
    let target_triple = &profile.target_triple;
    tracing::info!(
        surface = "dispatch", service = "deploy.build", action = "build.start",
        target_triple = %target_triple,
        profile = %cargo_profile,
        role = ?profile.role,
        required_free_bytes,
        "starting local build",
    );
    let free = tokio::task::spawn_blocking(estimate_free_bytes)
        .await
        .map_err(|e| DeployError::BuildFailed {
            reason: format!("disk-space check join: {e}"),
        })??;
    check_disk_space(free, required_free_bytes)?;
    let path = expected_artifact_path_for_profile(profile);
    let rebuild_needed = tokio::task::spawn_blocking({
        let path = path.clone();
        move || rebuild_needed(&path)
    })
    .await
    .map_err(|e| DeployError::BuildFailed {
        reason: format!("rebuild check join: {e}"),
    })??;
    let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    if rebuild_needed {
        let cargo_started = Instant::now();
        let features = profile.cargo_features.join(",");
        let timeout_secs = profile.build_timeout_secs.unwrap_or(1800);
        let output = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            tokio::process::Command::new("cargo")
                .args([
                    "build",
                    "--profile",
                    cargo_profile.as_str(),
                    "--features",
                    features.as_str(),
                    "--manifest-path",
                ])
                .arg(&manifest_path)
                .output(),
        )
        .await
        .map_err(|_| DeployError::BuildFailed {
            reason: format!("build timed out after {timeout_secs}s"),
        })?
        .map_err(|e| DeployError::BuildFailed {
            reason: format!("spawn cargo: {e}"),
        })?;
        let cargo_elapsed_ms = cargo_started.elapsed().as_millis();
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail: Vec<&str> = stderr.lines().rev().take(10).collect();
            let tail = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
            tracing::warn!(
                surface = "dispatch", service = "deploy.build", action = "build.finish",
                target_triple = %target_triple,
                profile = %cargo_profile,
                elapsed_ms = build_started.elapsed().as_millis(),
                cargo_elapsed_ms,
                kind = "build_failed",
                "local build failed",
            );
            return Err(DeployError::BuildFailed { reason: tail });
        }
    } else {
        tracing::info!(
            surface = "dispatch", service = "deploy.build", action = "build.reuse",
            target_triple = %target_triple,
            profile = %cargo_profile,
            artifact = %path.display(),
            "deploy.build.reuse_existing_release",
        );
    }
    let (metadata, sha256, target_triple_owned) = tokio::task::spawn_blocking({
        let p = path.clone();
        let triple = target_triple.clone();
        move || -> Result<_, DeployError> {
            let meta = std::fs::metadata(&p).map_err(|e| DeployError::BuildFailed {
                reason: format!("stat artifact: {e}"),
            })?;
            let sha256 = sha256_file_blocking(&p)?;
            Ok((meta, sha256, triple))
        }
    })
    .await
    .map_err(|e| DeployError::BuildFailed {
        reason: format!("post-build join: {e}"),
    })??;
    let outcome = BuildOutcome {
        path,
        sha256: sha256.clone(),
        size_bytes: metadata.len(),
        target_triple: target_triple_owned.clone(),
        role: profile.role,
    };
    tracing::info!(
        surface = "dispatch", service = "deploy.build", action = "build.finish",
        profile = %cargo_profile,
        elapsed_ms = build_started.elapsed().as_millis(),
        size_bytes = outcome.size_bytes,
        sha256 = %sha256,
        target_triple = %target_triple_owned,
        "local build complete",
    );
    Ok(outcome)
}

/// Legacy wrapper: builds with the `release` profile, all features.
///
/// Delegates to [`build_artifact`] with an explicit [`ArtifactProfile`] using the
/// `"release"` cargo profile. Preserved for callers that have not yet been updated
/// to pass an explicit profile.
///
/// New code should call [`build_artifact`] directly with an [`ArtifactProfile`].
#[allow(dead_code)]
pub async fn build_release() -> Result<BuildOutcome, DeployError> {
    let profile = ArtifactProfile {
        role: ArtifactRole::Controller,
        target_triple: detect_host_triple(),
        bin: "labby".to_string(),
        cargo_features: vec!["all".to_string()],
        cargo_profile: "release".to_string(),
        build_timeout_secs: None,
    };
    build_artifact(&profile).await
}

/// Path where cargo places the binary for `target_triple`.
///
/// - **Host triple** (`target_triple == detect_host_triple()`): `target/release/<bin>`
/// - **Cross-compilation target**: `target/<triple>/release/<bin>`
/// - When `target_triple` contains `"windows"`, `.exe` is appended even on a
///   non-Windows build host. Never use `cfg!(target_os)` here — this is about
///   the *target*, not the host.
pub fn expected_artifact_path_for(bin: &str, target_triple: &str) -> PathBuf {
    let name = if target_triple.contains("windows") {
        format!("{bin}.exe")
    } else {
        bin.to_string()
    };
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("target"))
        .unwrap_or_else(|| PathBuf::from("target"));
    // Cargo places the artifact under `target/<triple>/release/` only when
    // cross-compiling (i.e., the target differs from the host triple).
    if target_triple == detect_host_triple() {
        workspace.join("release").join(&name)
    } else {
        workspace.join(target_triple).join("release").join(&name)
    }
}

/// Path under the workspace `target/release/` directory (host triple).
///
/// Delegates to `expected_artifact_path_for` using the current host triple.
/// Use `expected_artifact_path_for` directly when cross-compiling.
pub fn expected_artifact_path(bin: &str) -> PathBuf {
    expected_artifact_path_for(bin, &detect_host_triple())
}

/// Blocking SHA-256 of a file; call from `spawn_blocking`.
pub fn sha256_file_blocking(path: &Path) -> Result<String, DeployError> {
    let mut f = std::fs::File::open(path).map_err(|e| DeployError::BuildFailed {
        reason: format!("open: {e}"),
    })?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| DeployError::BuildFailed {
            reason: format!("read: {e}"),
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Error out when the local filesystem has less than `required` bytes free.
pub fn check_disk_space(available: u64, required: u64) -> Result<(), DeployError> {
    if available < required {
        return Err(DeployError::PreflightFailed {
            host: "localhost".into(),
            reason: format!("insufficient disk space: have {available} need {required}"),
        });
    }
    Ok(())
}

fn estimate_free_bytes() -> Result<u64, DeployError> {
    // Use POSIX-compatible `df -k` (kilobytes) — works on Linux, BSD, and macOS.
    // Output format: Filesystem 1K-blocks Used Available Capacity Mounted-on
    // "Available" is column index 3 (0-based), in 1 KiB units.
    let out = std::process::Command::new("df").arg("-k").arg(".").output();
    if let Ok(o) = out {
        if o.status.success() {
            if let Some(line) = String::from_utf8_lossy(&o.stdout).lines().nth(1) {
                let mut fields = line.split_whitespace();
                // Skip: Filesystem, 1K-blocks, Used; take Available
                if let Some(avail_kib) = fields.nth(3) {
                    if let Ok(kib) = avail_kib.parse::<u64>() {
                        return Ok(kib.saturating_mul(1024));
                    }
                }
            }
        }
    }
    // df unavailable or unparseable — skip the disk check rather than blocking.
    tracing::warn!("could not determine free disk space; skipping preflight disk check");
    Ok(u64::MAX)
}

fn rebuild_needed(artifact_path: &Path) -> Result<bool, DeployError> {
    let artifact_meta = match std::fs::metadata(artifact_path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(err) => {
            return Err(DeployError::BuildFailed {
                reason: format!("stat artifact: {err}"),
            });
        }
    };
    let artifact_mtime = artifact_meta
        .modified()
        .map_err(|err| DeployError::BuildFailed {
            reason: format!("artifact modified time: {err}"),
        })?;

    Ok(newest_build_input_mtime()? > artifact_mtime)
}

fn newest_build_input_mtime() -> Result<SystemTime, DeployError> {
    let root = workspace_root();
    let inputs = [
        root.join("Cargo.toml"),
        root.join("Cargo.lock"),
        root.join("crates/lab/Cargo.toml"),
        root.join("crates/lab/src"),
        root.join("crates/lab-apis/Cargo.toml"),
        root.join("crates/lab-apis/src"),
        root.join("crates/lab-auth/Cargo.toml"),
        root.join("crates/lab-auth/src"),
    ];

    let mut newest = SystemTime::UNIX_EPOCH;
    for input in inputs {
        newest = newest.max(path_latest_mtime(&input)?);
    }
    Ok(newest)
}

fn path_latest_mtime(path: &Path) -> Result<SystemTime, DeployError> {
    let metadata = std::fs::metadata(path).map_err(|err| DeployError::BuildFailed {
        reason: format!("stat build input `{}`: {err}", path.display()),
    })?;
    if metadata.is_file() {
        return metadata.modified().map_err(|err| DeployError::BuildFailed {
            reason: format!("mtime build input `{}`: {err}", path.display()),
        });
    }

    let mut newest = SystemTime::UNIX_EPOCH;
    for entry in std::fs::read_dir(path).map_err(|err| DeployError::BuildFailed {
        reason: format!("read build input dir `{}`: {err}", path.display()),
    })? {
        let entry = entry.map_err(|err| DeployError::BuildFailed {
            reason: format!("read build input entry `{}`: {err}", path.display()),
        })?;
        newest = newest.max(path_latest_mtime(&entry.path())?);
    }
    Ok(newest)
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn detect_host_triple() -> String {
    let out = std::process::Command::new("rustc").arg("-vV").output();
    if let Ok(o) = out {
        if o.status.success() {
            for line in String::from_utf8_lossy(&o.stdout).lines() {
                if let Some(rest) = line.strip_prefix("host: ") {
                    return rest.trim().to_string();
                }
            }
        }
    }
    std::env::consts::ARCH.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn sha256_of_known_bytes_is_deterministic() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("artifact");
        std::fs::write(&path, b"labby-binary-v1").unwrap();
        let hex = sha256_file_blocking(&path).unwrap();
        assert_eq!(hex.len(), 64);
        assert_eq!(hex, sha256_file_blocking(&path).unwrap());
    }

    #[test]
    fn build_target_path_matches_cargo_layout() {
        // Host triple → target/release/<bin> (no triple in path).
        let host = detect_host_triple();
        let p = expected_artifact_path_for("labby", &host);
        let expected = if host.contains("windows") {
            "target/release/labby.exe"
        } else {
            "target/release/labby"
        };
        assert!(p.ends_with(expected), "got {}", p.display());
    }

    #[test]
    fn cross_target_path_includes_triple() {
        // A cross-compilation target that differs from the host must include
        // the triple so cargo's output directory layout is matched correctly.
        let host = detect_host_triple();
        let cross = if host.contains("x86_64") {
            "aarch64-unknown-linux-gnu"
        } else {
            "x86_64-unknown-linux-gnu"
        };
        let p = expected_artifact_path_for("labby", cross);
        let expected = format!("target/{cross}/release/labby");
        assert!(p.ends_with(&expected), "got {}", p.display());
    }

    #[test]
    fn windows_target_appends_exe_suffix() {
        let host = detect_host_triple();
        let target = "x86_64-pc-windows-msvc";
        let p = expected_artifact_path_for("labby", target);
        if host == target {
            // Running on Windows: no triple in path
            assert!(
                p.ends_with("target/release/labby.exe"),
                "got {}",
                p.display()
            );
        } else {
            // Cross-compiling: triple is included in path
            assert!(
                p.ends_with("target/x86_64-pc-windows-msvc/release/labby.exe"),
                "got {}",
                p.display()
            );
        }
    }

    #[test]
    fn disk_preflight_rejects_below_threshold() {
        let err = check_disk_space(10, 100).unwrap_err();
        assert_eq!(err.kind(), "preflight_failed");
    }

    #[test]
    fn workspace_root_points_at_repo_root() {
        let root = workspace_root();
        assert!(root.join("Cargo.toml").exists(), "got {}", root.display());
        assert!(
            root.join("crates/lab/Cargo.toml").exists(),
            "got {}",
            root.display()
        );
    }

    #[test]
    fn controller_deploy_profile_path() {
        let controller = ArtifactProfile::controller();
        let p = expected_artifact_path_for_profile(&controller);
        // `expected_artifact_path_for_profile` appends `.exe` for windows host
        // triples, so assert against the host's exe suffix to stay correct on
        // both platforms.
        let expected = format!(
            "target/controller-deploy/labby{}",
            std::env::consts::EXE_SUFFIX
        );
        assert!(p.ends_with(&expected), "got {}", p.display());
    }

    #[test]
    fn node_deploy_profile_path() {
        let node = ArtifactProfile::node();
        let p = expected_artifact_path_for_profile(&node);
        let expected = format!("target/node-deploy/labby{}", std::env::consts::EXE_SUFFIX);
        assert!(p.ends_with(&expected), "got {}", p.display());
    }

    #[test]
    fn cross_compile_profile_path_includes_triple() {
        let host = detect_host_triple();
        let cross_triple = if host.contains("x86_64") {
            "aarch64-unknown-linux-gnu"
        } else {
            "x86_64-unknown-linux-gnu"
        };
        let profile = ArtifactProfile {
            role: ArtifactRole::Controller,
            target_triple: cross_triple.to_string(),
            bin: "labby".to_string(),
            cargo_features: vec!["all".to_string()],
            cargo_profile: "controller-deploy".to_string(),
            build_timeout_secs: None,
        };
        let p = expected_artifact_path_for_profile(&profile);
        let expected = format!("target/{cross_triple}/controller-deploy/labby");
        assert!(p.ends_with(&expected), "got {}", p.display());
    }
}
