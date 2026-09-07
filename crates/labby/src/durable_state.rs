//! Offline, lifecycle-locked disaster-recovery bundles for one installation.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::installation::{InstallationLifecycleLock, InstallationPaths};

const MANIFEST_VERSION: u32 = 1;
const RECOVERY_KEY_ENV: &str = "LABBY_RECOVERY_KEY_PATH";
type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableStateManifest {
    pub manifest_version: u32,
    pub labby_version: String,
    pub installation_root: PathBuf,
    pub entries: Vec<DurableStateEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableStateEntry {
    pub source: PathBuf,
    pub payload: PathBuf,
    pub size: u64,
    pub sha256: String,
    pub mode: u32,
}

#[derive(Debug)]
pub struct DurableStateRestore {
    pub manifest: DurableStateManifest,
    pub maintenance_warning: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RestoreJournal {
    committed: bool,
    entries: Vec<RestoreJournalEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RestoreJournalEntry {
    target: PathBuf,
    prior: Option<PathBuf>,
    staged: Option<PathBuf>,
    phase: RestorePhase,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RestorePhase {
    Planned,
    PriorMoved,
    Activated,
}

pub fn export_bundle(destination: &Path) -> Result<DurableStateManifest> {
    ensure_supported_platform()?;
    let paths = InstallationPaths::resolve()?;
    ensure_bundle_outside_installation_root(destination, &paths)?;
    let _lock = InstallationLifecycleLock::acquire_offline(&paths)?;
    recover_interrupted_restore(&paths)?;
    ensure!(!destination.exists(), "backup destination already exists");
    private_dir(destination)?;
    private_dir(&destination.join("payload"))?;
    let result = export_locked(&paths, destination);
    let result = result.and_then(|manifest| {
        write_bundle_authentication(destination, &paths)?;
        Ok(manifest)
    });
    if result.is_err() {
        drop(fs::remove_dir_all(destination));
    }
    result
}

fn export_locked(paths: &InstallationPaths, destination: &Path) -> Result<DurableStateManifest> {
    let mut sources = Vec::new();
    collect_files(paths.root(), paths.root(), &mut sources)?;
    for external in configured_external_files(paths)? {
        if external.exists() && !external.starts_with(paths.root()) {
            validate_regular_source(&external)?;
            sources.push(external);
        }
    }
    sources.sort();
    sources.dedup();
    let mut entries = Vec::new();
    for (index, source) in sources.into_iter().enumerate() {
        if source == paths.lifecycle_lock() {
            continue;
        }
        let payload = PathBuf::from("payload").join(format!("{index:08}"));
        let target = destination.join(&payload);
        let (size, sha256, mode) = secure_copy_and_digest(&source, &target)?;
        entries.push(DurableStateEntry {
            source,
            payload,
            size,
            sha256,
            mode,
        });
    }
    let manifest = DurableStateManifest {
        manifest_version: MANIFEST_VERSION,
        labby_version: env!("CARGO_PKG_VERSION").to_owned(),
        installation_root: paths.root().to_path_buf(),
        entries,
    };
    write_private(
        &destination.join("manifest.json"),
        &serde_json::to_vec_pretty(&manifest)?,
    )?;
    verify_bundle_locked(destination, Some(paths.root()))?;
    Ok(manifest)
}

pub fn verify_bundle(bundle: &Path) -> Result<DurableStateManifest> {
    ensure_supported_platform()?;
    let paths = InstallationPaths::resolve()?;
    ensure_bundle_outside_installation_root(bundle, &paths)?;
    let _lock = InstallationLifecycleLock::acquire_offline(&paths)?;
    recover_interrupted_restore(&paths)?;
    verify_bundle_authentication(bundle, &paths)?;
    verify_bundle_locked(bundle, None)
}

fn verify_bundle_locked(
    bundle: &Path,
    expected_root: Option<&Path>,
) -> Result<DurableStateManifest> {
    reject_symlink_chain(bundle)?;
    validate_secure_metadata(bundle, &fs::symlink_metadata(bundle)?)?;
    validate_regular_source(&bundle.join("manifest.json"))?;
    let canonical_payload_root = fs::canonicalize(bundle.join("payload"))?;
    let raw = fs::read(bundle.join("manifest.json")).context("read durable-state manifest")?;
    let manifest: DurableStateManifest =
        serde_json::from_slice(&raw).context("parse durable-state manifest")?;
    ensure!(
        manifest.manifest_version == MANIFEST_VERSION,
        "unsupported durable-state manifest version {}",
        manifest.manifest_version
    );
    ensure_version_compatible(&manifest.labby_version)?;
    if let Some(root) = expected_root {
        ensure!(
            manifest.installation_root == root,
            "manifest installation root changed during export"
        );
    }
    let mut seen = BTreeSet::new();
    let mut seen_payloads = BTreeSet::new();
    for entry in &manifest.entries {
        ensure!(
            entry.source.is_absolute(),
            "manifest source is not absolute"
        );
        ensure!(
            safe_relative(&entry.payload),
            "unsafe manifest payload path"
        );
        ensure!(
            seen.insert(entry.source.clone()),
            "duplicate manifest source"
        );
        ensure!(
            seen_payloads.insert(entry.payload.clone()),
            "duplicate manifest payload"
        );
        ensure!(
            entry.mode <= 0o777 && entry.mode & 0o022 == 0,
            "insecure manifest mode"
        );
        let payload = bundle.join(&entry.payload);
        validate_regular_source(&payload)?;
        ensure!(
            fs::canonicalize(&payload)?.starts_with(&canonical_payload_root),
            "payload escapes the bundle"
        );
        let (size, digest) = digest_file(&payload)?;
        ensure!(
            size == entry.size && digest == entry.sha256,
            "backup payload integrity mismatch for {}",
            entry.source.display()
        );
    }
    let mut actual_payloads = Vec::new();
    collect_files(bundle, &bundle.join("payload"), &mut actual_payloads)?;
    let actual_payloads: BTreeSet<PathBuf> = actual_payloads
        .into_iter()
        .map(|path| path.strip_prefix(bundle).map(Path::to_path_buf))
        .collect::<std::result::Result<_, _>>()?;
    ensure!(
        actual_payloads == seen_payloads,
        "bundle has missing or extra payload files"
    );
    Ok(manifest)
}

pub fn restore_bundle(bundle: &Path) -> Result<DurableStateRestore> {
    ensure_supported_platform()?;
    let paths = InstallationPaths::resolve()?;
    ensure_bundle_outside_installation_root(bundle, &paths)?;
    let _lock = InstallationLifecycleLock::acquire_offline(&paths)?;
    recover_interrupted_restore(&paths)?;
    verify_bundle_authentication(bundle, &paths)?;
    restore_bundle_locked(&paths, bundle)
}

fn recovery_key(paths: &InstallationPaths, bundle: &Path) -> Result<Vec<u8>> {
    let path = std::env::var_os(RECOVERY_KEY_ENV)
        .map(PathBuf::from)
        .context(
            "LABBY_RECOVERY_KEY_PATH must name the separately stored recovery authentication key",
        )?;
    ensure!(
        path.is_absolute(),
        "recovery authentication key path must be absolute"
    );
    ensure!(
        !path.starts_with(paths.root()) && !path.starts_with(bundle),
        "recovery authentication key must be stored outside Labby state and the backup bundle"
    );
    validate_regular_source(&path)?;
    validate_recovery_key_metadata(&path)?;
    let key = fs::read(&path).context("read recovery authentication key")?;
    ensure!(
        key.len() >= 32,
        "recovery authentication key must contain at least 32 bytes"
    );
    Ok(key)
}

fn write_bundle_authentication(bundle: &Path, paths: &InstallationPaths) -> Result<()> {
    let key = recovery_key(paths, bundle)?;
    let manifest = fs::read(bundle.join("manifest.json"))?;
    let tag = authentication_tag(&key, &manifest)?;
    write_private(&bundle.join("manifest.hmac"), hex::encode(tag).as_bytes())?;
    sync_parent(&bundle.join("manifest.hmac"))
}

fn verify_bundle_authentication(bundle: &Path, paths: &InstallationPaths) -> Result<()> {
    let key = recovery_key(paths, bundle)?;
    validate_regular_source(&bundle.join("manifest.hmac"))?;
    let signature = fs::read_to_string(bundle.join("manifest.hmac"))?;
    let signature = hex::decode(signature.trim()).context("invalid recovery authentication tag")?;
    let manifest = fs::read(bundle.join("manifest.json"))?;
    verify_authentication(&key, &manifest, &signature)
}

fn authentication_tag(key: &[u8], manifest: &[u8]) -> Result<Vec<u8>> {
    let mut mac = HmacSha256::new_from_slice(&key).context("initialize recovery authenticator")?;
    mac.update(&manifest);
    Ok(mac.finalize().into_bytes().to_vec())
}

fn verify_authentication(key: &[u8], manifest: &[u8], signature: &[u8]) -> Result<()> {
    let mut mac = HmacSha256::new_from_slice(key).context("initialize recovery authenticator")?;
    mac.update(manifest);
    mac.verify_slice(&signature)
        .context("recovery authentication failed: wrong key or modified manifest")
}

fn restore_bundle_locked(paths: &InstallationPaths, bundle: &Path) -> Result<DurableStateRestore> {
    restore_bundle_locked_with_cleanup(paths, bundle, &mut |path| fs::remove_file(path))
}

fn restore_bundle_locked_with_cleanup(
    paths: &InstallationPaths,
    bundle: &Path,
    cleanup_file: &mut dyn FnMut(&Path) -> std::io::Result<()>,
) -> Result<DurableStateRestore> {
    restore_bundle_locked_with_hooks(paths, bundle, cleanup_file, &mut |_| Ok(()))
}

fn restore_bundle_locked_with_hooks(
    paths: &InstallationPaths,
    bundle: &Path,
    cleanup_file: &mut dyn FnMut(&Path) -> std::io::Result<()>,
    after_prior_moved: &mut dyn FnMut(&Path) -> Result<()>,
) -> Result<DurableStateRestore> {
    let manifest = verify_bundle_locked(bundle, None)?;
    ensure!(
        manifest.installation_root == paths.root(),
        "backup targets a different installation root"
    );
    let external = configured_external_files(paths)?;
    let destinations = trusted_restore_destinations(paths, &manifest, &external)?;
    let transaction_id = format!("{}-{}", std::process::id(), ulid::Ulid::new());
    let journal_path = paths.root().join("restore.journal.json");
    let mut journal = RestoreJournal {
        committed: false,
        entries: Vec::new(),
    };
    persist_journal(&journal_path, &journal)?;
    let mut installed: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    let result = (|| -> Result<()> {
        let expected: BTreeSet<_> = destinations.iter().cloned().collect();
        let mut current = Vec::new();
        collect_files(paths.root(), paths.root(), &mut current)?;
        // External SQLite sidecars are part of the same snapshot as the database.
        // Keeping a newer WAL beside a restored database can replay post-backup data.
        for path in &external {
            if !path.starts_with(paths.root()) && path.exists() {
                validate_regular_source(path)?;
                current.push(path.clone());
            }
        }
        current.sort();
        current.dedup();
        for extra in current.into_iter().filter(|path| !expected.contains(path)) {
            let name = extra
                .file_name()
                .context("state path has no name")?
                .to_string_lossy();
            let prior = extra
                .parent()
                .context("state path has no parent")?
                .join(format!(".{name}.labby-rollback-{transaction_id}"));
            journal.entries.push(RestoreJournalEntry {
                target: extra.clone(),
                prior: Some(prior.clone()),
                staged: None,
                phase: RestorePhase::Planned,
            });
            persist_journal(&journal_path, &journal)?;
            fs::rename(&extra, &prior)?;
            installed.push((extra.clone(), Some(prior)));
            after_prior_moved(&extra)?;
            sync_parent(&extra)?;
            journal.entries.last_mut().unwrap().phase = RestorePhase::PriorMoved;
            persist_journal(&journal_path, &journal)?;
        }
        for (entry, destination) in manifest.entries.iter().zip(&destinations) {
            reject_symlink_chain(destination)?;
            let parent = destination.parent().context("state path has no parent")?;
            private_dir(parent)?;
            let name = entry
                .source
                .file_name()
                .context("state path has no name")?
                .to_string_lossy();
            let staged = parent.join(format!(".{name}.labby-restore-{transaction_id}"));
            let prior = parent.join(format!(".{name}.labby-rollback-{transaction_id}"));
            let backup = destination.exists().then_some(prior.clone());
            journal.entries.push(RestoreJournalEntry {
                target: destination.clone(),
                prior: backup.clone(),
                staged: Some(staged.clone()),
                phase: RestorePhase::Planned,
            });
            persist_journal(&journal_path, &journal)?;
            secure_copy_and_digest(&bundle.join(&entry.payload), &staged)?;
            if destination.exists() {
                validate_regular_source(destination)?;
                fs::rename(destination, &prior)
                    .with_context(|| format!("stage rollback for {}", destination.display()))?;
                installed.push((destination.clone(), Some(prior)));
                after_prior_moved(destination)?;
                sync_parent(destination)?;
            } else {
                installed.push((destination.clone(), None));
            }
            journal.entries.last_mut().unwrap().phase = RestorePhase::PriorMoved;
            persist_journal(&journal_path, &journal)?;
            fs::rename(&staged, destination)
                .with_context(|| format!("atomically restore {}", destination.display()))?;
            set_mode(destination, entry.mode)?;
            sync_parent(destination)?;
            journal.entries.last_mut().unwrap().phase = RestorePhase::Activated;
            persist_journal(&journal_path, &journal)?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        let mut rollback_errors = Vec::new();
        for (target, prior) in installed.into_iter().rev() {
            if let Err(rollback) = fs::remove_file(&target) {
                if rollback.kind() != std::io::ErrorKind::NotFound {
                    rollback_errors.push(format!("remove {}: {rollback}", target.display()));
                }
            }
            if let Some(prior) = prior {
                if let Err(rollback) = fs::rename(&prior, &target) {
                    rollback_errors.push(format!("restore {}: {rollback}", target.display()));
                } else if let Err(rollback) = sync_parent(&target) {
                    rollback_errors.push(format!("sync {}: {rollback}", target.display()));
                }
            }
        }
        for staged in journal
            .entries
            .iter()
            .filter_map(|entry| entry.staged.as_ref())
        {
            if let Err(cleanup) = cleanup_file(staged) {
                if cleanup.kind() != std::io::ErrorKind::NotFound {
                    rollback_errors.push(format!("remove staged {}: {cleanup}", staged.display()));
                }
            } else if let Err(cleanup) = sync_parent(staged) {
                rollback_errors.push(format!("sync staged {}: {cleanup}", staged.display()));
            }
        }
        if rollback_errors.is_empty() {
            fs::remove_file(&journal_path)?;
            sync_parent(&journal_path)?;
            return Err(error.context("restore failed; prior files rolled back"));
        }
        bail!(
            "restore failed: {error:#}; rollback was incomplete: {}",
            rollback_errors.join("; ")
        );
    }
    journal.committed = true;
    persist_journal(&journal_path, &journal)?;
    let mut cleanup_failure_count = 0_usize;
    for (_, prior) in installed {
        if let Some(prior) = prior {
            if cleanup_file(&prior).is_err() || sync_parent(&prior).is_err() {
                cleanup_failure_count += 1;
            }
        }
    }
    if cleanup_file(&journal_path).is_err() || sync_parent(&journal_path).is_err() {
        cleanup_failure_count += 1;
    }
    Ok(DurableStateRestore {
        manifest,
        maintenance_warning: (cleanup_failure_count > 0).then(|| {
            format!(
                "restore committed, but {cleanup_failure_count} cleanup operation(s) failed; inspect the installation root for retained rollback or journal artifacts"
            )
        }),
    })
}

fn persist_journal(path: &Path, journal: &RestoreJournal) -> Result<()> {
    let parent = path.parent().context("restore journal has no parent")?;
    private_dir(parent)?;
    let temporary = parent.join(format!(".restore-journal-{}.tmp", ulid::Ulid::new()));
    write_private(&temporary, &serde_json::to_vec(journal)?)?;
    fs::rename(&temporary, path)?;
    sync_parent(path)
}

fn recover_interrupted_restore(paths: &InstallationPaths) -> Result<()> {
    let journal_path = paths.root().join("restore.journal.json");
    if !journal_path.exists() {
        return Ok(());
    }
    validate_regular_source(&journal_path)?;
    let journal: RestoreJournal = serde_json::from_slice(&fs::read(&journal_path)?)?;
    let committed = journal.committed;
    let external: BTreeSet<_> = configured_external_files(paths)?.into_iter().collect();
    let mut errors = Vec::new();
    for entry in journal.entries.into_iter().rev() {
        ensure!(
            entry.target.starts_with(paths.root()) || external.contains(&entry.target),
            "restore journal contains an untrusted destination"
        );
        if entry.target.starts_with(paths.root()) {
            reject_symlinks_below(paths.root(), &entry.target)?;
        } else {
            reject_symlink_chain(&entry.target)?;
        }
        let parent = entry
            .target
            .parent()
            .context("journal target has no parent")?;
        for artifact in entry.prior.iter().chain(entry.staged.iter()) {
            ensure!(
                artifact.parent() == Some(parent),
                "restore journal artifact escaped destination parent"
            );
            reject_symlink_chain(artifact)?;
        }
        let uncommitted_new_target_activation = entry.phase == RestorePhase::PriorMoved
            && entry.prior.is_none()
            && entry.target.exists();
        // In-process rollback may already have moved the prior back before a
        // later cleanup/sync failure retained this journal. Without a remaining
        // prior, an existing target can be the only surviving original copy.
        let can_replace_target = entry.prior.as_ref().is_none_or(|prior| prior.exists());
        if !committed
            && (entry.phase == RestorePhase::Activated || uncommitted_new_target_activation)
            && can_replace_target
            && entry.target.exists()
        {
            if let Err(error) = fs::remove_file(&entry.target) {
                errors.push(format!("remove {}: {error}", entry.target.display()));
            }
        }
        if let Some(prior) = entry.prior {
            if prior.exists() {
                let result = if committed {
                    fs::remove_file(&prior)
                } else if entry.phase != RestorePhase::Planned || !entry.target.exists() {
                    fs::rename(&prior, &entry.target)
                } else {
                    Ok(())
                };
                if let Err(error) = result {
                    errors.push(format!("reconcile {}: {error}", entry.target.display()));
                }
            }
        }
        if let Some(staged) = entry.staged {
            if staged.exists() {
                if let Err(error) = fs::remove_file(&staged) {
                    errors.push(format!("remove {}: {error}", staged.display()));
                }
            }
        }
        if let Err(error) = sync_parent(&entry.target) {
            errors.push(format!("sync {}: {error}", entry.target.display()));
        }
    }
    ensure!(
        errors.is_empty(),
        "interrupted restore recovery incomplete: {}",
        errors.join("; ")
    );
    fs::remove_file(&journal_path)?;
    sync_parent(&journal_path)
}

fn configured_external_files(paths: &InstallationPaths) -> Result<Vec<PathBuf>> {
    let mut values = std::collections::HashMap::new();
    if paths.dotenv().is_file() {
        for item in dotenvy::from_path_iter(paths.dotenv())? {
            let (key, value) = item?;
            if matches!(
                key.as_str(),
                "LABBY_AUTH_SQLITE_PATH" | "LABBY_AUTH_KEY_PATH"
            ) {
                values.insert(key, value);
            }
        }
    }
    for key in ["LABBY_AUTH_SQLITE_PATH", "LABBY_AUTH_KEY_PATH"] {
        if let Ok(value) = std::env::var(key) {
            values.insert(key.to_owned(), value);
        }
    }
    if paths.config_toml().is_file() {
        let config: crate::config::LabConfig = crate::config::load_toml(&[paths.config_toml()])?;
        if config.file_stash.root.is_some() {
            let stash_root = crate::config::file_stash_root_path(&config)?;
            ensure!(
                stash_root.starts_with(paths.root()),
                "backup of an externally configured File Stash root is unsupported; move file_stash.root under the Labby installation root before exporting"
            );
        }
        if let Some(auth) = config.auth {
            if let Some(path) = auth.sqlite_path {
                values
                    .entry("LABBY_AUTH_SQLITE_PATH".into())
                    .or_insert_with(|| path.display().to_string());
            }
            if let Some(path) = auth.key_path {
                values
                    .entry("LABBY_AUTH_KEY_PATH".into())
                    .or_insert_with(|| path.display().to_string());
            }
        }
    }
    let sqlite_path = values.get("LABBY_AUTH_SQLITE_PATH").map(PathBuf::from);
    let mut files: Vec<PathBuf> = values
        .into_values()
        .map(PathBuf::from)
        .map(|path| {
            ensure!(
                path.is_absolute(),
                "configured external auth/key path must be absolute: {}",
                path.display()
            );
            canonicalize_external_allow_missing(&path)
        })
        .collect::<Result<_>>()?;
    if let Some(database) = sqlite_path {
        for suffix in ["-wal", "-shm"] {
            let sidecar = PathBuf::from(format!("{}{suffix}", database.display()));
            files.push(canonicalize_external_allow_missing(&sidecar)?);
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn ensure_bundle_outside_installation_root(bundle: &Path, paths: &InstallationPaths) -> Result<()> {
    reject_symlink_chain(bundle)?;
    let root = fs::canonicalize(paths.root()).context("canonicalize installation root")?;
    let absolute = if bundle.is_absolute() {
        bundle.to_path_buf()
    } else {
        std::env::current_dir()?.join(bundle)
    };
    let mut cursor = absolute.as_path();
    let mut missing = Vec::new();
    while !cursor.exists() {
        missing.push(cursor.file_name().context("bundle path has no file name")?);
        cursor = cursor
            .parent()
            .context("bundle path has no existing ancestor")?;
    }
    let mut resolved = fs::canonicalize(cursor).context("canonicalize bundle ancestor")?;
    for component in missing.into_iter().rev() {
        resolved.push(component);
    }
    ensure!(
        !resolved.starts_with(&root),
        "backup bundle must be stored outside the Labby installation root"
    );
    Ok(())
}

fn canonicalize_external_allow_missing(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return Ok(fs::canonicalize(path)?);
    }
    let parent = path.parent().context("external state path has no parent")?;
    let name = path
        .file_name()
        .context("external state path has no name")?;
    Ok(fs::canonicalize(parent)?.join(name))
}

fn trusted_restore_destinations(
    paths: &InstallationPaths,
    manifest: &DurableStateManifest,
    external: &[PathBuf],
) -> Result<Vec<PathBuf>> {
    let allowed_external: BTreeSet<_> = external.iter().cloned().collect();
    let mut destinations = Vec::with_capacity(manifest.entries.len());
    for entry in &manifest.entries {
        let destination =
            if let Ok(relative) = entry.source.strip_prefix(&manifest.installation_root) {
                ensure!(
                    safe_relative(relative),
                    "unsafe installation-relative restore path"
                );
                paths.root().join(relative)
            } else {
                ensure!(
                    allowed_external.contains(&entry.source),
                    "manifest targets an unconfigured external state path: {}",
                    entry.source.display()
                );
                entry.source.clone()
            };
        if destination.starts_with(paths.root()) {
            reject_symlinks_below(paths.root(), &destination)?;
        }
        destinations.push(destination);
    }
    let mut sorted = destinations.clone();
    sorted.sort();
    for pair in sorted.windows(2) {
        ensure!(
            pair[0] != pair[1] && !pair[1].starts_with(&pair[0]),
            "restore destination collision"
        );
    }
    Ok(destinations)
}

fn collect_files(root: &Path, path: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(path).with_context(|| format!("read {}", path.display()))? {
        let path = entry?.path();
        let metadata = fs::symlink_metadata(&path)?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "durable state contains symlink: {}",
            path.display()
        );
        if metadata.is_dir() {
            validate_secure_metadata(&path, &metadata)?;
            collect_files(root, &path, out)?;
        } else {
            validate_regular_source(&path)?;
            let relative = path.strip_prefix(root)?;
            if relative != Path::new("lifecycle.lock")
                && relative != Path::new("restore.journal.json")
            {
                out.push(path);
            }
        }
    }
    Ok(())
}

fn validate_regular_source(path: &Path) -> Result<()> {
    reject_symlink_chain(path)?;
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "not a regular file: {}",
        path.display()
    );
    validate_secure_metadata(path, &metadata)
}
fn validate_secure_metadata(path: &Path, metadata: &fs::Metadata) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        ensure!(
            metadata.mode() & 0o022 == 0,
            "insecure group/world-writable state: {}",
            path.display()
        );
        ensure!(
            !metadata.is_file() || metadata.nlink() == 1,
            "hard-linked durable state rejected: {}",
            path.display()
        );
    }
    #[cfg(not(unix))]
    let _ = (path, metadata);
    Ok(())
}

fn validate_recovery_key_metadata(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let metadata = fs::symlink_metadata(path)?;
        ensure!(
            metadata.uid() == nix::unistd::Uid::effective().as_raw()
                && metadata.mode().trailing_zeros() >= 6,
            "recovery authentication key must be an owner-only file owned by the current user: {}",
            path.display()
        );
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn reject_symlink_chain(path: &Path) -> Result<()> {
    let mut components = path.components();
    let mut current = if path.is_absolute() {
        let root = components.next().context("absolute path has no root")?;
        let first = components
            .next()
            .context("path must extend beyond the platform root")?;
        let mut platform_prefix = PathBuf::from(root.as_os_str());
        platform_prefix.push(first.as_os_str());
        fs::canonicalize(platform_prefix)?
    } else {
        fs::canonicalize(".")?
    };
    for component in components {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                bail!("symlink path rejected: {}", current.display());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn reject_symlinks_below(anchor: &Path, path: &Path) -> Result<()> {
    let relative = path
        .strip_prefix(anchor)
        .context("path is outside trusted anchor")?;
    let mut current = anchor.to_path_buf();
    reject_symlink_chain(&current)?;
    for component in relative.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                bail!("symlink path rejected: {}", current.display());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
fn safe_relative(path: &Path) -> bool {
    !path.is_absolute() && path.components().all(|c| matches!(c, Component::Normal(_)))
}
fn digest_file(path: &Path) -> Result<(u64, String)> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 65_536];
    let mut size = 0;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((size, hex::encode(hash.finalize())))
}
fn secure_copy_and_digest(source: &Path, target: &Path) -> Result<(u64, String, u32)> {
    validate_regular_source(source)?;
    if let Some(parent) = target.parent() {
        private_dir(parent)?;
    }
    let mut input = fs::File::open(source)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut output = options.open(target)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    let mut size = 0;
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        output.write_all(&buffer[..n])?;
        hash.update(&buffer[..n]);
        size += n as u64;
    }
    output.sync_all()?;
    let mode = file_mode(source)?;
    Ok((size, hex::encode(hash.finalize()), mode))
}
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
fn private_dir(path: &Path) -> Result<()> {
    reject_symlink_chain(path)?;
    if path.exists() {
        let metadata = fs::symlink_metadata(path)?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "unsafe directory: {}",
            path.display()
        );
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(path)?;
    sync_parent(path)?;
    Ok(())
}
#[cfg(unix)]
fn file_mode(path: &Path) -> Result<u32> {
    use std::os::unix::fs::MetadataExt as _;
    Ok(fs::metadata(path)?.mode() & 0o777)
}
#[cfg(not(unix))]
fn file_mode(_: &Path) -> Result<u32> {
    Ok(0)
}
#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o777))?;
    Ok(())
}
#[cfg(not(unix))]
fn set_mode(_: &Path, _: u32) -> Result<()> {
    Ok(())
}
fn ensure_version_compatible(producer: &str) -> Result<()> {
    let parse = |version: &str| -> Result<(u64, u64, u64)> {
        let core = version.split_once('-').map_or(version, |(core, _)| core);
        let mut parts = core.split('.');
        let major = parts.next().context("missing major version")?.parse()?;
        let minor = parts.next().context("missing minor version")?.parse()?;
        let patch = parts.next().context("missing patch version")?.parse()?;
        ensure!(parts.next().is_none(), "invalid Labby version");
        Ok((major, minor, patch))
    };
    let producer = parse(producer).context("invalid backup Labby version")?;
    let current = parse(env!("CARGO_PKG_VERSION"))?;
    ensure!(
        producer.0 == current.0 && producer <= current,
        "backup was created by incompatible Labby {}.{}.{}",
        producer.0,
        producer.1,
        producer.2
    );
    Ok(())
}

#[cfg(windows)]
fn ensure_supported_platform() -> Result<()> {
    bail!(
        "durable-state recovery is unavailable on Windows until owner-only ACL enforcement is implemented"
    )
}

#[cfg(not(windows))]
fn ensure_supported_platform() -> Result<()> {
    Ok(())
}

fn sync_parent(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let parent = path.parent().context("path has no parent directory")?;
        fs::File::open(parent)?.sync_all()?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewrite_manifest(bundle: &Path, mutate: impl FnOnce(&mut DurableStateManifest)) {
        let path = bundle.join("manifest.json");
        let mut manifest: DurableStateManifest =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        mutate(&mut manifest);
        fs::write(path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    }

    #[test]
    fn external_file_discovery_ignores_unrelated_dotenv_values() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("installation");
        private_dir(&root).unwrap();
        let external_db = temp.path().join("auth.db");
        write_private(&external_db, b"auth").unwrap();
        write_private(
            &root.join(".env"),
            format!(
                "LABBY_BIND_ADDR=127.0.0.1:3030\nUNRELATED_ABSOLUTE='{}'\nLABBY_AUTH_SQLITE_PATH='{}'\n",
                temp.path().join("unrelated").display(),
                external_db.display()
            )
            .as_bytes(),
        )
        .unwrap();
        let paths = InstallationPaths::from_root(&root).unwrap();

        let files = configured_external_files(&paths).unwrap();

        assert!(files.contains(&fs::canonicalize(&external_db).unwrap()));
        assert!(!files.contains(&temp.path().join("unrelated")));
    }

    #[cfg(unix)]
    #[test]
    fn bundle_location_rejects_direct_and_symlinked_installation_descendants() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("installation");
        private_dir(&root).unwrap();
        let paths = InstallationPaths::from_root(&root).unwrap();
        let direct = root.join("backup");
        assert!(
            ensure_bundle_outside_installation_root(&direct, &paths)
                .unwrap_err()
                .to_string()
                .contains("outside")
        );

        let alias = temp.path().join("installation-alias");
        symlink(&root, &alias).unwrap();
        assert!(ensure_bundle_outside_installation_root(&alias.join("backup"), &paths).is_err());
        ensure_bundle_outside_installation_root(&temp.path().join("backup"), &paths).unwrap();
    }

    #[test]
    fn full_state_and_external_auth_files_round_trip_with_integrity() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("installation");
        private_dir(&root).unwrap();
        let external_db = temp.path().join("external-auth.db");
        let external_key = temp.path().join("external-auth.pem");
        write_private(&external_db, b"auth-v1").unwrap();
        write_private(&external_key, b"key-v1").unwrap();
        write_private(&root.join("state.db"), b"state-v1").unwrap();
        private_dir(&root.join("snippets")).unwrap();
        write_private(&root.join("snippets/operator.ts"), b"durable").unwrap();
        let config = format!(
            "config_version = 1\n[auth]\nsqlite_path = {:?}\nkey_path = {:?}\n",
            external_db.display().to_string(),
            external_key.display().to_string()
        );
        write_private(&root.join("config.toml"), config.as_bytes()).unwrap();
        let paths = InstallationPaths::from_root(&root).unwrap();
        let _lock = InstallationLifecycleLock::acquire_offline(&paths).unwrap();
        let bundle = temp.path().join("backup");
        private_dir(&bundle).unwrap();
        private_dir(&bundle.join("payload")).unwrap();
        let manifest = export_locked(&paths, &bundle).unwrap();
        assert!(
            manifest
                .entries
                .iter()
                .any(|entry| entry.source == fs::canonicalize(&external_db).unwrap())
        );
        assert!(
            !manifest
                .entries
                .iter()
                .any(|entry| entry.source == paths.lifecycle_lock())
        );

        fs::write(root.join("state.db"), b"broken").unwrap();
        fs::write(&external_db, b"broken").unwrap();
        let external_wal = external_db.with_extension("db-wal");
        let external_shm = external_db.with_extension("db-shm");
        write_private(&external_wal, b"post-backup WAL").unwrap();
        write_private(&external_shm, b"post-backup shared memory").unwrap();
        write_private(&root.join("post-backup"), b"remove-me").unwrap();
        restore_bundle_locked(&paths, &bundle).unwrap();
        assert_eq!(fs::read(root.join("state.db")).unwrap(), b"state-v1");
        assert_eq!(fs::read(external_db).unwrap(), b"auth-v1");
        assert!(!root.join("post-backup").exists());
        assert!(!external_wal.exists());
        assert!(!external_shm.exists());

        let payload = bundle.join(&manifest.entries[0].payload);
        fs::write(payload, b"tampered").unwrap();
        assert!(
            verify_bundle_locked(&bundle, None)
                .unwrap_err()
                .to_string()
                .contains("integrity mismatch")
        );
    }

    #[cfg(unix)]
    #[test]
    fn export_rejects_symlinked_state() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("installation");
        private_dir(&root).unwrap();
        write_private(&temp.path().join("secret"), b"secret").unwrap();
        symlink(temp.path().join("secret"), root.join("escape")).unwrap();
        let paths = InstallationPaths::from_root(&root).unwrap();
        let bundle = temp.path().join("backup");
        private_dir(&bundle).unwrap();
        private_dir(&bundle.join("payload")).unwrap();
        assert!(
            export_locked(&paths, &bundle)
                .unwrap_err()
                .to_string()
                .contains("symlink")
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_ancestor_chain() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let actual = temp.path().join("actual");
        private_dir(&actual).unwrap();
        write_private(&actual.join("key"), &[7_u8; 32]).unwrap();
        let linked = temp.path().join("linked");
        symlink(&actual, &linked).unwrap();
        let error = reject_symlink_chain(&linked.join("key")).unwrap_err();
        assert!(error.to_string().contains("symlink path rejected"));
    }

    #[test]
    fn verification_rejects_schema_integrity_mode_and_payload_set_drift() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("installation");
        private_dir(&root).unwrap();
        write_private(&root.join("state"), b"value").unwrap();
        let paths = InstallationPaths::from_root(&root).unwrap();
        let make = |name: &str| {
            let bundle = temp.path().join(name);
            private_dir(&bundle).unwrap();
            private_dir(&bundle.join("payload")).unwrap();
            export_locked(&paths, &bundle).unwrap();
            bundle
        };
        let schema = make("schema");
        rewrite_manifest(&schema, |m| m.manifest_version += 1);
        assert!(
            verify_bundle_locked(&schema, None)
                .unwrap_err()
                .to_string()
                .contains("unsupported")
        );
        let size = make("size");
        rewrite_manifest(&size, |m| m.entries[0].size += 1);
        assert!(
            verify_bundle_locked(&size, None)
                .unwrap_err()
                .to_string()
                .contains("integrity mismatch")
        );
        let mode = make("mode");
        rewrite_manifest(&mode, |m| m.entries[0].mode = 0o666);
        assert!(
            verify_bundle_locked(&mode, None)
                .unwrap_err()
                .to_string()
                .contains("insecure manifest mode")
        );
        let missing = make("missing");
        let manifest = verify_bundle_locked(&missing, None).unwrap();
        fs::remove_file(missing.join(&manifest.entries[0].payload)).unwrap();
        assert!(verify_bundle_locked(&missing, None).is_err());
        let extra = make("extra");
        write_private(&extra.join("payload/extra"), b"extra").unwrap();
        assert!(
            verify_bundle_locked(&extra, None)
                .unwrap_err()
                .to_string()
                .contains("extra payload")
        );
    }

    #[test]
    fn lifecycle_lock_contention_blocks_offline_dr() {
        let temp = tempfile::tempdir().unwrap();
        let paths = InstallationPaths::from_root(temp.path().join("installation")).unwrap();
        let _held = InstallationLifecycleLock::acquire_daemon(&paths).unwrap();
        assert!(
            InstallationLifecycleLock::acquire_offline(&paths)
                .unwrap_err()
                .to_string()
                .contains("already held")
        );
    }

    #[test]
    fn duplicate_external_destination_mapping_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("installation");
        private_dir(&root).unwrap();
        write_private(&root.join("state"), b"value").unwrap();
        let paths = InstallationPaths::from_root(root).unwrap();
        let bundle = temp.path().join("bundle");
        private_dir(&bundle).unwrap();
        private_dir(&bundle.join("payload")).unwrap();
        export_locked(&paths, &bundle).unwrap();
        let source_bundle = bundle.clone();
        rewrite_manifest(&bundle, |m| {
            let original = &m.entries[0];
            let payload = PathBuf::from("payload/duplicate");
            fs::copy(
                source_bundle.join(&original.payload),
                source_bundle.join(&payload),
            )
            .unwrap();
            m.entries.push(DurableStateEntry {
                source: original.source.clone(),
                payload,
                size: original.size,
                sha256: original.sha256.clone(),
                mode: original.mode,
            });
        });
        assert!(
            verify_bundle_locked(&bundle, None)
                .unwrap_err()
                .to_string()
                .contains("duplicate manifest source")
        );
    }

    #[test]
    fn restore_rejects_manifest_controlled_absolute_destination() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("installation");
        private_dir(&root).unwrap();
        write_private(&root.join("state"), b"value").unwrap();
        let paths = InstallationPaths::from_root(&root).unwrap();
        let bundle = temp.path().join("bundle");
        private_dir(&bundle).unwrap();
        private_dir(&bundle.join("payload")).unwrap();
        export_locked(&paths, &bundle).unwrap();
        rewrite_manifest(&bundle, |manifest| {
            manifest.entries[0].source = temp.path().join("attacker-selected");
        });
        let error = restore_bundle_locked(&paths, &bundle).unwrap_err();
        assert!(format!("{error:#}").contains("unconfigured external"));
        assert!(!temp.path().join("attacker-selected").exists());
    }

    #[test]
    fn future_minor_backup_is_rejected() {
        assert!(ensure_version_compatible("1.999.0").is_err());
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn same_major_backup_restores_and_migrates_real_databases() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("installation");
        private_dir(&root).unwrap();
        let auth = root.join("auth.db");
        let access = root.join("access.db");
        let usage = root.join("usage.db");
        {
            let connection = rusqlite::Connection::open(&auth).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE registered_clients (
                       client_id TEXT PRIMARY KEY, redirect_uris TEXT NOT NULL,
                       created_at INTEGER NOT NULL
                     );
                     CREATE TABLE refresh_tokens (
                       refresh_token_hash TEXT PRIMARY KEY, client_id TEXT NOT NULL,
                       subject TEXT NOT NULL, resource TEXT NOT NULL DEFAULT '',
                       scope TEXT NOT NULL, provider_refresh_token TEXT,
                       created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL
                     );
                     INSERT INTO registered_clients VALUES ('n1','[\"http://127.0.0.1/callback\"]',1);
                     PRAGMA user_version = 3;",
                )
                .unwrap();
        }
        {
            let connection = rusqlite::Connection::open(&usage).unwrap();
            connection
                .execute_batch(
                    "PRAGMA user_version = 1;
                     CREATE TABLE upstream_calls (
                       id INTEGER PRIMARY KEY AUTOINCREMENT, ts_unix INTEGER NOT NULL,
                       upstream_name TEXT NOT NULL, tool_name TEXT NOT NULL,
                       actor TEXT NOT NULL DEFAULT 'unattributed', outcome TEXT NOT NULL,
                       elapsed_ms INTEGER NOT NULL
                     );
                     INSERT INTO upstream_calls
                       (ts_unix,upstream_name,tool_name,actor,outcome,elapsed_ms)
                       VALUES (1,'n1','probe','operator','ok',1);",
                )
                .unwrap();
        }
        {
            let connection = rusqlite::Connection::open(&access).unwrap();
            connection
                .execute_batch(crate::access::migration_fixture::V1_METADATA_SCHEMA)
                .unwrap();
            connection
                .execute_batch(crate::access::migration_fixture::DOMAIN_SCHEMA)
                .unwrap();
            connection
                .execute(
                    "INSERT INTO access_metadata(
                       singleton, schema_version, schema_fingerprint, global_revision, updated_at
                     ) VALUES(1, ?1, ?2, 9, 123)",
                    rusqlite::params![
                        crate::access::migration_fixture::V1_SCHEMA_VERSION,
                        crate::access::migration_fixture::V1_SCHEMA_FINGERPRINT
                    ],
                )
                .unwrap();
            connection
                .pragma_update(
                    None,
                    "application_id",
                    crate::access::migration_fixture::APPLICATION_ID,
                )
                .unwrap();
            connection
                .pragma_update(
                    None,
                    "user_version",
                    crate::access::migration_fixture::V1_SCHEMA_VERSION,
                )
                .unwrap();
        }
        #[cfg(unix)]
        for database in [&auth, &access, &usage] {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(database, fs::Permissions::from_mode(0o600)).unwrap();
        }

        let paths = InstallationPaths::from_root(&root).unwrap();
        let bundle = temp.path().join("bundle");
        private_dir(&bundle).unwrap();
        private_dir(&bundle.join("payload")).unwrap();
        export_locked(&paths, &bundle).unwrap();
        rewrite_manifest(&bundle, |manifest| manifest.labby_version = "1.0.0".into());
        restore_bundle_locked(&paths, &bundle).unwrap();

        drop(
            labby_auth::sqlite::SqliteStore::open(auth.clone())
                .await
                .unwrap(),
        );
        drop(
            crate::access::AccessStore::open(access.clone())
                .await
                .unwrap(),
        );
        drop(
            labby_gateway::usage::UsageStore::open(usage.clone())
                .await
                .unwrap(),
        );
        let auth_db = rusqlite::Connection::open(auth).unwrap();
        let access_db = rusqlite::Connection::open(access).unwrap();
        let usage_db = rusqlite::Connection::open(usage).unwrap();
        assert_eq!(
            auth_db
                .query_row(
                    "SELECT client_id FROM registered_clients WHERE client_id='n1'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "n1"
        );
        assert_eq!(
            usage_db
                .query_row(
                    "SELECT upstream_name FROM upstream_calls WHERE upstream_name='n1'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "n1"
        );
        assert!(
            auth_db
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap()
                > 3
        );
        assert!(
            access_db
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap()
                > crate::access::migration_fixture::V1_SCHEMA_VERSION
        );
        assert_eq!(
            access_db
                .query_row(
                    "SELECT global_revision FROM access_metadata WHERE singleton=1",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            9
        );
        assert!(
            usage_db
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap()
                > 1
        );
    }

    #[test]
    fn recovery_authentication_rejects_manifest_tamper_wrong_key_and_replayed_tag() {
        let key = [7_u8; 32];
        let wrong_key = [8_u8; 32];
        let original = br#"{"installation_root":"/trusted/a"}"#;
        let changed = br#"{"installation_root":"/trusted/b"}"#;
        let tag = authentication_tag(&key, original).unwrap();
        verify_authentication(&key, original, &tag).unwrap();
        assert!(verify_authentication(&key, changed, &tag).is_err());
        assert!(verify_authentication(&wrong_key, original, &tag).is_err());
        assert!(
            verify_authentication(&key, changed, &tag)
                .unwrap_err()
                .to_string()
                .contains("wrong key or modified manifest")
        );
    }

    #[test]
    fn fsynced_journal_recovers_an_interrupted_activation() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("installation");
        private_dir(&root).unwrap();
        let paths = InstallationPaths::from_root(&root).unwrap();
        let target = paths.root().join("state");
        let prior = paths.root().join(".state.labby-rollback-test");
        let staged = paths.root().join(".state.labby-restore-test");
        write_private(&prior, b"prior").unwrap();
        write_private(&target, b"replacement").unwrap();
        write_private(&staged, b"staged").unwrap();
        persist_journal(
            &paths.root().join("restore.journal.json"),
            &RestoreJournal {
                committed: false,
                entries: vec![RestoreJournalEntry {
                    target: target.clone(),
                    prior: Some(prior.clone()),
                    staged: Some(staged.clone()),
                    phase: RestorePhase::Activated,
                }],
            },
        )
        .unwrap();
        recover_interrupted_restore(&paths).unwrap();
        assert_eq!(fs::read(target).unwrap(), b"prior");
        assert!(!prior.exists());
        assert!(!staged.exists());
        assert!(!paths.root().join("restore.journal.json").exists());
    }

    #[test]
    fn planned_journal_recovery_never_removes_an_unmoved_existing_target() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("installation");
        private_dir(&root).unwrap();
        let paths = InstallationPaths::from_root(&root).unwrap();
        let target = paths.root().join("state");
        let prior = paths.root().join(".state.labby-rollback-test");
        let staged = paths.root().join(".state.labby-restore-test");
        write_private(&target, b"original").unwrap();
        persist_journal(
            &paths.root().join("restore.journal.json"),
            &RestoreJournal {
                committed: false,
                entries: vec![RestoreJournalEntry {
                    target: target.clone(),
                    prior: Some(prior),
                    staged: Some(staged),
                    phase: RestorePhase::Planned,
                }],
            },
        )
        .unwrap();
        recover_interrupted_restore(&paths).unwrap();
        assert_eq!(fs::read(target).unwrap(), b"original");
    }

    #[test]
    fn journal_recovery_is_safe_at_every_existing_and_new_target_boundary() {
        struct Case {
            name: &'static str,
            committed: bool,
            phase: RestorePhase,
            target: Option<&'static [u8]>,
            prior: Option<&'static [u8]>,
            staged: bool,
            expected: Option<&'static [u8]>,
        }
        let cases = [
            Case {
                name: "existing-planned",
                committed: false,
                phase: RestorePhase::Planned,
                target: Some(b"old"),
                prior: None,
                staged: false,
                expected: Some(b"old"),
            },
            Case {
                name: "existing-renamed-before-phase",
                committed: false,
                phase: RestorePhase::Planned,
                target: None,
                prior: Some(b"old"),
                staged: true,
                expected: Some(b"old"),
            },
            Case {
                name: "existing-prior-moved",
                committed: false,
                phase: RestorePhase::PriorMoved,
                target: None,
                prior: Some(b"old"),
                staged: true,
                expected: Some(b"old"),
            },
            Case {
                name: "existing-activated",
                committed: false,
                phase: RestorePhase::Activated,
                target: Some(b"new"),
                prior: Some(b"old"),
                staged: false,
                expected: Some(b"old"),
            },
            Case {
                name: "new-planned",
                committed: false,
                phase: RestorePhase::Planned,
                target: None,
                prior: None,
                staged: true,
                expected: None,
            },
            Case {
                name: "new-prior-moved",
                committed: false,
                phase: RestorePhase::PriorMoved,
                target: None,
                prior: None,
                staged: true,
                expected: None,
            },
            Case {
                name: "new-activated",
                committed: false,
                phase: RestorePhase::Activated,
                target: Some(b"new"),
                prior: None,
                staged: false,
                expected: None,
            },
            Case {
                name: "committed-activated",
                committed: true,
                phase: RestorePhase::Activated,
                target: Some(b"new"),
                prior: Some(b"old"),
                staged: false,
                expected: Some(b"new"),
            },
        ];
        for case in cases {
            let temp = tempfile::tempdir().unwrap();
            let paths = InstallationPaths::from_root(temp.path().join(case.name)).unwrap();
            private_dir(paths.root()).unwrap();
            let target = paths.root().join("state");
            let prior = paths.root().join(".state.labby-rollback-test");
            let staged = paths.root().join(".state.labby-restore-test");
            if let Some(bytes) = case.target {
                write_private(&target, bytes).unwrap();
            }
            if let Some(bytes) = case.prior {
                write_private(&prior, bytes).unwrap();
            }
            if case.staged {
                write_private(&staged, b"new").unwrap();
            }
            persist_journal(
                &paths.root().join("restore.journal.json"),
                &RestoreJournal {
                    committed: case.committed,
                    entries: vec![RestoreJournalEntry {
                        target: target.clone(),
                        prior: case.prior.map(|_| prior.clone()),
                        staged: case.staged.then_some(staged.clone()),
                        phase: case.phase,
                    }],
                },
            )
            .unwrap();
            recover_interrupted_restore(&paths).unwrap();
            assert_eq!(target.exists(), case.expected.is_some(), "{}", case.name);
            if let Some(expected) = case.expected {
                assert_eq!(fs::read(&target).unwrap(), expected, "{}", case.name);
            }
            assert!(!prior.exists(), "{} retained prior", case.name);
            assert!(!staged.exists(), "{} retained staged", case.name);
        }
    }

    #[test]
    fn new_target_activation_before_activated_phase_is_rolled_back() {
        let temp = tempfile::tempdir().unwrap();
        let paths = InstallationPaths::from_root(temp.path().join("installation")).unwrap();
        private_dir(paths.root()).unwrap();
        let target = paths.root().join("state");
        let staged = paths.root().join(".state.labby-restore-test");

        // Exact crash point: the staged file was renamed into place, but the
        // durable journal still records the preceding PriorMoved phase.
        write_private(&target, b"new").unwrap();
        persist_journal(
            &paths.root().join("restore.journal.json"),
            &RestoreJournal {
                committed: false,
                entries: vec![RestoreJournalEntry {
                    target: target.clone(),
                    prior: None,
                    staged: Some(staged.clone()),
                    phase: RestorePhase::PriorMoved,
                }],
            },
        )
        .unwrap();

        recover_interrupted_restore(&paths).unwrap();

        assert!(!target.exists());
        assert!(!staged.exists());
        assert!(!paths.root().join("restore.journal.json").exists());
    }

    #[test]
    fn retained_journal_after_completed_rollback_preserves_original_target() {
        let temp = tempfile::tempdir().unwrap();
        let paths = InstallationPaths::from_root(temp.path().join("installation")).unwrap();
        private_dir(paths.root()).unwrap();
        for name in ["a", "b"] {
            write_private(&paths.root().join(name), b"backup").unwrap();
        }
        let bundle = temp.path().join("bundle");
        private_dir(&bundle).unwrap();
        private_dir(&bundle.join("payload")).unwrap();
        export_locked(&paths, &bundle).unwrap();
        for name in ["a", "b"] {
            fs::write(paths.root().join(name), b"current original").unwrap();
        }
        let mut moved = 0;
        let error = restore_bundle_locked_with_hooks(
            &paths,
            &bundle,
            &mut |path| {
                if path.exists() && path.to_string_lossy().contains(".labby-restore-") {
                    Err(std::io::Error::other("injected staged cleanup failure"))
                } else {
                    fs::remove_file(path)
                }
            },
            &mut |_| {
                moved += 1;
                ensure!(moved != 2, "injected failure after first target activation");
                Ok(())
            },
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("injected staged cleanup failure"));
        assert!(paths.root().join("restore.journal.json").exists());
        recover_interrupted_restore(&paths).unwrap();
        for name in ["a", "b"] {
            assert_eq!(
                fs::read(paths.root().join(name)).unwrap(),
                b"current original"
            );
        }
        assert!(!paths.root().join("restore.journal.json").exists());
        assert_eq!(fs::read_dir(paths.root()).unwrap().count(), 2);
    }

    #[test]
    fn committed_journal_finishes_cleanup_without_rolling_back() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("installation");
        private_dir(&root).unwrap();
        let paths = InstallationPaths::from_root(&root).unwrap();
        let target = paths.root().join("state");
        let prior = paths.root().join(".state.labby-rollback-test");
        write_private(&prior, b"prior").unwrap();
        write_private(&target, b"replacement").unwrap();
        persist_journal(
            &paths.root().join("restore.journal.json"),
            &RestoreJournal {
                committed: true,
                entries: vec![RestoreJournalEntry {
                    target: target.clone(),
                    prior: Some(prior.clone()),
                    staged: None,
                    phase: RestorePhase::Activated,
                }],
            },
        )
        .unwrap();
        recover_interrupted_restore(&paths).unwrap();
        assert_eq!(fs::read(target).unwrap(), b"replacement");
        assert!(!prior.exists());
    }

    #[test]
    fn failure_after_moving_prior_restores_original_files() {
        for remove_extra in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("installation");
            private_dir(&root).unwrap();
            write_private(&root.join("state"), b"backup").unwrap();
            let paths = InstallationPaths::from_root(&root).unwrap();
            let bundle = temp.path().join("bundle");
            private_dir(&bundle).unwrap();
            private_dir(&bundle.join("payload")).unwrap();
            export_locked(&paths, &bundle).unwrap();
            fs::write(root.join("state"), b"current").unwrap();
            if remove_extra {
                write_private(&root.join("extra"), b"extra-current").unwrap();
            }
            let error = restore_bundle_locked_with_hooks(
                &paths,
                &bundle,
                &mut |path| fs::remove_file(path),
                &mut |_| bail!("injected failure before directory sync"),
            )
            .unwrap_err();
            assert!(format!("{error:#}").contains("prior files rolled back"));
            assert_eq!(fs::read(root.join("state")).unwrap(), b"current");
            if remove_extra {
                assert_eq!(fs::read(root.join("extra")).unwrap(), b"extra-current");
            }
            assert!(!root.join("restore.journal.json").exists());
            assert!(fs::read_dir(&root).unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".labby-")
            }));
        }
    }

    #[test]
    fn committed_restore_reports_deterministic_cleanup_failure() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("installation");
        private_dir(&root).unwrap();
        write_private(&root.join("state"), b"backup").unwrap();
        let paths = InstallationPaths::from_root(&root).unwrap();
        let bundle = temp.path().join("bundle");
        private_dir(&bundle).unwrap();
        private_dir(&bundle.join("payload")).unwrap();
        export_locked(&paths, &bundle).unwrap();
        fs::write(root.join("state"), b"changed").unwrap();

        let mut cleanup = |path: &Path| {
            if path.to_string_lossy().contains(".labby-rollback-") {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected cleanup failure",
                ))
            } else {
                fs::remove_file(path)
            }
        };
        let outcome = restore_bundle_locked_with_cleanup(&paths, &bundle, &mut cleanup).unwrap();
        assert_eq!(fs::read(root.join("state")).unwrap(), b"backup");
        let warning = outcome
            .maintenance_warning
            .expect("committed cleanup warning");
        assert!(warning.contains("1 cleanup operation(s) failed"));
        assert!(!warning.contains("injected cleanup failure"));
        assert!(!warning.contains(root.to_string_lossy().as_ref()));
        assert!(!paths.root().join("restore.journal.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn export_rejects_hardlinked_state() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("installation");
        private_dir(&root).unwrap();
        write_private(&root.join("state"), b"value").unwrap();
        fs::hard_link(root.join("state"), root.join("alias")).unwrap();
        let paths = InstallationPaths::from_root(&root).unwrap();
        let bundle = temp.path().join("bundle");
        private_dir(&bundle).unwrap();
        private_dir(&bundle.join("payload")).unwrap();
        assert!(
            format!("{:#}", export_locked(&paths, &bundle).unwrap_err()).contains("hard-linked")
        );
    }

    #[cfg(unix)]
    #[test]
    fn restore_rejects_symlinked_destination_ancestor() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("installation");
        private_dir(&root).unwrap();
        write_private(&root.join("state"), b"value").unwrap();
        let paths = InstallationPaths::from_root(&root).unwrap();
        let bundle = temp.path().join("bundle");
        private_dir(&bundle).unwrap();
        private_dir(&bundle.join("payload")).unwrap();
        export_locked(&paths, &bundle).unwrap();
        fs::remove_file(root.join("state")).unwrap();
        private_dir(&temp.path().join("outside")).unwrap();
        symlink(temp.path().join("outside"), root.join("linked")).unwrap();
        rewrite_manifest(&bundle, |manifest| {
            manifest.entries[0].source = paths.root().join("linked/state");
        });
        let error = restore_bundle_locked(&paths, &bundle).unwrap_err();
        assert!(format!("{error:#}").contains("symlink"));
        assert!(!temp.path().join("outside/state").exists());
    }

    #[cfg(unix)]
    #[test]
    fn insecure_bundle_and_destination_symlink_are_rejected() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("installation");
        private_dir(&root).unwrap();
        write_private(&root.join("state"), b"value").unwrap();
        let paths = InstallationPaths::from_root(&root).unwrap();
        let bundle = temp.path().join("bundle");
        private_dir(&bundle).unwrap();
        private_dir(&bundle.join("payload")).unwrap();
        export_locked(&paths, &bundle).unwrap();
        fs::set_permissions(&bundle, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(
            verify_bundle_locked(&bundle, None)
                .unwrap_err()
                .to_string()
                .contains("world-writable")
        );
        fs::set_permissions(&bundle, fs::Permissions::from_mode(0o700)).unwrap();
        fs::remove_file(root.join("state")).unwrap();
        let outside = temp.path().join("outside");
        write_private(&outside, b"outside").unwrap();
        symlink(&outside, root.join("state")).unwrap();
        let error = restore_bundle_locked(&paths, &bundle).unwrap_err();
        assert!(format!("{error:#}").contains("symlink"));
        assert_eq!(fs::read(outside).unwrap(), b"outside");
    }

    #[cfg(unix)]
    #[test]
    fn mid_transaction_failure_restores_exact_prior_bytes_and_mode() {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("installation");
        private_dir(&root).unwrap();
        let first = root.join("first");
        write_private(&first, b"prior").unwrap();
        fs::set_permissions(&first, fs::Permissions::from_mode(0o600)).unwrap();
        let blocker = root.join("blocker");
        write_private(&blocker, b"not-a-directory").unwrap();
        let bundle = temp.path().join("bundle");
        private_dir(&bundle).unwrap();
        private_dir(&bundle.join("payload")).unwrap();
        write_private(&bundle.join("payload/one"), b"replacement").unwrap();
        write_private(&bundle.join("payload/two"), b"unreachable").unwrap();
        let entry = |source: PathBuf, payload: &str, bytes: &[u8]| DurableStateEntry {
            source,
            payload: PathBuf::from(payload),
            size: bytes.len() as u64,
            sha256: hex::encode(Sha256::digest(bytes)),
            mode: 0o600,
        };
        let manifest = DurableStateManifest {
            manifest_version: MANIFEST_VERSION,
            labby_version: env!("CARGO_PKG_VERSION").into(),
            installation_root: root.clone(),
            entries: vec![
                entry(first.clone(), "payload/one", b"replacement"),
                entry(blocker.join("child"), "payload/two", b"unreachable"),
            ],
        };
        write_private(
            &bundle.join("manifest.json"),
            &serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        let paths = InstallationPaths::from_root(root).unwrap();
        assert!(restore_bundle_locked(&paths, &bundle).is_err());
        assert_eq!(fs::read(&first).unwrap(), b"prior");
        assert_eq!(fs::metadata(first).unwrap().mode() & 0o777, 0o600);
    }
}
