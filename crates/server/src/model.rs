//! Row types. Each struct is both the `sqlx` decode target and the GraphQL
//! object, so a column and its field can never drift apart silently.

use async_graphql::SimpleObject;
use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize, SimpleObject, sqlx::FromRow)]
pub struct Workspace {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, serde::Serialize, SimpleObject, sqlx::FromRow)]
pub struct Project {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, serde::Serialize, SimpleObject, sqlx::FromRow)]
pub struct Repo {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub path: String,
    pub vcs: String,
    pub base_revision: Option<String>,
    pub base_bookmark: String,
    pub contract: String,
    pub contract_version: String,
    pub fetched_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, serde::Serialize, SimpleObject, sqlx::FromRow)]
pub struct Agent {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub adapter: String,
    pub model: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, serde::Serialize, SimpleObject, sqlx::FromRow)]
pub struct AgentPreflight {
    pub agent_id: Uuid,
    pub repo_id: Uuid,
    pub ok: bool,
    pub detail: String,
    pub checked_at: DateTime<Utc>,
}

#[derive(Debug, Clone, serde::Serialize, SimpleObject, sqlx::FromRow)]
pub struct ProjectAgent {
    pub project_id: Uuid,
    pub agent_id: Uuid,
    pub role: String,
}

// `complex` lets the resolvers in `gql.rs` add the nested reads to this object.
#[derive(Debug, Clone, serde::Serialize, SimpleObject, sqlx::FromRow)]
#[graphql(complex)]
pub struct Issue {
    pub id: Uuid,
    pub project_id: Uuid,
    pub repo_id: Uuid,
    pub title: String,
    pub body: String,
    pub status: String,
    pub priority: String,
    pub status_before_blocked: Option<String>,
    pub pinned_comment_id: Option<Uuid>,
    pub run_cap_cleared_at: Option<DateTime<Utc>>,
    pub attempt: i64,
    pub suite_seq: i64,
    pub run_count: i64,
    pub current_run_id: Option<Uuid>,
    pub tests_revision: Option<String>,
    pub tests_bookmark: Option<String>,
    #[graphql(skip)]
    pub frozen_paths: String,
    pub tests_frozen_at: Option<DateTime<Utc>>,
    pub merge_revision: Option<String>,
    pub merged_at: Option<DateTime<Utc>>,
    pub abandon_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Issue {
    /// Typed view of the `status` column. The name carries a suffix because
    /// async-graphql generates a `status()` accessor for the exposed field.
    pub fn status_enum(&self) -> hulahoop_core::IssueStatus {
        hulahoop_core::IssueStatus::parse(&self.status)
            .unwrap_or(hulahoop_core::IssueStatus::Backlog)
    }

    pub fn frozen_path_list(&self) -> Vec<String> {
        crate::db::json_list(&self.frozen_paths)
    }
}

#[derive(Debug, Clone, serde::Serialize, SimpleObject, sqlx::FromRow)]
pub struct Criterion {
    pub id: Uuid,
    pub issue_id: Uuid,
    pub position: i64,
    pub text: String,
    pub test_name: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, serde::Serialize, SimpleObject, sqlx::FromRow)]
pub struct Checkout {
    pub id: Uuid,
    pub project_id: Uuid,
    pub repo_id: Uuid,
    pub owner_kind: String,
    pub owner_id: String,
    pub provider: String,
    pub name: String,
    pub path: String,
    pub state: String,
    pub created_at: DateTime<Utc>,
    pub cleaned_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, serde::Serialize, SimpleObject, sqlx::FromRow)]
#[graphql(complex)]
pub struct Run {
    pub id: Uuid,
    pub issue_id: Uuid,
    pub repo_id: Uuid,
    pub agent_id: Uuid,
    pub role: String,
    pub attempt: i64,
    pub base_bookmark: String,
    pub base_revision: Option<String>,
    pub head_revision: Option<String>,
    pub head_bookmark: Option<String>,
    pub candidate_bookmark: Option<String>,
    #[graphql(skip)]
    pub changed_paths: String,
    pub parent_run_id: Option<Uuid>,
    pub runner_id: Option<String>,
    pub claimed_at: Option<DateTime<Utc>>,
    pub last_heartbeat_at: Option<DateTime<Utc>>,
    pub pid: Option<i64>,
    pub pgid: Option<i64>,
    pub toolchain_lock: Option<String>,
    pub prompt_revision: String,
    pub sandbox: String,
    #[graphql(skip)]
    pub env_keys: String,
    pub artifact_dir: String,
    /// The hash `approveIssue` compares, so the desktop can show what was reviewed.
    pub diff_sha: Option<String>,
    #[graphql(skip)]
    pub flags: String,
    pub status: String,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub exit_code: Option<i64>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl Run {
    pub fn status_enum(&self) -> hulahoop_core::RunStatus {
        hulahoop_core::RunStatus::parse(&self.status).unwrap_or(hulahoop_core::RunStatus::Queued)
    }

    pub fn role_enum(&self) -> hulahoop_core::AgentRole {
        hulahoop_core::AgentRole::parse(&self.role).unwrap_or(hulahoop_core::AgentRole::Implementer)
    }

    pub fn flags_enum(&self) -> Vec<hulahoop_core::RunFlag> {
        crate::db::json_list_of_flags(&self.flags)
    }
}

#[derive(Debug, Clone, serde::Serialize, SimpleObject, sqlx::FromRow)]
pub struct Replay {
    pub id: Uuid,
    pub issue_id: Uuid,
    pub repo_id: Uuid,
    pub run_id: Option<Uuid>,
    pub candidate_run_id: Option<Uuid>,
    pub approval_id: Option<Uuid>,
    pub kind: String,
    pub attempt: i64,
    pub revision: Option<String>,
    pub tests_bookmark: Option<String>,
    pub status: String,
    pub exit_code: Option<i64>,
    pub ancestry_ok: Option<bool>,
    pub artifact_dir: String,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, serde::Serialize, SimpleObject, sqlx::FromRow)]
pub struct Review {
    pub id: Uuid,
    pub issue_id: Uuid,
    pub reviewed_run_id: Uuid,
    pub reviewer_run_id: Option<Uuid>,
    pub verdict: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, serde::Serialize, SimpleObject, sqlx::FromRow)]
pub struct Comment {
    pub id: Uuid,
    pub issue_id: Uuid,
    pub author_type: String,
    pub author_id: Option<String>,
    pub body: String,
    pub pinned: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, serde::Serialize, SimpleObject, sqlx::FromRow)]
pub struct Approval {
    pub id: Uuid,
    pub issue_id: Uuid,
    pub run_id: Uuid,
    pub head_revision: String,
    pub diff_sha: String,
    pub landing_bookmark: Option<String>,
    pub acknowledged_flags: String,
    pub approved_by: Uuid,
    pub voided_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// Runner work that is not a suite execution: preflight probes, fetches, and
/// merge verification. Keeping these out of `replays` keeps replay counts
/// meaningful.
#[derive(Debug, Clone, serde::Serialize, SimpleObject, sqlx::FromRow)]
pub struct RunnerJob {
    pub id: Uuid,
    pub repo_id: Uuid,
    pub issue_id: Option<Uuid>,
    pub kind: String,
    pub payload: String,
    pub status: String,
    pub result: Option<String>,
    pub error: Option<String>,
    pub claimed_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}
