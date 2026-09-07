use super::{
    blob::BlobStore,
    store::{FileStashStore, FileStashStoreError},
};
use crate::config::FileStashPreferences;
#[cfg(unix)]
use std::io::{Read, Write};
use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::{Mutex, Semaphore};

const SHUTDOWN_STEP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FileStashBlockedReason {
    UnsafeRoot,
    Permission,
    Corrupt,
    NewerSchema,
    BackupMismatch,
    Unavailable,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FileStashStatus {
    Ready,
    Blocked(FileStashBlockedReason),
    Shutdown,
}
enum State {
    Ready(FileStashStore),
    Blocked(FileStashBlockedReason),
    Shutdown,
}

/// Sole process owner for Stash persistence. The retained root handle pins the
/// verified directory while SQLite and later blob operations address children.
pub(crate) struct FileStashRuntime {
    root: Arc<PathBuf>,
    _root_handle: Option<Arc<File>>,
    state: Arc<Mutex<State>>,
    blobs: Option<BlobStore>,
    janitor_admission: Arc<Semaphore>,
    janitor_cancel: tokio_util::sync::CancellationToken,
    janitor_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    page_limit: usize,
    max_query_bytes: usize,
}
impl FileStashRuntime {
    pub(crate) fn blocked() -> Self {
        Self {
            root: Arc::new(PathBuf::new()),
            _root_handle: None,
            state: Arc::new(Mutex::new(State::Blocked(
                FileStashBlockedReason::Unavailable,
            ))),
            blobs: None,
            janitor_admission: Arc::new(Semaphore::new(1)),
            janitor_cancel: tokio_util::sync::CancellationToken::new(),
            janitor_task: Mutex::new(None),
            page_limit: usize::from(FileStashPreferences::default().page_size),
            max_query_bytes: FileStashPreferences::default().max_query_bytes,
        }
    }
    pub(crate) async fn initialize(root: PathBuf) -> Self {
        Self::initialize_with_preferences(root, FileStashPreferences::default()).await
    }
    pub(crate) async fn initialize_with_interval(
        root: PathBuf,
        janitor_interval: std::time::Duration,
    ) -> Self {
        let preferences = FileStashPreferences {
            janitor_interval_seconds: janitor_interval.as_secs(),
            ..FileStashPreferences::default()
        };
        Self::initialize_with_preferences(root, preferences).await
    }

    pub(crate) async fn initialize_with_preferences(
        root: PathBuf,
        preferences: FileStashPreferences,
    ) -> Self {
        let page_limit = usize::from(preferences.page_size);
        let max_query_bytes = preferences.max_query_bytes;
        #[cfg(not(target_os = "linux"))]
        {
            drop(preferences);
            tracing::warn!("File Stash is unavailable: this target is not Linux-qualified");
            Self {
                root: Arc::new(root),
                _root_handle: None,
                state: Arc::new(Mutex::new(State::Blocked(
                    FileStashBlockedReason::UnsafeRoot,
                ))),
                blobs: None,
                janitor_admission: Arc::new(Semaphore::new(1)),
                janitor_cancel: tokio_util::sync::CancellationToken::new(),
                janitor_task: Mutex::new(None),
                page_limit,
                max_query_bytes,
            }
        }
        #[cfg(target_os = "linux")]
        {
            let initialized = initialize_owned(&root, &preferences).await;
            let admission = Arc::new(Semaphore::new(1));
            let cancel = tokio_util::sync::CancellationToken::new();
            let (state, root_handle, blobs, janitor_task) = match initialized {
                Ok((store, handle, tmp, blob_dir)) => {
                    let blob_store =
                        BlobStore::new(tmp, blob_dir, store.clone(), preferences.clone());
                    if let Err(error) = blob_store.recover_pending().await {
                        tracing::warn!(?error, "file stash recovery blocked initialization");
                        return Self {
                            root: Arc::new(root),
                            _root_handle: Some(Arc::new(handle)),
                            state: Arc::new(Mutex::new(State::Blocked(map_store_error(error)))),
                            blobs: None,
                            janitor_admission: Arc::new(Semaphore::new(1)),
                            janitor_cancel: tokio_util::sync::CancellationToken::new(),
                            janitor_task: Mutex::new(None),
                            page_limit,
                            max_query_bytes,
                        };
                    }
                    let task = spawn_janitor(
                        blob_store.clone(),
                        Arc::clone(&admission),
                        cancel.clone(),
                        std::time::Duration::from_secs(preferences.janitor_interval_seconds),
                        std::time::Duration::from_secs(preferences.janitor_backoff_max_seconds),
                    );
                    (
                        State::Ready(store),
                        Some(Arc::new(handle)),
                        Some(blob_store),
                        Some(task),
                    )
                }
                Err(reason) => {
                    tracing::warn!(?reason, "file stash runtime initialization blocked");
                    (State::Blocked(reason), None, None, None)
                }
            };
            Self {
                root: Arc::new(root),
                _root_handle: root_handle,
                state: Arc::new(Mutex::new(state)),
                blobs,
                janitor_admission: admission,
                janitor_cancel: cancel,
                janitor_task: Mutex::new(janitor_task),
                page_limit,
                max_query_bytes,
            }
        }
    }
    pub(crate) fn page_limit(&self) -> usize {
        self.page_limit
    }
    pub(crate) fn max_query_bytes(&self) -> usize {
        self.max_query_bytes
    }
    pub(crate) async fn status(&self) -> FileStashStatus {
        match &*self.state.lock().await {
            State::Ready(_) => FileStashStatus::Ready,
            State::Blocked(reason) => FileStashStatus::Blocked(*reason),
            State::Shutdown => FileStashStatus::Shutdown,
        }
    }
    pub(crate) async fn store(&self) -> Result<FileStashStore, FileStashBlockedReason> {
        match &*self.state.lock().await {
            State::Ready(store) => Ok(store.clone()),
            State::Blocked(reason) => Err(*reason),
            State::Shutdown => Err(FileStashBlockedReason::Unavailable),
        }
    }
    pub(crate) async fn blob_store(&self) -> Result<BlobStore, FileStashBlockedReason> {
        match self.status().await {
            FileStashStatus::Ready => self
                .blobs
                .clone()
                .ok_or(FileStashBlockedReason::Unavailable),
            FileStashStatus::Blocked(reason) => Err(reason),
            FileStashStatus::Shutdown => Err(FileStashBlockedReason::Unavailable),
        }
    }
    #[cfg(test)]
    pub(crate) async fn stop_janitor_for_test(&self) {
        self.janitor_cancel.cancel();
        if let Some(task) = self.janitor_task.lock().await.take() {
            let _unused = task.await;
        }
    }
    pub(crate) async fn shutdown(&self) {
        let store = match &*self.state.lock().await {
            State::Ready(store) => Some(store.clone()),
            State::Blocked(_) | State::Shutdown => None,
        };
        self.janitor_admission.close();
        self.janitor_cancel.cancel();
        if let Some(mut task) = self.janitor_task.lock().await.take()
            && tokio::time::timeout(SHUTDOWN_STEP_TIMEOUT, &mut task)
                .await
                .is_err()
        {
            tracing::warn!("file stash janitor did not stop before the shutdown deadline");
            task.abort();
        }
        if let Some(store) = store {
            match tokio::time::timeout(SHUTDOWN_STEP_TIMEOUT, store.checkpoint()).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => tracing::warn!(?error, "file stash shutdown checkpoint failed"),
                Err(_) => tracing::warn!("file stash shutdown checkpoint exceeded its deadline"),
            }
        }
        if let Some(store) = match &*self.state.lock().await {
            State::Ready(store) => Some(store.clone()),
            State::Blocked(_) | State::Shutdown => None,
        } {
            store.close();
        }
        *self.state.lock().await = State::Shutdown;
    }
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
}

fn spawn_janitor(
    blobs: BlobStore,
    admission: Arc<Semaphore>,
    cancel: tokio_util::sync::CancellationToken,
    interval: std::time::Duration,
    max_backoff: std::time::Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(error) = blobs.scrub_integrity(cancel.clone()).await {
            tracing::warn!(?error, "file stash background integrity scrub failed");
        }
        let mut delay = interval;
        loop {
            tokio::select! {
                () = cancel.cancelled() => break,
                () = tokio::time::sleep(delay) => {
                    let Ok(_permit) = Arc::clone(&admission).try_acquire_owned() else { continue };
                    match blobs.cleanup_expired().await {
                        Ok(_) => delay = interval,
                        Err(error) => {
                            tracing::warn!(?error, "file stash janitor pass failed");
                            delay = next_janitor_delay(delay, interval, max_backoff);
                        }
                    }
                }
            }
        }
    })
}

fn next_janitor_delay(
    previous: std::time::Duration,
    interval: std::time::Duration,
    max_backoff: std::time::Duration,
) -> std::time::Duration {
    previous
        .saturating_mul(2)
        .max(interval)
        .min(max_backoff.max(interval))
}

async fn initialize_owned(
    root: &Path,
    preferences: &FileStashPreferences,
) -> Result<(FileStashStore, File, File, File), FileStashBlockedReason> {
    let owned = root.to_path_buf();
    let verified = tokio::task::spawn_blocking(move || prepare_root(&owned))
        .await
        .map_err(|_| FileStashBlockedReason::Unavailable)??;
    prepare_database_files(&verified.handle).map_err(|_| FileStashBlockedReason::UnsafeRoot)?;
    let marker =
        read_or_create_marker(&verified.handle).map_err(|_| FileStashBlockedReason::UnsafeRoot)?;
    let database = anchored_child_path(&verified.handle, &verified.path, "metadata.sqlite3")?;
    let store = FileStashStore::open_with_limits(
        database,
        marker,
        preferences.queue_capacity,
        std::time::Duration::from_millis(preferences.database_deadline_ms),
    )
    .await
    .map_err(map_store_error)?;
    verify_database_identity(&verified.handle, store.path())?;
    let tmp = open_private_directory(&verified.handle, "tmp")?;
    let blobs = open_private_directory(&verified.handle, "blobs")?;
    Ok((store, verified.handle, tmp, blobs))
}
fn map_store_error(error: FileStashStoreError) -> FileStashBlockedReason {
    match error {
        FileStashStoreError::Corrupt => FileStashBlockedReason::Corrupt,
        FileStashStoreError::NewerSchema(_) => FileStashBlockedReason::NewerSchema,
        FileStashStoreError::BackupMismatch => FileStashBlockedReason::BackupMismatch,
        FileStashStoreError::Integrity | FileStashStoreError::LengthMismatch => {
            FileStashBlockedReason::Corrupt
        }
        FileStashStoreError::Busy
        | FileStashStoreError::Unavailable
        | FileStashStoreError::QuotaExceeded
        | FileStashStoreError::Conflict
        | FileStashStoreError::NotFound => FileStashBlockedReason::Unavailable,
    }
}

#[cfg(target_os = "linux")]
fn open_private_directory(root: &File, name: &str) -> Result<File, FileStashBlockedReason> {
    use rustix::fs::{Mode, OFlags, openat};
    let fd = openat(
        root,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| FileStashBlockedReason::UnsafeRoot)?;
    let file = File::from(fd);
    validate_private_directory(&file)?;
    Ok(file)
}

#[cfg(not(target_os = "linux"))]
fn open_private_directory(_: &File, _: &str) -> Result<File, FileStashBlockedReason> {
    Err(FileStashBlockedReason::UnsafeRoot)
}

#[cfg(unix)]
struct VerifiedRoot {
    handle: File,
    path: PathBuf,
}
#[cfg(not(unix))]
struct VerifiedRoot {
    handle: File,
    path: PathBuf,
}

#[cfg(unix)]
fn prepare_root(root: &Path) -> Result<VerifiedRoot, FileStashBlockedReason> {
    use rustix::fs::{Mode, OFlags, mkdirat, openat};
    if !root.is_absolute() {
        return Err(FileStashBlockedReason::UnsafeRoot);
    }
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut fd = openat(rustix::fs::CWD, "/", flags, Mode::empty())
        .map_err(|_| FileStashBlockedReason::UnsafeRoot)?;
    for component in root.components() {
        let std::path::Component::Normal(name) = component else {
            continue;
        };
        fd = match openat(&fd, name, flags, Mode::empty()) {
            Ok(next) => next,
            Err(rustix::io::Errno::NOENT) => {
                mkdirat(&fd, name, Mode::RUSR | Mode::WUSR | Mode::XUSR)
                    .map_err(|_| FileStashBlockedReason::Permission)?;
                openat(&fd, name, flags, Mode::empty())
                    .map_err(|_| FileStashBlockedReason::UnsafeRoot)?
            }
            Err(_) => return Err(FileStashBlockedReason::UnsafeRoot),
        };
    }
    let path = std::fs::canonicalize(root).map_err(|_| FileStashBlockedReason::UnsafeRoot)?;
    let root = File::from(fd);
    validate_private_directory(&root)?;
    for name in ["blobs", "tmp"] {
        let child = match openat(&root, name, flags, Mode::empty()) {
            Ok(child) => child,
            Err(rustix::io::Errno::NOENT) => {
                mkdirat(&root, name, Mode::RUSR | Mode::WUSR | Mode::XUSR)
                    .map_err(|_| FileStashBlockedReason::Permission)?;
                openat(&root, name, flags, Mode::empty())
                    .map_err(|_| FileStashBlockedReason::UnsafeRoot)?
            }
            Err(_) => return Err(FileStashBlockedReason::UnsafeRoot),
        };
        validate_private_directory(&File::from(child))?;
    }
    verify_root_identity(&root, &path)?;
    Ok(VerifiedRoot { handle: root, path })
}
#[cfg(not(unix))]
fn prepare_root(_: &Path) -> Result<VerifiedRoot, FileStashBlockedReason> {
    // Fail closed until a handle-relative Windows creator is available in the
    // sanctioned labby-winjob boundary.
    Err(FileStashBlockedReason::UnsafeRoot)
}

#[cfg(unix)]
fn verify_root_identity(handle: &File, path: &Path) -> Result<(), FileStashBlockedReason> {
    use std::os::unix::fs::MetadataExt as _;
    let expected = handle
        .metadata()
        .map_err(|_| FileStashBlockedReason::Unavailable)?;
    let observed = std::fs::metadata(path).map_err(|_| FileStashBlockedReason::Unavailable)?;
    if expected.dev() != observed.dev() || expected.ino() != observed.ino() {
        return Err(FileStashBlockedReason::UnsafeRoot);
    }
    Ok(())
}
#[cfg(unix)]
fn validate_private_directory(file: &File) -> Result<(), FileStashBlockedReason> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    let metadata = file
        .metadata()
        .map_err(|_| FileStashBlockedReason::Unavailable)?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(FileStashBlockedReason::Permission);
    }
    Ok(())
}

#[cfg(unix)]
fn read_or_create_marker(root: &File) -> std::io::Result<String> {
    use rustix::fs::{Mode, OFlags, openat};
    let opened = openat(
        root,
        "snapshot-id",
        OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    );
    match opened {
        Ok(fd) => {
            let mut file = File::from(fd);
            let id = ulid::Ulid::new().to_string();
            file.write_all(id.as_bytes())?;
            file.sync_all()?;
            Ok(id)
        }
        Err(rustix::io::Errno::EXIST) => {
            let fd = openat(
                root,
                "snapshot-id",
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(std::io::Error::from)?;
            let mut file = File::from(fd);
            validate_private_marker(&file)?;
            let mut id = String::new();
            (&mut file).take(27).read_to_string(&mut id)?;
            if id.len() != 26 || !id.bytes().all(valid_ulid_byte) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid snapshot marker",
                ));
            }
            Ok(id)
        }
        Err(error) => Err(std::io::Error::from(error)),
    }
}
#[cfg(not(unix))]
fn read_or_create_marker(_: &File) -> std::io::Result<String> {
    Err(std::io::Error::other("unsupported File Stash platform"))
}
fn valid_ulid_byte(byte: u8) -> bool {
    byte.is_ascii_digit()
        || matches!(byte, b'A'..=b'H' | b'J'..=b'K' | b'M'..=b'N' | b'P'..=b'T' | b'V'..=b'Z')
}
#[cfg(unix)]
fn validate_private_marker(file: &File) -> std::io::Result<()> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o777 != 0o600
        || metadata.len() != 26
    {
        return Err(std::io::Error::other("unsafe File Stash snapshot marker"));
    }
    Ok(())
}

#[cfg(unix)]
fn prepare_database_files(root: &File) -> std::io::Result<()> {
    use rustix::fs::{Mode, OFlags, openat};
    match openat(
        root,
        "metadata.sqlite3",
        OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    ) {
        Ok(fd) => validate_private_regular(&File::from(fd)),
        Err(rustix::io::Errno::EXIST) => {
            let fd = openat(
                root,
                "metadata.sqlite3",
                OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(std::io::Error::from)?;
            validate_private_regular(&File::from(fd))
        }
        Err(error) => Err(std::io::Error::from(error)),
    }?;
    for name in ["metadata.sqlite3-wal", "metadata.sqlite3-shm"] {
        match openat(
            root,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => validate_private_regular(&File::from(fd))?,
            Err(rustix::io::Errno::NOENT) => {}
            Err(error) => return Err(std::io::Error::from(error)),
        }
    }
    Ok(())
}
#[cfg(not(unix))]
fn prepare_database_files(_: &File) -> std::io::Result<()> {
    Err(std::io::Error::other("unsupported File Stash platform"))
}
#[cfg(unix)]
fn validate_private_regular(file: &File) -> std::io::Result<()> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o777 != 0o600
    {
        return Err(std::io::Error::other("unsafe File Stash database file"));
    }
    Ok(())
}

#[cfg(unix)]
fn verify_database_identity(root: &File, opened_path: &Path) -> Result<(), FileStashBlockedReason> {
    use rustix::fs::{Mode, OFlags, openat};
    use std::os::unix::fs::MetadataExt as _;
    let fd = openat(
        root,
        "metadata.sqlite3",
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| FileStashBlockedReason::UnsafeRoot)?;
    let anchored = File::from(fd);
    let expected = anchored
        .metadata()
        .map_err(|_| FileStashBlockedReason::Unavailable)?;
    let observed =
        std::fs::metadata(opened_path).map_err(|_| FileStashBlockedReason::Unavailable)?;
    if expected.dev() != observed.dev() || expected.ino() != observed.ino() {
        return Err(FileStashBlockedReason::UnsafeRoot);
    }
    Ok(())
}
#[cfg(not(unix))]
fn verify_database_identity(_: &File, _: &Path) -> Result<(), FileStashBlockedReason> {
    Err(FileStashBlockedReason::UnsafeRoot)
}

#[cfg(target_os = "linux")]
fn anchored_child_path(
    root: &File,
    _: &Path,
    child: &str,
) -> Result<PathBuf, FileStashBlockedReason> {
    use std::os::fd::AsRawFd as _;
    if child.is_empty() || child.contains('/') {
        return Err(FileStashBlockedReason::UnsafeRoot);
    }
    // SQLite accepts only pathnames. On Linux, address the child through the
    // already-verified directory descriptor so renaming or replacing the
    // configured pathname cannot redirect database or WAL/SHM writes.
    Ok(PathBuf::from(format!(
        "/proc/self/fd/{}/{}",
        root.as_raw_fd(),
        child
    )))
}
#[cfg(not(target_os = "linux"))]
fn anchored_child_path(_: &File, _: &Path, _: &str) -> Result<PathBuf, FileStashBlockedReason> {
    Err(FileStashBlockedReason::UnsafeRoot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn janitor_failure_delay_never_drops_below_the_normal_interval() {
        let interval = std::time::Duration::from_mins(1);
        assert_eq!(
            next_janitor_delay(interval, interval, std::time::Duration::from_secs(1)),
            interval
        );
        assert_eq!(
            next_janitor_delay(interval, interval, std::time::Duration::from_mins(5)),
            std::time::Duration::from_mins(2)
        );
    }
    #[cfg(target_os = "linux")]
    fn root(temp: &tempfile::TempDir, name: &str) -> PathBuf {
        std::fs::canonicalize(temp.path()).unwrap().join(name)
    }
    #[tokio::test]
    #[cfg(target_os = "linux")]
    async fn initializes_restarts_checkpoints_and_detects_mismatch() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = root(&temp, "stash");
        let runtime = FileStashRuntime::initialize(root.clone()).await;
        assert_eq!(runtime.status().await, FileStashStatus::Ready);
        runtime.shutdown().await;
        assert_eq!(runtime.status().await, FileStashStatus::Shutdown);
        assert_eq!(
            FileStashRuntime::initialize(root.clone())
                .await
                .status()
                .await,
            FileStashStatus::Ready
        );
        std::fs::write(root.join("snapshot-id"), "01J00000000000000000000000").unwrap();
        assert_eq!(
            FileStashRuntime::initialize(root).await.status().await,
            FileStashStatus::Blocked(FileStashBlockedReason::BackupMismatch)
        );
    }
    #[tokio::test]
    #[cfg(target_os = "linux")]
    async fn schema_enforces_cross_table_names_and_grantee_separation() {
        let temp = tempfile::TempDir::new().unwrap();
        let runtime = FileStashRuntime::initialize(root(&temp, "stash")).await;
        let store = runtime.store().await.unwrap();
        store.with_connection(|connection| {
            connection.execute("INSERT INTO files VALUES('a','owner','Name','name',0,'a',1,1,1)",[]).map_err(FileStashStoreError::sqlite)?;
            assert!(connection.execute("INSERT INTO pending_uploads VALUES('u','owner','NAME','name',0,'pending',9,1,1)",[]).is_err());
            assert!(connection.execute("INSERT INTO grants VALUES('g','a','owner','active',1,NULL)",[]).is_err());
            connection.execute("INSERT INTO grants VALUES('g','a','other','active',1,NULL)",[]).map_err(FileStashStoreError::sqlite)?;
            assert!(connection.execute("UPDATE grants SET grantee_principal_id='owner' WHERE grant_id='g'",[]).is_err());
            let plan: String = connection.query_row(
                "EXPLAIN QUERY PLAN SELECT file_id FROM files WHERE owner_principal_id='owner' AND ready=1 ORDER BY created_at DESC,file_id DESC LIMIT 50",
                [],
                |row| row.get(3),
            ).map_err(FileStashStoreError::sqlite)?;
            assert!(plan.contains("stash_files_owner_list"), "unexpected plan: {plan}");
            Ok(())
        }).await.unwrap();
    }
    #[tokio::test]
    #[cfg(target_os = "linux")]
    async fn rejects_intermediate_symlink_and_insecure_existing_mode() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};
        let temp = tempfile::TempDir::new().unwrap();
        let canonical = std::fs::canonicalize(temp.path()).unwrap();
        let target = canonical.join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).unwrap();
        let link = canonical.join("link");
        symlink(&target, &link).unwrap();
        assert_eq!(
            FileStashRuntime::initialize(link.join("stash"))
                .await
                .status()
                .await,
            FileStashStatus::Blocked(FileStashBlockedReason::UnsafeRoot)
        );
        let insecure = canonical.join("insecure");
        std::fs::create_dir(&insecure).unwrap();
        std::fs::set_permissions(&insecure, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            FileStashRuntime::initialize(insecure).await.status().await,
            FileStashStatus::Blocked(FileStashBlockedReason::Permission)
        );
    }

    #[tokio::test]
    #[cfg(target_os = "linux")]
    async fn database_symlink_substitution_is_rejected_before_target_mutation() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::TempDir::new().unwrap();
        let stash_root = root(&temp, "stash");
        let runtime = FileStashRuntime::initialize(stash_root.clone()).await;
        assert_eq!(runtime.status().await, FileStashStatus::Ready);
        runtime.shutdown().await;
        std::fs::remove_file(stash_root.join("metadata.sqlite3")).unwrap();
        let target = temp.path().join("victim");
        std::fs::write(&target, b"must-not-change").unwrap();
        symlink(&target, stash_root.join("metadata.sqlite3")).unwrap();
        assert_eq!(
            FileStashRuntime::initialize(stash_root)
                .await
                .status()
                .await,
            FileStashStatus::Blocked(FileStashBlockedReason::UnsafeRoot)
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"must-not-change");
    }

    #[tokio::test]
    #[cfg(target_os = "linux")]
    async fn database_open_stays_bound_to_verified_root_after_path_replacement() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::TempDir::new().unwrap();
        let stash_root = root(&temp, "stash");
        let verified = prepare_root(&stash_root).unwrap();
        prepare_database_files(&verified.handle).unwrap();
        let marker = read_or_create_marker(&verified.handle).unwrap();

        let displaced = temp.path().join("verified-root");
        std::fs::rename(&stash_root, &displaced).unwrap();
        std::fs::create_dir(&stash_root).unwrap();
        std::fs::set_permissions(&stash_root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let victim = stash_root.join("metadata.sqlite3");
        std::fs::write(&victim, b"must-not-change").unwrap();
        std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o600)).unwrap();

        let database =
            anchored_child_path(&verified.handle, &verified.path, "metadata.sqlite3").unwrap();
        let store = FileStashStore::open(database, marker).await.unwrap();
        store.checkpoint().await.unwrap();
        store.close();

        assert_eq!(std::fs::read(&victim).unwrap(), b"must-not-change");
        assert!(!stash_root.join("metadata.sqlite3-wal").exists());
        assert!(!stash_root.join("metadata.sqlite3-shm").exists());
        assert!(displaced.join("metadata.sqlite3").metadata().unwrap().len() > 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(target_os = "linux")]
    async fn database_queue_saturates_and_shutdown_closes_existing_handles() {
        let temp = tempfile::TempDir::new().unwrap();
        let runtime = FileStashRuntime::initialize(root(&temp, "stash")).await;
        let store = runtime.store().await.unwrap();
        let held = store.clone();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let entered = Arc::clone(&barrier);
        let task = tokio::spawn(async move {
            held.with_connection(move |_| {
                entered.wait();
                std::thread::sleep(std::time::Duration::from_millis(250));
                Ok(())
            })
            .await
        });
        barrier.wait();
        assert!(matches!(
            store.with_connection(|_| Ok(())).await,
            Err(FileStashStoreError::Busy)
        ));
        task.await.unwrap().unwrap();
        runtime.shutdown().await;
        assert!(matches!(
            store.with_connection(|_| Ok(())).await,
            Err(FileStashStoreError::Unavailable)
        ));
    }

    #[tokio::test]
    #[cfg(target_os = "linux")]
    async fn rejects_corrupt_schema_fingerprint() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = root(&temp, "stash");
        let runtime = FileStashRuntime::initialize(root.clone()).await;
        runtime.shutdown().await;
        let connection = rusqlite::Connection::open(root.join("metadata.sqlite3")).unwrap();
        connection
            .execute(
                "UPDATE stash_metadata SET schema_fingerprint='tampered' WHERE singleton=1",
                [],
            )
            .unwrap();
        drop(connection);
        assert_eq!(
            FileStashRuntime::initialize(root).await.status().await,
            FileStashStatus::Blocked(FileStashBlockedReason::Corrupt)
        );
    }

    #[tokio::test]
    #[cfg(target_os = "linux")]
    async fn rejects_future_schema_without_partial_migration() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = root(&temp, "stash");
        let runtime = FileStashRuntime::initialize(root.clone()).await;
        runtime.shutdown().await;
        let connection = rusqlite::Connection::open(root.join("metadata.sqlite3")).unwrap();
        connection.pragma_update(None, "user_version", 2).unwrap();
        drop(connection);
        assert_eq!(
            FileStashRuntime::initialize(root.clone())
                .await
                .status()
                .await,
            FileStashStatus::Blocked(FileStashBlockedReason::NewerSchema)
        );
        let connection = rusqlite::Connection::open(root.join("metadata.sqlite3")).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            2
        );
    }

    #[cfg(not(target_os = "linux"))]
    #[tokio::test]
    async fn unsupported_targets_fail_closed_without_descriptor_rooted_sqlite() {
        let temp = tempfile::TempDir::new().unwrap();
        let runtime = FileStashRuntime::initialize(temp.path().join("stash")).await;
        assert_eq!(
            runtime.status().await,
            FileStashStatus::Blocked(FileStashBlockedReason::UnsafeRoot)
        );
        assert!(!temp.path().join("stash").exists());
    }
}
