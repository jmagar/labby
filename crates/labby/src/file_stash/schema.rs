use super::store::{FileStashStoreError, Result};
use rusqlite::{Connection, TransactionBehavior, params};
pub(super) const APPLICATION_ID: i64 = 0x4c_46_53_31;
pub(super) const SCHEMA_VERSION: i64 = 1;
pub(super) const SCHEMA_FINGERPRINT: &str = "labby-file-stash-v1-20260905-service-3";
pub(super) fn migrate(connection: &mut Connection, snapshot_id: &str) -> Result<()> {
    let found: i64 = connection
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(FileStashStoreError::sqlite)?;
    if found > SCHEMA_VERSION {
        return Err(FileStashStoreError::NewerSchema(found));
    }
    if found == 0 {
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Exclusive)
            .map_err(FileStashStoreError::sqlite)?;
        tx.execute_batch(SCHEMA)
            .map_err(FileStashStoreError::sqlite)?;
        tx.execute(
            "INSERT INTO stash_metadata VALUES(1,?1,?2,?3,unixepoch())",
            params![SCHEMA_VERSION, SCHEMA_FINGERPRINT, snapshot_id],
        )
        .map_err(FileStashStoreError::sqlite)?;
        tx.pragma_update(None, "application_id", APPLICATION_ID)
            .map_err(FileStashStoreError::sqlite)?;
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(FileStashStoreError::sqlite)?;
        tx.commit().map_err(FileStashStoreError::sqlite)?;
    }
    validate(connection, snapshot_id)
}
fn validate(c: &Connection, snapshot_id: &str) -> Result<()> {
    let app: i64 = c
        .query_row("PRAGMA application_id", [], |r| r.get(0))
        .map_err(FileStashStoreError::sqlite)?;
    if app != APPLICATION_ID {
        return Err(FileStashStoreError::Corrupt);
    }
    let m:(i64,String,String)=c.query_row("SELECT schema_version,schema_fingerprint,snapshot_id FROM stash_metadata WHERE singleton=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(FileStashStoreError::sqlite)?;
    if m.0 != SCHEMA_VERSION || m.1 != SCHEMA_FINGERPRINT {
        return Err(FileStashStoreError::Corrupt);
    }
    if m.2 != snapshot_id {
        return Err(FileStashStoreError::BackupMismatch);
    }
    let check: String = c
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .map_err(FileStashStoreError::sqlite)?;
    if check == "ok" {
        Ok(())
    } else {
        Err(FileStashStoreError::Corrupt)
    }
}
const SCHEMA: &str = r"
CREATE TABLE stash_metadata(singleton INTEGER PRIMARY KEY CHECK(singleton=1),schema_version INTEGER NOT NULL CHECK(schema_version=1),schema_fingerprint TEXT NOT NULL,snapshot_id TEXT NOT NULL CHECK(length(snapshot_id)>0),updated_at INTEGER NOT NULL) STRICT;
CREATE TABLE name_claims(owner_principal_id TEXT NOT NULL,collision_key TEXT NOT NULL,record_kind TEXT NOT NULL CHECK(record_kind IN('pending','file')),record_id TEXT NOT NULL,PRIMARY KEY(owner_principal_id,collision_key),UNIQUE(record_kind,record_id)) STRICT;
CREATE TABLE pending_uploads(upload_id TEXT PRIMARY KEY,owner_principal_id TEXT NOT NULL CHECK(length(trim(owner_principal_id))>0),display_name TEXT NOT NULL,collision_key TEXT NOT NULL,reserved_bytes INTEGER NOT NULL CHECK(reserved_bytes>=0),state TEXT NOT NULL CHECK(state IN('pending','blob_published')),expires_at INTEGER NOT NULL,created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL) STRICT;
CREATE UNIQUE INDEX stash_pending_owner_name ON pending_uploads(owner_principal_id,collision_key); CREATE INDEX stash_pending_janitor ON pending_uploads(expires_at,upload_id);
CREATE TRIGGER stash_pending_claim_insert AFTER INSERT ON pending_uploads BEGIN INSERT INTO name_claims VALUES(NEW.owner_principal_id,NEW.collision_key,'pending',NEW.upload_id); END;
CREATE TRIGGER stash_pending_claim_delete AFTER DELETE ON pending_uploads BEGIN DELETE FROM name_claims WHERE record_kind='pending' AND record_id=OLD.upload_id; END;
CREATE TABLE files(file_id TEXT PRIMARY KEY,owner_principal_id TEXT NOT NULL CHECK(length(trim(owner_principal_id))>0),display_name TEXT NOT NULL,collision_key TEXT NOT NULL,size_bytes INTEGER NOT NULL CHECK(size_bytes>=0),blob_key TEXT NOT NULL UNIQUE,ready INTEGER NOT NULL CHECK(ready IN(0,1)),created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL) STRICT;
CREATE UNIQUE INDEX stash_files_owner_name ON files(owner_principal_id,collision_key); CREATE INDEX stash_files_owner_list ON files(owner_principal_id,ready,created_at DESC,file_id DESC);
CREATE TRIGGER stash_file_claim_insert AFTER INSERT ON files BEGIN INSERT INTO name_claims VALUES(NEW.owner_principal_id,NEW.collision_key,'file',NEW.file_id); END;
CREATE TRIGGER stash_file_claim_delete AFTER DELETE ON files BEGIN DELETE FROM name_claims WHERE record_kind='file' AND record_id=OLD.file_id; END;
CREATE TRIGGER stash_file_claim_update AFTER UPDATE OF collision_key ON files BEGIN UPDATE name_claims SET collision_key=NEW.collision_key WHERE record_kind='file' AND record_id=OLD.file_id; END;
CREATE TABLE grants(grant_id TEXT PRIMARY KEY,file_id TEXT NOT NULL REFERENCES files(file_id) ON DELETE CASCADE,grantee_principal_id TEXT NOT NULL CHECK(length(trim(grantee_principal_id))>0),state TEXT NOT NULL CHECK(state IN('active','revoked')),created_at INTEGER NOT NULL,revoked_at INTEGER,CHECK((state='active' AND revoked_at IS NULL)OR(state='revoked' AND revoked_at IS NOT NULL))) STRICT;
CREATE UNIQUE INDEX stash_grants_active_unique ON grants(file_id,grantee_principal_id) WHERE state='active'; CREATE INDEX stash_grants_file_grantee ON grants(file_id,grantee_principal_id,state);
CREATE INDEX stash_grants_active_page ON grants(file_id,grant_id) WHERE state='active';
CREATE INDEX stash_grants_grantee_files ON grants(grantee_principal_id,state,file_id);
CREATE TRIGGER stash_grant_not_owner BEFORE INSERT ON grants WHEN EXISTS(SELECT 1 FROM files WHERE file_id=NEW.file_id AND owner_principal_id=NEW.grantee_principal_id) BEGIN SELECT RAISE(ABORT,'owner cannot be grantee'); END;
CREATE TRIGGER stash_grant_update_not_owner BEFORE UPDATE OF file_id,grantee_principal_id ON grants WHEN EXISTS(SELECT 1 FROM files WHERE file_id=NEW.file_id AND owner_principal_id=NEW.grantee_principal_id) BEGIN SELECT RAISE(ABORT,'owner cannot be grantee'); END;
";
