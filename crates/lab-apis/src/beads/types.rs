//! Beads response types.

use serde::{Deserialize, Serialize};

/// Beads CLI contract metadata.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ContractStatus {
    pub status: &'static str,
    pub reason: &'static str,
    pub safe_v1_actions: &'static [&'static str],
    pub deferred: &'static [&'static str],
}

/// One Dolt database, treated as a Beads project.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Project {
    pub name: String,
}

/// `version.get` response — Dolt server version reported by `SELECT @@version`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DoltVersion {
    pub version: String,
    pub dolt_version: Option<String>,
}

/// `context.get` response — currently selected project + headline counters.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BeadsContext {
    pub project: String,
    pub total_issues: u64,
    pub open_issues: u64,
}

/// One row in the `status.summary` response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatusCount {
    pub status: String,
    pub count: u64,
}

/// `status.summary` response — per-status counts plus a total.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatusSummary {
    pub project: String,
    pub total: u64,
    pub by_status: Vec<StatusCount>,
}

/// Health/status result for the Dolt-backed Beads service.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BeadsHealth {
    pub reachable: bool,
    pub status: &'static str,
    pub version: Option<String>,
    pub default_project: Option<String>,
    pub message: Option<String>,
}

/// One issue row, restricted to the columns the Lab UI surfaces.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Issue {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub status: String,
    pub priority: i32,
    pub issue_type: String,
    pub assignee: Option<String>,
    pub created_by: Option<String>,
    pub owner: Option<String>,
    pub external_ref: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub closed_at: Option<String>,
    pub started_at: Option<String>,
    pub due_at: Option<String>,
    pub defer_until: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
}

/// One dependency row for `graph.show`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Dependency {
    pub issue_id: String,
    pub depends_on_id: String,
    pub r#type: String,
    pub created_at: Option<String>,
    pub created_by: Option<String>,
}

/// One comment row for `issue.show`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Comment {
    pub id: String,
    pub author: String,
    pub text: String,
    pub created_at: Option<String>,
}

/// `issue.show` response — issue plus context for the detail panel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssueDetail {
    pub issue: Issue,
    pub blocked_by: Vec<Dependency>,
    pub blocks: Vec<Dependency>,
    pub parents: Vec<Dependency>,
    pub children: Vec<Dependency>,
    pub comments: Vec<Comment>,
}

/// `graph.show` response — node + edge bundle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DependencyGraph {
    pub root: String,
    pub nodes: Vec<Issue>,
    pub edges: Vec<Dependency>,
}
