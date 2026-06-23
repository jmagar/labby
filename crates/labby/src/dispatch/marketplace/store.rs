//! SQLite-backed persistence for the MCP server registry.
//!
//! Uses an r2d2 connection pool with WAL mode for concurrent access from
//! multiple axum handlers. Pool size is 4 — sufficient for homelab workloads.
//! All pool operations run inside `tokio::task::spawn_blocking` since
//! `Pool::get()` is blocking.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::Connection;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use labby_apis::mcpregistry::McpRegistryClient;
use labby_apis::mcpregistry::types::{
    LabRegistryAudit, LabRegistryMetadata, ListServersParams, ResponseMeta, ServerJSON,
    ServerResponse,
};

/// Errors produced by [`RegistryStore`] and its helpers.
#[derive(Debug, Error)]
pub enum RegistryStoreError {
    #[error("sqlite error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("connection pool error: {0}")]
    Pool(#[from] r2d2::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("blocking task join error: {0}")]
    Join(#[from] tokio::task::JoinError),

    #[error("invalid cursor: {0}")]
    InvalidCursor(String),

    #[error("upstream registry error: {0}")]
    Upstream(String),
}

/// Internal cursor representation — base64-encoded JSON in wire form.
#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize)]
struct CursorData {
    /// `server_name` of the last row on the previous page.
    s: String,
    /// `version` of the last row on the previous page.
    v: String,
}

/// DB-side list params — distinct from the upstream API `ListServersParams`.
///
/// Search input is capped at 512 bytes here to prevent DoS via enormous LIKE
/// pattern expansion.
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct StoreListParams {
    /// Substring search on server_name (max 512 bytes enforced at construction).
    pub search: Option<String>,
    /// Exact version match on the stored `version` column.
    pub version: Option<String>,
    /// Inclusive lower bound on `upstream_updated_at` (RFC 3339 string compare).
    pub updated_since: Option<String>,
    /// Opaque pagination cursor from a previous `PagedServers.next_cursor`.
    pub cursor: Option<String>,
    /// Page size — clamped server-side to `[1, 100]`; default 20.
    pub limit: Option<u32>,
    /// Include rows with `status = 'deleted'`. Default: false.
    pub include_deleted: bool,
    /// Only return upstream-designated latest versions. Default: false.
    pub latest_only: bool,
    /// Filter to featured Lab-curated entries.
    pub featured: Option<bool>,
    /// Filter to reviewed Lab-curated entries.
    pub reviewed: Option<bool>,
    /// Filter to recommended Lab-curated entries.
    pub recommended: Option<bool>,
    /// Filter to hidden Lab-curated entries.
    pub hidden: Option<bool>,
    /// Filter to a single curation tag.
    pub tag: Option<String>,
}

impl StoreListParams {
    /// Enforce the 512-byte search input cap, truncating silently.
    ///
    /// LEARNED: truncating at 512 bytes rather than rejecting prevents user-visible
    /// errors from over-long pastes while still bounding LIKE pattern expansion cost.
    #[must_use]
    pub fn with_search(mut self, s: impl Into<String>) -> Self {
        let s = s.into();
        self.search = Some(truncate_utf8_bytes(s, 512));
        self
    }
}

/// Paginated result from `RegistryStore::list_servers`.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct PagedServers {
    pub servers: Vec<ServerResponse>,
    /// Opaque cursor for the next page; `None` when this is the last page.
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone)]
struct LocalMetadataRecord {
    metadata: serde_json::Value,
    updated_at: String,
    updated_by: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SyncStats {
    pub inserted: usize,
    pub updated: usize,
    pub deleted: usize,
}

/// SQLite-backed store for registry server records.
///
/// Cloning the store is cheap — the inner `Pool` is `Arc`-backed.
///
/// Query methods are used by versioned REST consumers — `dead_code` is
/// expected until those consumers are wired in.
#[allow(dead_code)]
pub struct RegistryStore {
    pool: Pool<SqliteConnectionManager>,
    path: PathBuf,
}

impl Clone for RegistryStore {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
            path: self.path.clone(),
        }
    }
}

#[allow(dead_code)]
impl RegistryStore {
    /// Open (or create) the registry database at `path`.
    ///
    /// Sets file permissions to 0o600, enables WAL mode, and runs schema
    /// migrations inside a `BEGIN EXCLUSIVE` transaction to prevent TOCTOU
    /// races when two processes start concurrently.
    pub async fn open(path: &Path) -> Result<Self, RegistryStoreError> {
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || {
            // Ensure parent directory exists.
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            if !path.exists() {
                std::fs::OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .write(true)
                    .open(&path)?;
            }

            // Lock the file to 0o600 before SQLite opens it.
            set_restrictive_permissions(&path)?;

            // Per-connection init: set busy_timeout, foreign_keys, synchronous.
            // These are connection-local pragmas and must be set on every
            // connection that the pool hands out.
            let manager = SqliteConnectionManager::file(&path).with_init(|conn| {
                conn.pragma_update(None, "journal_mode", "WAL")?;
                conn.execute_batch(
                    "PRAGMA busy_timeout = 5000;\
                         PRAGMA foreign_keys = ON;\
                         PRAGMA synchronous = NORMAL;",
                )
            });

            let pool = Pool::builder()
                .max_size(4)
                .connection_timeout(Duration::from_secs(5))
                .build(manager)?;

            // Run schema migration on a freshly acquired connection.
            let conn = pool.get()?;
            migrate(&conn)?;
            drop(conn);

            Ok(Self { pool, path })
        })
        .await?
    }

    #[must_use]
    pub fn db_path(&self) -> &Path {
        &self.path
    }

    /// Acquire a pooled connection for use inside a `spawn_blocking` closure.
    ///
    /// This is intentionally not public — callers should use higher-level
    /// methods. It is `pub(crate)` so the dispatch module can reach it without
    /// exposing `r2d2` types to the rest of lab.
    #[allow(dead_code)]
    pub(crate) fn pool(&self) -> &Pool<SqliteConnectionManager> {
        &self.pool
    }

    // ── Query methods ─────────────────────────────────────────────────────────

    /// List servers with optional cursor pagination, LIKE search, status filter,
    /// and sort.
    ///
    /// Returns at most `params.limit` (clamped to `[1, 100]`) rows plus an
    /// opaque `next_cursor` when more rows exist.
    pub async fn list_servers(
        &self,
        params: StoreListParams,
    ) -> Result<PagedServers, RegistryStoreError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            list_servers_sync(&conn, &params)
        })
        .await?
    }

    /// Get a single server by name and version.
    ///
    /// Pass `version = "latest"` to resolve via the `is_latest = 1` flag.
    /// Returns `None` when no matching row exists.
    pub async fn get_server(
        &self,
        name: &str,
        version: &str,
    ) -> Result<Option<ServerResponse>, RegistryStoreError> {
        let pool = self.pool.clone();
        let name = name.to_string();
        let version = version.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            get_server_sync(&conn, &name, &version)
        })
        .await?
    }

    /// List all stored versions for a given server name, ordered by `version`
    /// (lexicographic — callers should not assume semver order).
    ///
    /// LEARNED: `ORDER BY version` is lexicographic on TEXT; "0.9.10" < "0.9.9"
    /// in byte order. Downstream consumers that need semver ordering must sort
    /// after retrieval.
    pub async fn list_versions(
        &self,
        name: &str,
    ) -> Result<Vec<ServerResponse>, RegistryStoreError> {
        let pool = self.pool.clone();
        let name = name.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            list_versions_sync(&conn, &name)
        })
        .await?
    }

    /// Count latest, non-deleted servers stored in the local mirror.
    pub async fn count_latest_servers(&self) -> Result<u32, RegistryStoreError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            count_latest_servers_sync(&conn)
        })
        .await?
    }

    pub async fn get_local_metadata(
        &self,
        name: &str,
        version: &str,
    ) -> Result<Option<serde_json::Value>, RegistryStoreError> {
        let pool = self.pool.clone();
        let name = name.to_string();
        let version = version.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            get_local_metadata_sync(&conn, &name, &version)
        })
        .await?
    }

    pub async fn set_local_metadata(
        &self,
        name: &str,
        version: &str,
        metadata: &serde_json::Value,
        updated_by: Option<&str>,
    ) -> Result<(), RegistryStoreError> {
        let pool = self.pool.clone();
        let name = name.to_string();
        let version = version.to_string();
        let metadata = metadata.clone();
        let updated_by = updated_by.map(str::to_string);
        tokio::task::spawn_blocking(move || {
            let mut conn = pool.get()?;
            set_local_metadata_sync(&mut conn, &name, &version, &metadata, updated_by.as_deref())
        })
        .await?
    }

    pub async fn delete_local_metadata(
        &self,
        name: &str,
        version: &str,
    ) -> Result<bool, RegistryStoreError> {
        let pool = self.pool.clone();
        let name = name.to_string();
        let version = version.to_string();
        tokio::task::spawn_blocking(move || {
            let mut conn = pool.get()?;
            delete_local_metadata_sync(&mut conn, &name, &version)
        })
        .await?
    }

    /// Upsert a batch of `ServerResponse` entries in a single transaction.
    ///
    /// `server_json` is re-serialized from the parsed struct — raw upstream
    /// bytes are never stored directly.
    ///
    /// Returns insert/update/delete counts observed during the upsert.
    pub async fn upsert_page(
        &self,
        servers: &[ServerResponse],
    ) -> Result<SyncStats, RegistryStoreError> {
        if servers.is_empty() {
            return Ok(SyncStats::default());
        }
        let pool = self.pool.clone();
        let servers = servers.to_vec();
        Ok(
            tokio::task::spawn_blocking(move || -> Result<SyncStats, RegistryStoreError> {
                let mut conn = pool.get()?;
                Ok(upsert_page_sync(&mut conn, &servers)?)
            })
            .await??,
        )
    }

    /// Recompute `is_latest` for all servers in a single batch UPDATE.
    ///
    /// Called once at the end of a full sync — not per-page — to avoid N+1 pattern.
    ///
    /// IMPORTANT: Uses the upstream `is_latest` flag that was stored during upsert.
    /// Does NOT recompute from `MAX(version)` — lexicographic MAX is incorrect for
    /// semver (e.g. "0.9.10" < "0.9.9" in byte order). Trusting upstream avoids
    /// this class of bug entirely.
    pub async fn update_is_latest(&self) -> Result<(), RegistryStoreError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = pool.get()?;
            update_is_latest_sync(&mut conn)
        })
        .await?
    }

    /// Full resync from the upstream MCP registry.
    ///
    /// Cursor-paginates through all pages at 100 servers/page, stages the full
    /// upstream result set in memory, then performs a single store update once
    /// the crawl completes successfully.
    ///
    /// Returns total count of rows upserted across all pages.
    pub async fn sync_from_upstream(
        &self,
        client: &McpRegistryClient,
        trigger: &'static str,
    ) -> Result<usize, RegistryStoreError> {
        let started_at = std::time::Instant::now();
        let mut total = 0usize;
        let mut page_num = 0usize;
        let mut cursor: Option<String> = None;
        let mut seen_cursors = HashSet::new();
        let mut staged = Vec::new();
        let mut sync_stats = SyncStats::default();

        tracing::info!(
            service = "mcpregistry",
            event = "sync.start",
            trigger,
            db_path = %self.db_path().display(),
            "starting full registry sync from upstream"
        );

        loop {
            let params = ListServersParams {
                limit: Some(100),
                cursor: cursor.clone(),
                ..Default::default()
            };

            // HTTP fetch — no pool connection held during this await.
            let page = client
                .list_servers(params)
                .await
                .map_err(|e| RegistryStoreError::Upstream(e.to_string()))?;

            let page_len = page.servers.len();
            page_num += 1;

            if page_len > 0 {
                total += page_len;
                staged.extend(page.servers);
                tracing::debug!(
                    service = "mcpregistry",
                    event = "sync.page",
                    page = page_num,
                    page_size = page_len,
                    total_so_far = total,
                    "staged page"
                );
            }

            match page.metadata.next_cursor {
                Some(c) if !c.is_empty() => {
                    if cursor.as_deref() == Some(c.as_str()) || !seen_cursors.insert(c.clone()) {
                        return Err(RegistryStoreError::Upstream(format!(
                            "non-advancing cursor returned by upstream: {c}"
                        )));
                    }
                    cursor = Some(c);
                }
                _ => break,
            }

            // Safety valve: if upstream returns empty page but a cursor, stop.
            if page_len == 0 {
                break;
            }
        }

        if !staged.is_empty() {
            sync_stats = self.upsert_page(&staged).await?;
        }
        self.update_is_latest().await?;

        tracing::info!(
            service = "mcpregistry",
            event = "sync.finish",
            trigger,
            db_path = %self.db_path().display(),
            total_servers = total,
            pages = page_num,
            inserted = sync_stats.inserted,
            updated = sync_stats.updated,
            deleted = sync_stats.deleted,
            elapsed_ms = started_at.elapsed().as_millis(),
            "registry sync complete"
        );

        Ok(total)
    }
}

// ── Sync helpers (run inside spawn_blocking) ──────────────────────────────────

#[allow(dead_code)]
fn list_servers_sync(
    conn: &Connection,
    params: &StoreListParams,
) -> Result<PagedServers, RegistryStoreError> {
    let limit = params.limit.unwrap_or(20).clamp(1, 100) as usize;

    let mut sql = format!(
        "SELECT s.server_name, s.version, s.server_json, s.response_meta_json, lm.meta_json, lm.updated_at, lm.updated_by \
         FROM registry_servers s \
         LEFT JOIN registry_server_meta lm \
           ON lm.server_name = s.server_name \
          AND lm.version = s.version \
          AND lm.namespace = '{}' \
         WHERE 1=1",
        super::LAB_REGISTRY_META_NAMESPACE
    );
    let mut args: Vec<rusqlite::types::Value> = Vec::new();

    // Status filter.
    if !params.include_deleted {
        sql.push_str(" AND s.status != 'deleted'");
    }
    if params.latest_only {
        sql.push_str(" AND s.is_latest = 1");
    }

    // LIKE search on server_name.
    if let Some(search) = &params.search {
        let escaped = escape_like(search);
        sql.push_str(" AND s.server_name LIKE '%' || ?  || '%' ESCAPE '\\'");
        args.push(escaped.into());
    }
    if let Some(featured) = params.featured {
        sql.push_str(" AND COALESCE(json_extract(lm.meta_json, '$.curation.featured'), 0) = ?");
        args.push((featured as i64).into());
    }
    if let Some(reviewed) = params.reviewed {
        sql.push_str(" AND COALESCE(json_extract(lm.meta_json, '$.trust.reviewed'), 0) = ?");
        args.push((reviewed as i64).into());
    }
    if let Some(recommended) = params.recommended {
        sql.push_str(
            " AND COALESCE(json_extract(lm.meta_json, '$.ux.recommended_for_homelab'), 0) = ?",
        );
        args.push((recommended as i64).into());
    }
    if let Some(hidden) = params.hidden {
        sql.push_str(" AND COALESCE(json_extract(lm.meta_json, '$.curation.hidden'), 0) = ?");
        args.push((hidden as i64).into());
    }
    if let Some(tag) = &params.tag {
        sql.push_str(" AND EXISTS (SELECT 1 FROM json_each(lm.meta_json, '$.curation.tags') WHERE value = ?)");
        args.push(tag.clone().into());
    }

    if let Some(version) = &params.version {
        sql.push_str(" AND s.version = ?");
        args.push(version.clone().into());
    }

    if let Some(updated_since) = &params.updated_since {
        sql.push_str(
            " AND s.upstream_updated_at IS NOT NULL \
              AND julianday(s.upstream_updated_at) IS NOT NULL \
              AND julianday(s.upstream_updated_at) >= julianday(?)",
        );
        args.push(updated_since.clone().into());
    }

    // Cursor: base64-decode → {"s": name, "v": version} → compound WHERE clause.
    if let Some(cursor_str) = &params.cursor {
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(cursor_str.as_bytes())
            .map_err(|e| RegistryStoreError::InvalidCursor(format!("base64 decode: {e}")))?;
        let cursor_data: CursorData = serde_json::from_slice(&decoded)
            .map_err(|e| RegistryStoreError::InvalidCursor(format!("json decode: {e}")))?;
        // Compound tiebreaker: OR is load-bearing to handle equal server_name with different version.
        sql.push_str(" AND (s.server_name > ? OR (s.server_name = ? AND s.version > ?))");
        args.push(cursor_data.s.clone().into());
        args.push(cursor_data.s.into());
        args.push(cursor_data.v.into());
    }

    sql.push_str(" ORDER BY s.server_name ASC, s.version ASC");

    // Fetch limit+1 to detect whether a next page exists, then truncate.
    sql.push_str(&format!(" LIMIT {}", limit + 1));

    let mut stmt = conn.prepare(&sql)?;
    use rusqlite::params_from_iter;
    let mut rows: Vec<ServerResponse> = stmt
        .query_map(params_from_iter(args.iter()), |row| {
            Ok((
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5).unwrap_or(None),
                row.get::<_, Option<String>>(6).unwrap_or(None),
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(
            |(
                server_json,
                response_meta_json,
                local_meta_json,
                local_updated_at,
                local_updated_by,
            )| {
                let local_meta = match (local_meta_json, local_updated_at) {
                    (Some(meta_json), Some(updated_at)) => Some(LocalMetadataRecord {
                        metadata: serde_json::from_str::<serde_json::Value>(&meta_json).map_err(
                            |e| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    4,
                                    rusqlite::types::Type::Text,
                                    Box::new(e),
                                )
                            },
                        )?,
                        updated_at,
                        updated_by: local_updated_by,
                    }),
                    _ => None,
                };
                decode_server_response(server_json, response_meta_json, local_meta).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })
            },
        )
        .collect::<Result<Vec<_>, _>>()?;

    // Determine next cursor before truncating.
    let has_more = rows.len() > limit;
    if has_more {
        rows.truncate(limit);
    }

    let next_cursor = if has_more {
        let last = rows.last().ok_or_else(|| {
            RegistryStoreError::InvalidCursor(
                "cannot build next cursor from an empty result page".to_string(),
            )
        })?;
        Some(encode_cursor(&last.server)?)
    } else {
        None
    };

    Ok(PagedServers {
        servers: rows,
        next_cursor,
    })
}

fn count_latest_servers_sync(conn: &Connection) -> Result<u32, RegistryStoreError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM registry_servers WHERE status != 'deleted' AND is_latest = 1",
        [],
        |row| row.get(0),
    )?;
    Ok(u32::try_from(count).unwrap_or(u32::MAX))
}

#[allow(dead_code)]
fn get_server_sync(
    conn: &Connection,
    name: &str,
    version: &str,
) -> Result<Option<ServerResponse>, RegistryStoreError> {
    let result = if version == "latest" {
        conn.query_row(
            &format!(
                "SELECT s.server_json, s.response_meta_json, lm.meta_json, lm.updated_at, lm.updated_by \
                 FROM registry_servers s \
                 LEFT JOIN registry_server_meta lm \
                   ON lm.server_name = s.server_name \
                  AND lm.version = s.version \
                  AND lm.namespace = '{}' \
                 WHERE s.server_name = ?1 AND s.is_latest = 1 LIMIT 1",
                super::LAB_REGISTRY_META_NAMESPACE
            ),
            rusqlite::params![name],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
    } else {
        conn.query_row(
            &format!(
                "SELECT s.server_json, s.response_meta_json, lm.meta_json, lm.updated_at, lm.updated_by \
                 FROM registry_servers s \
                 LEFT JOIN registry_server_meta lm \
                   ON lm.server_name = s.server_name \
                  AND lm.version = s.version \
                  AND lm.namespace = '{}' \
                 WHERE s.server_name = ?1 AND s.version = ?2 LIMIT 1",
                super::LAB_REGISTRY_META_NAMESPACE
            ),
            rusqlite::params![name, version],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
    };

    match result {
        Ok((
            server_json,
            response_meta_json,
            local_meta_json,
            local_updated_at,
            local_updated_by,
        )) => {
            let local_meta = match (local_meta_json, local_updated_at) {
                (Some(meta_json), Some(updated_at)) => Some(LocalMetadataRecord {
                    metadata: serde_json::from_str(&meta_json)?,
                    updated_at,
                    updated_by: local_updated_by,
                }),
                _ => None,
            };
            Ok(Some(decode_server_response(
                server_json,
                response_meta_json,
                local_meta,
            )?))
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(RegistryStoreError::Db(e)),
    }
}

#[allow(dead_code)]
fn list_versions_sync(
    conn: &Connection,
    name: &str,
) -> Result<Vec<ServerResponse>, RegistryStoreError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT s.server_json, s.response_meta_json, lm.meta_json, lm.updated_at, lm.updated_by \
         FROM registry_servers s \
         LEFT JOIN registry_server_meta lm \
           ON lm.server_name = s.server_name \
          AND lm.version = s.version \
          AND lm.namespace = '{}' \
         WHERE s.server_name = ?1 ORDER BY s.version ASC",
        super::LAB_REGISTRY_META_NAMESPACE
    ))?;

    let rows = stmt
        .query_map(rusqlite::params![name], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(
            |(
                server_json,
                response_meta_json,
                local_meta_json,
                local_updated_at,
                local_updated_by,
            )| {
                let local_meta = match (local_meta_json, local_updated_at) {
                    (Some(meta_json), Some(updated_at)) => Some(LocalMetadataRecord {
                        metadata: serde_json::from_str(&meta_json).map_err(|e| {
                            rusqlite::Error::FromSqlConversionFailure(
                                2,
                                rusqlite::types::Type::Text,
                                Box::new(e),
                            )
                        })?,
                        updated_at,
                        updated_by: local_updated_by,
                    }),
                    _ => None,
                };
                decode_server_response(server_json, response_meta_json, local_meta).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })
            },
        )
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

/// Upsert a batch of `ServerResponse` entries in a single transaction.
///
/// Re-serializes `resp.server` from the parsed struct — never stores raw upstream bytes.
#[allow(dead_code)]
fn upsert_page_sync(
    conn: &mut Connection,
    servers: &[ServerResponse],
) -> rusqlite::Result<SyncStats> {
    let tx = conn.transaction()?;
    let mut stats = SyncStats::default();

    for resp in servers {
        let server_json = serde_json::to_string(&resp.server)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let response_meta_json = resp
            .meta
            .as_ref()
            .map(|meta| serde_json::to_string(meta))
            .transpose()
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;

        let is_latest = resp
            .meta
            .as_ref()
            .and_then(|m| m.official.as_ref())
            .map(|ext| ext.is_latest)
            .unwrap_or(false) as i32;

        let status = resp
            .meta
            .as_ref()
            .and_then(|m| m.official.as_ref())
            .map(|ext| ext.status.as_str())
            .unwrap_or("active")
            .to_string();

        let updated_at: Option<String> = resp
            .meta
            .as_ref()
            .and_then(|m| m.official.as_ref())
            .and_then(|ext| ext.updated_at.clone());

        let existing = tx
            .query_row(
                "SELECT status, server_json, response_meta_json, upstream_updated_at
                 FROM registry_servers
                 WHERE server_name = ?1 AND version = ?2",
                rusqlite::params![resp.server.name, resp.server.version],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()?;

        let synced_at = jiff::Timestamp::now().to_string();

        tx.execute(
            "INSERT INTO registry_servers
             (server_name, version, is_latest, status, server_json, response_meta_json, upstream_updated_at, synced_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(server_name, version) DO UPDATE SET
               is_latest            = excluded.is_latest,
               status               = excluded.status,
               server_json          = excluded.server_json,
               response_meta_json   = excluded.response_meta_json,
               upstream_updated_at  = excluded.upstream_updated_at,
               synced_at            = excluded.synced_at",
            rusqlite::params![
                resp.server.name,
                resp.server.version,
                is_latest,
                status,
                server_json,
                response_meta_json,
                updated_at,
                synced_at,
            ],
        )?;

        match existing {
            None => {
                stats.inserted += 1;
                if status == "deleted" {
                    stats.deleted += 1;
                }
            }
            Some((
                existing_status,
                existing_server_json,
                existing_meta_json,
                existing_updated_at,
            )) => {
                let changed = existing_status != status
                    || existing_server_json != server_json
                    || existing_meta_json != response_meta_json
                    || existing_updated_at != updated_at;
                if changed {
                    if existing_status != "deleted" && status == "deleted" {
                        stats.deleted += 1;
                    } else {
                        stats.updated += 1;
                    }
                }
            }
        }
    }

    tx.commit()?;
    Ok(stats)
}

/// Recompute `is_latest` in a single batch UPDATE.
///
/// Uses the upstream `is_latest` flag stored during upsert as the source of truth.
/// If upstream sent conflicting flags (two rows both is_latest=1), the one with the
/// lexicographically greatest version wins as a tiebreaker — acceptable because
/// upstream should guarantee at most one is_latest per server_name.
///
/// IMPORTANT: Does NOT use MAX(version) for recomputation — lexicographic MAX is
/// incorrect for semver ("0.9.10" < "0.9.9"). Trusting upstream avoids this bug.
#[allow(dead_code)]
fn update_is_latest_sync(conn: &mut Connection) -> Result<(), RegistryStoreError> {
    let tx = conn.transaction()?;

    let mut stmt = tx.prepare(
        "SELECT server_name, version
         FROM registry_servers
         WHERE is_latest = 1
           AND status != 'deleted'
         ORDER BY server_name ASC, version ASC",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);

    let mut latest_by_server = std::collections::BTreeMap::<String, String>::new();
    for (server_name, version) in rows {
        latest_by_server
            .entry(server_name)
            .and_modify(|current| {
                if compare_versions(&version, current).is_gt() {
                    *current = version.clone();
                }
            })
            .or_insert(version);
    }

    tx.execute("UPDATE registry_servers SET is_latest = 0", [])?;

    for (server_name, version) in latest_by_server {
        tx.execute(
            "UPDATE registry_servers
             SET is_latest = 1
             WHERE server_name = ?1
               AND version = ?2
               AND status != 'deleted'",
            rusqlite::params![server_name, version],
        )?;
    }

    tx.commit()?;
    Ok(())
}

fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    let parse = |value: &str| {
        value
            .split('.')
            .map(|segment| segment.parse::<u64>())
            .collect::<Result<Vec<_>, _>>()
    };
    match (parse(left), parse(right)) {
        (Ok(left_parts), Ok(right_parts)) => left_parts.cmp(&right_parts),
        _ => left.cmp(right),
    }
}

// ── LIKE escaping ─────────────────────────────────────────────────────────────

/// Escape `\`, `%`, and `_` in a user-supplied string so they are treated as
/// literals by a SQLite `LIKE` expression using `ESCAPE '\'`.
///
/// Escape order is critical: backslash must be escaped first to prevent
/// double-escaping the escape character itself.
fn escape_like(s: &str) -> String {
    s.replace('\\', r"\\")
        .replace('%', r"\%")
        .replace('_', r"\_")
}

fn encode_cursor(server: &ServerJSON) -> Result<String, RegistryStoreError> {
    let data = CursorData {
        s: server.name.clone(),
        v: server.version.clone(),
    };
    let json = serde_json::to_vec(&data)?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json))
}

fn decode_server_response(
    server_json: String,
    response_meta_json: Option<String>,
    local_meta: Option<LocalMetadataRecord>,
) -> Result<ServerResponse, RegistryStoreError> {
    let server: ServerJSON = serde_json::from_str(&server_json)?;
    let mut meta = match response_meta_json {
        Some(raw) => serde_json::from_str::<ResponseMeta>(&raw)?,
        None => ResponseMeta::default(),
    };
    if let Some(local) = local_meta {
        meta.insert_extension(
            super::LAB_REGISTRY_META_NAMESPACE,
            audited_metadata_value(
                local.metadata,
                &local.updated_at,
                local.updated_by.as_deref(),
            )?,
        );
    }
    let meta = if meta.is_empty() { None } else { Some(meta) };
    Ok(ServerResponse { server, meta })
}

fn get_local_metadata_sync(
    conn: &Connection,
    name: &str,
    version: &str,
) -> Result<Option<serde_json::Value>, RegistryStoreError> {
    let result = conn.query_row(
        "SELECT meta_json, updated_at, updated_by FROM registry_server_meta WHERE server_name = ?1 AND version = ?2 AND namespace = ?3",
        rusqlite::params![
            name,
            version,
            super::LAB_REGISTRY_META_NAMESPACE
        ],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        },
    );
    match result {
        Ok((raw, updated_at, updated_by)) => Ok(Some(audited_metadata_value(
            serde_json::from_str(&raw)?,
            &updated_at,
            updated_by.as_deref(),
        )?)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(RegistryStoreError::Db(e)),
    }
}

fn set_local_metadata_sync(
    conn: &mut Connection,
    name: &str,
    version: &str,
    metadata: &serde_json::Value,
    updated_by: Option<&str>,
) -> Result<(), RegistryStoreError> {
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO registry_server_meta (server_name, version, namespace, meta_json, updated_at, updated_by)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(server_name, version, namespace) DO UPDATE SET
           meta_json = excluded.meta_json,
           updated_at = excluded.updated_at,
           updated_by = excluded.updated_by",
        rusqlite::params![
            name,
            version,
            super::LAB_REGISTRY_META_NAMESPACE,
            serde_json::to_string(metadata)?,
            jiff::Timestamp::now().to_string(),
            updated_by,
        ],
    )?;
    tx.commit()?;
    Ok(())
}

fn delete_local_metadata_sync(
    conn: &mut Connection,
    name: &str,
    version: &str,
) -> Result<bool, RegistryStoreError> {
    let tx = conn.transaction()?;
    let deleted = tx.execute(
        "DELETE FROM registry_server_meta WHERE server_name = ?1 AND version = ?2 AND namespace = ?3",
        rusqlite::params![
            name,
            version,
            super::LAB_REGISTRY_META_NAMESPACE,
        ],
    )?;
    tx.commit()?;
    Ok(deleted > 0)
}

fn truncate_utf8_bytes(s: String, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s;
    }

    let mut cut = 0usize;
    for (idx, _) in s.char_indices() {
        if idx > max_bytes {
            break;
        }
        cut = idx;
    }

    s[..cut].to_string()
}

// ── Migration ─────────────────────────────────────────────────────────────────

/// Apply pending schema migrations using PRAGMA user_version as the version
/// counter.
///
/// **MUST use `BEGIN EXCLUSIVE`** to prevent a TOCTOU race: two processes
/// starting simultaneously can both read `user_version = 0` and both attempt
/// to run the schema DDL.  The exclusive lock serialises them so only one
/// applies the migration.
fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("BEGIN EXCLUSIVE;")?;
    let version: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version < 1 {
        conn.execute_batch(include_str!("store_schema.sql"))?;
        conn.pragma_update(None, "user_version", 3)?;
    }
    if version == 1 {
        conn.execute_batch(
            "ALTER TABLE registry_servers ADD COLUMN response_meta_json TEXT;
             CREATE TABLE IF NOT EXISTS registry_server_meta (
                 server_name TEXT NOT NULL,
                 version     TEXT NOT NULL,
                 namespace   TEXT NOT NULL,
                 meta_json   TEXT NOT NULL,
                 updated_at  TEXT NOT NULL,
                 PRIMARY KEY (server_name, version, namespace)
             );
             CREATE INDEX IF NOT EXISTS idx_registry_server_meta_lookup
                 ON registry_server_meta(server_name, version, namespace);",
        )?;
        conn.pragma_update(None, "user_version", 2)?;
    } else if version == 2 {
        conn.execute_batch("ALTER TABLE registry_server_meta ADD COLUMN updated_by TEXT;")?;
        conn.pragma_update(None, "user_version", 3)?;
    }
    conn.execute_batch("COMMIT;")?;
    Ok(())
}

fn audited_metadata_value(
    metadata: serde_json::Value,
    updated_at: &str,
    updated_by: Option<&str>,
) -> Result<serde_json::Value, RegistryStoreError> {
    let mut typed: LabRegistryMetadata = serde_json::from_value(metadata)?;
    typed.audit = Some(LabRegistryAudit {
        updated_at: Some(updated_at.to_string()),
        updated_by: updated_by.map(str::to_string),
    });
    Ok(serde_json::to_value(typed)?)
}

// ── Permissions ───────────────────────────────────────────────────────────────

/// Set the database file to owner-read/write only (0o600).
///
/// Called immediately after pool creation — before any data is written — so
/// the file is never world-readable even momentarily.
fn set_restrictive_permissions(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    // On non-Unix targets permissions are a no-op (homelab is Linux-only).
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DB_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_db_path(prefix: &str) -> PathBuf {
        let unique = TEST_DB_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("{prefix}-{}-{unique}.db", std::process::id()))
    }

    async fn temp_store() -> RegistryStore {
        let path = temp_db_path("lab-registry-test");
        RegistryStore::open(&path).await.unwrap()
    }

    #[tokio::test]
    async fn store_opens_and_migrates() {
        // Verify open() succeeds and migration runs without panic.
        let store = temp_store().await;
        // Pool is accessible.
        let conn = store.pool().get().unwrap();
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, 3, "schema version must be 3 after migration");
    }

    #[tokio::test]
    async fn migration_is_idempotent() {
        let path = temp_db_path("lab-registry-idem");
        // Open twice — second open re-runs migrate() which must be a no-op.
        drop(RegistryStore::open(&path).await.unwrap());
        let store2 = RegistryStore::open(&path).await.unwrap();
        let conn = store2.pool().get().unwrap();
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, 3);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn store_sets_restrictive_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp_db_path("lab-registry-perm");
        drop(RegistryStore::open(&path).await.unwrap());
        let meta = std::fs::metadata(&path).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "DB file must be 0o600");
    }

    fn make_server_response(name: &str, version: &str, is_latest: bool) -> ServerResponse {
        use labby_apis::mcpregistry::types::{RegistryExtensions, ResponseMeta};
        ServerResponse {
            server: ServerJSON {
                schema: None,
                name: name.to_string(),
                title: None,
                description: format!("Test server {name}"),
                version: version.to_string(),
                packages: vec![],
                remotes: vec![],
                repository: None,
                icons: vec![],
                website_url: None,
            },
            meta: Some(ResponseMeta {
                official: Some(RegistryExtensions {
                    is_latest,
                    published_at: "2025-01-01T00:00:00Z".to_string(),
                    status: "active".to_string(),
                    status_changed_at: "2025-01-01T00:00:00Z".to_string(),
                    status_message: None,
                    updated_at: Some("2025-01-01T00:00:00Z".to_string()),
                }),
                extensions: Default::default(),
            }),
        }
    }

    #[tokio::test]
    async fn upsert_and_get_server() {
        let store = temp_store().await;

        let resp = make_server_response("io.github.user/weather", "1.0.0", true);
        let stats = store.upsert_page(&[resp]).await.unwrap();
        assert_eq!(stats.inserted, 1);
        assert_eq!(stats.updated, 0);
        assert_eq!(stats.deleted, 0);

        let result = store
            .get_server("io.github.user/weather", "latest")
            .await
            .unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().server.version, "1.0.0");
    }

    #[tokio::test]
    async fn get_server_by_version() {
        let store = temp_store().await;

        let resp = make_server_response("io.github.user/weather", "2.0.0", false);
        store.upsert_page(&[resp]).await.unwrap();

        let result = store
            .get_server("io.github.user/weather", "2.0.0")
            .await
            .unwrap();
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn get_server_not_found_returns_none() {
        let store = temp_store().await;

        let result = store.get_server("does.not/exist", "latest").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn list_versions_returns_all() {
        let store = temp_store().await;

        let r1 = make_server_response("io.github.user/weather", "1.0.0", false);
        let r2 = make_server_response("io.github.user/weather", "2.0.0", true);
        store.upsert_page(&[r1, r2]).await.unwrap();

        let versions = store.list_versions("io.github.user/weather").await.unwrap();
        assert_eq!(versions.len(), 2);
    }

    #[tokio::test]
    async fn list_servers_returns_paged() {
        let store = temp_store().await;

        let mut batch = Vec::new();
        for i in 0..5u32 {
            batch.push(make_server_response(
                &format!("io.github.user/server{i:02}"),
                "1.0.0",
                true,
            ));
        }
        store.upsert_page(&batch).await.unwrap();

        let params = StoreListParams {
            limit: Some(3),
            ..Default::default()
        };
        let page = store.list_servers(params).await.unwrap();
        assert_eq!(page.servers.len(), 3);
        assert!(page.next_cursor.is_some());

        // Fetch second page.
        let params2 = StoreListParams {
            limit: Some(3),
            cursor: page.next_cursor,
            ..Default::default()
        };
        let page2 = store.list_servers(params2).await.unwrap();
        assert_eq!(page2.servers.len(), 2);
        assert!(page2.next_cursor.is_none());
    }

    #[tokio::test]
    async fn list_servers_can_filter_to_latest_versions() {
        let store = temp_store().await;

        let old = make_server_response("io.github.user/weather", "1.0.0", false);
        let latest = make_server_response("io.github.user/weather", "2.0.0", true);
        store.upsert_page(&[old, latest]).await.unwrap();

        let params = StoreListParams {
            latest_only: true,
            ..Default::default()
        };
        let page = store.list_servers(params).await.unwrap();

        assert_eq!(page.servers.len(), 1);
        assert_eq!(page.servers[0].server.version, "2.0.0");
    }

    #[tokio::test]
    async fn list_servers_excludes_deleted_by_default() {
        let store = temp_store().await;
        use labby_apis::mcpregistry::types::{RegistryExtensions, ResponseMeta};

        // Insert one active, one deleted.
        let active = make_server_response("io.github.user/active", "1.0.0", true);
        let mut deleted = make_server_response("io.github.user/deleted", "1.0.0", false);
        deleted.meta = Some(ResponseMeta {
            official: Some(RegistryExtensions {
                is_latest: false,
                published_at: "2025-01-01T00:00:00Z".to_string(),
                status: "deleted".to_string(),
                status_changed_at: "2025-01-01T00:00:00Z".to_string(),
                status_message: None,
                updated_at: None,
            }),
            extensions: Default::default(),
        });

        store.upsert_page(&[active, deleted]).await.unwrap();

        let params = StoreListParams::default();
        let page = store.list_servers(params).await.unwrap();
        // Only the active server should appear.
        assert_eq!(page.servers.len(), 1);
        assert_eq!(page.servers[0].server.name, "io.github.user/active");
    }

    #[tokio::test]
    async fn list_servers_filters_by_version_and_updated_since() {
        use labby_apis::mcpregistry::types::{RegistryExtensions, ResponseMeta};

        let store = temp_store().await;
        let mut older = make_server_response("io.github.user/weather", "1.0.0", false);
        older.meta = Some(ResponseMeta {
            official: Some(RegistryExtensions {
                is_latest: false,
                published_at: "2025-01-01T00:00:00Z".to_string(),
                status: "active".to_string(),
                status_changed_at: "2025-01-01T00:00:00Z".to_string(),
                status_message: None,
                updated_at: Some("2025-01-15T00:00:00Z".to_string()),
            }),
            extensions: Default::default(),
        });
        let mut newer = make_server_response("io.github.user/weather", "2.0.0", true);
        newer.meta = Some(ResponseMeta {
            official: Some(RegistryExtensions {
                is_latest: true,
                published_at: "2025-02-01T00:00:00Z".to_string(),
                status: "active".to_string(),
                status_changed_at: "2025-02-01T00:00:00Z".to_string(),
                status_message: None,
                updated_at: Some("2025-02-15T00:00:00Z".to_string()),
            }),
            extensions: Default::default(),
        });
        store.upsert_page(&[older, newer]).await.unwrap();

        let filtered = store
            .list_servers(StoreListParams {
                version: Some("2.0.0".to_string()),
                updated_since: Some("2025-02-01T00:00:00Z".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(filtered.servers.len(), 1);
        assert_eq!(filtered.servers[0].server.version, "2.0.0");

        let no_match = store
            .list_servers(StoreListParams {
                version: Some("1.0.0".to_string()),
                updated_since: Some("2025-02-01T00:00:00Z".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(no_match.servers.is_empty());
    }

    #[tokio::test]
    async fn list_servers_updated_since_handles_rfc3339_offsets() {
        use labby_apis::mcpregistry::types::{RegistryExtensions, ResponseMeta};

        let store = temp_store().await;
        let mut response = make_server_response("io.github.user/weather", "1.0.0", true);
        response.meta = Some(ResponseMeta {
            official: Some(RegistryExtensions {
                is_latest: true,
                published_at: "2025-02-01T00:00:00Z".to_string(),
                status: "active".to_string(),
                status_changed_at: "2025-02-01T00:00:00Z".to_string(),
                status_message: None,
                updated_at: Some("2025-02-01T01:00:00+01:00".to_string()),
            }),
            extensions: Default::default(),
        });
        store.upsert_page(&[response]).await.unwrap();

        let filtered = store
            .list_servers(StoreListParams {
                updated_since: Some("2025-01-31T23:30:00Z".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(filtered.servers.len(), 1);
    }

    #[tokio::test]
    async fn update_is_latest_batch_runs() {
        let store = temp_store().await;

        let resp = make_server_response("io.github.user/weather", "1.0.0", true);
        store.upsert_page(&[resp]).await.unwrap();

        // Should not panic or error.
        store.update_is_latest().await.unwrap();

        let result = store
            .get_server("io.github.user/weather", "latest")
            .await
            .unwrap();
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn update_is_latest_prefers_semver_order_over_lexicographic_order() {
        let store = temp_store().await;

        let older = make_server_response("io.github.user/weather", "0.9.9", true);
        let newer = make_server_response("io.github.user/weather", "0.9.10", true);
        store.upsert_page(&[older, newer]).await.unwrap();

        store.update_is_latest().await.unwrap();

        let latest = store
            .get_server("io.github.user/weather", "latest")
            .await
            .unwrap()
            .expect("latest server");
        assert_eq!(latest.server.version, "0.9.10");
    }

    #[tokio::test]
    async fn count_latest_servers_excludes_deleted_and_non_latest_rows() {
        let store = temp_store().await;
        use labby_apis::mcpregistry::types::{RegistryExtensions, ResponseMeta};

        let mut deleted = make_server_response("io.github.user/beta", "2.0.0", true);
        deleted.meta = Some(ResponseMeta {
            official: Some(RegistryExtensions {
                is_latest: true,
                published_at: "2025-01-01T00:00:00Z".to_string(),
                status: "deleted".to_string(),
                status_changed_at: "2025-01-01T00:00:00Z".to_string(),
                status_message: None,
                updated_at: None,
            }),
            extensions: Default::default(),
        });
        store
            .upsert_page(&[
                make_server_response("io.github.user/alpha", "1.0.0", true),
                make_server_response("io.github.user/alpha", "0.9.0", false),
                deleted,
            ])
            .await
            .unwrap();

        assert_eq!(store.count_latest_servers().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn escape_like_escapes_special_chars() {
        assert_eq!(escape_like("a%b_c\\d"), r"a\%b\_c\\d");
    }
}
