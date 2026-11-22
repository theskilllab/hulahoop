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
    /// What an issue is called when it belongs to no project.
    pub default_prefix: String,
    pub created_at: DateTime<Utc>,
}

// `complex` lets the resolvers in `gql.rs` add the nested reads to this object.
#[derive(Debug, Clone, serde::Serialize, SimpleObject, sqlx::FromRow)]
#[graphql(complex)]
pub struct Project {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    /// `BAL` in `BAL-15`. Unique per workspace, uppercase.
    pub prefix: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, serde::Serialize, SimpleObject, sqlx::FromRow)]
pub struct Repo {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    /// A checkout on this machine. Null when the repository was attached from the
    /// GitHub listing and only a runner that clones can use it.
    pub path: Option<String>,
    /// The `origin` remote, when the repo has one. A pull request needs it.
    pub remote_url: Option<String>,
    pub vcs: String,
    pub base_revision: Option<String>,
    pub base_bookmark: String,
    pub contract: String,
    pub contract_version: String,
    pub fetched_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, serde::Serialize, SimpleObject, sqlx::FromRow)]
#[graphql(complex)]
pub struct Agent {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub adapter: String,
    pub model: String,
    /// How this agent should behave, written by the person who knows it best.
    pub agents_md: String,
    /// Which connector this agent authenticates with, where its harness offers a
    /// choice: `claude-sub` or `claude-api`. Empty means "whichever is connected".
    pub connector: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A skill is reusable knowledge, kept in one place and attached to agents that
/// need it: a convention, a checklist, a house rule with the story behind it.
#[derive(Debug, Clone, serde::Serialize, SimpleObject, sqlx::FromRow)]
pub struct Skill {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub description: String,
    pub source: String,
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

/// A person on a workspace, joined with their row in `users` so the UI can name
/// them. Local mode has exactly one.
#[derive(Debug, Clone, serde::Serialize, SimpleObject, sqlx::FromRow)]
pub struct WorkspaceMember {
    pub user_id: Uuid,
    pub name: String,
    pub email: Option<String>,
    pub role: String,
}

// `complex` lets the resolvers in `gql.rs` add the nested reads to this object.
#[derive(Debug, Clone, serde::Serialize, SimpleObject, sqlx::FromRow)]
#[graphql(complex)]
pub struct Issue {
    pub id: Uuid,
    pub workspace_id: Uuid,
    /// Null when the issue is planning work that belongs to no project yet.
    pub project_id: Option<Uuid>,
    pub repo_id: Uuid,
    pub milestone_id: Option<Uuid>,
    /// The number inside its project (or workspace, when it has no project).
    pub number: i64,
    /// `BAL-15`. Stored, not derived, so a renamed prefix is a deliberate rewrite.
    #[graphql(name = "key")]
    pub issue_key: String,
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
    /// Agents still owed a turn in this status, in the workflow's order, plus the
    /// ones already served and how many turns the status has taken.
    pub turn_queue: String,
    pub turn_served: String,
    pub turns_used: i64,
    pub tests_revision: Option<String>,
    pub tests_bookmark: Option<String>,
    #[graphql(skip)]
    pub frozen_paths: String,
    pub tests_frozen_at: Option<DateTime<Utc>>,
    pub merge_revision: Option<String>,
    pub merged_at: Option<DateTime<Utc>>,
    pub pr_url: Option<String>,
    pub pr_number: Option<i64>,
    pub pr_branch: Option<String>,
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
    /// The identity the commit carries. `None` means the run never produced one,
    /// or produced it before the loop could name itself.
    pub commit_name: Option<String>,
    pub commit_email: Option<String>,
    /// The model this run was given, frozen at spawn: the agent's model may
    /// change later and a cost report must not rewrite history.
    pub model: String,
    /// Where the loop opened a pull request for this run, when it did.
    pub pr_url: Option<String>,
    pub pr_number: Option<i64>,
    pub pr_branch: Option<String>,
    #[graphql(skip)]
    pub flags: String,
    pub status: String,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub exit_code: Option<i64>,
    pub error: Option<String>,
    /// Summed over the run's finished messages, from the usage its adapter
    /// writes into the transcript. A stream that reports none spends zero.
    pub tokens_in: i64,
    pub tokens_out: i64,
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
#[graphql(complex)]
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
#[graphql(complex)]
pub struct Approval {
    pub id: Uuid,
    pub issue_id: Uuid,
    pub run_id: Uuid,
    pub head_revision: String,
    pub diff_sha: String,
    pub landing_bookmark: Option<String>,
    #[graphql(skip)]
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

/// What the connections screen shows. The credentials themselves never leave the
/// server, so the object carries the facts a user can act on: which provider
/// this is, whether the stored credential still works, and which app it names.
#[derive(Debug, Clone, serde::Serialize, SimpleObject)]
pub struct Connection {
    /// The screen's id for this entry: `github`, `claude-sub`, `claude-api`, `pi`.
    pub id: String,
    pub provider: String,
    pub kind: String,
    /// `Source control` or `Agent access`.
    pub group: String,
    pub name: String,
    pub icon: String,
    pub desc: String,
    /// The line under the description: what connecting this makes possible.
    pub enables: String,
    pub connected: bool,
    pub app_id: Option<String>,
    pub label: Option<String>,
    pub connected_at: Option<DateTime<Utc>>,
}

/// The stored row: the credentials as vault ciphertext, never as plaintext.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ConnectorRow {
    pub id: Uuid,
    pub provider: String,
    pub kind: String,
    /// Sealed JSON. The fields inside are sealed one by one, so opening a connector
    /// never requires opening all of it: the app id is public, the key is not.
    pub secret_ciphertext: String,
    pub label: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, serde::Serialize, SimpleObject, sqlx::FromRow)]
pub struct Milestone {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub title: String,
    pub body: String,
    pub status: String,
    pub target_date: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, serde::Serialize, SimpleObject, sqlx::FromRow)]
pub struct Artifact {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub milestone_id: Option<Uuid>,
    pub title: String,
    /// `document`, `image`, `archive`, `link` — a label, never a rule.
    pub kind: String,
    pub filename: String,
    pub mime: String,
    pub size: i64,
    pub sha256: String,
    pub created_at: DateTime<Utc>,
}
