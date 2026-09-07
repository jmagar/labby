use super::schema;
use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension, TransactionBehavior, params};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Semaphore;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const ADMISSION_TIMEOUT: Duration = Duration::from_millis(100);
pub(super) type Result<T> = std::result::Result<T, FileStashStoreError>;
#[derive(Debug, thiserror::Error)]
pub(crate) enum FileStashStoreError {
    #[error("File Stash is busy")]
    Busy,
    #[error("File Stash quota is exhausted")]
    QuotaExceeded,
    #[error("File Stash name already exists")]
    Conflict,
    #[error("File Stash object was not found")]
    NotFound,
    #[error("File Stash upload length does not match its reservation")]
    LengthMismatch,
    #[error("File Stash metadata and blob state do not agree")]
    Integrity,
    #[error("File Stash schema {0} is newer than this binary")]
    NewerSchema(i64),
    #[error("File Stash metadata is corrupt")]
    Corrupt,
    #[error("File Stash database and blob snapshot markers do not match")]
    BackupMismatch,
    #[error("File Stash storage is unavailable")]
    Unavailable,
}

#[derive(Clone, Debug)]
pub(crate) struct UploadReservation {
    pub(crate) upload_id: String,
    pub(crate) owner_principal_id: String,
    pub(crate) display_name: String,
    pub(crate) collision_key: String,
    pub(crate) reserved_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct StashUsage {
    pub(crate) committed_bytes: u64,
    pub(crate) reserved_bytes: u64,
    pub(crate) live_files: u64,
    pub(crate) owned_shared_file_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StashFile {
    pub(crate) file_id: String,
    pub(crate) display_name: String,
    pub(crate) size_bytes: u64,
    pub(crate) blob_key: String,
    pub(crate) created_at: i64,
    pub(crate) updated_at: i64,
    pub(crate) owned: bool,
}

impl StashFile {
    pub(crate) fn uri(&self) -> String {
        format!("stash://me/files/{}", self.file_id)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StashGrant {
    pub(crate) grant_id: String,
    pub(crate) file_id: String,
    pub(crate) grantee_principal_id: String,
    pub(crate) created_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StashCursor {
    pub(crate) created_at: i64,
    pub(crate) id: String,
}

#[derive(Clone, Debug)]
pub(crate) struct PendingRecovery {
    pub(crate) upload_id: String,
    pub(crate) state: String,
    pub(crate) reserved_bytes: u64,
}
impl FileStashStoreError {
    pub(super) fn sqlite(e: rusqlite::Error) -> Self {
        match &e {
            rusqlite::Error::SqliteFailure(c, _)
                if matches!(c.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) =>
            {
                Self::Busy
            }
            rusqlite::Error::SqliteFailure(c, _)
                if matches!(c.code, ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase) =>
            {
                Self::Corrupt
            }
            rusqlite::Error::SqliteFailure(c, _) if c.code == ErrorCode::ReadOnly => {
                Self::Unavailable
            }
            _ => Self::Unavailable,
        }
    }
}
#[derive(Clone)]
pub(crate) struct FileStashStore {
    connection: Arc<Mutex<Connection>>,
    admission: Arc<Semaphore>,
    queue: Arc<Semaphore>,
    admission_timeout: Duration,
    path: Arc<PathBuf>,
}
impl FileStashStore {
    pub(super) async fn open(path: PathBuf, snapshot_id: String) -> Result<Self> {
        Self::open_with_limits(path, snapshot_id, 64, ADMISSION_TIMEOUT).await
    }

    pub(super) async fn open_with_limits(
        path: PathBuf,
        snapshot_id: String,
        queue_capacity: usize,
        admission_timeout: Duration,
    ) -> Result<Self> {
        let p = path.clone();
        let connection = tokio::task::spawn_blocking(move || open_connection(&p, &snapshot_id))
            .await
            .map_err(|_| FileStashStoreError::Unavailable)??;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            admission: Arc::new(Semaphore::new(1)),
            queue: Arc::new(Semaphore::new(queue_capacity)),
            admission_timeout,
            path: Arc::new(path),
        })
    }
    pub(super) async fn with_connection<T: Send + 'static>(
        &self,
        op: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let queued = Arc::clone(&self.queue)
            .try_acquire_owned()
            .map_err(|error| match error {
                tokio::sync::TryAcquireError::Closed => FileStashStoreError::Unavailable,
                tokio::sync::TryAcquireError::NoPermits => FileStashStoreError::Busy,
            })?;
        let permit = tokio::time::timeout(
            self.admission_timeout,
            Arc::clone(&self.admission).acquire_owned(),
        )
        .await
        .map_err(|_| FileStashStoreError::Busy)?
        .map_err(|_| FileStashStoreError::Unavailable)?;
        let connection = Arc::clone(&self.connection);
        tokio::task::spawn_blocking(move || {
            let _queued = queued;
            let _permit = permit;
            let mut c = connection
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            op(&mut c)
        })
        .await
        .map_err(|_| FileStashStoreError::Unavailable)?
    }
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
    pub(super) async fn checkpoint(&self) -> Result<()> {
        self.with_connection(|connection| {
            connection
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                .map_err(FileStashStoreError::sqlite)
        })
        .await
    }
    pub(super) fn close(&self) {
        self.queue.close();
        self.admission.close();
    }

    pub(crate) async fn reserve_upload_with_instance_limit(
        &self,
        owner: String,
        display_name: String,
        collision_key: String,
        declared_bytes: u64,
        expires_at: i64,
        principal_quota: u64,
        instance_quota: u64,
        max_live_files: u32,
        max_instance_live_files: u32,
    ) -> Result<UploadReservation> {
        let upload_id = ulid::Ulid::new().to_string();
        self.with_connection(move |connection| {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(FileStashStoreError::sqlite)?;
            let (principal_committed, principal_reserved, live_files, pending_files): (i64, i64, i64, i64) = tx
                .query_row(
                    "SELECT COALESCE((SELECT SUM(size_bytes) FROM files WHERE owner_principal_id=?1 AND ready=1),0),COALESCE((SELECT SUM(reserved_bytes) FROM pending_uploads WHERE owner_principal_id=?1),0),COALESCE((SELECT COUNT(*) FROM files WHERE owner_principal_id=?1 AND ready=1),0),COALESCE((SELECT COUNT(*) FROM pending_uploads WHERE owner_principal_id=?1),0)",
                    [&owner],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .map_err(FileStashStoreError::sqlite)?;
            let (instance_used, instance_live_files): (i64, i64) = tx
                .query_row(
                    "SELECT COALESCE((SELECT SUM(size_bytes) FROM files WHERE ready=1),0)+COALESCE((SELECT SUM(reserved_bytes) FROM pending_uploads),0),COALESCE((SELECT COUNT(*) FROM files WHERE ready=1),0)+COALESCE((SELECT COUNT(*) FROM pending_uploads),0)",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(FileStashStoreError::sqlite)?;
            let declared = i64::try_from(declared_bytes).map_err(|_| FileStashStoreError::QuotaExceeded)?;
            if live_files.saturating_add(pending_files) >= i64::from(max_live_files)
                || instance_live_files >= i64::from(max_instance_live_files)
                || principal_committed.saturating_add(principal_reserved).saturating_add(declared)
                    > i64::try_from(principal_quota).unwrap_or(i64::MAX)
                || instance_used.saturating_add(declared)
                    > i64::try_from(instance_quota).unwrap_or(i64::MAX)
            {
                return Err(FileStashStoreError::QuotaExceeded);
            }
            let now = unix_now();
            tx.execute(
                "INSERT INTO pending_uploads(upload_id,owner_principal_id,display_name,collision_key,reserved_bytes,state,expires_at,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,'pending',?6,?7,?7)",
                params![upload_id, owner, display_name, collision_key, declared, expires_at, now],
            )
            .map_err(map_constraint)?;
            tx.commit().map_err(FileStashStoreError::sqlite)?;
            Ok(UploadReservation { upload_id, owner_principal_id: owner, display_name, collision_key, reserved_bytes: declared_bytes })
        }).await
    }

    #[cfg(test)]
    pub(crate) async fn reserve_upload(
        &self,
        owner: String,
        display_name: String,
        collision_key: String,
        declared_bytes: u64,
        expires_at: i64,
        principal_quota: u64,
        instance_quota: u64,
        max_live_files: u32,
    ) -> Result<UploadReservation> {
        self.reserve_upload_with_instance_limit(
            owner,
            display_name,
            collision_key,
            declared_bytes,
            expires_at,
            principal_quota,
            instance_quota,
            max_live_files,
            u32::MAX,
        )
        .await
    }

    pub(crate) async fn mark_blob_published(&self, upload_id: String) -> Result<()> {
        self.with_connection(move |connection| {
            let changed = connection.execute(
                "UPDATE pending_uploads SET state='blob_published',updated_at=unixepoch() WHERE upload_id=?1 AND state='pending'",
                [&upload_id],
            ).map_err(FileStashStoreError::sqlite)?;
            if changed == 1 { Ok(()) } else { Err(FileStashStoreError::Integrity) }
        }).await
    }

    pub(crate) async fn commit_upload(&self, upload_id: String) -> Result<String> {
        self.with_connection(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(FileStashStoreError::sqlite)?;
            let pending: Option<(String,String,String,i64,String)> = tx.query_row(
                "SELECT owner_principal_id,display_name,collision_key,reserved_bytes,state FROM pending_uploads WHERE upload_id=?1",
                [&upload_id],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
            ).optional().map_err(FileStashStoreError::sqlite)?;
            let Some((owner,name,key,size,state)) = pending else { return Err(FileStashStoreError::Integrity) };
            if state != "blob_published" { return Err(FileStashStoreError::Integrity); }
            // Delete pending first so its cross-table name claim is released in this transaction.
            tx.execute("DELETE FROM pending_uploads WHERE upload_id=?1", [&upload_id]).map_err(FileStashStoreError::sqlite)?;
            tx.execute(
                "INSERT INTO files(file_id,owner_principal_id,display_name,collision_key,size_bytes,blob_key,ready,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?1,1,unixepoch(),unixepoch())",
                params![upload_id,owner,name,key,size],
            ).map_err(map_constraint)?;
            tx.commit().map_err(FileStashStoreError::sqlite)?;
            Ok(upload_id)
        }).await
    }

    pub(crate) async fn cancel_upload(&self, upload_id: String) -> Result<()> {
        #[cfg(all(test, target_os = "linux"))]
        {
            let mut injected = FAIL_CANCEL_ID
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if injected.as_deref() == Some(upload_id.as_str()) {
                *injected = None;
                return Err(FileStashStoreError::Busy);
            }
        }
        self.with_connection(move |connection| {
            connection
                .execute(
                    "DELETE FROM pending_uploads WHERE upload_id=?1",
                    [&upload_id],
                )
                .map_err(FileStashStoreError::sqlite)?;
            Ok(())
        })
        .await
    }

    pub(crate) async fn usage(&self, owner: String) -> Result<StashUsage> {
        self.with_connection(move |connection| {
            connection.query_row(
                "SELECT COALESCE((SELECT SUM(size_bytes) FROM files WHERE owner_principal_id=?1 AND ready=1),0),COALESCE((SELECT SUM(reserved_bytes) FROM pending_uploads WHERE owner_principal_id=?1),0),COALESCE((SELECT COUNT(*) FROM files WHERE owner_principal_id=?1 AND ready=1),0),COALESCE((SELECT COUNT(*) FROM files f WHERE f.owner_principal_id=?1 AND f.ready=1 AND EXISTS(SELECT 1 FROM grants g WHERE g.file_id=f.file_id AND g.state='active')),0)",
                [&owner],
                |r| Ok(StashUsage { committed_bytes: r.get::<_, i64>(0)? as u64, reserved_bytes: r.get::<_, i64>(1)? as u64, live_files: r.get::<_, i64>(2)? as u64, owned_shared_file_count: r.get::<_, i64>(3)? as u64 }),
            ).map_err(FileStashStoreError::sqlite)
        }).await
    }

    pub(crate) async fn list_files(
        &self,
        principal: String,
        after: Option<StashCursor>,
        limit: usize,
    ) -> Result<Vec<StashFile>> {
        self.with_connection(move |connection| {
            let (after_created, after_id) = after
                .map(|cursor| (cursor.created_at, cursor.id))
                .unwrap_or((i64::MAX, String::new()));
            let mut statement = connection.prepare(
                "SELECT f.file_id,f.display_name,f.size_bytes,f.blob_key,f.created_at,f.updated_at,\
                 CASE WHEN f.owner_principal_id=?1 THEN 1 ELSE 0 END \
                 FROM files f WHERE f.ready=1 \
                 AND (f.owner_principal_id=?1 OR EXISTS(SELECT 1 FROM grants g WHERE g.file_id=f.file_id AND g.grantee_principal_id=?1 AND g.state='active')) \
                 AND (f.created_at<?2 OR (f.created_at=?2 AND (?3='' OR f.file_id<?3))) \
                 ORDER BY f.created_at DESC,f.file_id DESC LIMIT ?4"
            ).map_err(FileStashStoreError::sqlite)?;
            let rows = statement.query_map(
                params![principal, after_created, after_id, i64::try_from(limit).unwrap_or(i64::MAX)],
                |row| Ok(StashFile {
                    file_id: row.get(0)?, display_name: row.get(1)?,
                    size_bytes: row.get::<_, i64>(2)? as u64, blob_key: row.get(3)?,
                    created_at: row.get(4)?, updated_at: row.get(5)?, owned: row.get::<_, i64>(6)? != 0,
                }),
            ).map_err(FileStashStoreError::sqlite)?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_err(FileStashStoreError::sqlite)
        }).await
    }

    pub(crate) async fn authorized_file(
        &self,
        principal: String,
        file_id: String,
    ) -> Result<StashFile> {
        self.with_connection(move |connection| {
            connection.query_row(
                "SELECT f.file_id,f.display_name,f.size_bytes,f.blob_key,f.created_at,f.updated_at,CASE WHEN f.owner_principal_id=?1 THEN 1 ELSE 0 END FROM files f WHERE f.file_id=?2 AND f.ready=1 AND (f.owner_principal_id=?1 OR EXISTS(SELECT 1 FROM grants g WHERE g.file_id=f.file_id AND g.grantee_principal_id=?1 AND g.state='active'))",
                params![principal,file_id],
                |row| Ok(StashFile { file_id:row.get(0)?,display_name:row.get(1)?,size_bytes:row.get::<_,i64>(2)? as u64,blob_key:row.get(3)?,created_at:row.get(4)?,updated_at:row.get(5)?,owned:row.get::<_,i64>(6)? != 0 }),
            ).optional().map_err(FileStashStoreError::sqlite)?.ok_or(FileStashStoreError::NotFound)
        }).await
    }

    pub(crate) async fn rename_file(
        &self,
        owner: String,
        file_id: String,
        display_name: String,
        collision_key: String,
    ) -> Result<StashFile> {
        self.with_connection(move |connection| {
            let changed = connection.execute(
                "UPDATE files SET display_name=?3,collision_key=?4,updated_at=unixepoch() WHERE file_id=?2 AND owner_principal_id=?1 AND ready=1",
                params![owner,file_id,display_name,collision_key],
            ).map_err(map_constraint)?;
            if changed != 1 { return Err(FileStashStoreError::NotFound); }
            connection.query_row("SELECT file_id,display_name,size_bytes,blob_key,created_at,updated_at FROM files WHERE file_id=?1", [&file_id], |row| Ok(StashFile { file_id:row.get(0)?,display_name:row.get(1)?,size_bytes:row.get::<_,i64>(2)? as u64,blob_key:row.get(3)?,created_at:row.get(4)?,updated_at:row.get(5)?,owned:true })).map_err(FileStashStoreError::sqlite)
        }).await
    }

    pub(crate) async fn delete_file(&self, owner: String, file_id: String) -> Result<String> {
        self.with_connection(move |connection| {
            let tx=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(FileStashStoreError::sqlite)?;
            let blob:Option<String>=tx.query_row("SELECT blob_key FROM files WHERE file_id=?2 AND owner_principal_id=?1 AND ready=1",params![owner,file_id],|r|r.get(0)).optional().map_err(FileStashStoreError::sqlite)?;
            let Some(blob)=blob else{return Err(FileStashStoreError::NotFound)};
            tx.execute("DELETE FROM files WHERE file_id=?1",[&file_id]).map_err(FileStashStoreError::sqlite)?;
            tx.commit().map_err(FileStashStoreError::sqlite)?;
            Ok(blob)
        }).await
    }

    pub(crate) async fn create_grant(
        &self,
        owner: String,
        file_id: String,
        grantee: String,
    ) -> Result<StashGrant> {
        let grant_id = ulid::Ulid::new().to_string();
        self.with_connection(move|connection|{
            if owner==grantee{return Err(FileStashStoreError::Conflict)}
            let owns:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM files WHERE file_id=?2 AND owner_principal_id=?1 AND ready=1)",params![owner,file_id],|r|r.get(0)).map_err(FileStashStoreError::sqlite)?;
            if !owns{return Err(FileStashStoreError::NotFound)}
            let now=unix_now();
            connection.execute("INSERT INTO grants(grant_id,file_id,grantee_principal_id,state,created_at,revoked_at) VALUES(?1,?2,?3,'active',?4,NULL)",params![grant_id,file_id,grantee,now]).map_err(map_constraint)?;
            Ok(StashGrant{grant_id,file_id,grantee_principal_id:grantee,created_at:now})
        }).await
    }

    pub(crate) async fn revoke_grant(
        &self,
        owner: String,
        file_id: String,
        grant_id: String,
    ) -> Result<()> {
        self.with_connection(move|connection|{
            let changed=connection.execute("UPDATE grants SET state='revoked',revoked_at=unixepoch() WHERE grant_id=?3 AND file_id=?2 AND state='active' AND EXISTS(SELECT 1 FROM files WHERE file_id=?2 AND owner_principal_id=?1 AND ready=1)",params![owner,file_id,grant_id]).map_err(FileStashStoreError::sqlite)?;
            if changed==1{Ok(())}else{Err(FileStashStoreError::NotFound)}
        }).await
    }

    pub(crate) async fn list_grants(
        &self,
        owner: String,
        file_id: String,
        after: String,
        limit: usize,
    ) -> Result<Vec<StashGrant>> {
        self.with_connection(move|connection|{
            let owns:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM files WHERE file_id=?2 AND owner_principal_id=?1 AND ready=1)",params![owner,file_id],|r|r.get(0)).map_err(FileStashStoreError::sqlite)?;
            if !owns{return Err(FileStashStoreError::NotFound)}
            let mut statement=connection.prepare("SELECT grant_id,file_id,grantee_principal_id,created_at FROM grants WHERE file_id=?1 AND state='active' AND grant_id>?2 ORDER BY grant_id LIMIT ?3").map_err(FileStashStoreError::sqlite)?;
            let rows=statement.query_map(params![file_id,after,i64::try_from(limit).unwrap_or(i64::MAX)],|r|Ok(StashGrant{grant_id:r.get(0)?,file_id:r.get(1)?,grantee_principal_id:r.get(2)?,created_at:r.get(3)?})).map_err(FileStashStoreError::sqlite)?;
            rows.collect::<std::result::Result<Vec<_>,_>>().map_err(FileStashStoreError::sqlite)
        }).await
    }

    pub(crate) async fn pending_for_recovery(
        &self,
        after: String,
        limit: usize,
    ) -> Result<Vec<PendingRecovery>> {
        self.with_connection(move |connection| {
            let mut statement = connection
                .prepare(
                    "SELECT upload_id,state,reserved_bytes FROM pending_uploads WHERE upload_id>?1 ORDER BY upload_id LIMIT ?2",
                )
                .map_err(FileStashStoreError::sqlite)?;
            let rows = statement
                .query_map(params![after, i64::try_from(limit).unwrap_or(i64::MAX)], |r| {
                    Ok(PendingRecovery {
                        upload_id: r.get(0)?,
                        state: r.get(1)?,
                        reserved_bytes: r.get::<_, i64>(2)? as u64,
                    })
                })
                .map_err(FileStashStoreError::sqlite)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(FileStashStoreError::sqlite)
        })
        .await
    }

    pub(crate) async fn expired_pending(
        &self,
        now: i64,
        limit: usize,
    ) -> Result<Vec<PendingRecovery>> {
        self.with_connection(move |connection| {
            let mut statement = connection.prepare("SELECT upload_id,state,reserved_bytes FROM pending_uploads WHERE expires_at<=?1 ORDER BY expires_at,upload_id LIMIT ?2").map_err(FileStashStoreError::sqlite)?;
            let rows = statement.query_map(params![now, i64::try_from(limit).unwrap_or(i64::MAX)], |r| Ok(PendingRecovery { upload_id:r.get(0)?, state:r.get(1)?, reserved_bytes:r.get::<_, i64>(2)? as u64 })).map_err(FileStashStoreError::sqlite)?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_err(FileStashStoreError::sqlite)
        }).await
    }

    pub(crate) async fn committed_blob_keys(
        &self,
        after: String,
        limit: usize,
    ) -> Result<Vec<(String, u64)>> {
        self.with_connection(move |connection| {
            let mut statement = connection
                .prepare("SELECT blob_key,size_bytes FROM files WHERE ready=1 AND blob_key>?1 ORDER BY blob_key LIMIT ?2")
                .map_err(FileStashStoreError::sqlite)?;
            let rows = statement
                .query_map(params![after, i64::try_from(limit).unwrap_or(i64::MAX)], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64)))
                .map_err(FileStashStoreError::sqlite)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(FileStashStoreError::sqlite)
        })
        .await
    }

    pub(crate) async fn committed_blob_membership(
        &self,
        keys: Vec<String>,
    ) -> Result<HashSet<String>> {
        self.with_connection(move |connection| {
            if keys.is_empty() {
                return Ok(HashSet::new());
            }
            let placeholders = std::iter::repeat_n("?", keys.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql =
                format!("SELECT blob_key FROM files WHERE ready=1 AND blob_key IN({placeholders})");
            let mut statement = connection
                .prepare(&sql)
                .map_err(FileStashStoreError::sqlite)?;
            let rows = statement
                .query_map(rusqlite::params_from_iter(keys.iter()), |row| row.get(0))
                .map_err(FileStashStoreError::sqlite)?;
            rows.collect::<std::result::Result<HashSet<_>, _>>()
                .map_err(FileStashStoreError::sqlite)
        })
        .await
    }

    pub(crate) async fn expire_upload_now(&self, upload_id: String) -> Result<()> {
        self.with_connection(move |connection| {
            connection.execute("UPDATE pending_uploads SET expires_at=0,updated_at=unixepoch() WHERE upload_id=?1", [&upload_id]).map_err(FileStashStoreError::sqlite)?;
            Ok(())
        }).await
    }
}

#[cfg(all(test, target_os = "linux"))]
static FAIL_CANCEL_ID: std::sync::LazyLock<Mutex<Option<String>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

#[cfg(all(test, target_os = "linux"))]
pub(super) fn inject_cancel_failure(upload_id: String) {
    *FAIL_CANCEL_ID
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(upload_id);
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .try_into()
        .unwrap_or(i64::MAX)
}

fn map_constraint(error: rusqlite::Error) -> FileStashStoreError {
    match &error {
        rusqlite::Error::SqliteFailure(code, _) if code.code == ErrorCode::ConstraintViolation => {
            FileStashStoreError::Conflict
        }
        _ => FileStashStoreError::sqlite(error),
    }
}
fn open_connection(path: &Path, snapshot_id: &str) -> Result<Connection> {
    // The Linux runtime supplies a path beneath `/proc/self/fd/<root-fd>` so
    // SQLite and its WAL sidecars remain anchored to the verified directory.
    // SQLITE_OPEN_NOFOLLOW rejects that required procfs descriptor link; the
    // final database entry is instead opened and identity-checked with
    // openat(..., NOFOLLOW) by the runtime before this connection is exposed.
    let mut c = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(FileStashStoreError::sqlite)?;
    c.busy_timeout(BUSY_TIMEOUT)
        .map_err(FileStashStoreError::sqlite)?;
    c.pragma_update(None, "journal_mode", "WAL")
        .map_err(FileStashStoreError::sqlite)?;
    c.pragma_update(None, "synchronous", "FULL")
        .map_err(FileStashStoreError::sqlite)?;
    c.pragma_update(None, "foreign_keys", true)
        .map_err(FileStashStoreError::sqlite)?;
    schema::migrate(&mut c, snapshot_id)?;
    Ok(c)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    async fn store() -> (tempfile::TempDir, FileStashStore) {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("metadata.sqlite3");
        std::fs::File::create(&path).unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let store = FileStashStore::open(path, ulid::Ulid::new().to_string())
            .await
            .unwrap();
        (temp, store)
    }

    #[tokio::test]
    async fn reservations_enforce_name_and_both_byte_quotas_transactionally() {
        let (_temp, store) = store().await;
        let first = store
            .reserve_upload(
                "owner".into(),
                "Report".into(),
                "report".into(),
                6,
                i64::MAX,
                10,
                20,
                2,
            )
            .await
            .unwrap();
        assert_eq!(first.owner_principal_id, "owner");
        assert_eq!(first.display_name, "Report");
        assert_eq!(first.collision_key, "report");
        assert!(matches!(
            store
                .reserve_upload(
                    "owner".into(),
                    "REPORT".into(),
                    "report".into(),
                    1,
                    i64::MAX,
                    10,
                    20,
                    2
                )
                .await,
            Err(FileStashStoreError::Conflict)
        ));
        assert!(matches!(
            store
                .reserve_upload(
                    "owner".into(),
                    "other".into(),
                    "other".into(),
                    5,
                    i64::MAX,
                    10,
                    20,
                    2
                )
                .await,
            Err(FileStashStoreError::QuotaExceeded)
        ));
        assert!(matches!(
            store
                .reserve_upload(
                    "second".into(),
                    "other".into(),
                    "other".into(),
                    15,
                    i64::MAX,
                    20,
                    20,
                    2
                )
                .await,
            Err(FileStashStoreError::QuotaExceeded)
        ));
        assert_eq!(store.usage("owner".into()).await.unwrap().reserved_bytes, 6);
        store.cancel_upload(first.upload_id).await.unwrap();
        assert_eq!(
            store.usage("owner".into()).await.unwrap(),
            StashUsage::default()
        );
    }

    #[tokio::test]
    async fn pending_uploads_count_toward_the_live_file_limit_under_concurrency() {
        let (_temp, store) = store().await;
        let left = store.clone();
        let right = store.clone();
        let (a, b) = tokio::join!(
            left.reserve_upload(
                "owner".into(),
                "a".into(),
                "a".into(),
                0,
                i64::MAX,
                10,
                20,
                1
            ),
            right.reserve_upload(
                "owner".into(),
                "b".into(),
                "b".into(),
                0,
                i64::MAX,
                10,
                20,
                1
            ),
        );
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        assert!(matches!(
            a.err().or_else(|| b.err()),
            Some(FileStashStoreError::QuotaExceeded)
        ));
    }

    #[tokio::test]
    async fn pending_uploads_count_toward_the_instance_live_file_limit() {
        let (_temp, store) = store().await;
        store
            .reserve_upload_with_instance_limit(
                "owner-a".into(),
                "a".into(),
                "a".into(),
                0,
                i64::MAX,
                10,
                20,
                10,
                1,
            )
            .await
            .unwrap();
        assert!(matches!(
            store
                .reserve_upload_with_instance_limit(
                    "owner-b".into(),
                    "b".into(),
                    "b".into(),
                    0,
                    i64::MAX,
                    10,
                    20,
                    10,
                    1,
                )
                .await,
            Err(FileStashStoreError::QuotaExceeded)
        ));
    }

    #[tokio::test]
    async fn publication_moves_reservation_to_committed_usage_in_one_transaction() {
        let (_temp, store) = store().await;
        let pending = store
            .reserve_upload(
                "owner".into(),
                "a".into(),
                "a".into(),
                7,
                i64::MAX,
                20,
                20,
                2,
            )
            .await
            .unwrap();
        store
            .mark_blob_published(pending.upload_id.clone())
            .await
            .unwrap();
        let file = store
            .commit_upload(pending.upload_id.clone())
            .await
            .unwrap();
        assert_eq!(file, pending.upload_id);
        assert_eq!(
            store.usage("owner".into()).await.unwrap(),
            StashUsage {
                committed_bytes: 7,
                reserved_bytes: 0,
                live_files: 1,
                owned_shared_file_count: 0,
            }
        );
    }

    #[tokio::test]
    async fn shared_count_is_distinct_per_file_and_ignores_revoked_grants() {
        let (_temp, store) = store().await;
        let pending = store
            .reserve_upload(
                "owner".into(),
                "a".into(),
                "a".into(),
                1,
                i64::MAX,
                10,
                20,
                2,
            )
            .await
            .unwrap();
        store
            .mark_blob_published(pending.upload_id.clone())
            .await
            .unwrap();
        let file_id = store.commit_upload(pending.upload_id).await.unwrap();
        store
            .with_connection(move |connection| {
                connection
                    .execute(
                        "INSERT INTO grants VALUES('g1',?1,'p1','active',1,NULL)",
                        [&file_id],
                    )
                    .map_err(FileStashStoreError::sqlite)?;
                connection
                    .execute(
                        "INSERT INTO grants VALUES('g2',?1,'p2','active',1,NULL)",
                        [&file_id],
                    )
                    .map_err(FileStashStoreError::sqlite)?;
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(
            store
                .usage("owner".into())
                .await
                .unwrap()
                .owned_shared_file_count,
            1
        );
        store.with_connection(|connection| {
            connection.execute("UPDATE grants SET state='revoked',revoked_at=2 WHERE grant_id IN('g1','g2')", []).map_err(FileStashStoreError::sqlite)?;
            Ok(())
        }).await.unwrap();
        assert_eq!(
            store
                .usage("owner".into())
                .await
                .unwrap()
                .owned_shared_file_count,
            0
        );
    }

    #[tokio::test]
    async fn grants_are_non_enumerating_and_revocation_removes_access() {
        let (_temp, store) = store().await;
        let pending = store
            .reserve_upload(
                "owner".into(),
                "Report.txt".into(),
                "report.txt".into(),
                4,
                i64::MAX,
                20,
                20,
                3,
            )
            .await
            .unwrap();
        store
            .mark_blob_published(pending.upload_id.clone())
            .await
            .unwrap();
        let file_id = store.commit_upload(pending.upload_id).await.unwrap();

        assert!(matches!(
            store
                .authorized_file("stranger".into(), file_id.clone())
                .await,
            Err(FileStashStoreError::NotFound)
        ));
        assert!(matches!(
            store
                .create_grant("stranger".into(), file_id.clone(), "reader".into())
                .await,
            Err(FileStashStoreError::NotFound)
        ));
        assert!(matches!(
            store
                .create_grant("owner".into(), file_id.clone(), "owner".into())
                .await,
            Err(FileStashStoreError::Conflict)
        ));
        let grant = store
            .create_grant("owner".into(), file_id.clone(), "reader".into())
            .await
            .unwrap();
        assert!(
            !store
                .authorized_file("reader".into(), file_id.clone())
                .await
                .unwrap()
                .owned
        );
        assert!(matches!(
            store
                .create_grant("owner".into(), file_id.clone(), "reader".into())
                .await,
            Err(FileStashStoreError::Conflict)
        ));
        store
            .revoke_grant("owner".into(), file_id.clone(), grant.grant_id)
            .await
            .unwrap();
        assert!(matches!(
            store.authorized_file("reader".into(), file_id).await,
            Err(FileStashStoreError::NotFound)
        ));
    }

    #[tokio::test]
    async fn list_search_and_delete_preserve_principal_boundaries() {
        let (_temp, store) = store().await;
        let mut ids = Vec::new();
        for (owner, name) in [
            ("owner", "Alpha.txt"),
            ("owner", "Beta.txt"),
            ("other", "Alpha other.txt"),
        ] {
            let pending = store
                .reserve_upload(
                    owner.into(),
                    name.into(),
                    name.to_lowercase(),
                    1,
                    i64::MAX,
                    20,
                    20,
                    3,
                )
                .await
                .unwrap();
            store
                .mark_blob_published(pending.upload_id.clone())
                .await
                .unwrap();
            ids.push(store.commit_upload(pending.upload_id).await.unwrap());
        }
        store
            .create_grant("other".into(), ids[2].clone(), "owner".into())
            .await
            .unwrap();
        let available = store.list_files("owner".into(), None, 10).await.unwrap();
        assert_eq!(available.len(), 3);
        assert_eq!(available.iter().filter(|file| file.owned).count(), 2);
        assert!(matches!(
            store.delete_file("other".into(), ids[0].clone()).await,
            Err(FileStashStoreError::NotFound)
        ));
        store
            .delete_file("owner".into(), ids[0].clone())
            .await
            .unwrap();
        assert!(matches!(
            store.authorized_file("owner".into(), ids[0].clone()).await,
            Err(FileStashStoreError::NotFound)
        ));
    }

    #[tokio::test]
    async fn rename_moves_the_name_claim_and_rejects_pending_and_committed_collisions() {
        let (_temp, store) = store().await;
        let mut ids = Vec::new();
        for name in ["first", "second"] {
            let pending = store
                .reserve_upload(
                    "owner".into(),
                    name.into(),
                    name.into(),
                    1,
                    i64::MAX,
                    20,
                    20,
                    5,
                )
                .await
                .unwrap();
            store
                .mark_blob_published(pending.upload_id.clone())
                .await
                .unwrap();
            ids.push(store.commit_upload(pending.upload_id).await.unwrap());
        }
        let pending = store
            .reserve_upload(
                "owner".into(),
                "pending".into(),
                "pending".into(),
                1,
                i64::MAX,
                20,
                20,
                5,
            )
            .await
            .unwrap();
        assert!(matches!(
            store
                .rename_file(
                    "owner".into(),
                    ids[0].clone(),
                    "second".into(),
                    "second".into()
                )
                .await,
            Err(FileStashStoreError::Conflict)
        ));
        assert!(matches!(
            store
                .rename_file(
                    "owner".into(),
                    ids[0].clone(),
                    "pending".into(),
                    "pending".into()
                )
                .await,
            Err(FileStashStoreError::Conflict)
        ));
        store
            .rename_file(
                "owner".into(),
                ids[0].clone(),
                "renamed".into(),
                "renamed".into(),
            )
            .await
            .unwrap();
        let reused = store
            .reserve_upload(
                "owner".into(),
                "first".into(),
                "first".into(),
                1,
                i64::MAX,
                20,
                20,
                5,
            )
            .await
            .unwrap();
        store.cancel_upload(reused.upload_id).await.unwrap();
        store.cancel_upload(pending.upload_id).await.unwrap();
    }

    #[tokio::test]
    async fn scale_queries_are_cardinality_bounded_and_use_authority_indexes() {
        const FILES: usize = 5_000;
        const PENDING: usize = 1_000;
        let (_temp, store) = store().await;
        store
            .with_connection(|connection| {
                let transaction = connection
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(FileStashStoreError::sqlite)?;
                {
                    let mut files = transaction.prepare("INSERT INTO files(file_id,owner_principal_id,display_name,collision_key,size_bytes,blob_key,ready,created_at,updated_at) VALUES(?1,?2,?3,?4,1,?5,1,?6,?6)").map_err(FileStashStoreError::sqlite)?;
                    let mut grants = transaction.prepare("INSERT INTO grants(grant_id,file_id,grantee_principal_id,state,created_at,revoked_at) VALUES(?1,?2,'reader','active',?3,NULL)").map_err(FileStashStoreError::sqlite)?;
                    for index in 0..FILES {
                        let ordinal = i64::try_from(index).unwrap();
                        let id = format!("file-{index:05}");
                        let owner = if index % 2 == 0 { "owner" } else { "other" };
                        let name = if index % 100 == 0 {
                            format!("needle-{index:05}")
                        } else {
                            format!("ordinary-{index:05}")
                        };
                        files.execute(params![id, owner, name, name, format!("blob-{index:05}"), ordinal]).map_err(FileStashStoreError::sqlite)?;
                        grants.execute(params![format!("grant-{index:05}"), id, ordinal]).map_err(FileStashStoreError::sqlite)?;
                    }
                    let mut pending = transaction.prepare("INSERT INTO pending_uploads(upload_id,owner_principal_id,display_name,collision_key,reserved_bytes,state,expires_at,created_at,updated_at) VALUES(?1,'owner',?2,?2,1,'pending',?3,?3,?3)").map_err(FileStashStoreError::sqlite)?;
                    for index in 0..PENDING {
                        let ordinal = i64::try_from(index).unwrap();
                        let id = format!("pending-{index:05}");
                        pending.execute(params![id, format!("pending-name-{index:05}"), ordinal]).map_err(FileStashStoreError::sqlite)?;
                    }
                }
                transaction.commit().map_err(FileStashStoreError::sqlite)
            })
            .await
            .unwrap();

        assert_eq!(
            store
                .list_files("owner".into(), None, 37)
                .await
                .unwrap()
                .len(),
            37
        );
        assert_eq!(
            store
                .list_files("owner".into(), None, 13)
                .await
                .unwrap()
                .len(),
            13
        );
        let usage = store.usage("owner".into()).await.unwrap();
        assert_eq!(usage.live_files, (FILES / 2) as u64);
        assert_eq!(usage.committed_bytes, (FILES / 2) as u64);
        assert_eq!(usage.reserved_bytes, PENDING as u64);
        assert_eq!(usage.owned_shared_file_count, (FILES / 2) as u64);
        assert_eq!(
            store
                .list_grants("owner".into(), "file-00000".into(), String::new(), 7)
                .await
                .unwrap()
                .len(),
            1
        );
        let expired = store.expired_pending(10_000, 19).await.unwrap();
        assert_eq!(expired.len(), 19);
        assert_eq!(expired.first().unwrap().upload_id, "pending-00000");
        assert_eq!(expired.last().unwrap().upload_id, "pending-00018");

        let plans = store
            .with_connection(|connection| {
                let statements = [
                    ("list", "EXPLAIN QUERY PLAN SELECT f.file_id FROM files f WHERE f.ready=1 AND (f.owner_principal_id='owner' OR EXISTS(SELECT 1 FROM grants g WHERE g.file_id=f.file_id AND g.grantee_principal_id='owner' AND g.state='active')) AND (f.created_at<99999 OR (f.created_at=99999 AND f.file_id<'z')) ORDER BY f.created_at DESC,f.file_id DESC LIMIT 51"),
                    ("stats", "EXPLAIN QUERY PLAN SELECT COUNT(*) FROM files f WHERE f.owner_principal_id='owner' AND f.ready=1 AND EXISTS(SELECT 1 FROM grants g WHERE g.file_id=f.file_id AND g.state='active')"),
                    ("grants", "EXPLAIN QUERY PLAN SELECT grant_id FROM grants WHERE file_id='file-00000' AND state='active' AND grant_id>'' ORDER BY grant_id LIMIT 51"),
                    ("janitor", "EXPLAIN QUERY PLAN SELECT upload_id FROM pending_uploads WHERE expires_at<=10000 ORDER BY expires_at,upload_id LIMIT 51"),
                ];
                statements.into_iter().map(|(name, sql)| {
                    let mut statement=connection.prepare(sql).map_err(FileStashStoreError::sqlite)?;
                    let rows=statement.query_map([],|row|row.get::<_,String>(3)).map_err(FileStashStoreError::sqlite)?;
                    let detail=rows.collect::<std::result::Result<Vec<_>,_>>().map_err(FileStashStoreError::sqlite)?.join(" | ");
                    Ok((name.to_owned(),detail))
                }).collect::<Result<Vec<_>>>()
            })
            .await
            .unwrap();
        let detail = |name: &str| plans.iter().find(|plan| plan.0 == name).unwrap().1.as_str();
        assert!(
            detail("list").contains("stash_grants_active_unique")
                || detail("list").contains("stash_grants_file_grantee")
                || detail("list").contains("stash_grants_grantee_files"),
            "unexpected list plan: {}",
            detail("list")
        );
        assert!(detail("stats").contains("stash_files_owner_list"));
        assert!(
            detail("stats").contains("stash_grants_active_unique")
                || detail("stats").contains("stash_grants_file_grantee")
                || detail("stats").contains("stash_grants_grantee_files")
                || detail("stats").contains("stash_grants_active_page"),
            "unexpected stats plan: {}",
            detail("stats")
        );
        assert!(detail("grants").contains("stash_grants_active_page"));
        assert!(!detail("grants").contains("USE TEMP B-TREE"));
        assert!(detail("janitor").contains("stash_pending_janitor"));
        assert!(!detail("janitor").contains("USE TEMP B-TREE"));
    }
}
