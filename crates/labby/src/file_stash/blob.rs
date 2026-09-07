use super::store::{FileStashStore, FileStashStoreError, PendingRecovery, UploadReservation};
use crate::config::FileStashPreferences;
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    sync::{Arc, Mutex, Weak},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

pub(super) type Result<T> = std::result::Result<T, FileStashStoreError>;

#[derive(Clone)]
pub(crate) struct BlobStore {
    tmp: Arc<File>,
    blobs: Arc<File>,
    store: FileStashStore,
    limits: Arc<FileStashPreferences>,
    instance_uploads: Arc<Semaphore>,
    downloads: Arc<Semaphore>,
    mcp_reads: Arc<Semaphore>,
    principal_uploads: Arc<Mutex<HashMap<String, Weak<Semaphore>>>>,
    active_uploads: Arc<Mutex<HashSet<String>>>,
}

impl BlobStore {
    pub(super) fn new(
        tmp: File,
        blobs: File,
        store: FileStashStore,
        limits: FileStashPreferences,
    ) -> Self {
        Self {
            tmp: Arc::new(tmp),
            blobs: Arc::new(blobs),
            instance_uploads: Arc::new(Semaphore::new(limits.max_concurrent_uploads_per_instance)),
            downloads: Arc::new(Semaphore::new(limits.max_concurrent_downloads)),
            mcp_reads: Arc::new(Semaphore::new(limits.max_concurrent_mcp_reads)),
            principal_uploads: Arc::new(Mutex::new(HashMap::new())),
            active_uploads: Arc::new(Mutex::new(HashSet::new())),
            store,
            limits: Arc::new(limits),
        }
    }

    pub(crate) async fn reserve(
        &self,
        owner: &super::PrincipalId,
        display_name: String,
        collision_key: String,
        declared_bytes: u64,
    ) -> Result<(UploadReservation, UploadAdmission)> {
        self.reserve_for_owner(owner.as_str(), display_name, collision_key, declared_bytes)
            .await
    }

    async fn reserve_for_owner(
        &self,
        owner: &str,
        display_name: String,
        collision_key: String,
        declared_bytes: u64,
    ) -> Result<(UploadReservation, UploadAdmission)> {
        if declared_bytes > self.limits.max_file_bytes {
            return Err(FileStashStoreError::QuotaExceeded);
        }
        let owner_id = owner.to_owned();
        let mut admission = self.acquire_upload(&owner_id).await?;
        let expires_at = unix_now()
            .saturating_add(i64::try_from(self.limits.pending_ttl_seconds).unwrap_or(i64::MAX));
        let reservation = self
            .store
            .reserve_upload(
                owner_id,
                display_name,
                collision_key,
                declared_bytes,
                expires_at,
                self.limits.principal_quota_bytes,
                self.limits.instance_quota_bytes,
                self.limits.max_live_files_per_principal,
            )
            .await?;
        self.active_uploads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(reservation.upload_id.clone());
        admission.bind(
            reservation.upload_id.clone(),
            Arc::clone(&self.active_uploads),
        );
        Ok((reservation, admission))
    }

    pub(crate) async fn write_reserved<R: AsyncRead + Unpin>(
        &self,
        reservation: UploadReservation,
        _admission: UploadAdmission,
        mut reader: R,
        cancel: CancellationToken,
    ) -> Result<String> {
        let upload_id = reservation.upload_id.clone();
        if cancel.is_cancelled() {
            self.store.cancel_upload(upload_id).await?;
            return Err(FileStashStoreError::Unavailable);
        }
        let temp_name = format!("{}.part", upload_id);
        let mut file = match create_regular_exclusive(&self.tmp, &temp_name) {
            Ok(file) => tokio::fs::File::from_std(file),
            Err(error) => {
                self.store.cancel_upload(upload_id).await?;
                return Err(error);
            }
        };
        let transfer = async {
            let mut written = 0_u64;
            let mut buffer = vec![0_u8; 64 * 1024];
            loop {
                let read = tokio::time::timeout(
                    Duration::from_secs(self.limits.upload_idle_seconds),
                    reader.read(&mut buffer),
                )
                .await
                .map_err(|_| FileStashStoreError::Busy)?
                .map_err(|_| FileStashStoreError::Unavailable)?;
                if read == 0 {
                    break;
                }
                written = written
                    .checked_add(read as u64)
                    .ok_or(FileStashStoreError::LengthMismatch)?;
                if written > reservation.reserved_bytes {
                    return Err(FileStashStoreError::LengthMismatch);
                }
                file.write_all(&buffer[..read])
                    .await
                    .map_err(|_| FileStashStoreError::Unavailable)?;
            }
            if written != reservation.reserved_bytes {
                return Err(FileStashStoreError::LengthMismatch);
            }
            file.sync_all()
                .await
                .map_err(|_| FileStashStoreError::Unavailable)?;
            drop(file);
            publish_exclusive(&self.tmp, &temp_name, &self.blobs, &upload_id)?;
            sync_directory(&self.blobs)?;
            sync_directory(&self.tmp)?;
            Ok(())
        };
        let transfer_outcome = tokio::select! {
            biased;
            () = cancel.cancelled() => Err(FileStashStoreError::Unavailable),
            result = tokio::time::timeout(Duration::from_secs(self.limits.upload_total_seconds), transfer) => {
                result.map_err(|_| FileStashStoreError::Busy)?
            },
        };
        if let Err(error) = transfer_outcome {
            self.cleanup_failed_upload(&temp_name, &upload_id).await;
            return Err(error);
        }
        // SQLite work runs on spawn_blocking and cannot be cancelled safely.
        // Once publication is durable, finish both short metadata transitions.
        self.store.mark_blob_published(upload_id.clone()).await?;
        self.store.commit_upload(upload_id).await
    }

    pub(crate) async fn open_blob(
        &self,
        blob_key: &str,
        expected_size: u64,
        mcp: bool,
    ) -> Result<OpenedBlob> {
        validate_blob_key(blob_key)?;
        if mcp && expected_size > self.limits.max_mcp_read_bytes {
            return Err(FileStashStoreError::QuotaExceeded);
        }
        // Both admission permits are acquired before the filesystem is
        // touched, so overload and MCP-specific saturation cannot be used to
        // force descriptor churn.
        let mcp_permit = if mcp {
            Some(
                tokio::time::timeout(
                    Duration::from_millis(self.limits.database_deadline_ms),
                    Arc::clone(&self.mcp_reads).acquire_owned(),
                )
                .await
                .map_err(|_| FileStashStoreError::Busy)?
                .map_err(|_| FileStashStoreError::Unavailable)?,
            )
        } else {
            None
        };
        let download_permit = tokio::time::timeout(
            Duration::from_millis(self.limits.database_deadline_ms),
            Arc::clone(&self.downloads).acquire_owned(),
        )
        .await
        .map_err(|_| FileStashStoreError::Busy)?
        .map_err(|_| FileStashStoreError::Unavailable)?;
        let file = open_regular(&self.blobs, blob_key)?;
        let size = file
            .metadata()
            .map_err(|_| FileStashStoreError::Unavailable)?
            .len();
        if size != expected_size {
            return Err(FileStashStoreError::Integrity);
        }
        Ok(OpenedBlob {
            file: tokio::fs::File::from_std(file),
            size,
            _download_permit: download_permit,
            _mcp_permit: mcp_permit,
        })
    }

    pub(crate) fn remove_blob(&self, blob_key: &str) -> Result<()> {
        validate_blob_key(blob_key)?;
        remove_regular_if_exists(&self.blobs, blob_key)?;
        sync_directory(&self.blobs)
    }

    pub(crate) async fn recover(&self) -> Result<()> {
        #[cfg(all(test, any(target_os = "linux", target_os = "android")))]
        if regular_size(&self.tmp, "recovery.pause")?.is_some() {
            TEST_RECOVERY_RESUME.notified().await;
            remove_regular_if_exists(&self.tmp, "recovery.pause")?;
        }
        let page_size = self.limits.janitor_batch_size;
        let mut after = self.store.begin_recovery().await?;
        loop {
            let page = self
                .store
                .pending_for_recovery(after.clone(), page_size)
                .await?;
            if page.is_empty() {
                break;
            }
            after = page
                .last()
                .map(|item| item.upload_id.clone())
                .unwrap_or_default();
            for pending in page {
                if let Err(error) = self.reconcile_pending(pending).await {
                    return Err(error);
                }
            }
            self.store.checkpoint_recovery(after.clone()).await?;
            tokio::task::yield_now().await;
        }
        self.store.complete_recovery().await
    }

    pub(super) async fn cleanup_after_recovery(&self) -> Result<()> {
        let page_size = self.limits.janitor_batch_size;
        let mut after = String::new();
        loop {
            let page = self.store.committed_blob_keys(after, page_size).await?;
            if page.is_empty() {
                break;
            }
            after = page.last().map(|item| item.0.clone()).unwrap_or_default();
            for (name, expected) in page {
                validate_blob_key(&name)?;
                if regular_size_async(Arc::clone(&self.blobs), name).await? != Some(expected) {
                    return Err(FileStashStoreError::Integrity);
                }
            }
            tokio::task::yield_now().await;
        }
        let active = self
            .active_uploads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        remove_unreferenced_tmp(Arc::clone(&self.tmp), page_size, active).await?;
        self.remove_orphan_blobs().await?;
        Ok(())
    }

    pub(crate) async fn cleanup_expired(&self) -> Result<usize> {
        let expired = self
            .store
            .expired_pending(unix_now(), self.limits.janitor_batch_size)
            .await?;
        for pending in &expired {
            if self
                .active_uploads
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains(&pending.upload_id)
            {
                continue;
            }
            self.rollback_pending(pending).await?;
        }
        Ok(expired.len())
    }

    pub(super) fn close_store(&self) {
        self.store.close();
    }

    async fn reconcile_pending(&self, pending: PendingRecovery) -> Result<()> {
        validate_blob_key(&pending.upload_id)?;
        let temp_name = format!("{}.part", pending.upload_id);
        remove_regular_async(Arc::clone(&self.tmp), temp_name).await?;
        match pending.state.as_str() {
            "pending" => {
                remove_regular_async(Arc::clone(&self.blobs), pending.upload_id.clone()).await?;
                self.store.cancel_upload(pending.upload_id).await
            }
            "blob_published" => {
                if regular_size_async(Arc::clone(&self.blobs), pending.upload_id.clone()).await?
                    != Some(pending.reserved_bytes)
                {
                    return Err(FileStashStoreError::Integrity);
                }
                self.store
                    .commit_upload(pending.upload_id)
                    .await
                    .map(|_| ())
            }
            _ => Err(FileStashStoreError::Integrity),
        }
    }

    async fn rollback_pending(&self, pending: &PendingRecovery) -> Result<()> {
        remove_regular_if_exists(&self.tmp, &format!("{}.part", pending.upload_id))?;
        remove_regular_if_exists(&self.blobs, &pending.upload_id)?;
        self.store.cancel_upload(pending.upload_id.clone()).await
    }

    async fn acquire_upload(&self, owner: &str) -> Result<UploadAdmission> {
        let deadline = Duration::from_millis(self.limits.database_deadline_ms);
        let instance =
            tokio::time::timeout(deadline, Arc::clone(&self.instance_uploads).acquire_owned())
                .await
                .map_err(|_| FileStashStoreError::Busy)?
                .map_err(|_| FileStashStoreError::Unavailable)?;
        let principal = {
            let mut permits = self
                .principal_uploads
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            permits.retain(|_, permit| permit.strong_count() > 0);
            match permits.get(owner).and_then(Weak::upgrade) {
                Some(permit) => permit,
                None => {
                    let permit = Arc::new(Semaphore::new(
                        self.limits.max_concurrent_uploads_per_principal,
                    ));
                    permits.insert(owner.to_owned(), Arc::downgrade(&permit));
                    permit
                }
            }
        };
        let principal = tokio::time::timeout(deadline, principal.acquire_owned())
            .await
            .map_err(|_| FileStashStoreError::Busy)?
            .map_err(|_| FileStashStoreError::Unavailable)?;
        Ok(UploadAdmission {
            _instance: instance,
            _principal: principal,
            active: None,
        })
    }

    async fn cleanup_failed_upload(&self, temp_name: &str, upload_id: &str) {
        let temp_removed = remove_regular_if_exists(&self.tmp, temp_name).is_ok();
        let blob_removed = remove_regular_if_exists(&self.blobs, upload_id).is_ok();
        if temp_removed && blob_removed {
            if let Err(error) = self.store.cancel_upload(upload_id.to_owned()).await {
                tracing::warn!(error_kind = %error, cleanup_stage = "metadata_cancel", "file stash upload cleanup deferred");
                if let Err(schedule_error) =
                    self.store.expire_upload_now(upload_id.to_owned()).await
                {
                    tracing::warn!(error_kind = %schedule_error, cleanup_stage = "schedule_retry", "file stash upload cleanup retry scheduling failed");
                }
            }
        } else {
            tracing::warn!(
                temp_removed,
                blob_removed,
                cleanup_stage = "filesystem",
                "file stash upload cleanup deferred"
            );
            if let Err(error) = self.store.expire_upload_now(upload_id.to_owned()).await {
                tracing::warn!(error_kind = %error, cleanup_stage = "schedule_retry", "file stash upload cleanup retry scheduling failed");
            }
        }
    }

    async fn remove_orphan_blobs(&self) -> Result<()> {
        let directory = Arc::clone(&self.blobs);
        let names = tokio::task::spawn_blocking(move || directory_names(&directory))
            .await
            .map_err(|_| FileStashStoreError::Unavailable)??;
        let mut batch = Vec::with_capacity(self.limits.janitor_batch_size);
        for name in names {
            validate_blob_key(&name)?;
            batch.push(name);
            if batch.len() == self.limits.janitor_batch_size {
                self.remove_orphan_batch(std::mem::take(&mut batch)).await?;
            }
        }
        self.remove_orphan_batch(batch).await?;
        Ok(())
    }

    async fn remove_orphan_batch(&self, batch: Vec<String>) -> Result<()> {
        let committed = self.store.committed_blob_membership(batch.clone()).await?;
        for name in batch {
            let active = self
                .active_uploads
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains(&name);
            if !committed.contains(&name) && !active {
                remove_regular_async(Arc::clone(&self.blobs), name).await?;
            }
        }
        tokio::task::yield_now().await;
        Ok(())
    }
}

async fn remove_regular_async(directory: Arc<File>, name: String) -> Result<()> {
    tokio::task::spawn_blocking(move || remove_regular_if_exists(&directory, &name))
        .await
        .map_err(|_| FileStashStoreError::Unavailable)?
}

async fn regular_size_async(directory: Arc<File>, name: String) -> Result<Option<u64>> {
    tokio::task::spawn_blocking(move || regular_size(&directory, &name))
        .await
        .map_err(|_| FileStashStoreError::Unavailable)?
}

pub(crate) struct UploadAdmission {
    _instance: OwnedSemaphorePermit,
    _principal: OwnedSemaphorePermit,
    active: Option<(String, Arc<Mutex<HashSet<String>>>)>,
}

impl UploadAdmission {
    fn bind(&mut self, id: String, active: Arc<Mutex<HashSet<String>>>) {
        self.active = Some((id, active));
    }
}

impl Drop for UploadAdmission {
    fn drop(&mut self) {
        if let Some((id, active)) = &self.active {
            active
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(id);
        }
    }
}

pub(crate) struct OpenedBlob {
    pub(crate) file: tokio::fs::File,
    pub(crate) size: u64,
    _download_permit: OwnedSemaphorePermit,
    _mcp_permit: Option<OwnedSemaphorePermit>,
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .try_into()
        .unwrap_or(i64::MAX)
}

fn validate_blob_key(name: &str) -> Result<()> {
    ulid::Ulid::from_string(name)
        .map(|_| ())
        .map_err(|_| FileStashStoreError::Integrity)
}

fn validate_child_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || name == "."
        || name == ".."
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.')
    {
        return Err(FileStashStoreError::Integrity);
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn create_regular_exclusive(directory: &File, name: &str) -> Result<File> {
    validate_child_name(name)?;
    use rustix::fs::{Mode, OFlags, openat};
    let fd = openat(
        directory,
        name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(|error| match error {
        rustix::io::Errno::EXIST => FileStashStoreError::Integrity,
        _ => FileStashStoreError::Unavailable,
    })?;
    let file = File::from(fd);
    validate_regular(&file)?;
    Ok(file)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn publish_exclusive(from: &File, old: &str, to: &File, new: &str) -> Result<()> {
    validate_child_name(old)?;
    validate_blob_key(new)?;
    use rustix::fs::{RenameFlags, renameat_with};
    renameat_with(from, old, to, new, RenameFlags::NOREPLACE).map_err(|error| match error {
        rustix::io::Errno::EXIST => FileStashStoreError::Integrity,
        _ => FileStashStoreError::Unavailable,
    })
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn regular_size(directory: &File, name: &str) -> Result<Option<u64>> {
    validate_child_name(name)?;
    use rustix::fs::{Mode, OFlags, openat};
    let fd = match openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(_) => return Err(FileStashStoreError::Integrity),
    };
    let file = File::from(fd);
    validate_regular(&file)?;
    Ok(Some(
        file.metadata()
            .map_err(|_| FileStashStoreError::Unavailable)?
            .len(),
    ))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn open_regular(directory: &File, name: &str) -> Result<File> {
    validate_blob_key(name)?;
    use rustix::fs::{Mode, OFlags, openat};
    let fd = openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| match error {
        rustix::io::Errno::NOENT => FileStashStoreError::Integrity,
        _ => FileStashStoreError::Unavailable,
    })?;
    let file = File::from(fd);
    validate_regular(&file)?;
    Ok(file)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn remove_regular_if_exists(directory: &File, name: &str) -> Result<()> {
    validate_child_name(name)?;
    use rustix::fs::{AtFlags, unlinkat};
    if regular_size(directory, name)?.is_none() {
        return Ok(());
    }
    #[cfg(test)]
    {
        let mut injected = FAIL_UNLINK_NAME
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if injected.as_deref() == Some(name) {
            *injected = None;
            return Err(FileStashStoreError::Unavailable);
        }
    }
    unlinkat(directory, name, AtFlags::empty()).map_err(|_| FileStashStoreError::Unavailable)
}

#[cfg(all(test, any(target_os = "linux", target_os = "android")))]
static FAIL_UNLINK_NAME: std::sync::LazyLock<Mutex<Option<String>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

#[cfg(all(test, any(target_os = "linux", target_os = "android")))]
pub(super) static TEST_RECOVERY_RESUME: tokio::sync::Notify = tokio::sync::Notify::const_new();

#[cfg(any(target_os = "linux", target_os = "android"))]
async fn remove_unreferenced_tmp(
    directory: Arc<File>,
    batch_size: usize,
    active: HashSet<String>,
) -> Result<()> {
    let active = Arc::new(active);
    let listed = Arc::clone(&directory);
    let names = tokio::task::spawn_blocking(move || directory_names(&listed))
        .await
        .map_err(|_| FileStashStoreError::Unavailable)??;
    for batch in names.chunks(batch_size.max(1)) {
        let directory = Arc::clone(&directory);
        let active = Arc::clone(&active);
        let batch = batch.to_vec();
        tokio::task::spawn_blocking(move || {
            for name in batch {
                validate_child_name(&name)?;
                if name
                    .strip_suffix(".part")
                    .is_some_and(|upload_id| active.contains(upload_id))
                {
                    continue;
                }
                if name
                    .strip_suffix(".part")
                    .is_some_and(|upload_id| active.contains(upload_id))
                {
                    continue;
                }
                remove_regular_if_exists(&directory, &name)?;
            }
            sync_directory(&directory)
        })
        .await
        .map_err(|_| FileStashStoreError::Unavailable)??;
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn directory_names(directory: &File) -> Result<Vec<String>> {
    let entries =
        rustix::fs::Dir::read_from(directory).map_err(|_| FileStashStoreError::Unavailable)?;
    entries
        .filter_map(|entry| match entry {
            Ok(entry) => {
                let name = entry.file_name().to_string_lossy().into_owned();
                (name != "." && name != "..").then_some(Ok(name))
            }
            Err(_) => Some(Err(FileStashStoreError::Unavailable)),
        })
        .collect()
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn directory_names(_: &File) -> Result<Vec<String>> {
    Err(FileStashStoreError::Unavailable)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn validate_regular(file: &File) -> Result<()> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    let metadata = file
        .metadata()
        .map_err(|_| FileStashStoreError::Unavailable)?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o777 != 0o600
    {
        return Err(FileStashStoreError::Integrity);
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn sync_directory(directory: &File) -> Result<()> {
    directory
        .sync_all()
        .map_err(|_| FileStashStoreError::Unavailable)
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn create_regular_exclusive(_: &File, _: &str) -> Result<File> {
    Err(FileStashStoreError::Unavailable)
}
#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn publish_exclusive(_: &File, _: &str, _: &File, _: &str) -> Result<()> {
    Err(FileStashStoreError::Unavailable)
}
#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn regular_size(_: &File, _: &str) -> Result<Option<u64>> {
    Err(FileStashStoreError::Unavailable)
}
#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn open_regular(_: &File, _: &str) -> Result<File> {
    Err(FileStashStoreError::Unavailable)
}
#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn remove_regular_if_exists(_: &File, _: &str) -> Result<()> {
    Err(FileStashStoreError::Unavailable)
}
#[cfg(not(any(target_os = "linux", target_os = "android")))]
async fn remove_unreferenced_tmp(_: Arc<File>, _: usize, _: HashSet<String>) -> Result<()> {
    Err(FileStashStoreError::Unavailable)
}
#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn sync_directory(_: &File) -> Result<()> {
    Err(FileStashStoreError::Unavailable)
}

#[cfg(all(test, any(target_os = "linux", target_os = "android")))]
mod tests {
    use super::*;
    use crate::file_stash::StashUsage;
    use std::io::Write as _;

    fn root(temp: &tempfile::TempDir) -> std::path::PathBuf {
        std::fs::canonicalize(temp.path()).unwrap().join("stash")
    }

    fn preferences() -> FileStashPreferences {
        FileStashPreferences {
            max_file_bytes: 16,
            principal_quota_bytes: 16,
            instance_quota_bytes: 32,
            upload_idle_seconds: 1,
            upload_total_seconds: 2,
            janitor_interval_seconds: 3_600,
            ..FileStashPreferences::default()
        }
    }

    #[tokio::test]
    async fn exact_upload_publishes_and_body_mismatch_releases_everything() {
        let temp = tempfile::TempDir::new().unwrap();
        let runtime =
            super::super::FileStashRuntime::initialize_with_preferences(root(&temp), preferences())
                .await;
        let blobs = runtime.blob_store().await.unwrap();
        let (reservation, admission) = blobs
            .reserve_for_owner("owner", "a".into(), "a".into(), 3)
            .await
            .unwrap();
        let file_id = blobs
            .write_reserved(
                reservation,
                admission,
                &b"abc"[..],
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            runtime
                .store()
                .await
                .unwrap()
                .usage("owner".into())
                .await
                .unwrap()
                .committed_bytes,
            3
        );
        assert_eq!(regular_size(&blobs.blobs, &file_id).unwrap(), Some(3));

        let (reservation, admission) = blobs
            .reserve_for_owner("owner", "b".into(), "b".into(), 4)
            .await
            .unwrap();
        let failed_id = reservation.upload_id.clone();
        assert!(matches!(
            blobs
                .write_reserved(
                    reservation,
                    admission,
                    &b"abc"[..],
                    CancellationToken::new()
                )
                .await,
            Err(FileStashStoreError::LengthMismatch)
        ));
        assert_eq!(regular_size(&blobs.blobs, &failed_id).unwrap(), None);
        let store = runtime.store().await.unwrap();
        assert_eq!(store.usage("owner".into()).await.unwrap().reserved_bytes, 0);
    }

    #[tokio::test]
    async fn failed_metadata_cancel_remains_durable_for_janitor_retry() {
        let temp = tempfile::TempDir::new().unwrap();
        let runtime =
            super::super::FileStashRuntime::initialize_with_preferences(root(&temp), preferences())
                .await;
        let blobs = runtime.blob_store().await.unwrap();
        let store = runtime.store().await.unwrap();
        let (reservation, admission) = blobs
            .reserve_for_owner("owner", "a".into(), "a".into(), 2)
            .await
            .unwrap();
        super::super::store::inject_cancel_failure(reservation.upload_id.clone());
        assert!(matches!(
            blobs
                .write_reserved(reservation, admission, &b"x"[..], CancellationToken::new())
                .await,
            Err(FileStashStoreError::LengthMismatch)
        ));
        assert_eq!(store.usage("owner".into()).await.unwrap().reserved_bytes, 2);
        blobs.cleanup_expired().await.unwrap();
        assert_eq!(store.usage("owner".into()).await.unwrap().reserved_bytes, 0);
    }

    #[tokio::test]
    async fn cleanup_cancels_metadata_even_when_unlink_fails() {
        let temp = tempfile::TempDir::new().unwrap();
        let runtime =
            super::super::FileStashRuntime::initialize_with_preferences(root(&temp), preferences())
                .await;
        let blobs = runtime.blob_store().await.unwrap();
        let (reservation, admission) = blobs
            .reserve_for_owner("owner", "a".into(), "a".into(), 2)
            .await
            .unwrap();
        *FAIL_UNLINK_NAME.lock().unwrap() = Some(format!("{}.part", reservation.upload_id));
        assert!(matches!(
            blobs
                .write_reserved(reservation, admission, &b"x"[..], CancellationToken::new())
                .await,
            Err(FileStashStoreError::LengthMismatch)
        ));
        let store = runtime.store().await.unwrap();
        assert_eq!(store.usage("owner".into()).await.unwrap().reserved_bytes, 2);
        blobs.cleanup_expired().await.unwrap();
        assert_eq!(store.usage("owner".into()).await.unwrap().reserved_bytes, 0);
    }

    #[tokio::test]
    async fn cancellation_before_and_during_transfer_never_publishes() {
        let temp = tempfile::TempDir::new().unwrap();
        let runtime =
            super::super::FileStashRuntime::initialize_with_preferences(root(&temp), preferences())
                .await;
        let blobs = runtime.blob_store().await.unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let (reservation, admission) = blobs
            .reserve_for_owner("owner", "a".into(), "a".into(), 2)
            .await
            .unwrap();
        assert!(
            blobs
                .write_reserved(reservation, admission, &b"xx"[..], cancel)
                .await
                .is_err()
        );

        let (reservation, admission) = blobs
            .reserve_for_owner("owner", "b".into(), "b".into(), 2)
            .await
            .unwrap();
        let id = reservation.upload_id.clone();
        let cancel = CancellationToken::new();
        let triggered = cancel.clone();
        let (mut writer, reader) = tokio::io::duplex(8);
        writer.write_all(b"x").await.unwrap();
        let task_blobs = blobs.clone();
        let task = tokio::spawn(async move {
            task_blobs
                .write_reserved(reservation, admission, reader, cancel)
                .await
        });
        tokio::task::yield_now().await;
        triggered.cancel();
        assert!(task.await.unwrap().is_err());
        assert_eq!(regular_size(&blobs.blobs, &id).unwrap(), None);
        assert_eq!(
            runtime
                .store()
                .await
                .unwrap()
                .usage("owner".into())
                .await
                .unwrap()
                .reserved_bytes,
            0
        );
    }

    #[tokio::test]
    async fn mcp_reads_consume_global_download_capacity_too() {
        let temp = tempfile::TempDir::new().unwrap();
        let mut limits = preferences();
        limits.max_concurrent_downloads = 1;
        limits.max_concurrent_mcp_reads = 1;
        let runtime =
            super::super::FileStashRuntime::initialize_with_preferences(root(&temp), limits).await;
        let blobs = runtime.blob_store().await.unwrap();
        let (reservation, admission) = blobs
            .reserve_for_owner("owner", "a".into(), "a".into(), 1)
            .await
            .unwrap();
        let id = blobs
            .write_reserved(reservation, admission, &b"x"[..], CancellationToken::new())
            .await
            .unwrap();
        let held = blobs.open_blob(&id, 1, true).await.unwrap();
        assert!(matches!(
            blobs.open_blob(&id, 1, false).await,
            Err(FileStashStoreError::Busy)
        ));
        drop(held);
        let held = blobs.open_blob(&id, 1, false).await.unwrap();
        assert!(matches!(
            blobs.open_blob(&id, 1, true).await,
            Err(FileStashStoreError::Busy)
        ));
        drop(held);
    }

    #[tokio::test]
    async fn oversized_mcp_read_is_rejected_before_blob_open_or_permit_use() {
        let temp = tempfile::TempDir::new().unwrap();
        let mut limits = preferences();
        limits.max_mcp_read_bytes = 1;
        let runtime =
            super::super::FileStashRuntime::initialize_with_preferences(root(&temp), limits).await;
        let blobs = runtime.blob_store().await.unwrap();
        let missing = ulid::Ulid::new().to_string();
        assert!(matches!(
            blobs.open_blob(&missing, 2, true).await,
            Err(FileStashStoreError::QuotaExceeded)
        ));
    }

    #[tokio::test]
    async fn per_principal_upload_admission_is_bounded() {
        let temp = tempfile::TempDir::new().unwrap();
        let runtime =
            super::super::FileStashRuntime::initialize_with_preferences(root(&temp), preferences())
                .await;
        let blobs = runtime.blob_store().await.unwrap();
        let (_first, first_admission) = blobs
            .reserve_for_owner("owner", "a".into(), "a".into(), 1)
            .await
            .unwrap();
        let (_second, second_admission) = blobs
            .reserve_for_owner("owner", "b".into(), "b".into(), 1)
            .await
            .unwrap();
        assert!(matches!(
            blobs
                .reserve_for_owner("owner", "c".into(), "c".into(), 1)
                .await,
            Err(FileStashStoreError::Busy)
        ));
        drop((first_admission, second_admission));
    }

    #[tokio::test]
    async fn janitor_does_not_reap_an_active_upload_lease() {
        let temp = tempfile::TempDir::new().unwrap();
        let runtime =
            super::super::FileStashRuntime::initialize_with_preferences(root(&temp), preferences())
                .await;
        let blobs = runtime.blob_store().await.unwrap();
        let store = runtime.store().await.unwrap();
        let (reservation, admission) = blobs
            .reserve_for_owner("owner", "a".into(), "a".into(), 1)
            .await
            .unwrap();
        let id = reservation.upload_id.clone();
        store
            .with_connection(move |connection| {
                connection
                    .execute(
                        "UPDATE pending_uploads SET expires_at=0 WHERE upload_id=?1",
                        [&id],
                    )
                    .map_err(FileStashStoreError::sqlite)?;
                Ok(())
            })
            .await
            .unwrap();
        blobs.cleanup_expired().await.unwrap();
        assert_eq!(store.usage("owner".into()).await.unwrap().reserved_bytes, 1);
        drop(admission);
        blobs.cleanup_expired().await.unwrap();
        assert_eq!(store.usage("owner".into()).await.unwrap().reserved_bytes, 0);
    }

    #[tokio::test]
    async fn integrity_scrub_does_not_reap_a_published_active_upload() {
        use std::io::Write;

        let temp = tempfile::TempDir::new().unwrap();
        let runtime =
            super::super::FileStashRuntime::initialize_with_preferences(root(&temp), preferences())
                .await;
        let blobs = runtime.blob_store().await.unwrap();
        let (reservation, admission) = blobs
            .reserve_for_owner("owner", "a".into(), "a".into(), 1)
            .await
            .unwrap();
        let temp_name = format!("{}.part", reservation.upload_id);
        let mut file = create_regular_exclusive(&blobs.tmp, &temp_name).unwrap();
        file.write_all(b"x").unwrap();
        file.sync_all().unwrap();
        drop(file);
        remove_unreferenced_tmp(
            &blobs.tmp,
            10,
            CancellationToken::new(),
            Arc::clone(&blobs.active_uploads),
        )
        .await
        .unwrap();
        assert_eq!(regular_size(&blobs.tmp, &temp_name).unwrap(), Some(1));
        publish_exclusive(&blobs.tmp, &temp_name, &blobs.blobs, &reservation.upload_id).unwrap();

        blobs
            .remove_orphan_batch(vec![reservation.upload_id.clone()])
            .await
            .unwrap();

        assert_eq!(
            regular_size(&blobs.blobs, &reservation.upload_id).unwrap(),
            Some(1)
        );
        drop(admission);
    }

    #[tokio::test]
    async fn restart_completes_blob_published_and_rolls_back_pending_boundaries() {
        let temp = tempfile::TempDir::new().unwrap();
        let stash_root = root(&temp);
        let runtime = super::super::FileStashRuntime::initialize_with_preferences(
            stash_root.clone(),
            preferences(),
        )
        .await;
        let blobs = runtime.blob_store().await.unwrap();
        let store = runtime.store().await.unwrap();
        // This test manually constructs crash-recovery boundaries outside the
        // upload admission path, so stop the live janitor before injecting
        // those otherwise unreachable intermediate states.
        runtime.stop_janitor_for_test().await;
        let published = store
            .reserve_upload(
                "owner".into(),
                "a".into(),
                "a".into(),
                3,
                i64::MAX,
                16,
                32,
                10,
            )
            .await
            .unwrap();
        let temp_name = format!("{}.part", published.upload_id);
        let mut file = create_regular_exclusive(&blobs.tmp, &temp_name).unwrap();
        file.write_all(b"abc").unwrap();
        file.sync_all().unwrap();
        drop(file);
        publish_exclusive(&blobs.tmp, &temp_name, &blobs.blobs, &published.upload_id).unwrap();
        sync_directory(&blobs.blobs).unwrap();
        store
            .mark_blob_published(published.upload_id.clone())
            .await
            .unwrap();

        let rolled_back = store
            .reserve_upload(
                "owner".into(),
                "b".into(),
                "b".into(),
                2,
                i64::MAX,
                16,
                32,
                10,
            )
            .await
            .unwrap();
        let mut temp_file =
            create_regular_exclusive(&blobs.tmp, &format!("{}.part", rolled_back.upload_id))
                .unwrap();
        temp_file.write_all(b"x").unwrap();
        drop(temp_file);
        runtime.shutdown().await;

        let restarted =
            super::super::FileStashRuntime::initialize_with_preferences(stash_root, preferences())
                .await;
        assert_eq!(
            restarted.wait_for_recovery().await,
            super::super::FileStashStatus::Ready
        );
        let usage = restarted
            .store()
            .await
            .unwrap()
            .usage("owner".into())
            .await
            .unwrap();
        assert_eq!(
            usage,
            StashUsage {
                committed_bytes: 3,
                reserved_bytes: 0,
                live_files: 1,
                owned_shared_file_count: 0,
            }
        );
        let restarted_blobs = restarted.blob_store().await.unwrap();
        assert_eq!(
            regular_size(&restarted_blobs.blobs, &rolled_back.upload_id).unwrap(),
            None
        );
        assert_eq!(
            regular_size(
                &restarted_blobs.tmp,
                &format!("{}.part", rolled_back.upload_id)
            )
            .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn restart_fails_closed_when_published_length_disagrees() {
        let temp = tempfile::TempDir::new().unwrap();
        let stash_root = root(&temp);
        let runtime = super::super::FileStashRuntime::initialize_with_preferences(
            stash_root.clone(),
            preferences(),
        )
        .await;
        let blobs = runtime.blob_store().await.unwrap();
        let store = runtime.store().await.unwrap();
        let pending = store
            .reserve_upload(
                "owner".into(),
                "a".into(),
                "a".into(),
                3,
                i64::MAX,
                16,
                32,
                10,
            )
            .await
            .unwrap();
        let mut file = create_regular_exclusive(&blobs.blobs, &pending.upload_id).unwrap();
        file.write_all(b"xx").unwrap();
        file.sync_all().unwrap();
        drop(file);
        store.mark_blob_published(pending.upload_id).await.unwrap();
        runtime.shutdown().await;
        assert_eq!(
            super::super::FileStashRuntime::initialize_with_preferences(stash_root, preferences())
                .await
                .wait_for_recovery()
                .await,
            super::super::FileStashStatus::Blocked(super::super::FileStashBlockedReason::Corrupt)
        );
    }

    #[tokio::test]
    async fn revoke_wins_before_final_authorization_but_an_authorized_open_may_finish() {
        let temp = tempfile::TempDir::new().unwrap();
        let runtime =
            super::super::FileStashRuntime::initialize_with_preferences(root(&temp), preferences())
                .await;
        let blobs = runtime.blob_store().await.unwrap();
        let store = runtime.store().await.unwrap();
        let (reservation, admission) = blobs
            .reserve_for_owner("owner", "shared.txt".into(), "shared.txt".into(), 3)
            .await
            .unwrap();
        let file_id = blobs
            .write_reserved(
                reservation,
                admission,
                &b"abc"[..],
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let grant = store
            .create_grant("owner".into(), file_id.clone(), "reader".into())
            .await
            .unwrap();

        let first_snapshot = store
            .authorized_file("reader".into(), file_id.clone())
            .await
            .unwrap();
        let opened = blobs
            .open_blob(&first_snapshot.blob_key, first_snapshot.size_bytes, false)
            .await
            .unwrap();
        let reached_open = Arc::new(tokio::sync::Barrier::new(2));
        let permit_recheck = Arc::new(tokio::sync::Barrier::new(2));
        let task_store = store.clone();
        let task_file_id = file_id.clone();
        let task_reached = Arc::clone(&reached_open);
        let task_permit = Arc::clone(&permit_recheck);
        let recheck = tokio::spawn(async move {
            let _opened = opened;
            task_reached.wait().await;
            task_permit.wait().await;
            task_store
                .authorized_file("reader".into(), task_file_id)
                .await
        });
        reached_open.wait().await;
        store
            .revoke_grant("owner".into(), file_id.clone(), grant.grant_id)
            .await
            .unwrap();
        permit_recheck.wait().await;
        assert!(matches!(
            recheck.await.unwrap(),
            Err(FileStashStoreError::NotFound)
        ));

        let owner = store
            .authorized_file("owner".into(), file_id.clone())
            .await
            .unwrap();
        let mut already_open = blobs
            .open_blob(&owner.blob_key, owner.size_bytes, false)
            .await
            .unwrap();
        let blob_key = store.delete_file("owner".into(), file_id).await.unwrap();
        blobs.remove_blob(&blob_key).unwrap();
        let mut bytes = Vec::new();
        already_open.file.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(bytes, b"abc");
    }
}
