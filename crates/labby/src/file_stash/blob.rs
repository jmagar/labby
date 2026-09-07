use super::store::{FileStashStore, FileStashStoreError, PendingRecovery, UploadReservation};
use crate::config::FileStashPreferences;
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, Weak},
    task::{Context, Poll},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, ReadBuf};
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
    mcp_read_bytes: Arc<Semaphore>,
    principal_uploads: Arc<Mutex<HashMap<String, Weak<Semaphore>>>>,
    principal_downloads: Arc<Mutex<HashMap<String, Weak<Semaphore>>>>,
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
            mcp_read_bytes: Arc::new(Semaphore::new(
                usize::try_from(limits.max_mcp_read_bytes).unwrap_or(usize::MAX),
            )),
            principal_uploads: Arc::new(Mutex::new(HashMap::new())),
            principal_downloads: Arc::new(Mutex::new(HashMap::new())),
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
            .reserve_upload_with_instance_limit(
                owner_id,
                display_name,
                collision_key,
                declared_bytes,
                expires_at,
                self.limits.principal_quota_bytes,
                self.limits.instance_quota_bytes,
                self.limits.max_live_files_per_principal,
                self.limits.max_live_files_per_instance,
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
        principal: &super::PrincipalId,
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
        let mcp_bytes = if mcp {
            Some(
                tokio::time::timeout(
                    Duration::from_millis(self.limits.database_deadline_ms),
                    Arc::clone(&self.mcp_read_bytes).acquire_many_owned(
                        u32::try_from(expected_size)
                            .map_err(|_| FileStashStoreError::QuotaExceeded)?,
                    ),
                )
                .await
                .map_err(|_| FileStashStoreError::Busy)?
                .map_err(|_| FileStashStoreError::Unavailable)?,
            )
        } else {
            None
        };
        let principal_download = {
            let mut permits = self
                .principal_downloads
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            permits.retain(|_, permit| permit.strong_count() > 0);
            match permits.get(principal.as_str()).and_then(Weak::upgrade) {
                Some(permit) => permit,
                None => {
                    // Preserve capacity for other callers even when one
                    // authenticated principal deliberately slow-reads.
                    let permit =
                        Arc::new(Semaphore::new(self.limits.max_concurrent_downloads.min(4)));
                    permits.insert(principal.as_str().to_owned(), Arc::downgrade(&permit));
                    permit
                }
            }
        };
        let principal_download = tokio::time::timeout(
            Duration::from_millis(self.limits.database_deadline_ms),
            principal_download.acquire_owned(),
        )
        .await
        .map_err(|_| FileStashStoreError::Busy)?
        .map_err(|_| FileStashStoreError::Unavailable)?;
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
        let admission = DownloadAdmission::new(
            download_permit,
            principal_download,
            mcp_permit,
            mcp_bytes,
            Duration::from_secs(self.limits.download_total_seconds),
        );
        let cancellation_wait = Box::pin(admission.cancel.clone().cancelled_owned());
        let idle_timeout = Duration::from_secs(self.limits.download_idle_seconds);
        Ok(OpenedBlob {
            file: tokio::fs::File::from_std(file),
            size,
            admission,
            cancellation_wait,
            idle_timeout,
            idle: Box::pin(tokio::time::sleep(idle_timeout)),
        })
    }

    pub(crate) fn remove_blob(&self, blob_key: &str) -> Result<()> {
        validate_blob_key(blob_key)?;
        remove_regular_if_exists(&self.blobs, blob_key)?;
        sync_directory(&self.blobs)
    }

    pub(crate) async fn recover_pending(&self) -> Result<()> {
        let page_size = self.limits.janitor_batch_size;
        let mut after = String::new();
        loop {
            let page = self.store.pending_for_recovery(after, page_size).await?;
            if page.is_empty() {
                break;
            }
            after = page
                .last()
                .map(|item| item.upload_id.clone())
                .unwrap_or_default();
            for pending in page {
                self.reconcile_pending(pending).await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn scrub_integrity(&self, cancel: CancellationToken) -> Result<()> {
        let page_size = self.limits.janitor_batch_size;
        let mut after = String::new();
        loop {
            if cancel.is_cancelled() {
                return Ok(());
            }
            let page = self.store.committed_blob_keys(after, page_size).await?;
            if page.is_empty() {
                break;
            }
            after = page.last().map(|item| item.0.clone()).unwrap_or_default();
            let blobs = Arc::clone(&self.blobs);
            tokio::task::spawn_blocking(move || {
                for (name, expected) in page {
                    validate_blob_key(&name)?;
                    if regular_size(&blobs, &name)? != Some(expected) {
                        return Err(FileStashStoreError::Integrity);
                    }
                }
                Ok(())
            })
            .await
            .map_err(|_| FileStashStoreError::Unavailable)??;
        }
        remove_unreferenced_tmp(
            &self.tmp,
            page_size,
            cancel.clone(),
            Arc::clone(&self.active_uploads),
        )
        .await?;
        self.remove_orphan_blobs(page_size, cancel).await?;
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

    async fn reconcile_pending(&self, pending: PendingRecovery) -> Result<()> {
        validate_blob_key(&pending.upload_id)?;
        let temp_name = format!("{}.part", pending.upload_id);
        let tmp = Arc::clone(&self.tmp);
        let blobs = Arc::clone(&self.blobs);
        let upload_id = pending.upload_id.clone();
        let state = pending.state.clone();
        let reserved_bytes = pending.reserved_bytes;
        tokio::task::spawn_blocking(move || {
            remove_regular_if_exists(&tmp, &temp_name)?;
            match state.as_str() {
                "pending" => remove_regular_if_exists(&blobs, &upload_id),
                "blob_published" => {
                    if regular_size(&blobs, &upload_id)? == Some(reserved_bytes) {
                        Ok(())
                    } else {
                        Err(FileStashStoreError::Integrity)
                    }
                }
                _ => Err(FileStashStoreError::Integrity),
            }
        })
        .await
        .map_err(|_| FileStashStoreError::Unavailable)??;
        match pending.state.as_str() {
            "pending" => self.store.cancel_upload(pending.upload_id).await,
            "blob_published" => self
                .store
                .commit_upload(pending.upload_id)
                .await
                .map(|_| ()),
            _ => Err(FileStashStoreError::Integrity),
        }
    }

    async fn rollback_pending(&self, pending: &PendingRecovery) -> Result<()> {
        let tmp = Arc::clone(&self.tmp);
        let blobs = Arc::clone(&self.blobs);
        let upload_id = pending.upload_id.clone();
        let temp_name = format!("{}.part", pending.upload_id);
        tokio::task::spawn_blocking(move || {
            remove_regular_if_exists(&tmp, &temp_name)?;
            remove_regular_if_exists(&blobs, &upload_id)
        })
        .await
        .map_err(|_| FileStashStoreError::Unavailable)??;
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

    async fn remove_orphan_blobs(
        &self,
        batch_size: usize,
        cancel: CancellationToken,
    ) -> Result<()> {
        let mut batches =
            stream_directory_names(Arc::clone(&self.blobs), true, batch_size, cancel.clone());
        loop {
            let batch = tokio::select! {
                () = cancel.cancelled() => return Ok(()),
                batch = batches.recv() => batch,
            };
            let Some(batch) = batch else { break };
            self.remove_orphan_batch(batch?).await?;
        }
        Ok(())
    }

    async fn remove_orphan_batch(&self, batch: Vec<String>) -> Result<()> {
        let committed = self.store.committed_blob_membership(batch.clone()).await?;
        let active = self
            .active_uploads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let orphans = batch
            .into_iter()
            .filter(|name| !committed.contains(name) && !active.contains(name))
            .collect::<Vec<_>>();
        let directory = Arc::clone(&self.blobs);
        tokio::task::spawn_blocking(move || {
            for name in orphans {
                remove_regular_if_exists(&directory, &name)?;
            }
            Ok(())
        })
        .await
        .map_err(|_| FileStashStoreError::Unavailable)?
    }
}

#[cfg(target_os = "linux")]
fn stream_directory_names(
    directory: Arc<File>,
    blob_names: bool,
    batch_size: usize,
    cancel: CancellationToken,
) -> tokio::sync::mpsc::Receiver<Result<Vec<String>>> {
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    tokio::task::spawn_blocking(move || {
        let produce = || -> Result<()> {
            let entries = rustix::fs::Dir::read_from(&directory)
                .map_err(|_| FileStashStoreError::Unavailable)?;
            let mut batch = Vec::with_capacity(batch_size);
            for entry in entries {
                if cancel.is_cancelled() {
                    return Ok(());
                }
                let entry = entry.map_err(|_| FileStashStoreError::Unavailable)?;
                let name = entry.file_name().to_string_lossy();
                if name == "." || name == ".." {
                    continue;
                }
                if blob_names {
                    validate_blob_key(&name)?;
                } else {
                    validate_child_name(&name)?;
                }
                batch.push(name.into_owned());
                if batch.len() == batch_size
                    && sender
                        .blocking_send(Ok(std::mem::take(&mut batch)))
                        .is_err()
                {
                    return Ok(());
                }
            }
            if !batch.is_empty() {
                let _unused = sender.blocking_send(Ok(batch));
            }
            Ok(())
        };
        if let Err(error) = produce() {
            let _unused = sender.blocking_send(Err(error));
        }
    });
    receiver
}

#[cfg(not(target_os = "linux"))]
fn stream_directory_names(
    _: Arc<File>,
    _: bool,
    _: usize,
    _: CancellationToken,
) -> tokio::sync::mpsc::Receiver<Result<Vec<String>>> {
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    let _unused = sender.try_send(Err(FileStashStoreError::Unavailable));
    receiver
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
    file: tokio::fs::File,
    pub(crate) size: u64,
    admission: DownloadAdmission,
    cancellation_wait: Pin<Box<dyn Future<Output = ()> + Send>>,
    pub(crate) idle_timeout: Duration,
    idle: Pin<Box<tokio::time::Sleep>>,
}

impl OpenedBlob {
    pub(crate) fn cancellation(&self) -> CancellationToken {
        self.admission.cancel.clone()
    }
}

impl AsyncRead for OpenedBlob {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if this.admission.cancel.is_cancelled()
            || this.cancellation_wait.as_mut().poll(cx).is_ready()
        {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "File Stash download exceeded its total deadline",
            )));
        }
        let before = buf.filled().len();
        match Pin::new(&mut this.file).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                if buf.filled().len() > before {
                    this.idle
                        .as_mut()
                        .reset(tokio::time::Instant::now() + this.idle_timeout);
                }
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => match this.idle.as_mut().poll(cx) {
                Poll::Ready(()) => Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "File Stash download exceeded its idle deadline",
                ))),
                Poll::Pending => Poll::Pending,
            },
        }
    }
}

struct DownloadAdmission {
    permits: Arc<Mutex<Option<DownloadPermits>>>,
    cancel: CancellationToken,
}

struct DownloadPermits {
    _instance: OwnedSemaphorePermit,
    _principal: OwnedSemaphorePermit,
    _mcp: Option<OwnedSemaphorePermit>,
    _mcp_bytes: Option<OwnedSemaphorePermit>,
}

impl DownloadAdmission {
    fn new(
        instance: OwnedSemaphorePermit,
        principal: OwnedSemaphorePermit,
        mcp: Option<OwnedSemaphorePermit>,
        mcp_bytes: Option<OwnedSemaphorePermit>,
        total: Duration,
    ) -> Self {
        let permits = Arc::new(Mutex::new(Some(DownloadPermits {
            _instance: instance,
            _principal: principal,
            _mcp: mcp,
            _mcp_bytes: mcp_bytes,
        })));
        let cancel = CancellationToken::new();
        let watchdog_permits = Arc::downgrade(&permits);
        let watchdog_cancel = cancel.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = tokio::time::sleep(total) => {
                    watchdog_cancel.cancel();
                    if let Some(permits) = watchdog_permits.upgrade() {
                        permits.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
                    }
                }
                () = watchdog_cancel.cancelled() => {}
            }
        });
        Self { permits, cancel }
    }
}

impl Drop for DownloadAdmission {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.permits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
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

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
fn publish_exclusive(from: &File, old: &str, to: &File, new: &str) -> Result<()> {
    validate_child_name(old)?;
    validate_blob_key(new)?;
    use rustix::fs::{RenameFlags, renameat_with};
    renameat_with(from, old, to, new, RenameFlags::NOREPLACE).map_err(|error| match error {
        rustix::io::Errno::EXIST => FileStashStoreError::Integrity,
        _ => FileStashStoreError::Unavailable,
    })
}

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
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

#[cfg(all(test, target_os = "linux"))]
static FAIL_UNLINK_NAME: std::sync::LazyLock<Mutex<Option<String>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

#[cfg(target_os = "linux")]
async fn remove_unreferenced_tmp(
    directory: &File,
    batch_size: usize,
    cancel: CancellationToken,
    active_uploads: Arc<Mutex<HashSet<String>>>,
) -> Result<()> {
    let directory = Arc::new(
        directory
            .try_clone()
            .map_err(|_| FileStashStoreError::Unavailable)?,
    );
    let mut batches =
        stream_directory_names(Arc::clone(&directory), false, batch_size, cancel.clone());
    loop {
        let batch = tokio::select! {
            () = cancel.cancelled() => return Ok(()),
            batch = batches.recv() => batch,
        };
        let Some(batch) = batch else { break };
        let names = batch?;
        let directory = Arc::clone(&directory);
        let batch_cancel = cancel.clone();
        let active = active_uploads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        tokio::task::spawn_blocking(move || {
            for name in names {
                if batch_cancel.is_cancelled() {
                    return Ok(());
                }
                if name
                    .strip_suffix(".part")
                    .is_some_and(|upload_id| active.contains(upload_id))
                {
                    continue;
                }
                remove_regular_if_exists(&directory, &name)?;
            }
            Ok(())
        })
        .await
        .map_err(|_| FileStashStoreError::Unavailable)??;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
fn sync_directory(directory: &File) -> Result<()> {
    directory
        .sync_all()
        .map_err(|_| FileStashStoreError::Unavailable)
}

#[cfg(not(target_os = "linux"))]
fn create_regular_exclusive(_: &File, _: &str) -> Result<File> {
    Err(FileStashStoreError::Unavailable)
}
#[cfg(not(target_os = "linux"))]
fn publish_exclusive(_: &File, _: &str, _: &File, _: &str) -> Result<()> {
    Err(FileStashStoreError::Unavailable)
}
#[cfg(not(target_os = "linux"))]
fn regular_size(_: &File, _: &str) -> Result<Option<u64>> {
    Err(FileStashStoreError::Unavailable)
}
#[cfg(not(target_os = "linux"))]
fn open_regular(_: &File, _: &str) -> Result<File> {
    Err(FileStashStoreError::Unavailable)
}
#[cfg(not(target_os = "linux"))]
fn remove_regular_if_exists(_: &File, _: &str) -> Result<()> {
    Err(FileStashStoreError::Unavailable)
}
#[cfg(not(target_os = "linux"))]
async fn remove_unreferenced_tmp(
    _: &File,
    _: usize,
    _: CancellationToken,
    _: Arc<Mutex<HashSet<String>>>,
) -> Result<()> {
    Err(FileStashStoreError::Unavailable)
}
#[cfg(not(target_os = "linux"))]
fn sync_directory(_: &File) -> Result<()> {
    Err(FileStashStoreError::Unavailable)
}

#[cfg(test)]
mod admission_tests {
    use super::*;

    #[tokio::test]
    async fn total_deadline_cancels_and_releases_all_download_permits() {
        let instance = Arc::new(Semaphore::new(1));
        let principal = Arc::new(Semaphore::new(1));
        let mcp = Arc::new(Semaphore::new(1));
        let admission = DownloadAdmission::new(
            Arc::clone(&instance).acquire_owned().await.unwrap(),
            Arc::clone(&principal).acquire_owned().await.unwrap(),
            Some(Arc::clone(&mcp).acquire_owned().await.unwrap()),
            None,
            Duration::from_millis(10),
        );
        assert_eq!(instance.available_permits(), 0);
        assert_eq!(principal.available_permits(), 0);
        assert_eq!(mcp.available_permits(), 0);

        tokio::time::timeout(Duration::from_secs(1), admission.cancel.cancelled())
            .await
            .unwrap();
        assert_eq!(instance.available_permits(), 1);
        assert_eq!(principal.available_permits(), 1);
        assert_eq!(mcp.available_permits(), 1);
    }
}

#[cfg(all(test, target_os = "linux"))]
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
    async fn directory_scrub_streams_bounded_batches_and_honors_cancellation() {
        let temp = tempfile::TempDir::new().unwrap();
        for index in 0..5 {
            std::fs::write(temp.path().join(format!("item{index}")), b"x").unwrap();
        }
        let directory = Arc::new(File::open(temp.path()).unwrap());
        let mut batches =
            stream_directory_names(Arc::clone(&directory), false, 2, CancellationToken::new());
        let mut total = 0;
        while let Some(batch) = batches.recv().await {
            let batch = batch.unwrap();
            assert!(batch.len() <= 2);
            total += batch.len();
        }
        assert_eq!(total, 5);

        let cancel = CancellationToken::new();
        cancel.cancel();
        let mut cancelled = stream_directory_names(directory, false, 2, cancel);
        assert!(cancelled.recv().await.is_none());
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
        let held = blobs
            .open_blob(&super::super::PrincipalId::for_test("owner"), &id, 1, true)
            .await
            .unwrap();
        assert!(matches!(
            blobs
                .open_blob(&super::super::PrincipalId::for_test("owner"), &id, 1, false)
                .await,
            Err(FileStashStoreError::Busy)
        ));
        drop(held);
        let held = blobs
            .open_blob(&super::super::PrincipalId::for_test("owner"), &id, 1, false)
            .await
            .unwrap();
        assert!(matches!(
            blobs
                .open_blob(&super::super::PrincipalId::for_test("owner"), &id, 1, true)
                .await,
            Err(FileStashStoreError::Busy)
        ));
        drop(held);
    }

    #[tokio::test]
    async fn one_principal_cannot_monopolize_instance_download_capacity() {
        let temp = tempfile::TempDir::new().unwrap();
        let mut limits = preferences();
        limits.max_concurrent_downloads = 8;
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
        let owner = super::super::PrincipalId::for_test("owner");
        let other = super::super::PrincipalId::for_test("other");
        let mut held = Vec::new();
        for _ in 0..4 {
            held.push(blobs.open_blob(&owner, &id, 1, false).await.unwrap());
        }
        assert!(matches!(
            blobs.open_blob(&owner, &id, 1, false).await,
            Err(FileStashStoreError::Busy)
        ));
        held.push(blobs.open_blob(&other, &id, 1, false).await.unwrap());
        assert_eq!(held.len(), 5);
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
            blobs
                .open_blob(
                    &super::super::PrincipalId::for_test("owner"),
                    &missing,
                    2,
                    true
                )
                .await,
            Err(FileStashStoreError::QuotaExceeded)
        ));
    }

    #[tokio::test]
    async fn concurrent_mcp_reads_share_a_weighted_byte_budget() {
        let temp = tempfile::TempDir::new().unwrap();
        let mut limits = preferences();
        limits.max_mcp_read_bytes = 3;
        limits.max_concurrent_mcp_reads = 2;
        let runtime =
            super::super::FileStashRuntime::initialize_with_preferences(root(&temp), limits).await;
        let blobs = runtime.blob_store().await.unwrap();
        let mut ids = Vec::new();
        for name in ["a", "b"] {
            let (reservation, admission) = blobs
                .reserve_for_owner("owner", name.into(), name.into(), 2)
                .await
                .unwrap();
            ids.push(
                blobs
                    .write_reserved(reservation, admission, &b"xx"[..], CancellationToken::new())
                    .await
                    .unwrap(),
            );
        }
        let owner = super::super::PrincipalId::for_test("owner");
        let held = blobs.open_blob(&owner, &ids[0], 2, true).await.unwrap();
        assert!(matches!(
            blobs.open_blob(&owner, &ids[1], 2, true).await,
            Err(FileStashStoreError::Busy)
        ));
        drop(held);
        blobs.open_blob(&owner, &ids[1], 2, true).await.unwrap();
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
            restarted.status().await,
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
                .status()
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
            .open_blob(
                &super::super::PrincipalId::for_test("owner"),
                &first_snapshot.blob_key,
                first_snapshot.size_bytes,
                false,
            )
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
            .open_blob(
                &super::super::PrincipalId::for_test("owner"),
                &owner.blob_key,
                owner.size_bytes,
                false,
            )
            .await
            .unwrap();
        let blob_key = store.delete_file("owner".into(), file_id).await.unwrap();
        blobs.remove_blob(&blob_key).unwrap();
        let mut bytes = Vec::new();
        already_open.file.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(bytes, b"abc");
    }
}
