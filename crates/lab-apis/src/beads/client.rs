//! Dolt-backed Beads client.
//!
//! Connects to a Dolt SQL server over the MySQL protocol. Each Dolt database
//! on the server is treated as one Beads project. Queries are fully-qualified
//! (`<project>.<table>`) so callers can switch projects per request without a
//! `USE` round trip.

use std::collections::BTreeMap;

use mysql_async::prelude::*;
use mysql_async::{Conn, Opts, OptsBuilder, Pool, Row};

use super::error::BeadsError;
use super::types::{
    BeadsContext, BeadsHealth, Comment, ContractStatus, Dependency, DependencyGraph, DoltVersion,
    Issue, IssueDetail, Project, StatusCount, StatusSummary,
};

/// Connection parameters for the Dolt SQL server.
///
/// `Debug` is intentionally hand-written and never includes the password —
/// per the lab-apis library invariant, anything holding secrets must redact in
/// `Debug`.
#[derive(Clone)]
pub struct DoltConnection {
    pub url: String,
    pub user: Option<String>,
    pub password: Option<String>,
    pub default_project: Option<String>,
}

impl std::fmt::Debug for DoltConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DoltConnection")
            .field("url", &self.url)
            .field("user", &self.user)
            .field("password", &self.password.as_ref().map(|_| "***"))
            .field("default_project", &self.default_project)
            .finish()
    }
}

impl DoltConnection {
    fn into_opts(self) -> Result<Opts, BeadsError> {
        let base = Opts::from_url(&self.url)?;
        let mut builder = OptsBuilder::from_opts(base);
        if let Some(user) = self.user.as_deref().filter(|s| !s.is_empty()) {
            builder = builder.user(Some(user));
        }
        if let Some(password) = self.password.as_deref() {
            builder = builder.pass(Some(password));
        }
        Ok(builder.into())
    }
}

/// Pool-backed Dolt SQL client.
#[derive(Clone)]
pub struct BeadsClient {
    pool: Pool,
    default_project: Option<String>,
}

impl std::fmt::Debug for BeadsClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BeadsClient")
            .field("default_project", &self.default_project)
            .finish_non_exhaustive()
    }
}

const SYSTEM_DATABASES: &[&str] = &[
    "information_schema",
    "mysql",
    "performance_schema",
    "sys",
    "dolt_cluster",
];

// Maximum length for a Beads status filter value. The actual `issues.status`
// column is `VARCHAR(32)`, so anything longer than that is meaningless.
// Beads supports user-defined statuses via `custom_statuses`, so the SDK no
// longer hardcodes the canonical built-ins — the value travels as a bound
// parameter and is validated for shape only.
const MAX_STATUS_LEN: usize = 64;

// Stable projection of `issues` columns. Kept narrow on purpose:
// - we only surface what the Lab UI displays
// - columns added in later migrations (e.g. `started_at` from migration 0027)
//   are deliberately omitted so the same projection works against older Beads
//   schemas, including the `WHERE status = 'open'` fallback path used when the
//   `ready_issues` view is missing.
const ISSUE_COLUMNS: &str = "id, title, description, status, priority, issue_type, assignee, \
    created_by, owner, external_ref, created_at, updated_at, closed_at, due_at, defer_until";

impl BeadsClient {
    /// Build a client from connection parameters. The pool itself is lazy — no
    /// network is opened until the first query.
    pub fn new(connection: DoltConnection) -> Result<Self, BeadsError> {
        let default_project = connection.default_project.clone().filter(|s| !s.is_empty());
        let opts = connection.into_opts()?;
        Ok(Self {
            pool: Pool::new(opts),
            default_project,
        })
    }

    #[must_use]
    pub fn default_project(&self) -> Option<&str> {
        self.default_project.as_deref()
    }

    pub fn contract_status(&self) -> ContractStatus {
        ContractStatus {
            status: "dolt_sql_implemented",
            reason: "Beads is exposed through a Dolt SQL server over the MySQL protocol. Lab v1 is read-only and never writes issues, comments, dependencies, or Dolt state.",
            safe_v1_actions: &[
                "contract.status",
                "health.status",
                "version.get",
                "context.get",
                "status.summary",
                "project.list",
                "issue.list",
                "issue.ready",
                "issue.show",
                "graph.show",
            ],
            deferred: &[
                "issue.create",
                "issue.update",
                "issue.close",
                "comments.add",
                "dep.add",
                "dolt.push",
                "sql.query",
            ],
        }
    }

    /// Resolve the project to use for a request, falling back to the default.
    pub fn resolve_project(&self, requested: Option<&str>) -> Result<String, BeadsError> {
        let raw = requested
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| self.default_project.clone())
            .ok_or_else(|| BeadsError::NotConfigured {
                message: "no project specified and no BEADS_DEFAULT_PROJECT configured".into(),
            })?;
        validate_identifier(&raw)?;
        Ok(raw)
    }

    /// `SHOW DATABASES`, filtered to user-visible DBs.
    ///
    /// Names that fail `validate_identifier` are dropped — every other action
    /// requires a project name that re-passes that check before being
    /// interpolated as a backtick-quoted database name, so surfacing names the
    /// dispatcher would later refuse just gives the UI a project that can't be
    /// queried.
    pub async fn databases(&self) -> Result<Vec<Project>, BeadsError> {
        let mut conn = self.conn().await?;
        let rows: Vec<String> = conn.query("SHOW DATABASES").await?;
        Ok(rows
            .into_iter()
            .filter(|name| {
                let lower = name.to_ascii_lowercase();
                !SYSTEM_DATABASES.contains(&lower.as_str())
            })
            .filter(|name| validate_identifier(name).is_ok())
            .map(|name| Project { name })
            .collect())
    }

    /// `SELECT @@version` plus best-effort `dolt_version()`.
    pub async fn version(&self) -> Result<DoltVersion, BeadsError> {
        let mut conn = self.conn().await?;
        let version: Option<String> = conn.query_first("SELECT @@version").await?;
        let dolt_version: Option<String> = conn
            .query_first("SELECT dolt_version()")
            .await
            .ok()
            .flatten();
        Ok(DoltVersion {
            version: version.unwrap_or_else(|| "unknown".to_string()),
            dolt_version,
        })
    }

    /// Headline counters for the requested project.
    pub async fn context(&self, project: Option<&str>) -> Result<BeadsContext, BeadsError> {
        let project = self.resolve_project(project)?;
        let mut conn = self.conn().await?;
        let total: Option<u64> = conn
            .query_first(format!("SELECT COUNT(*) FROM `{project}`.issues"))
            .await?;
        let open: Option<u64> = conn
            .query_first(format!(
                "SELECT COUNT(*) FROM `{project}`.issues WHERE status = 'open'"
            ))
            .await?;
        Ok(BeadsContext {
            project,
            total_issues: total.unwrap_or(0),
            open_issues: open.unwrap_or(0),
        })
    }

    /// `SELECT status, COUNT(*) FROM <project>.issues GROUP BY status`.
    pub async fn status_summary(&self, project: Option<&str>) -> Result<StatusSummary, BeadsError> {
        let project = self.resolve_project(project)?;
        let mut conn = self.conn().await?;
        let rows: Vec<(String, u64)> = conn
            .query(format!(
                "SELECT status, COUNT(*) AS c FROM `{project}`.issues GROUP BY status \
                 ORDER BY c DESC"
            ))
            .await?;
        let by_status: Vec<StatusCount> = rows
            .into_iter()
            .map(|(status, count)| StatusCount { status, count })
            .collect();
        let total = by_status.iter().map(|row| row.count).sum();
        Ok(StatusSummary {
            project,
            total,
            by_status,
        })
    }

    /// List issues, optionally filtered by stored status.
    pub async fn list(
        &self,
        project: Option<&str>,
        status: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Vec<Issue>, BeadsError> {
        let project = self.resolve_project(project)?;
        let mut conn = self.conn().await?;
        let limit_value = limit.unwrap_or(100).clamp(1, 500) as i64;
        let issues: Vec<Issue> = if let Some(status) = status {
            validate_status(status)?;
            conn.exec_map(
                format!(
                    "SELECT {ISSUE_COLUMNS} FROM `{project}`.issues \
                     WHERE status = ? ORDER BY priority ASC, updated_at DESC LIMIT ?"
                ),
                (status, limit_value),
                row_to_issue,
            )
            .await?
        } else {
            conn.exec_map(
                format!(
                    "SELECT {ISSUE_COLUMNS} FROM `{project}`.issues \
                     ORDER BY priority ASC, updated_at DESC LIMIT ?"
                ),
                (limit_value,),
                row_to_issue,
            )
            .await?
        };
        attach_labels(&mut conn, &project, issues).await
    }

    /// List ready (unblocked) issues. Reads the `ready_issues` view if the
    /// schema is recent enough; falls back to `issues WHERE status='open'`.
    pub async fn ready(
        &self,
        project: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Vec<Issue>, BeadsError> {
        let project = self.resolve_project(project)?;
        let mut conn = self.conn().await?;
        let limit_value = limit.unwrap_or(100).clamp(1, 500) as i64;
        let issues: Vec<Issue> = match conn
            .exec_map(
                format!(
                    "SELECT {ISSUE_COLUMNS} FROM `{project}`.ready_issues \
                     ORDER BY priority ASC, created_at ASC LIMIT ?"
                ),
                (limit_value,),
                row_to_issue,
            )
            .await
        {
            Ok(rows) => rows,
            Err(err) if is_missing_relation(&err) => {
                conn.exec_map(
                    format!(
                        "SELECT {ISSUE_COLUMNS} FROM `{project}`.issues \
                         WHERE status = 'open' \
                         ORDER BY priority ASC, created_at ASC LIMIT ?"
                    ),
                    (limit_value,),
                    row_to_issue,
                )
                .await?
            }
            Err(err) => return Err(err.into()),
        };
        attach_labels(&mut conn, &project, issues).await
    }

    /// Issue + dependency context + comments for the detail panel.
    pub async fn show(&self, project: Option<&str>, id: &str) -> Result<IssueDetail, BeadsError> {
        let project = self.resolve_project(project)?;
        let mut conn = self.conn().await?;
        let issue: Option<Issue> = conn
            .exec_map(
                format!("SELECT {ISSUE_COLUMNS} FROM `{project}`.issues WHERE id = ? LIMIT 1"),
                (id,),
                row_to_issue,
            )
            .await?
            .into_iter()
            .next();
        let mut issue = issue.ok_or_else(|| BeadsError::Query {
            command: "issue.show".into(),
            message: format!("issue not found: {id}"),
        })?;
        let labels: Vec<String> = conn
            .exec(
                format!("SELECT label FROM `{project}`.labels WHERE issue_id = ?"),
                (id,),
            )
            .await?;
        issue.labels = labels;
        let blocked_by: Vec<Dependency> = conn
            .exec_map(
                format!(
                    "SELECT issue_id, depends_on_id, type, created_at, created_by \
                     FROM `{project}`.dependencies WHERE issue_id = ? AND type = 'blocks'"
                ),
                (id,),
                row_to_dependency,
            )
            .await?;
        let blocks: Vec<Dependency> = conn
            .exec_map(
                format!(
                    "SELECT issue_id, depends_on_id, type, created_at, created_by \
                     FROM `{project}`.dependencies WHERE depends_on_id = ? AND type = 'blocks'"
                ),
                (id,),
                row_to_dependency,
            )
            .await?;
        let parents: Vec<Dependency> = conn
            .exec_map(
                format!(
                    "SELECT issue_id, depends_on_id, type, created_at, created_by \
                     FROM `{project}`.dependencies \
                     WHERE issue_id = ? AND type = 'parent-child'"
                ),
                (id,),
                row_to_dependency,
            )
            .await?;
        let children: Vec<Dependency> = conn
            .exec_map(
                format!(
                    "SELECT issue_id, depends_on_id, type, created_at, created_by \
                     FROM `{project}`.dependencies \
                     WHERE depends_on_id = ? AND type = 'parent-child'"
                ),
                (id,),
                row_to_dependency,
            )
            .await?;
        let comments: Vec<Comment> = conn
            .exec_map(
                format!(
                    "SELECT id, author, text, created_at FROM `{project}`.comments \
                     WHERE issue_id = ? ORDER BY created_at ASC"
                ),
                (id,),
                row_to_comment,
            )
            .await?;
        Ok(IssueDetail {
            issue,
            blocked_by,
            blocks,
            parents,
            children,
            comments,
        })
    }

    /// Walk the dependency graph from a root issue, capped at 100 nodes.
    pub async fn graph(
        &self,
        project: Option<&str>,
        id: &str,
    ) -> Result<DependencyGraph, BeadsError> {
        let project = self.resolve_project(project)?;
        let mut conn = self.conn().await?;
        let mut nodes: BTreeMap<String, Issue> = BTreeMap::new();
        let mut edges: Vec<Dependency> = Vec::new();
        let mut frontier: Vec<String> = vec![id.to_string()];
        let mut visited = std::collections::BTreeSet::new();
        while let Some(current) = frontier.pop() {
            if !visited.insert(current.clone()) {
                continue;
            }
            if nodes.len() >= 100 {
                break;
            }
            if let Some(issue) = conn
                .exec_map(
                    format!("SELECT {ISSUE_COLUMNS} FROM `{project}`.issues WHERE id = ? LIMIT 1"),
                    (&current,),
                    row_to_issue,
                )
                .await?
                .into_iter()
                .next()
            {
                nodes.insert(current.clone(), issue);
            }
            let outgoing: Vec<Dependency> = conn
                .exec_map(
                    format!(
                        "SELECT issue_id, depends_on_id, type, created_at, created_by \
                         FROM `{project}`.dependencies WHERE issue_id = ?"
                    ),
                    (&current,),
                    row_to_dependency,
                )
                .await?;
            let incoming: Vec<Dependency> = conn
                .exec_map(
                    format!(
                        "SELECT issue_id, depends_on_id, type, created_at, created_by \
                         FROM `{project}`.dependencies WHERE depends_on_id = ?"
                    ),
                    (&current,),
                    row_to_dependency,
                )
                .await?;
            for dep in &outgoing {
                if !visited.contains(&dep.depends_on_id) {
                    frontier.push(dep.depends_on_id.clone());
                }
            }
            for dep in &incoming {
                if !visited.contains(&dep.issue_id) {
                    frontier.push(dep.issue_id.clone());
                }
            }
            edges.extend(outgoing);
            edges.extend(incoming);
        }
        edges.sort_by(|a, b| {
            a.issue_id
                .cmp(&b.issue_id)
                .then_with(|| a.depends_on_id.cmp(&b.depends_on_id))
                .then_with(|| a.r#type.cmp(&b.r#type))
        });
        edges.dedup_by(|a, b| {
            a.issue_id == b.issue_id && a.depends_on_id == b.depends_on_id && a.r#type == b.r#type
        });
        Ok(DependencyGraph {
            root: id.to_string(),
            nodes: nodes.into_values().collect(),
            edges,
        })
    }

    /// Reachability check — does the pool produce a working session?
    pub async fn health_status(&self) -> Result<BeadsHealth, BeadsError> {
        match self.version().await {
            Ok(v) => Ok(BeadsHealth {
                reachable: true,
                status: "available",
                version: Some(v.version),
                default_project: self.default_project.clone(),
                message: None,
            }),
            Err(err) => Ok(BeadsHealth {
                reachable: false,
                status: "unavailable",
                version: None,
                default_project: self.default_project.clone(),
                message: Some(err.to_string()),
            }),
        }
    }

    async fn conn(&self) -> Result<Conn, BeadsError> {
        self.pool
            .get_conn()
            .await
            .map_err(|err| BeadsError::Connect {
                message: err.to_string(),
            })
    }
}

fn validate_identifier(value: &str) -> Result<(), BeadsError> {
    if value.is_empty() {
        return Err(BeadsError::InvalidIdentifier {
            value: value.to_string(),
            message: "identifier is empty".into(),
        });
    }
    for ch in value.chars() {
        if !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-') {
            return Err(BeadsError::InvalidIdentifier {
                value: value.to_string(),
                message: format!("contains disallowed character: {ch:?}"),
            });
        }
    }
    Ok(())
}

fn validate_status(status: &str) -> Result<(), BeadsError> {
    if status.is_empty() {
        return Err(BeadsError::InvalidIdentifier {
            value: status.to_string(),
            message: "status filter is empty".into(),
        });
    }
    if status.len() > MAX_STATUS_LEN {
        return Err(BeadsError::InvalidIdentifier {
            value: status.to_string(),
            message: format!("status filter exceeds {MAX_STATUS_LEN} characters"),
        });
    }
    for ch in status.chars() {
        if ch.is_control() || ch.is_whitespace() {
            return Err(BeadsError::InvalidIdentifier {
                value: status.to_string(),
                message: "status filter must not contain whitespace or control characters".into(),
            });
        }
    }
    Ok(())
}

async fn attach_labels(
    conn: &mut Conn,
    project: &str,
    mut issues: Vec<Issue>,
) -> Result<Vec<Issue>, BeadsError> {
    if issues.is_empty() {
        return Ok(issues);
    }
    let placeholders = std::iter::repeat_n("?", issues.len())
        .collect::<Vec<_>>()
        .join(",");
    let ids: Vec<String> = issues.iter().map(|i| i.id.clone()).collect();
    let rows: Vec<(String, String)> = conn
        .exec(
            format!(
                "SELECT issue_id, label FROM `{project}`.labels WHERE issue_id IN ({placeholders})"
            ),
            ids.clone(),
        )
        .await?;
    let mut by_issue: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (issue_id, label) in rows {
        by_issue.entry(issue_id).or_default().push(label);
    }
    for issue in &mut issues {
        if let Some(labels) = by_issue.remove(&issue.id) {
            issue.labels = labels;
        }
    }
    Ok(issues)
}

fn row_to_issue(mut row: Row) -> Issue {
    Issue {
        id: take_string(&mut row, "id"),
        title: take_string(&mut row, "title"),
        description: take_string(&mut row, "description"),
        status: take_string(&mut row, "status"),
        priority: take_opt::<i32>(&mut row, "priority").unwrap_or(2),
        issue_type: take_string(&mut row, "issue_type"),
        assignee: take_opt(&mut row, "assignee"),
        created_by: take_opt(&mut row, "created_by"),
        owner: take_opt(&mut row, "owner"),
        external_ref: take_opt(&mut row, "external_ref"),
        created_at: take_datetime_string(&mut row, "created_at"),
        updated_at: take_datetime_string(&mut row, "updated_at"),
        closed_at: take_datetime_string(&mut row, "closed_at"),
        due_at: take_datetime_string(&mut row, "due_at"),
        defer_until: take_datetime_string(&mut row, "defer_until"),
        labels: Vec::new(),
    }
}

fn row_to_dependency(mut row: Row) -> Dependency {
    Dependency {
        issue_id: take_string(&mut row, "issue_id"),
        depends_on_id: take_string(&mut row, "depends_on_id"),
        r#type: take_string(&mut row, "type"),
        created_at: take_datetime_string(&mut row, "created_at"),
        created_by: take_opt(&mut row, "created_by"),
    }
}

fn row_to_comment(mut row: Row) -> Comment {
    Comment {
        id: take_string(&mut row, "id"),
        author: take_string(&mut row, "author"),
        text: take_string(&mut row, "text"),
        created_at: take_datetime_string(&mut row, "created_at"),
    }
}

fn take_string(row: &mut Row, column: &str) -> String {
    take_opt::<String>(row, column).unwrap_or_default()
}

fn take_opt<T>(row: &mut Row, column: &str) -> Option<T>
where
    T: FromValue,
{
    row.take::<Option<T>, _>(column).flatten()
}

fn take_datetime_string(row: &mut Row, column: &str) -> Option<String> {
    let value: mysql_async::Value = row.take(column)?;
    match value {
        mysql_async::Value::NULL => None,
        mysql_async::Value::Date(year, month, day, hour, minute, second, micros) => {
            if micros > 0 {
                Some(format!(
                    "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.{micros:06}"
                ))
            } else {
                Some(format!(
                    "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}"
                ))
            }
        }
        mysql_async::Value::Bytes(bytes) => String::from_utf8(bytes).ok(),
        _ => None,
    }
}

/// Recognise MySQL "no such table/view" errors so callers can fall back to a
/// pre-view query path on older Beads schemas. Anything else propagates.
fn is_missing_relation(err: &mysql_async::Error) -> bool {
    if let mysql_async::Error::Server(server) = err {
        // 1146 = ER_NO_SUCH_TABLE, 1051 = ER_BAD_TABLE_ERROR
        return server.code == 1146 || server.code == 1051;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_identifier_allows_safe_chars() {
        assert!(validate_identifier("my_project-1").is_ok());
        assert!(validate_identifier("Plain").is_ok());
    }

    #[test]
    fn validates_identifier_rejects_unsafe_chars() {
        assert!(validate_identifier("").is_err());
        assert!(validate_identifier("with space").is_err());
        assert!(validate_identifier("`malicious").is_err());
        assert!(validate_identifier("drop;table").is_err());
    }

    #[test]
    fn validates_well_formed_statuses() {
        assert!(validate_status("open").is_ok());
        assert!(validate_status("closed").is_ok());
        assert!(validate_status("custom_done").is_ok());
        assert!(validate_status("backlog-q4").is_ok());
        assert!(validate_status("").is_err());
        assert!(validate_status("with space").is_err());
    }
}
