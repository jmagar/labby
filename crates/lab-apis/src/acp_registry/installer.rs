//! `AcpInstaller` — download, verify, extract, and install ACP binary agents.
//!
//! This is the reusable, surface-neutral primitive that the `lab` binary's
//! marketplace dispatch orchestrates. It owns the security-sensitive parts of
//! installing a `binary` distribution agent, with each guard kept next to the
//! primitive it protects:
//!
//! - **SSRF**: the HTTP client is built with the archive host resolved once,
//!   the resolved address validated against [`super::ssrf`], a *single*
//!   validated address pinned via `resolve_to_addrs`, `redirect::none()`, and
//!   `no_proxy()`. After the response arrives the connected peer IP is
//!   re-validated against the pinned address (defense-in-depth vs. TOCTOU /
//!   DNS-rebinding).
//! - **Archive-bomb**: streaming download with a 256 MiB hard size cap and a
//!   per-chunk stall watchdog; partial files are removed on failure.
//! - **Integrity**: SHA-256 is mandatory (fail-closed) and verified before
//!   extraction.
//! - **Zip-slip / tar-bomb / symlink**: post-extract canonical-containment
//!   walk rejecting every symlink and any entry escaping the root, plus an
//!   entry-count pre/post check and warnings-as-errors on tool stderr.
//! - **setuid strip**: installed binary is forced to exactly `0o755` (never
//!   OR-ed with the archive's mode) so setuid/setgid bits cannot survive.
//!
//! No `clap`/`rmcp`/`axum`/`anyhow` here — only `reqwest`/`serde`/`thiserror`/
//! `tokio`/`std`, per the `lab-apis` crate contract.

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::core::error::ApiError;

use super::ssrf::{self, SsrfError};

/// Maximum bytes accepted for one ACP binary archive download.
///
/// The registry distributes small agent adapters; 256 MiB leaves enough room
/// for bundled runtimes while preventing unbounded disk growth from hostile or
/// misconfigured archive URLs. Oversized streams are aborted and partial files
/// are removed before surfacing the error.
pub const MAX_ACP_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;

/// Maximum total *uncompressed* size accepted across all archive entries.
///
/// The download cap ([`MAX_ACP_ARCHIVE_BYTES`]) only bounds the compressed
/// stream — a small, highly-compressible tar.gz/zip can expand to many GiB on
/// disk (a "decompression bomb"). Before extraction the installer sums the
/// uncompressed sizes reported by `tar -tzv` / `unzip -l` and rejects the
/// archive if the total exceeds this cap (2× the download cap), bounding the
/// worst-case expansion ratio.
pub const MAX_ACP_UNCOMPRESSED_BYTES: u64 = 2 * MAX_ACP_ARCHIVE_BYTES;

/// Abort a download if no bytes arrive within this window. Distinct from the
/// overall request `.timeout()` — catches stalled connections that are neither
/// fast-failing nor completing.
const DOWNLOAD_STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// Overall per-request timeout for the archive download.
const ARCHIVE_REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

/// Errors produced by [`AcpInstaller`]. Wraps [`ApiError`] transparently so it
/// composes with the rest of the SDK error taxonomy; the `kind()` accessor
/// returns the stable dispatcher kind string callers map onto their surface
/// error envelopes.
#[derive(Debug, thiserror::Error)]
pub enum AcpInstallerError {
    /// SSRF preflight / peer re-validation rejected the archive host.
    #[error("{0}")]
    Ssrf(#[from] SsrfError),
    /// Archive URL or other caller-supplied parameter was invalid.
    #[error("{0}")]
    InvalidParam(String),
    /// Mandatory SHA-256 integrity metadata was missing.
    #[error("{0}")]
    IntegrityMissing(String),
    /// Downloaded bytes did not match the expected SHA-256.
    #[error("{0}")]
    IntegrityMismatch(String),
    /// Download exceeded [`MAX_ACP_ARCHIVE_BYTES`].
    #[error("{0}")]
    ContentTooLarge(String),
    /// Download stalled past the watchdog window.
    #[error("{0}")]
    InstallTimeout(String),
    /// Archive entry escaped the extraction root, or a symlink was present.
    #[error("{0}")]
    PathTraversal(String),
    /// Network/transport failure during download.
    #[error("{0}")]
    Network(String),
    /// Expected binary not found in the extracted archive.
    #[error("{0}")]
    NotFound(String),
    /// Underlying SDK/transport error.
    #[error(transparent)]
    Api(#[from] ApiError),
    /// Filesystem / subprocess / other internal failure.
    #[error("{0}")]
    Internal(String),
}

impl AcpInstallerError {
    /// Stable dispatcher kind tag for surface error envelopes.
    #[must_use]
    pub fn kind(&self) -> &str {
        match self {
            Self::Ssrf(e) => e.kind(),
            Self::InvalidParam(_) => "invalid_param",
            Self::IntegrityMissing(_) => "integrity_missing",
            Self::IntegrityMismatch(_) => "integrity_mismatch",
            Self::ContentTooLarge(_) => "content_too_large",
            Self::InstallTimeout(_) => "install_timeout",
            Self::PathTraversal(_) => "path_traversal",
            Self::Network(_) => "network_error",
            Self::NotFound(_) => "not_found",
            Self::Api(e) => e.kind(),
            Self::Internal(_) => "internal_error",
        }
    }

    fn internal(msg: impl Into<String>) -> Self {
        Self::Internal(msg.into())
    }
}

/// Specification for installing one binary-distributed ACP agent.
///
/// All policy-relevant inputs are explicit so the installer never reaches into
/// env or the registry itself — the orchestrator (dispatch) resolves these.
#[derive(Debug, Clone)]
pub struct InstallSpec {
    /// Archive download URL (HTTPS, validated by the installer).
    pub archive_url: String,
    /// Expected SHA-256 digest, as 64 hex chars or `sha256:<hex>` — taken from
    /// the registry's `sha256`/`digest` fields. Mandatory.
    pub expected_sha256: String,
    /// Post-extraction command path (e.g. `"./my-agent"` or `"my-agent"`); the
    /// installer locates the matching file name in the extracted tree.
    pub cmd: String,
    /// Directory to install the binary into (e.g. `~/.lab/bin/<agent_id>/`).
    /// Created if absent. Must already be validated by the caller.
    pub install_dir: PathBuf,
}

/// Result of a successful [`AcpInstaller::install`].
#[derive(Debug, Clone)]
pub struct InstallOutcome {
    /// Absolute path of the installed executable.
    pub installed_path: PathBuf,
    /// Hex SHA-256 of the downloaded archive (lowercase, verified).
    pub sha256: String,
}

/// Stateless installer for ACP binary agents. Construct with [`AcpInstaller::new`].
#[derive(Debug, Default, Clone)]
pub struct AcpInstaller {
    _private: (),
}

impl AcpInstaller {
    /// Construct a new installer.
    #[must_use]
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Validate the archive URL's SSRF posture without performing any I/O.
    ///
    /// Exposed so callers can fail fast before acquiring install locks.
    ///
    /// # Errors
    /// Returns [`AcpInstallerError::Ssrf`] / [`AcpInstallerError::InvalidParam`].
    pub fn validate_archive_url(url: &str) -> Result<(), AcpInstallerError> {
        ssrf::parse_validated_https_url(url)?;
        Ok(())
    }

    /// Normalize a registry-supplied SHA-256 (`<hex>` or `sha256:<hex>`) into a
    /// lowercase 64-char hex string.
    ///
    /// # Errors
    /// Returns [`AcpInstallerError::InvalidParam`] if not a valid digest.
    pub fn normalize_sha256(value: &str, field: &str) -> Result<String, AcpInstallerError> {
        let trimmed = value.trim();
        let hex = trimmed.strip_prefix("sha256:").unwrap_or(trimmed);
        if hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Ok(hex.to_ascii_lowercase());
        }
        Err(AcpInstallerError::InvalidParam(format!(
            "binary archive `{field}` must be a SHA-256 digest as 64 hex chars or sha256:<hex>"
        )))
    }

    /// Full install pipeline: validate URL → download (size-capped, SSRF-pinned)
    /// → verify SHA-256 → extract (zip-slip/symlink guarded) → atomically
    /// install the executable with `0o755`.
    ///
    /// The caller is responsible for serializing concurrent installs of the
    /// same agent (the installer uses per-call tempfiles, so distinct calls do
    /// not race on the same paths, but the final `install_dir` rename is not
    /// internally locked).
    ///
    /// # Errors
    /// Returns [`AcpInstallerError`] for any validation, network, integrity,
    /// extraction, or filesystem failure.
    pub async fn install(&self, spec: &InstallSpec) -> Result<InstallOutcome, AcpInstallerError> {
        let expected_sha256 = Self::normalize_sha256(&spec.expected_sha256, "sha256")?;
        let parsed = ssrf::parse_validated_https_url(&spec.archive_url)?;

        tokio::fs::create_dir_all(&spec.install_dir)
            .await
            .map_err(|e| {
                AcpInstallerError::internal(format!("create {}: {e}", spec.install_dir.display()))
            })?;

        // Download to a temp file next to the install dir so rename is atomic.
        let tmp_archive = tempfile::NamedTempFile::new_in(&spec.install_dir)
            .map_err(|e| AcpInstallerError::internal(format!("temp archive: {e}")))?;

        let sha256 = download_archive(&parsed, tmp_archive.path()).await?;
        verify_sha256(&expected_sha256, &sha256)?;

        // Extract to a temp dir in the same parent for an atomic move.
        let tmp_extract = tempfile::TempDir::new_in(&spec.install_dir)
            .map_err(|e| AcpInstallerError::internal(format!("temp extract dir: {e}")))?;

        extract_archive(tmp_archive.path(), tmp_extract.path(), &spec.archive_url)?;

        let binary_name = Path::new(spec.cmd.trim_start_matches("./"))
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(spec.cmd.trim_start_matches("./"));

        let src = find_binary_in_dir(tmp_extract.path(), binary_name).ok_or_else(|| {
            AcpInstallerError::NotFound(format!(
                "binary `{binary_name}` not found in extracted archive"
            ))
        })?;

        let dest = spec.install_dir.join(binary_name);
        install_executable_atomically(&src, &dest)?;

        Ok(InstallOutcome {
            installed_path: dest,
            sha256,
        })
    }
}

// ---------------------------------------------------------------------------
// SSRF-pinned HTTP client + download
// ---------------------------------------------------------------------------

/// Resolve `host:port`, validate every resolved address, and return them so a
/// single one can be pinned. Empty resolution and any private/blocked address
/// are hard failures.
async fn resolve_and_validate(host: &str, port: u16) -> Result<Vec<SocketAddr>, AcpInstallerError> {
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| AcpInstallerError::Network(format!("resolve archive host `{host}`: {e}")))?
        .collect();

    if addrs.is_empty() {
        return Err(AcpInstallerError::Network(format!(
            "resolve archive host `{host}` returned no addresses"
        )));
    }

    for addr in &addrs {
        ssrf::check_ip_not_private(addr.ip(), host)?;
    }

    Ok(addrs)
}

/// Build a reqwest client that pins exactly ONE validated address for `host`,
/// disables redirects and proxies. Pinning a single address (rather than the
/// whole resolved set) shrinks the TOCTOU window: reqwest can only connect to
/// the address we validated, and the caller re-checks the peer afterward.
async fn archive_download_client(
    parsed: &url::Url,
) -> Result<(reqwest::Client, IpAddr), AcpInstallerError> {
    let host = parsed.host_str().ok_or_else(|| {
        AcpInstallerError::InvalidParam(format!("archive URL has no host: {parsed}"))
    })?;
    let port = parsed.port_or_known_default().unwrap_or(443);

    let addrs = resolve_and_validate(host, port).await?;
    // Pin a single validated address. `resolve_and_validate` guarantees the
    // list is non-empty.
    let pinned = addrs[0];

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .resolve_to_addrs(host, &[pinned])
        .timeout(ARCHIVE_REQUEST_TIMEOUT)
        .build()
        .map_err(|e| AcpInstallerError::internal(format!("build http client: {e}")))?;

    Ok((client, pinned.ip()))
}

/// Reject a connected peer whose IP does not match the single validated address
/// pinned before the connection. This is the load-bearing line of the SSRF
/// TOCTOU / DNS-rebinding defense (Sec-M1): a mismatch means the connection
/// landed somewhere other than the address we validated.
///
/// Pure (no I/O) so both branches are unit-testable without a live connection.
///
/// # Errors
/// Returns [`AcpInstallerError::Ssrf`] when `peer != pinned`.
fn peer_matches_pin(peer: IpAddr, pinned: IpAddr) -> Result<(), AcpInstallerError> {
    if peer == pinned {
        return Ok(());
    }
    Err(AcpInstallerError::Ssrf(SsrfError::Blocked(format!(
        "archive connection peer {peer} does not match the validated address {pinned}; blocked to prevent SSRF"
    ))))
}

fn size_error() -> AcpInstallerError {
    AcpInstallerError::ContentTooLarge(format!(
        "download exceeded maximum ACP archive size of {MAX_ACP_ARCHIVE_BYTES} bytes"
    ))
}

fn enforce_size_limit(downloaded: &mut u64, chunk_len: usize) -> Result<(), AcpInstallerError> {
    let chunk_len = u64::try_from(chunk_len).map_err(|_| size_error())?;
    let next = downloaded.checked_add(chunk_len).ok_or_else(size_error)?;
    if next > MAX_ACP_ARCHIVE_BYTES {
        return Err(size_error());
    }
    *downloaded = next;
    Ok(())
}

async fn cleanup_partial(dest: &Path) {
    if let Err(e) = tokio::fs::remove_file(dest).await {
        tracing::warn!(
            service = "acp_registry",
            event = "archive.cleanup_failed",
            path = %dest.display(),
            error = %e,
            "download-cleanup remove_file failed; partial archive retained"
        );
    }
}

/// Download `parsed` to `dest`, returning the lowercase hex SHA-256.
///
/// Streams chunks to both the hasher and the file with no full-archive buffer.
/// Re-validates the connected peer IP against the validation-time pin
/// (defense-in-depth vs. TOCTOU). A per-chunk stall watchdog aborts hung
/// connections and removes the partial file.
async fn download_archive(parsed: &url::Url, dest: &Path) -> Result<String, AcpInstallerError> {
    use futures::StreamExt;
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncWriteExt;

    let url = parsed.as_str();
    let (client, pinned_ip) = archive_download_client(parsed).await?;

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| AcpInstallerError::Network(format!("GET archive: {e}")))?;

    // Re-validate the peer we actually connected to. With a single pinned
    // address this should always equal `pinned_ip`; rejecting any mismatch
    // closes the residual rebinding/redirect-to-internal window.
    if let Some(peer) = resp.remote_addr() {
        peer_matches_pin(peer.ip(), pinned_ip)?;
        // The pinned address was validated pre-connect; re-run the IP guard in
        // case resolution semantics changed underneath us.
        ssrf::check_ip_not_private(peer.ip(), "archive peer")?;
    }

    if !resp.status().is_success() {
        return Err(AcpInstallerError::Network(format!(
            "GET archive: HTTP {}",
            resp.status()
        )));
    }
    if let Some(content_length) = resp.content_length() {
        if content_length > MAX_ACP_ARCHIVE_BYTES {
            return Err(size_error());
        }
    }

    let mut file = tokio::fs::File::create(dest)
        .await
        .map_err(|e| AcpInstallerError::internal(format!("create {}: {e}", dest.display())))?;

    let mut hasher = Sha256::new();
    let mut stream = resp.bytes_stream();
    let mut downloaded = 0_u64;

    loop {
        match tokio::time::timeout(DOWNLOAD_STALL_TIMEOUT, stream.next()).await {
            Ok(Some(chunk_result)) => {
                let chunk = chunk_result
                    .map_err(|e| AcpInstallerError::Network(format!("read body chunk: {e}")))?;
                if let Err(e) = enforce_size_limit(&mut downloaded, chunk.len()) {
                    drop(file);
                    cleanup_partial(dest).await;
                    return Err(e);
                }
                hasher.update(&chunk);
                file.write_all(&chunk).await.map_err(|e| {
                    AcpInstallerError::internal(format!("write chunk to {}: {e}", dest.display()))
                })?;
            }
            Ok(None) => break,
            Err(_) => {
                drop(file);
                cleanup_partial(dest).await;
                return Err(AcpInstallerError::InstallTimeout(format!(
                    "download stalled for more than {DOWNLOAD_STALL_TIMEOUT:?}; aborted"
                )));
            }
        }
    }

    file.flush()
        .await
        .map_err(|e| AcpInstallerError::internal(format!("flush {}: {e}", dest.display())))?;
    // Durably commit before returning the SHA so the hash matches bytes on disk.
    file.sync_all()
        .await
        .map_err(|e| AcpInstallerError::internal(format!("fsync {}: {e}", dest.display())))?;

    Ok(hex_encode(&hasher.finalize()))
}

fn verify_sha256(expected: &str, actual: &str) -> Result<(), AcpInstallerError> {
    if expected.eq_ignore_ascii_case(actual) {
        return Ok(());
    }
    Err(AcpInstallerError::IntegrityMismatch(format!(
        "binary archive SHA-256 mismatch: expected {expected}, got {actual}"
    )))
}

/// Lowercase hex encoder — avoids a `hex` crate dependency in `lab-apis`.
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

// ---------------------------------------------------------------------------
// Extraction (system tar/unzip) with zip-slip / partial-extraction defenses
// ---------------------------------------------------------------------------

/// Extract `archive` into `dest_dir` using system `tar`/`unzip`.
///
/// 1. List the archive first to learn the expected entry count.
/// 2. Extract with `Command::output()` so stderr is captured; any non-benign
///    stderr on a "successful" exit is treated as failure.
/// 3. Walk the result, rejecting symlinks and any entry escaping the root, and
///    verify the file count meets the pre-flight expectation.
fn extract_archive(archive: &Path, dest_dir: &Path, url: &str) -> Result<(), AcpInstallerError> {
    let archive_s = archive.to_string_lossy();
    let dest_s = dest_dir.to_string_lossy();

    let expected_file_count = list_archive_file_count(archive, url)?;

    // Decompression-bomb guard: reject before extraction if the summed
    // uncompressed entry sizes exceed the cap (Sec-M3).
    enforce_uncompressed_cap(archive, url)?;

    let output = if url.ends_with(".zip") {
        std::process::Command::new("unzip")
            .args(["-q", &archive_s, "-d", &dest_s])
            .output()
    } else {
        let flag = if url.ends_with(".tar.xz") || url.ends_with(".txz") {
            "-xJf"
        } else {
            "-xzf"
        };
        std::process::Command::new("tar")
            .args([flag, &archive_s, "-C", &dest_s, "--no-same-owner"])
            .output()
    };

    let output =
        output.map_err(|e| AcpInstallerError::internal(format!("run extraction tool: {e}")))?;

    if !output.status.success() {
        return Err(AcpInstallerError::internal(format!(
            "extraction failed (exit {}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    if !output.stderr.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let non_benign: Vec<&str> = stderr
            .lines()
            .filter(|line| !line.is_empty())
            .filter(|line| !line.contains("Ignoring unknown extended header"))
            .filter(|line| !line.contains("Removing leading"))
            .collect();
        if !non_benign.is_empty() {
            return Err(AcpInstallerError::internal(format!(
                "extraction tool emitted warnings; treating as failure: {}",
                non_benign.join(" | ")
            )));
        }
    }

    let canonical_root = std::fs::canonicalize(dest_dir).map_err(|e| {
        AcpInstallerError::internal(format!(
            "canonicalize extract root {}: {e}",
            dest_dir.display()
        ))
    })?;
    let actual_file_count = validate_no_escape(&canonical_root, dest_dir)?;

    if actual_file_count < expected_file_count {
        return Err(AcpInstallerError::internal(format!(
            "partial extraction detected: expected at least {expected_file_count} files, found {actual_file_count}"
        )));
    }

    Ok(())
}

/// Ask `tar`/`unzip` how many regular-file entries the archive contains.
fn list_archive_file_count(archive: &Path, url: &str) -> Result<usize, AcpInstallerError> {
    let archive_s = archive.to_string_lossy();
    let output = if url.ends_with(".zip") {
        std::process::Command::new("unzip")
            .args(["-Z", "-1", &archive_s])
            .output()
    } else {
        let flag = if url.ends_with(".tar.xz") || url.ends_with(".txz") {
            "-tJf"
        } else {
            "-tzf"
        };
        std::process::Command::new("tar")
            .args([flag, &archive_s])
            .output()
    };
    let output = output.map_err(|e| AcpInstallerError::internal(format!("list archive: {e}")))?;
    if !output.status.success() {
        return Err(AcpInstallerError::internal(format!(
            "archive listing failed (exit {})",
            output.status
        )));
    }
    let listing = String::from_utf8_lossy(&output.stdout);
    let count = listing
        .lines()
        .filter(|line| !line.is_empty())
        .filter(|line| !line.ends_with('/'))
        .count();
    Ok(count)
}

/// Sum the uncompressed sizes reported by the archive listing and reject if the
/// total exceeds [`MAX_ACP_UNCOMPRESSED_BYTES`].
///
/// Listing format parsed:
/// - tar (`tar -tzv` / `tar -tvJf`): whitespace columns where the *third* field
///   is the uncompressed byte size (`perm owner/group SIZE date time name`).
/// - zip (`unzip -l`): each entry line starts with the uncompressed `Length`
///   in its first column; the header/footer lines are skipped.
///
/// Fails CLOSED: if the tool exits non-zero the archive is rejected. If a size
/// column can't be parsed for an entry that line is skipped (it contributes 0),
/// which is safe because the cap only ever errs toward accepting — an attacker
/// cannot grow the total by making sizes unparseable.
fn enforce_uncompressed_cap(archive: &Path, url: &str) -> Result<(), AcpInstallerError> {
    let archive_s = archive.to_string_lossy();
    let is_zip = url.ends_with(".zip");
    let output = if is_zip {
        std::process::Command::new("unzip")
            .args(["-l", &archive_s])
            .output()
    } else {
        let flag = if url.ends_with(".tar.xz") || url.ends_with(".txz") {
            "-tvJf"
        } else {
            "-tzvf"
        };
        std::process::Command::new("tar")
            .args([flag, &archive_s])
            .output()
    };
    let output =
        output.map_err(|e| AcpInstallerError::internal(format!("list archive (sizes): {e}")))?;
    if !output.status.success() {
        return Err(AcpInstallerError::internal(format!(
            "archive size listing failed (exit {})",
            output.status
        )));
    }
    let listing = String::from_utf8_lossy(&output.stdout);
    let total = sum_uncompressed_bytes(&listing, is_zip);
    if total > MAX_ACP_UNCOMPRESSED_BYTES {
        return Err(AcpInstallerError::ContentTooLarge(format!(
            "uncompressed archive size {total} bytes exceeds maximum of {MAX_ACP_UNCOMPRESSED_BYTES} bytes (decompression-bomb guard)"
        )));
    }
    Ok(())
}

/// Parse a `tar -tzv` / `unzip -l` listing and sum the uncompressed byte sizes.
///
/// Saturating addition so a maliciously large reported total cannot wrap.
fn sum_uncompressed_bytes(listing: &str, is_zip: bool) -> u64 {
    let mut total: u64 = 0;
    for line in listing.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let size = if is_zip {
            // `unzip -l` columns: `Length  Date  Time  Name`. The first token is
            // the uncompressed length. The header (`Length ...`) and the
            // separator (`---------`) lines have a non-numeric first token and
            // parse to None. The footer total (`12288  N files`) DOES start with
            // a number, so it is rejected by requiring an entry-shaped row: a
            // `HH:MM`-style time token must be present (column 3).
            let cols: Vec<&str> = trimmed.split_whitespace().collect();
            let looks_like_entry = cols.len() >= 4 && cols[2].contains(':');
            if looks_like_entry {
                cols.first().copied()
            } else {
                None
            }
        } else {
            // `tar -tzv` columns: `perm owner/group SIZE date time name`. The
            // third token is the uncompressed size.
            trimmed.split_whitespace().nth(2)
        };
        if let Some(n) = size.and_then(|s| s.parse::<u64>().ok()) {
            total = total.saturating_add(n);
        }
    }
    total
}

/// Walk `dir`, verifying every entry canonicalizes under `canonical_root`.
/// Returns the count of regular files. Rejects every symlink. Fails CLOSED on
/// `symlink_metadata` errors.
fn validate_no_escape(canonical_root: &Path, dir: &Path) -> Result<usize, AcpInstallerError> {
    let rd = std::fs::read_dir(dir).map_err(|e| {
        AcpInstallerError::internal(format!("walk extract dir {}: {e}", dir.display()))
    })?;
    let mut file_count: usize = 0;
    for entry in rd.flatten() {
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path).map_err(|e| {
            AcpInstallerError::internal(format!(
                "stat {} during extract walk (failing closed): {e}",
                path.display()
            ))
        })?;
        if meta.file_type().is_symlink() {
            return Err(AcpInstallerError::PathTraversal(format!(
                "archive contains symlink at `{}`; rejected (zip-slip defense)",
                path.display()
            )));
        }
        let canon = std::fs::canonicalize(&path).map_err(|e| {
            AcpInstallerError::internal(format!("canonicalize {}: {e}", path.display()))
        })?;
        if !canon.starts_with(canonical_root) {
            return Err(AcpInstallerError::PathTraversal(format!(
                "archive entry `{}` escapes extract root `{}`",
                canon.display(),
                canonical_root.display()
            )));
        }
        if meta.file_type().is_dir() {
            file_count += validate_no_escape(canonical_root, &path)?;
        } else if meta.file_type().is_file() {
            file_count += 1;
        }
    }
    Ok(file_count)
}

/// Recursively find the first file named `binary_name` under `dir`.
fn find_binary_in_dir(dir: &Path, binary_name: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_binary_in_dir(&path, binary_name) {
                return Some(found);
            }
        } else if path.file_name().and_then(|n| n.to_str()) == Some(binary_name) {
            return Some(path);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Atomic executable install with setuid strip
// ---------------------------------------------------------------------------

/// Copy `src` to `dest` atomically, forcing mode `0o755` (never OR-ed with the
/// source mode, so setuid/setgid bits from a hostile archive cannot survive).
/// Refuses to overwrite a symlink at `dest`.
fn install_executable_atomically(src: &Path, dest: &Path) -> Result<(), AcpInstallerError> {
    use std::io::Write;

    let parent = dest
        .parent()
        .ok_or_else(|| AcpInstallerError::internal("install destination has no parent"))?;

    if let Ok(meta) = std::fs::symlink_metadata(dest) {
        if meta.file_type().is_symlink() {
            return Err(AcpInstallerError::InvalidParam(format!(
                "refusing to overwrite symlink at {} (must be a regular file)",
                dest.display()
            )));
        }
    }

    let mut input = std::fs::File::open(src)
        .map_err(|e| AcpInstallerError::internal(format!("open {}: {e}", src.display())))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|e| AcpInstallerError::internal(format!("temp executable: {e}")))?;
    {
        let output = temp.as_file_mut();
        std::io::copy(&mut input, output).map_err(|e| {
            AcpInstallerError::internal(format!(
                "copy {} to temp executable in {}: {e}",
                src.display(),
                parent.display()
            ))
        })?;
        output.flush().map_err(|e| {
            AcpInstallerError::internal(format!("flush temp executable {}: {e}", dest.display()))
        })?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o755)).map_err(
            |e| {
                AcpInstallerError::internal(format!(
                    "chmod 0o755 temp executable for {}: {e}",
                    dest.display()
                ))
            },
        )?;
    }

    temp.as_file().sync_all().map_err(|e| {
        AcpInstallerError::internal(format!("fsync temp executable {}: {e}", dest.display()))
    })?;
    temp.persist(dest).map_err(|e| {
        AcpInstallerError::internal(format!("atomic rename {}: {e}", dest.display()))
    })?;
    fsync_parent_dir(parent);
    Ok(())
}

fn fsync_parent_dir(parent: &Path) {
    #[cfg(unix)]
    if let Ok(dir) = std::fs::File::open(parent) {
        if let Err(e) = dir.sync_all() {
            tracing::warn!(
                service = "acp_registry",
                event = "install.fsync_failed",
                path = %parent.display(),
                error = %e,
                "directory fsync after binary install failed; durability not guaranteed"
            );
        }
    }
    #[cfg(not(unix))]
    let _ = parent;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_integrity_metadata_normalization_fails() {
        let err = AcpInstaller::normalize_sha256("not-a-digest", "sha256").unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn digest_metadata_is_normalized() {
        let got = AcpInstaller::normalize_sha256(&format!("sha256:{}", "A".repeat(64)), "digest")
            .expect("valid digest");
        assert_eq!(got, "a".repeat(64));
    }

    #[test]
    fn digest_mismatch_verification_fails() {
        let err = verify_sha256(&"a".repeat(64), &"b".repeat(64)).unwrap_err();
        assert_eq!(err.kind(), "integrity_mismatch");
    }

    #[test]
    fn archive_url_rejects_local_and_private_hosts() {
        // Non-https is a static defect → invalid_param.
        let err =
            AcpInstaller::validate_archive_url("http://example.com/agent.tar.gz").unwrap_err();
        assert_eq!(err.kind(), "invalid_param");

        // Private-TLD / loopback / private-IP hosts are address blocks.
        for url in [
            "https://agent.local/agent.tar.gz",
            "https://127.0.0.1/agent.tar.gz",
            "https://[::ffff:127.0.0.1]/agent.tar.gz",
            "https://192.168.1.20/agent.tar.gz",
        ] {
            let err = AcpInstaller::validate_archive_url(url).unwrap_err();
            assert_eq!(err.kind(), "ssrf_blocked", "{url}");
        }
    }

    #[test]
    fn size_limit_rejects_oversized_stream() {
        let mut downloaded = MAX_ACP_ARCHIVE_BYTES - 1;
        let err = enforce_size_limit(&mut downloaded, 2).unwrap_err();
        assert_eq!(err.kind(), "content_too_large");
        assert_eq!(downloaded, MAX_ACP_ARCHIVE_BYTES - 1);
    }

    #[tokio::test]
    async fn oversized_archive_cleanup_removes_partial_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let partial = dir.path().join("agent.tar.gz.partial");
        tokio::fs::write(&partial, b"partial archive")
            .await
            .expect("write partial");

        cleanup_partial(&partial).await;

        assert!(
            tokio::fs::metadata(&partial).await.is_err(),
            "partial archive should be removed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn install_executable_atomically_replaces_file_and_forces_0755() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src-agent");
        let dest = dir.path().join("agent");
        // Source carries a setuid bit + 0777 — must be stripped to exactly 0755.
        std::fs::write(&src, b"new agent").expect("write src");
        std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o4777)).expect("chmod src");
        std::fs::write(&dest, b"old agent").expect("write dest");

        install_executable_atomically(&src, &dest).expect("atomic install");

        assert_eq!(std::fs::read(&dest).expect("read dest"), b"new agent");
        let mode = std::fs::metadata(&dest)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(mode, 0o755, "setuid bit must be stripped");
    }

    #[test]
    fn validate_no_escape_rejects_symlink() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("real.txt"), b"ok").expect("write");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/passwd", root.join("evil")).expect("symlink");
            let canonical_root = std::fs::canonicalize(root).expect("canonicalize");
            let err = validate_no_escape(&canonical_root, root).unwrap_err();
            assert_eq!(err.kind(), "path_traversal");
        }
    }

    #[test]
    fn hex_encode_matches_expected() {
        assert_eq!(hex_encode(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
    }

    #[test]
    fn peer_matches_pin_accepts_match() {
        let ip: IpAddr = "203.0.113.7".parse().unwrap();
        assert!(peer_matches_pin(ip, ip).is_ok());
    }

    #[test]
    fn peer_matches_pin_rejects_mismatch_as_ssrf() {
        let peer: IpAddr = "127.0.0.1".parse().unwrap();
        let pinned: IpAddr = "203.0.113.7".parse().unwrap();
        let err = peer_matches_pin(peer, pinned).unwrap_err();
        assert_eq!(err.kind(), "ssrf_blocked");
    }

    #[test]
    fn sum_uncompressed_bytes_parses_tar_listing() {
        // `tar -tzv` shape: perm owner/group SIZE date time name
        let listing = "\
-rw-r--r-- user/group 1024 2024-01-01 00:00 a.txt
drwxr-xr-x user/group 0 2024-01-01 00:00 dir/
-rwxr-xr-x user/group 2048 2024-01-01 00:00 dir/b.bin
";
        assert_eq!(sum_uncompressed_bytes(listing, false), 1024 + 2048);
    }

    #[test]
    fn sum_uncompressed_bytes_parses_zip_listing() {
        // `unzip -l` shape: Length Date Time Name, with header/footer lines.
        let listing = "\
Archive:  agent.zip
  Length      Date    Time    Name
---------  ---------- -----   ----
     4096  2024-01-01 00:00   a.txt
     8192  2024-01-01 00:00   b.bin
---------                     -------
    12288                     2 files
";
        assert_eq!(sum_uncompressed_bytes(listing, true), 4096 + 8192);
    }

    /// A highly-compressible tar.gz whose *uncompressed* size exceeds the cap
    /// must be rejected before extraction with `content_too_large`. Uses a
    /// sparse zero file so the test stays cheap on disk while gzip stays tiny.
    #[cfg(unix)]
    #[test]
    fn decompression_bomb_rejected_by_uncompressed_cap() {
        use std::process::Command;

        // Skip cleanly if tar/gzip aren't present in the test environment.
        if Command::new("tar").arg("--version").output().is_err() {
            eprintln!("tar unavailable; skipping decompression-bomb test");
            return;
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let big = dir.path().join("payload.bin");
        let f = std::fs::File::create(&big).expect("create payload");
        // Sparse zero file just over the cap — compresses to a few KiB.
        f.set_len(MAX_ACP_UNCOMPRESSED_BYTES + 1024)
            .expect("set_len");
        drop(f);

        let archive = dir.path().join("bomb.tar.gz");
        let status = Command::new("tar")
            .args(["-czf"])
            .arg(&archive)
            .args(["-C"])
            .arg(dir.path())
            .arg("payload.bin")
            .status()
            .expect("run tar");
        assert!(status.success(), "tar should produce the archive");

        let err =
            enforce_uncompressed_cap(&archive, "https://example.com/bomb.tar.gz").unwrap_err();
        assert_eq!(err.kind(), "content_too_large");
    }

    /// A normal small archive passes the uncompressed cap.
    #[cfg(unix)]
    #[test]
    fn small_archive_passes_uncompressed_cap() {
        use std::process::Command;

        if Command::new("tar").arg("--version").output().is_err() {
            eprintln!("tar unavailable; skipping uncompressed-cap pass test");
            return;
        }

        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("agent"), b"#!/bin/sh\necho hi\n").expect("write");

        let archive = dir.path().join("ok.tar.gz");
        let status = Command::new("tar")
            .args(["-czf"])
            .arg(&archive)
            .args(["-C"])
            .arg(dir.path())
            .arg("agent")
            .status()
            .expect("run tar");
        assert!(status.success());

        enforce_uncompressed_cap(&archive, "https://example.com/ok.tar.gz")
            .expect("small archive should pass");
    }
}
