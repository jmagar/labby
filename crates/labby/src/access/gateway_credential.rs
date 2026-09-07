//! Durable, secret-free Team credential bindings for Gateway loadouts.

use labby_runtime::gateway_authority::{TeamCredentialBinding, TeamCredentialStatus};
use rusqlite::{Connection, OptionalExtension, params};

use super::{AccessStoreError, error::AccessStoreResult, store::map_sqlite_error};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS gateway_team_credential_bindings (
  binding_id TEXT PRIMARY KEY CHECK(length(trim(binding_id)) BETWEEN 1 AND 256),
  team_id TEXT NOT NULL CHECK(length(trim(team_id)) BETWEEN 1 AND 256),
  upstream_name TEXT NOT NULL CHECK(length(trim(upstream_name)) BETWEEN 1 AND 256),
  custodian_principal_id TEXT NOT NULL
    CHECK(length(trim(custodian_principal_id)) BETWEEN 1 AND 256),
  generation INTEGER NOT NULL CHECK(generation > 0),
  rotated_at_millis INTEGER NOT NULL CHECK(rotated_at_millis > 0),
  status TEXT NOT NULL CHECK(status IN ('active','revoked')),
  revoked_at_millis INTEGER,
  CHECK ((status = 'revoked') = (revoked_at_millis IS NOT NULL)),
  UNIQUE(team_id, upstream_name)
) STRICT;
CREATE INDEX IF NOT EXISTS gateway_team_credential_bindings_team
  ON gateway_team_credential_bindings(team_id,status,upstream_name);
";

#[derive(Clone, Debug)]
pub(crate) struct PutTeamCredentialBinding {
    pub binding_id: String,
    pub team_id: String,
    pub upstream_name: String,
    pub custodian_principal_id: String,
    pub rotated_at_millis: u64,
}

pub(crate) fn put(
    connection: &mut Connection,
    input: &PutTeamCredentialBinding,
) -> AccessStoreResult<TeamCredentialBinding> {
    install(connection)?;
    let candidate = TeamCredentialBinding {
        binding_id: input.binding_id.clone(),
        team_id: input.team_id.clone(),
        upstream_name: input.upstream_name.clone(),
        custodian_principal_id: input.custodian_principal_id.clone(),
        generation: 1,
        rotated_at_millis: input.rotated_at_millis,
        status: TeamCredentialStatus::Active,
    };
    if !candidate.validate() {
        return Err(AccessStoreError::MalformedVocabulary);
    }
    let generation = connection
        .query_row(
            "SELECT generation FROM gateway_team_credential_bindings
             WHERE team_id=?1 AND upstream_name=?2",
            params![input.team_id, input.upstream_name],
            |row| checked_u64(row.get::<_, i64>(0)?),
        )
        .optional()
        .map_err(map_sqlite_error)?
        .map_or(1, |value| value.saturating_add(1));
    let generation_sql = checked_i64(generation)?;
    let rotated_at_sql = checked_i64(input.rotated_at_millis)?;
    connection
        .execute(
            "INSERT INTO gateway_team_credential_bindings VALUES
             (?1,?2,?3,?4,?5,?6,'active',NULL)
             ON CONFLICT(team_id,upstream_name) DO UPDATE SET
               binding_id=excluded.binding_id,
               custodian_principal_id=excluded.custodian_principal_id,
               generation=excluded.generation,
               rotated_at_millis=excluded.rotated_at_millis,
               status='active',revoked_at_millis=NULL",
            params![
                input.binding_id,
                input.team_id,
                input.upstream_name,
                input.custodian_principal_id,
                generation_sql,
                rotated_at_sql
            ],
        )
        .map_err(map_sqlite_error)?;
    get(connection, &input.team_id, &input.upstream_name)?
        .ok_or_else(|| AccessStoreError::Unavailable("credential binding write vanished".into()))
}

pub(crate) fn get(
    connection: &mut Connection,
    team_id: &str,
    upstream_name: &str,
) -> AccessStoreResult<Option<TeamCredentialBinding>> {
    install(connection)?;
    connection
        .query_row(
            "SELECT binding_id,team_id,upstream_name,custodian_principal_id,
                    generation,rotated_at_millis,status
             FROM gateway_team_credential_bindings
             WHERE team_id=?1 AND upstream_name=?2",
            params![team_id, upstream_name],
            decode,
        )
        .optional()
        .map_err(map_sqlite_error)
}

pub(crate) fn list(
    connection: &mut Connection,
    team_id: &str,
) -> AccessStoreResult<Vec<TeamCredentialBinding>> {
    install(connection)?;
    let mut statement = connection
        .prepare(
            "SELECT binding_id,team_id,upstream_name,custodian_principal_id,
                    generation,rotated_at_millis,status
             FROM gateway_team_credential_bindings WHERE team_id=?1
             ORDER BY upstream_name",
        )
        .map_err(map_sqlite_error)?;
    statement
        .query_map([team_id], decode)
        .map_err(map_sqlite_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(map_sqlite_error)
}

pub(crate) fn revoke(
    connection: &mut Connection,
    team_id: &str,
    upstream_name: &str,
    now_millis: u64,
) -> AccessStoreResult<Option<TeamCredentialBinding>> {
    install(connection)?;
    let now_sql = checked_i64(now_millis)?;
    connection
        .execute(
            "UPDATE gateway_team_credential_bindings SET
               generation=generation+1,status='revoked',revoked_at_millis=?3,
               rotated_at_millis=?3
             WHERE team_id=?1 AND upstream_name=?2 AND status='active'",
            params![team_id, upstream_name, now_sql],
        )
        .map_err(map_sqlite_error)?;
    get(connection, team_id, upstream_name)
}

fn install(connection: &Connection) -> AccessStoreResult<()> {
    connection.execute_batch(SCHEMA).map_err(map_sqlite_error)
}

fn decode(row: &rusqlite::Row<'_>) -> rusqlite::Result<TeamCredentialBinding> {
    Ok(TeamCredentialBinding {
        binding_id: row.get(0)?,
        team_id: row.get(1)?,
        upstream_name: row.get(2)?,
        custodian_principal_id: row.get(3)?,
        generation: checked_u64(row.get::<_, i64>(4)?)?,
        rotated_at_millis: checked_u64(row.get::<_, i64>(5)?)?,
        status: match row.get::<_, String>(6)?.as_str() {
            "active" => TeamCredentialStatus::Active,
            "revoked" => TeamCredentialStatus::Revoked,
            _ => return Err(rusqlite::Error::InvalidQuery),
        },
    })
}

fn checked_i64(value: u64) -> AccessStoreResult<i64> {
    i64::try_from(value).map_err(|_| AccessStoreError::MalformedVocabulary)
}

fn checked_u64(value: i64) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|_| rusqlite::Error::InvalidQuery)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_and_revocation_advance_generation_without_secret_columns() {
        let mut connection = Connection::open_in_memory().unwrap();
        let input = PutTeamCredentialBinding {
            binding_id: "binding-a-v1".into(),
            team_id: "alpha".into(),
            upstream_name: "shared".into(),
            custodian_principal_id: "owner-a".into(),
            rotated_at_millis: 1,
        };
        let first = put(&mut connection, &input).unwrap();
        let mut rotated = input;
        rotated.binding_id = "binding-a-v2".into();
        rotated.rotated_at_millis = 2;
        let second = put(&mut connection, &rotated).unwrap();
        assert_eq!(second.generation, first.generation + 1);
        let revoked = revoke(&mut connection, "alpha", "shared", 3)
            .unwrap()
            .unwrap();
        assert_eq!(revoked.generation, second.generation + 1);
        assert!(!revoked.usable(second.generation));
        let columns: String = connection
            .query_row(
                "SELECT group_concat(name, ',') FROM pragma_table_info('gateway_team_credential_bindings')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!columns.contains("secret"));
        assert!(!columns.contains("token"));
    }

    #[test]
    fn teams_sharing_an_upstream_cannot_observe_each_others_binding() {
        let mut connection = Connection::open_in_memory().unwrap();
        for team in ["alpha", "beta"] {
            put(
                &mut connection,
                &PutTeamCredentialBinding {
                    binding_id: format!("binding-{team}"),
                    team_id: team.into(),
                    upstream_name: "shared".into(),
                    custodian_principal_id: format!("owner-{team}"),
                    rotated_at_millis: 1,
                },
            )
            .unwrap();
        }
        let alpha = list(&mut connection, "alpha").unwrap();
        assert_eq!(alpha.len(), 1);
        assert_eq!(alpha[0].binding_id, "binding-alpha");
    }
}
