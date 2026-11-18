//! GraphQL surface. Resolvers stay thin: every rule lives in [`crate::engine`],
//! so a mutation and a runner callback cannot disagree about the gate.

use async_graphql::{
    ComplexObject, Context, EmptySubscription, InputObject, Object, Schema, SimpleObject,
};
use hulahoop_core::{AgentRole, RunFlag};
use uuid::Uuid;

use crate::engine::Engine;
use crate::model::*;

pub type HulahoopSchema = Schema<Query, Mutation, EmptySubscription>;

pub fn build_schema(engine: std::sync::Arc<Engine>) -> HulahoopSchema {
    Schema::build(Query, Mutation, EmptySubscription)
        .data(engine)
        .limit_depth(12)
        .limit_complexity(2000)
        .finish()
}

pub fn engine<'a>(ctx: &'a Context<'a>) -> &'a Engine {
    ctx.data_unchecked::<std::sync::Arc<Engine>>()
}

pub struct Query;

#[Object]
impl Query {
    async fn workspaces(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<Workspace>> {
        Ok(
            sqlx::query_as::<_, Workspace>("select * from workspaces order by created_at")
                .fetch_all(&engine(ctx).db.pool)
                .await?,
        )
    }

    async fn projects(
        &self,
        ctx: &Context<'_>,
        workspace_id: Uuid,
    ) -> async_graphql::Result<Vec<Project>> {
        Ok(sqlx::query_as::<_, Project>(
            "select * from projects where workspace_id = ? order by created_at",
        )
        .bind(workspace_id)
        .fetch_all(&engine(ctx).db.pool)
        .await?)
    }

    async fn repos(&self, ctx: &Context<'_>, project_id: Uuid) -> async_graphql::Result<Vec<Repo>> {
        Ok(sqlx::query_as::<_, Repo>(
            "select * from repos where project_id = ? order by created_at",
        )
        .bind(project_id)
        .fetch_all(&engine(ctx).db.pool)
        .await?)
    }

    async fn agents(
        &self,
        ctx: &Context<'_>,
        workspace_id: Uuid,
    ) -> async_graphql::Result<Vec<Agent>> {
        Ok(sqlx::query_as::<_, Agent>(
            "select * from agents where workspace_id = ? order by created_at",
        )
        .bind(workspace_id)
        .fetch_all(&engine(ctx).db.pool)
        .await?)
    }

    async fn project_agents(
        &self,
        ctx: &Context<'_>,
        project_id: Uuid,
    ) -> async_graphql::Result<Vec<ProjectAgent>> {
        Ok(
            sqlx::query_as::<_, ProjectAgent>("select * from project_agents where project_id = ?")
                .bind(project_id)
                .fetch_all(&engine(ctx).db.pool)
                .await?,
        )
    }

    async fn issues(
        &self,
        ctx: &Context<'_>,
        project_id: Uuid,
        status: Option<String>,
    ) -> async_graphql::Result<Vec<Issue>> {
        let rows = match status {
            Some(status) => sqlx::query_as::<_, Issue>(
                "select * from issues where project_id = ? and status = ? order by created_at desc",
            )
            .bind(project_id)
            .bind(status)
            .fetch_all(&engine(ctx).db.pool)
            .await?,
            None => {
                sqlx::query_as::<_, Issue>(
                    "select * from issues where project_id = ? order by created_at desc",
                )
                .bind(project_id)
                .fetch_all(&engine(ctx).db.pool)
                .await?
            }
        };
        Ok(rows)
    }

    async fn issue(&self, ctx: &Context<'_>, id: Uuid) -> async_graphql::Result<Option<Issue>> {
        Ok(
            sqlx::query_as::<_, Issue>("select * from issues where id = ?")
                .bind(id)
                .fetch_optional(&engine(ctx).db.pool)
                .await?,
        )
    }

    async fn runs(&self, ctx: &Context<'_>, issue_id: Uuid) -> async_graphql::Result<Vec<Run>> {
        Ok(sqlx::query_as::<_, Run>(
            "select * from runs where issue_id = ? order by created_at desc",
        )
        .bind(issue_id)
        .fetch_all(&engine(ctx).db.pool)
        .await?)
    }

    async fn run(&self, ctx: &Context<'_>, id: Uuid) -> async_graphql::Result<Option<Run>> {
        Ok(sqlx::query_as::<_, Run>("select * from runs where id = ?")
            .bind(id)
            .fetch_optional(&engine(ctx).db.pool)
            .await?)
    }

    async fn replays(
        &self,
        ctx: &Context<'_>,
        issue_id: Uuid,
    ) -> async_graphql::Result<Vec<Replay>> {
        Ok(sqlx::query_as::<_, Replay>(
            "select * from replays where issue_id = ? order by created_at desc",
        )
        .bind(issue_id)
        .fetch_all(&engine(ctx).db.pool)
        .await?)
    }

    async fn reviews(
        &self,
        ctx: &Context<'_>,
        issue_id: Uuid,
    ) -> async_graphql::Result<Vec<Review>> {
        Ok(sqlx::query_as::<_, Review>(
            "select * from reviews where issue_id = ? order by created_at desc",
        )
        .bind(issue_id)
        .fetch_all(&engine(ctx).db.pool)
        .await?)
    }

    async fn comments(
        &self,
        ctx: &Context<'_>,
        issue_id: Uuid,
    ) -> async_graphql::Result<Vec<Comment>> {
        Ok(sqlx::query_as::<_, Comment>(
            "select * from comments where issue_id = ? order by created_at",
        )
        .bind(issue_id)
        .fetch_all(&engine(ctx).db.pool)
        .await?)
    }

    async fn preflight(
        &self,
        ctx: &Context<'_>,
        project_id: Uuid,
    ) -> async_graphql::Result<Vec<AgentPreflight>> {
        Ok(sqlx::query_as::<_, AgentPreflight>(
            "select p.* from agent_preflight p join repos r on r.id = p.repo_id where r.project_id = ?",
        )
        .bind(project_id)
        .fetch_all(&engine(ctx).db.pool)
        .await?)
    }

    /// Live tail of the newest run on an issue, so the desktop can poll without
    /// re-reading the log file.
    async fn run_events(
        &self,
        ctx: &Context<'_>,
        run_id: Uuid,
        after_seq: Option<i64>,
    ) -> async_graphql::Result<Vec<RunEvent>> {
        Ok(sqlx::query_as::<_, RunEvent>(
            "select id, run_id, seq, kind, payload, created_at from run_events
             where run_id = ? and seq > ? order by seq limit 500",
        )
        .bind(run_id)
        .bind(after_seq.unwrap_or(0))
        .fetch_all(&engine(ctx).db.pool)
        .await?)
    }
}

#[derive(SimpleObject, sqlx::FromRow)]
pub struct RunEvent {
    pub id: Uuid,
    pub run_id: Uuid,
    pub seq: i64,
    pub kind: String,
    pub payload: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub struct Mutation;

#[Object]
impl Mutation {
    async fn create_project(
        &self,
        ctx: &Context<'_>,
        workspace_id: Uuid,
        name: String,
    ) -> async_graphql::Result<Project> {
        let id = Uuid::now_v7();
        sqlx::query(
            "insert into projects (id, workspace_id, name, created_at) values (?, ?, ?, ?)",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(&name)
        .bind(crate::db::now_rfc3339())
        .execute(&engine(ctx).db.pool)
        .await?;
        Ok(sqlx::query_as("select * from projects where id = ?")
            .bind(id)
            .fetch_one(&engine(ctx).db.pool)
            .await?)
    }

    async fn add_repo(
        &self,
        ctx: &Context<'_>,
        project_id: Uuid,
        name: String,
        path: String,
        vcs: Option<String>,
    ) -> async_graphql::Result<Repo> {
        let engine = engine(ctx);
        let root = std::path::Path::new(&path);
        // Structural contract check before anything is stored: a repo without
        // flake.nix and a justfile cannot run the loop at all.
        hulahoop_core::contract::check_files(root)
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        let id = Uuid::now_v7();
        let vcs = vcs.unwrap_or_else(|| {
            if root.join(".jj").exists() {
                "jj".to_string()
            } else {
                "git".to_string()
            }
        });
        sqlx::query(
            "insert into repos (id, project_id, name, path, vcs, base_bookmark, contract, contract_version, created_at)
             values (?, ?, ?, ?, ?, 'main', 'nix+just', ?, ?)",
        )
        .bind(id)
        .bind(project_id)
        .bind(&name)
        .bind(&path)
        .bind(&vcs)
        .bind(hulahoop_core::CONTRACT_VERSION)
        .bind(crate::db::now_rfc3339())
        .execute(&engine.db.pool)
        .await?;

        engine
            .queue_job(id, None, "fetch", serde_json::json!({}))
            .await?;

        Ok(sqlx::query_as("select * from repos where id = ?")
            .bind(id)
            .fetch_one(&engine.db.pool)
            .await?)
    }

    async fn update_repo(
        &self,
        ctx: &Context<'_>,
        id: Uuid,
        name: Option<String>,
        base_bookmark: Option<String>,
    ) -> async_graphql::Result<Repo> {
        let engine = engine(ctx);
        if let Some(name) = name {
            sqlx::query("update repos set name = ? where id = ?")
                .bind(name)
                .bind(id)
                .execute(&engine.db.pool)
                .await?;
        }
        if let Some(bookmark) = base_bookmark {
            sqlx::query("update repos set base_bookmark = ? where id = ?")
                .bind(bookmark)
                .bind(id)
                .execute(&engine.db.pool)
                .await?;
        }
        Ok(sqlx::query_as("select * from repos where id = ?")
            .bind(id)
            .fetch_one(&engine.db.pool)
            .await?)
    }

    async fn fetch_repo(&self, ctx: &Context<'_>, id: Uuid) -> async_graphql::Result<Repo> {
        let engine = engine(ctx);
        engine
            .queue_job(id, None, "fetch", serde_json::json!({}))
            .await?;
        Ok(sqlx::query_as("select * from repos where id = ?")
            .bind(id)
            .fetch_one(&engine.db.pool)
            .await?)
    }

    async fn create_agent(
        &self,
        ctx: &Context<'_>,
        workspace_id: Uuid,
        name: String,
        adapter: String,
        model: String,
    ) -> async_graphql::Result<Agent> {
        let id = Uuid::now_v7();
        let now = crate::db::now_rfc3339();
        sqlx::query(
            "insert into agents (id, workspace_id, name, adapter, model, created_at, updated_at)
             values (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(&name)
        .bind(&adapter)
        .bind(&model)
        .bind(&now)
        .bind(&now)
        .execute(&engine(ctx).db.pool)
        .await?;
        Ok(sqlx::query_as("select * from agents where id = ?")
            .bind(id)
            .fetch_one(&engine(ctx).db.pool)
            .await?)
    }

    async fn update_agent(
        &self,
        ctx: &Context<'_>,
        id: Uuid,
        name: Option<String>,
        adapter: Option<String>,
        model: Option<String>,
    ) -> async_graphql::Result<Agent> {
        let engine = engine(ctx);
        for (column, value) in [("name", name), ("adapter", adapter), ("model", model)] {
            if let Some(value) = value {
                sqlx::query(&format!(
                    "update agents set {column} = ?, updated_at = ? where id = ?"
                ))
                .bind(value)
                .bind(crate::db::now_rfc3339())
                .bind(id)
                .execute(&engine.db.pool)
                .await?;
            }
        }
        Ok(sqlx::query_as("select * from agents where id = ?")
            .bind(id)
            .fetch_one(&engine.db.pool)
            .await?)
    }

    async fn set_project_agents(
        &self,
        ctx: &Context<'_>,
        project_id: Uuid,
        bindings: Vec<RoleBinding>,
    ) -> async_graphql::Result<Vec<ProjectAgent>> {
        let engine = engine(ctx);
        sqlx::query("delete from project_agents where project_id = ?")
            .bind(project_id)
            .execute(&engine.db.pool)
            .await?;
        for binding in bindings {
            let role = AgentRole::parse(&binding.role).ok_or_else(|| {
                async_graphql::Error::new(format!("unknown role {}", binding.role))
            })?;
            sqlx::query("insert into project_agents (project_id, agent_id, role) values (?, ?, ?)")
                .bind(project_id)
                .bind(binding.agent_id)
                .bind(role.as_str())
                .execute(&engine.db.pool)
                .await?;
        }
        Ok(
            sqlx::query_as("select * from project_agents where project_id = ?")
                .bind(project_id)
                .fetch_all(&engine.db.pool)
                .await?,
        )
    }

    async fn run_preflight(
        &self,
        ctx: &Context<'_>,
        agent_id: Uuid,
        repo_id: Uuid,
    ) -> async_graphql::Result<bool> {
        let agent = engine(ctx).agent(agent_id).await?;
        engine(ctx)
            .queue_job(
                repo_id,
                None,
                "preflight",
                serde_json::json!({
                    "agent_id": agent_id,
                    "adapter": agent.adapter,
                    "model": agent.model,
                }),
            )
            .await?;
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    async fn create_issue(
        &self,
        ctx: &Context<'_>,
        project_id: Uuid,
        repo_id: Uuid,
        title: String,
        body: String,
        priority: Option<String>,
        criteria: Vec<String>,
    ) -> async_graphql::Result<Issue> {
        let engine = engine(ctx);
        Engine::validate_criteria(&criteria)
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        let id = Uuid::now_v7();
        let now = crate::db::now_rfc3339();
        sqlx::query(
            "insert into issues (id, project_id, repo_id, title, body, status, priority, created_at, updated_at)
             values (?, ?, ?, ?, ?, 'backlog', ?, ?, ?)",
        )
        .bind(id)
        .bind(project_id)
        .bind(repo_id)
        .bind(&title)
        .bind(&body)
        .bind(priority.unwrap_or_else(|| "medium".into()))
        .bind(&now)
        .bind(&now)
        .execute(&engine.db.pool)
        .await?;
        for (index, text) in criteria.iter().enumerate() {
            sqlx::query("insert into criteria (id, issue_id, position, text) values (?, ?, ?, ?)")
                .bind(Uuid::now_v7())
                .bind(id)
                .bind((index as i64) + 1)
                .bind(text)
                .execute(&engine.db.pool)
                .await?;
        }
        Ok(engine.issue(id).await?)
    }

    async fn update_issue(
        &self,
        ctx: &Context<'_>,
        id: Uuid,
        title: Option<String>,
        body: Option<String>,
        priority: Option<String>,
        criteria: Option<Vec<String>>,
    ) -> async_graphql::Result<Issue> {
        let engine = engine(ctx);
        let issue = engine.issue(id).await?;
        if matches!(
            issue.status_enum(),
            hulahoop_core::IssueStatus::TestsReady
                | hulahoop_core::IssueStatus::InProgress
                | hulahoop_core::IssueStatus::InReview
                | hulahoop_core::IssueStatus::Merging
                | hulahoop_core::IssueStatus::Done
        ) && criteria.is_some()
        {
            return Err(async_graphql::Error::new(
                "criteria cannot change while the suite is frozen: thaw it first, so the tests keep proving the criteria they were written for",
            ));
        }
        for (column, value) in [("title", title), ("body", body), ("priority", priority)] {
            if let Some(value) = value {
                sqlx::query(&format!(
                    "update issues set {column} = ?, updated_at = ? where id = ?"
                ))
                .bind(value)
                .bind(crate::db::now_rfc3339())
                .bind(id)
                .execute(&engine.db.pool)
                .await?;
            }
        }
        if let Some(criteria) = criteria {
            Engine::validate_criteria(&criteria)
                .map_err(|e| async_graphql::Error::new(e.to_string()))?;
            sqlx::query("delete from criteria where issue_id = ?")
                .bind(id)
                .execute(&engine.db.pool)
                .await?;
            for (index, text) in criteria.iter().enumerate() {
                sqlx::query(
                    "insert into criteria (id, issue_id, position, text) values (?, ?, ?, ?)",
                )
                .bind(Uuid::now_v7())
                .bind(id)
                .bind((index as i64) + 1)
                .bind(text)
                .execute(&engine.db.pool)
                .await?;
            }
        }
        Ok(engine.issue(id).await?)
    }

    async fn approve_criteria(
        &self,
        ctx: &Context<'_>,
        issue_id: Uuid,
    ) -> async_graphql::Result<Issue> {
        let engine = engine(ctx);
        let criteria = engine.criteria(issue_id).await?;
        let texts: Vec<String> = criteria.iter().map(|c| c.text.clone()).collect();
        Engine::validate_criteria(&texts).map_err(|e| async_graphql::Error::new(e.to_string()))?;
        Ok(engine
            .transition(issue_id, hulahoop_core::IssueEvent::ApproveCriteria)
            .await?)
    }

    async fn add_comment(
        &self,
        ctx: &Context<'_>,
        issue_id: Uuid,
        body: String,
    ) -> async_graphql::Result<Comment> {
        let engine = engine(ctx);
        engine
            .comment(
                issue_id,
                hulahoop_core::AuthorType::Human,
                None,
                &body,
                false,
            )
            .await?;
        Ok(sqlx::query_as::<_, Comment>(
            "select * from comments where issue_id = ? order by created_at desc limit 1",
        )
        .bind(issue_id)
        .fetch_one(&engine.db.pool)
        .await?)
    }

    async fn start_run(
        &self,
        ctx: &Context<'_>,
        issue_id: Uuid,
        agent_id: Option<Uuid>,
        role: String,
        parent_run_id: Option<Uuid>,
    ) -> async_graphql::Result<Run> {
        let role = AgentRole::parse(&role)
            .ok_or_else(|| async_graphql::Error::new(format!("unknown role {role}")))?;
        Ok(engine(ctx)
            .start_run(issue_id, agent_id, role, parent_run_id)
            .await?)
    }

    async fn cancel_run(&self, ctx: &Context<'_>, run_id: Uuid) -> async_graphql::Result<Run> {
        Ok(engine(ctx).cancel_run(run_id).await?)
    }

    async fn abandon_issue(
        &self,
        ctx: &Context<'_>,
        issue_id: Uuid,
        reason: String,
    ) -> async_graphql::Result<Issue> {
        let engine = engine(ctx);
        sqlx::query("update issues set abandon_reason = ? where id = ?")
            .bind(&reason)
            .bind(issue_id)
            .execute(&engine.db.pool)
            .await?;
        engine
            .comment(
                issue_id,
                hulahoop_core::AuthorType::Human,
                None,
                &format!("Abandoned: {reason}"),
                false,
            )
            .await?;
        Ok(engine
            .transition(issue_id, hulahoop_core::IssueEvent::Abandon)
            .await?)
    }

    async fn freeze_suite(
        &self,
        ctx: &Context<'_>,
        issue_id: Uuid,
    ) -> async_graphql::Result<Issue> {
        Ok(engine(ctx).freeze_suite(issue_id).await?)
    }

    async fn unfreeze_suite(
        &self,
        ctx: &Context<'_>,
        issue_id: Uuid,
        reason: String,
    ) -> async_graphql::Result<Issue> {
        Ok(engine(ctx).unfreeze_suite(issue_id, &reason).await?)
    }

    async fn approve_issue(
        &self,
        ctx: &Context<'_>,
        issue_id: Uuid,
        run_id: Uuid,
        reviewed_head: String,
        diff_sha: String,
        acknowledge_flags: Vec<String>,
    ) -> async_graphql::Result<Issue> {
        let flags: Vec<RunFlag> = acknowledge_flags
            .iter()
            .filter_map(|f| RunFlag::parse(f))
            .collect();
        Ok(engine(ctx)
            .approve_issue(issue_id, run_id, &reviewed_head, &diff_sha, flags)
            .await?)
    }

    async fn record_merge(
        &self,
        ctx: &Context<'_>,
        issue_id: Uuid,
        merge_revision: String,
    ) -> async_graphql::Result<Issue> {
        Ok(engine(ctx).record_merge(issue_id, &merge_revision).await?)
    }

    async fn resume_issue(
        &self,
        ctx: &Context<'_>,
        issue_id: Uuid,
        answer: String,
    ) -> async_graphql::Result<Issue> {
        Ok(engine(ctx).resume_issue(issue_id, &answer).await?)
    }

    async fn request_changes(
        &self,
        ctx: &Context<'_>,
        issue_id: Uuid,
        body: String,
    ) -> async_graphql::Result<Issue> {
        let engine = engine(ctx);
        engine
            .comment(
                issue_id,
                hulahoop_core::AuthorType::Human,
                None,
                &body,
                false,
            )
            .await?;
        Ok(engine
            .transition(issue_id, hulahoop_core::IssueEvent::RequestChanges)
            .await?)
    }

    async fn clear_run_cap(
        &self,
        ctx: &Context<'_>,
        issue_id: Uuid,
    ) -> async_graphql::Result<Issue> {
        let engine = engine(ctx);
        sqlx::query("update issues set run_cap_cleared_at = ?, updated_at = ? where id = ?")
            .bind(crate::db::now_rfc3339())
            .bind(crate::db::now_rfc3339())
            .bind(issue_id)
            .execute(&engine.db.pool)
            .await?;
        Ok(engine.issue(issue_id).await?)
    }
}

#[derive(InputObject)]
pub struct RoleBinding {
    pub role: String,
    pub agent_id: Uuid,
}

/// Nested reads stay on one screen's worth of data: the board asks for issues,
/// the detail screen asks for the rest.
#[ComplexObject]
impl Issue {
    async fn criteria(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<Criterion>> {
        Ok(engine(ctx).criteria(self.id).await?)
    }

    async fn runs(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<Run>> {
        Ok(sqlx::query_as::<_, Run>(
            "select * from runs where issue_id = ? order by created_at desc",
        )
        .bind(self.id)
        .fetch_all(&engine(ctx).db.pool)
        .await?)
    }

    async fn current_run(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<Run>> {
        let Some(id) = self.current_run_id else {
            return Ok(None);
        };
        Ok(sqlx::query_as::<_, Run>("select * from runs where id = ?")
            .bind(id)
            .fetch_optional(&engine(ctx).db.pool)
            .await?)
    }

    async fn reviews(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<Review>> {
        Ok(sqlx::query_as::<_, Review>(
            "select * from reviews where issue_id = ? order by created_at desc",
        )
        .bind(self.id)
        .fetch_all(&engine(ctx).db.pool)
        .await?)
    }

    async fn comments(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<Comment>> {
        Ok(sqlx::query_as::<_, Comment>(
            "select * from comments where issue_id = ? order by created_at",
        )
        .bind(self.id)
        .fetch_all(&engine(ctx).db.pool)
        .await?)
    }

    async fn replays(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<Replay>> {
        Ok(sqlx::query_as::<_, Replay>(
            "select * from replays where issue_id = ? order by created_at desc",
        )
        .bind(self.id)
        .fetch_all(&engine(ctx).db.pool)
        .await?)
    }

    async fn approval(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<Approval>> {
        Ok(sqlx::query_as::<_, Approval>(
            "select * from approvals where issue_id = ? and voided_at is null order by created_at desc limit 1",
        )
        .bind(self.id)
        .fetch_optional(&engine(ctx).db.pool)
        .await?)
    }

    /// The frozen paths as a list, since the column is JSON.
    async fn frozen_paths(&self) -> Vec<String> {
        self.frozen_path_list()
    }
}

#[ComplexObject]
impl Run {
    async fn flags(&self) -> Vec<String> {
        self.flags_enum()
            .iter()
            .map(|f| f.as_str().to_string())
            .collect()
    }

    /// The diff the review screen renders, read from the artifact the server
    /// wrote. Bounded: a diff is capped at the line limit, so this stays small.
    async fn diff(&self) -> async_graphql::Result<String> {
        let path = std::path::Path::new(&self.artifact_dir).join("diff.patch");
        match tokio::fs::read_to_string(&path).await {
            Ok(text) => Ok(text),
            Err(_) => Ok(String::new()),
        }
    }

    async fn log(&self, lines: Option<i64>) -> async_graphql::Result<String> {
        let path = std::path::Path::new(&self.artifact_dir).join("agent.jsonl");
        let text = tokio::fs::read_to_string(&path).await.unwrap_or_default();
        let limit = lines.unwrap_or(400).clamp(1, 2000) as usize;
        let all: Vec<&str> = text.lines().collect();
        let start = all.len().saturating_sub(limit);
        Ok(all[start..].join("\n"))
    }

    async fn reviews(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<Review>> {
        Ok(sqlx::query_as::<_, Review>(
            "select * from reviews where reviewed_run_id = ? order by created_at desc",
        )
        .bind(self.id)
        .fetch_all(&engine(ctx).db.pool)
        .await?)
    }
}
