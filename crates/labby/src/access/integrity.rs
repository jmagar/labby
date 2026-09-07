use rusqlite::Connection;

use super::error::{AccessStoreError, AccessStoreResult};

pub(super) fn validate(connection: &Connection) -> AccessStoreResult<()> {
    let quick_check = connection
        .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
        .map_err(super::store::map_sqlite_error)?;
    if quick_check != "ok" {
        return Err(integrity("quick_check"));
    }

    let foreign_key_failure = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(super::store::map_sqlite_error)?;
    if foreign_key_failure {
        return Err(integrity("foreign_key_check"));
    }

    let application_id = connection
        .query_row("PRAGMA application_id", [], |row| row.get::<_, i64>(0))
        .map_err(super::store::map_sqlite_error)?;
    if application_id != super::migrations::APPLICATION_ID {
        return Err(integrity("application_id"));
    }

    let metadata = connection.query_row(
        "SELECT schema_version, schema_fingerprint, global_revision,
                bootstrap_generation, bootstrap_identity_fingerprint
         FROM access_metadata WHERE singleton = 1",
        [],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        },
    );
    let Ok((
        schema_version,
        fingerprint,
        global_revision,
        bootstrap_generation,
        bootstrap_fingerprint,
    )) = metadata
    else {
        return Err(integrity("schema_metadata"));
    };
    let bootstrap_metadata_valid = match (bootstrap_generation, bootstrap_fingerprint.as_deref()) {
        (0, None) => true,
        (1, Some(value)) => !value.is_empty(),
        _ => false,
    };
    if schema_version != super::migrations::SCHEMA_VERSION
        || fingerprint != super::migrations::SCHEMA_FINGERPRINT
        || global_revision < 0
        || !bootstrap_metadata_valid
    {
        return Err(integrity("schema_metadata"));
    }

    validate_manifest(connection)?;
    validate_bootstrap_state(connection, bootstrap_generation)?;
    validate_team_authority(connection, bootstrap_generation)
}

pub(super) fn validate_v1_before_migration(connection: &Connection) -> AccessStoreResult<()> {
    let application_id = connection
        .query_row("PRAGMA application_id", [], |row| row.get::<_, i64>(0))
        .map_err(map_metadata_error)?;
    let metadata = connection
        .query_row(
            "SELECT schema_version, schema_fingerprint, global_revision FROM access_metadata WHERE singleton=1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?)),
        )
        .map_err(map_metadata_error)?;
    if application_id != super::migrations::APPLICATION_ID
        || metadata.0 != super::migrations::V1_SCHEMA_VERSION
        || metadata.1 != super::migrations::V1_SCHEMA_FINGERPRINT
        || metadata.2 < 0
    {
        return Err(integrity("schema_metadata"));
    }
    let actual = schema_manifest(connection)?;
    let canonical = Connection::open_in_memory().map_err(super::store::map_sqlite_error)?;
    canonical
        .execute_batch(super::migrations::V1_METADATA_SCHEMA)
        .map_err(super::store::map_sqlite_error)?;
    canonical
        .execute_batch(super::migrations::DOMAIN_SCHEMA)
        .map_err(super::store::map_sqlite_error)?;
    if actual != schema_manifest(&canonical)? {
        return Err(integrity("schema_manifest"));
    }
    Ok(())
}

pub(super) fn validate_bootstrap_state(
    connection: &Connection,
    generation: i64,
) -> AccessStoreResult<()> {
    let reserved: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM organizations WHERE organization_id='bootstrap-local') OR EXISTS(SELECT 1 FROM principals WHERE principal_id='bootstrap-owner') OR EXISTS(SELECT 1 FROM principal_links WHERE link_id='bootstrap-owner-link') OR EXISTS(SELECT 1 FROM projects WHERE project_id='bootstrap-default') OR EXISTS(SELECT 1 FROM project_memberships WHERE membership_id='bootstrap-owner-membership') OR EXISTS(SELECT 1 FROM access_audit WHERE event_id='bootstrap-owner-audit')", [], |r| r.get(0)).map_err(super::store::map_sqlite_error)?;
    if generation == 0 {
        return if reserved {
            Err(integrity("bootstrap_state"))
        } else {
            Ok(())
        };
    }
    let canonical: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM organizations WHERE organization_id='bootstrap-local' AND status='active') AND EXISTS(SELECT 1 FROM principals WHERE principal_id='bootstrap-owner' AND organization_id='bootstrap-local' AND kind='user' AND status='active') AND EXISTS(SELECT 1 FROM principal_links WHERE link_id='bootstrap-owner-link' AND principal_id='bootstrap-owner' AND status='active' AND verification_generation=1 AND link_generation=1) AND EXISTS(SELECT 1 FROM projects WHERE project_id='bootstrap-default' AND organization_id='bootstrap-local' AND status='active') AND EXISTS(SELECT 1 FROM project_memberships WHERE membership_id='bootstrap-owner-membership' AND organization_id='bootstrap-local' AND project_id='bootstrap-default' AND principal_id='bootstrap-owner' AND role='owner' AND status='active' AND created_by='bootstrap-owner') AND EXISTS(SELECT 1 FROM access_audit WHERE event_id='bootstrap-owner-audit' AND actor_principal_id='bootstrap-owner' AND organization_id='bootstrap-local' AND project_id='bootstrap-default' AND action='access.bootstrap_owner' AND decision='allow' AND reason_code='explicit_owner_bootstrap' AND target_fingerprint=(SELECT bootstrap_identity_fingerprint FROM access_metadata WHERE singleton=1))", [], |r| r.get(0)).map_err(super::store::map_sqlite_error)?;
    if canonical
        && persisted_link_fingerprint(connection)?.as_deref()
            == connection
                .query_row(
                    "SELECT bootstrap_identity_fingerprint FROM access_metadata WHERE singleton=1",
                    [],
                    |r| r.get::<_, Option<String>>(0),
                )
                .map_err(super::store::map_sqlite_error)?
                .as_deref()
    {
        Ok(())
    } else {
        Err(integrity("bootstrap_state"))
    }
}

fn persisted_link_fingerprint(connection: &Connection) -> AccessStoreResult<Option<String>> {
    use labby_auth::PrincipalLink;
    let row = connection.query_row(
        "SELECT link_kind, issuer, subject, credential_id FROM principal_links WHERE link_id='bootstrap-owner-link'",
        [],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?)),
    ).map_err(super::store::map_sqlite_error)?;
    let link = match row {
        (kind, Some(issuer), Some(subject), None) if kind == "external" => {
            PrincipalLink::External { issuer, subject }
        }
        (kind, None, None, Some(credential_id)) if kind == "local_credential" => {
            PrincipalLink::LocalCredential { credential_id }
        }
        _ => return Ok(None),
    };
    Ok(Some(link.safe_fingerprint()))
}

fn validate_manifest(connection: &Connection) -> AccessStoreResult<()> {
    let actual = schema_manifest(connection)?;
    let canonical = Connection::open_in_memory().map_err(super::store::map_sqlite_error)?;
    canonical
        .execute_batch(super::migrations::SCHEMA_V2_METADATA)
        .map_err(super::store::map_sqlite_error)?;
    canonical
        .execute_batch(super::migrations::DOMAIN_SCHEMA)
        .map_err(super::store::map_sqlite_error)?;
    canonical
        .execute_batch(super::migrations::TEAM_AUTHORITY_SCHEMA)
        .map_err(super::store::map_sqlite_error)?;
    canonical
        .execute_batch(super::dev_container::DEV_CONTAINER_SCHEMA)
        .map_err(super::store::map_sqlite_error)?;
    let expected = schema_manifest(&canonical)?;
    if actual != expected {
        return Err(integrity("schema_manifest"));
    }
    Ok(())
}

pub(super) fn validate_team_authority(
    connection: &Connection,
    generation: i64,
) -> AccessStoreResult<()> {
    let reserved: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM platform_administrators
                            WHERE principal_id='bootstrap-owner')
                 OR EXISTS(SELECT 1 FROM groups
                           WHERE group_id='bootstrap-initial-team')
                 OR EXISTS(SELECT 1 FROM team_memberships
                           WHERE membership_id='bootstrap-initial-team-owner')
                 OR EXISTS(SELECT 1 FROM access_audit
                           WHERE event_id IN ('bootstrap-platform-admin-audit',
                                              'bootstrap-initial-team-audit'))",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(super::store::map_sqlite_error)?;
    if generation == 0 {
        return if reserved {
            Err(integrity("team_bootstrap_state"))
        } else {
            Ok(())
        };
    }
    let canonical: bool = connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM platform_administrators
                 WHERE principal_id='bootstrap-owner' AND status='active'
                   AND authority_epoch=1 AND granted_by='bootstrap-owner')
             AND EXISTS(
                 SELECT 1 FROM groups
                 WHERE group_id='bootstrap-initial-team'
                   AND organization_id='bootstrap-local' AND kind='team'
                   AND status='active' AND policy_epoch=1 AND membership_epoch=1
                   AND created_by='bootstrap-owner')
             AND EXISTS(
                 SELECT 1 FROM team_memberships
                 WHERE membership_id='bootstrap-initial-team-owner'
                   AND organization_id='bootstrap-local'
                   AND team_id='bootstrap-initial-team'
                   AND principal_id='bootstrap-owner' AND role='owner'
                   AND status='active' AND membership_epoch=1
                   AND created_by='bootstrap-owner')
             AND EXISTS(
                 SELECT 1 FROM access_audit
                 WHERE event_id='bootstrap-platform-admin-audit'
                   AND actor_principal_id='bootstrap-owner'
                   AND action='access.platform_admin.bootstrap'
                   AND reason_code='canonical_bootstrap_principal')
             AND EXISTS(
                 SELECT 1 FROM access_audit
                 WHERE event_id='bootstrap-initial-team-audit'
                   AND actor_principal_id='bootstrap-owner'
                   AND action='access.team.bootstrap'
                   AND reason_code='canonical_bootstrap_principal')",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(super::store::map_sqlite_error)?;
    if canonical {
        Ok(())
    } else {
        Err(integrity("team_bootstrap_state"))
    }
}

fn map_metadata_error(error: rusqlite::Error) -> AccessStoreError {
    if let rusqlite::Error::SqliteFailure(failure, _) = &error
        && matches!(
            failure.code,
            rusqlite::ErrorCode::DatabaseBusy
                | rusqlite::ErrorCode::DatabaseLocked
                | rusqlite::ErrorCode::DatabaseCorrupt
                | rusqlite::ErrorCode::NotADatabase
                | rusqlite::ErrorCode::DiskFull
                | rusqlite::ErrorCode::ReadOnly
        )
    {
        return super::store::map_sqlite_error(error);
    }
    integrity("schema_metadata")
}

fn schema_manifest(
    connection: &Connection,
) -> AccessStoreResult<Vec<(String, String, String, String)>> {
    let mut statement = connection
        .prepare(
            "SELECT type, name, tbl_name, sql FROM sqlite_schema
             WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%'
             ORDER BY type, name, tbl_name",
        )
        .map_err(super::store::map_sqlite_error)?;
    statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                normalize_sql(&row.get::<_, String>(3)?),
            ))
        })
        .map_err(super::store::map_sqlite_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(super::store::map_sqlite_error)
}

fn normalize_sql(sql: &str) -> String {
    sql.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("( ", "(")
        .replace(" )", ")")
}

const fn integrity(check: &'static str) -> AccessStoreError {
    AccessStoreError::IntegrityViolation { check }
}
