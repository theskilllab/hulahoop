//! GraphQL surface. Resolvers stay thin: every rule lives in [`crate::engine`],
//! so a mutation and a runner callback cannot disagree about the gate.

use async_graphql::{
    ComplexObject, Context, EmptySubscription, MaybeUndefined, Object, Schema, SimpleObject,
};
use uuid::Uuid;

use crate::engine::Engine;
use crate::model::*;
use crate::workflows::WorkflowDoc;

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
    /// What the GitHub app can reach. Empty until an app is stored and installed.
    async fn github_repos(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<Vec<crate::github::GithubRepo>> {
        engine(ctx)
            .github_repositories()
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))
    }

    async fn workspace_members(
        &self,
        ctx: &Context<'_>,
        workspace_id: Uuid,
    ) -> async_graphql::Result<Vec<WorkspaceMember>> {
        engine(ctx)
            .workspace_members(workspace_id)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))
    }

    async fn workspaces(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<Workspace>> {
        Ok(
            sqlx::query_as::<_, Workspace>("select * from workspaces order by created_at")
                .fetch_all(&engine(ctx).db.pool)
                .await?,
        )
    }

    /// The Lumi conversation of one workspace, oldest first.
    async fn lumi_messages(
        &self,
        ctx: &Context<'_>,
        workspace_id: Uuid,
    ) -> async_graphql::Result<Vec<LumiMessage>> {
        Ok(engine(ctx).lumi_messages(workspace_id).await?)
    }

    /// Every provider the loop can act as, with whether the stored credential
    /// still works. This is the connections screen's one read.
    async fn connections(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<Connection>> {
        Ok(engine(ctx).connections().await?)
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

    /// What a harness can run, read from the harness's own catalogue: pi reports the
    /// providers this machine is signed in to, Claude Code reports its aliases.
    async fn models(&self, harness: String) -> Vec<crate::models::ModelOption> {
        crate::models::catalogue(&harness)
    }

    async fn issues(
        &self,
        ctx: &Context<'_>,
        workspace_id: Uuid,
        project_id: Option<Uuid>,
        status: Option<String>,
        // Finished tasks (done or cancelled) are hidden unless asked for; a
        // status filter that names one of them shows it either way.
        #[graphql(default = false)] include_done: bool,
    ) -> async_graphql::Result<Vec<Issue>> {
        let include_done = include_done || status.is_some();
        Ok(engine(ctx)
            .list_issues(workspace_id, project_id, status.as_deref(), include_done)
            .await?)
    }

    /// How many finished tasks the Active view hides, for its "+N" toggle.
    async fn done_issue_count(
        &self,
        ctx: &Context<'_>,
        workspace_id: Uuid,
        project_id: Option<Uuid>,
    ) -> async_graphql::Result<i64> {
        Ok(engine(ctx)
            .done_issue_count(workspace_id, project_id)
            .await?)
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

    async fn milestones(
        &self,
        ctx: &Context<'_>,
        workspace_id: Uuid,
    ) -> async_graphql::Result<Vec<Milestone>> {
        Ok(engine(ctx).milestones(workspace_id).await?)
    }

    async fn milestone(
        &self,
        ctx: &Context<'_>,
        id: Uuid,
    ) -> async_graphql::Result<Option<Milestone>> {
        Ok(
            sqlx::query_as::<_, Milestone>("select * from milestones where id = ?")
                .bind(id)
                .fetch_optional(&engine(ctx).db.pool)
                .await?,
        )
    }

    /// The workspace's reusable knowledge.
    /// Where each of the workspace's skills is loaded.
    async fn skill_usage(
        &self,
        ctx: &Context<'_>,
        workspace_id: Uuid,
    ) -> async_graphql::Result<Vec<crate::model::SkillUsage>> {
        Ok(engine(ctx).skill_usage(workspace_id).await?)
    }

    async fn skills(
        &self,
        ctx: &Context<'_>,
        workspace_id: Uuid,
    ) -> async_graphql::Result<Vec<Skill>> {
        Ok(engine(ctx).skills(workspace_id).await?)
    }

    /// The document that drives the board.
    async fn workflow(
        &self,
        ctx: &Context<'_>,
        workspace_id: Uuid,
    ) -> async_graphql::Result<WorkflowDoc> {
        Ok(engine(ctx).workflow(workspace_id).await?)
    }

    async fn artifacts(
        &self,
        ctx: &Context<'_>,
        workspace_id: Uuid,
        milestone_id: Option<Uuid>,
    ) -> async_graphql::Result<Vec<Artifact>> {
        Ok(engine(ctx).artifacts(workspace_id, milestone_id).await?)
    }

    async fn artifact(
        &self,
        ctx: &Context<'_>,
        id: Uuid,
    ) -> async_graphql::Result<Option<Artifact>> {
        Ok(
            sqlx::query_as::<_, Artifact>("select * from artifacts where id = ?")
                .bind(id)
                .fetch_optional(&engine(ctx).db.pool)
                .await?,
        )
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

    /// The newest runs across a workspace, so Activity and Cost control read one
    /// query instead of walking every issue.
    async fn recent_runs(
        &self,
        ctx: &Context<'_>,
        workspace_id: Uuid,
        limit: Option<i32>,
    ) -> async_graphql::Result<Vec<Run>> {
        let limit = limit.unwrap_or(60).clamp(1, 500);
        Ok(sqlx::query_as::<_, Run>(
            "select r.* from runs r join issues i on i.id = r.issue_id
             where i.workspace_id = ? order by r.created_at desc limit ?",
        )
        .bind(workspace_id)
        .bind(limit)
        .fetch_all(&engine(ctx).db.pool)
        .await?)
    }

    /// Full workspace period aggregates and completed-task cohorts; never capped
    /// to the Activity feed's most recent 500 runs.
    async fn cost_report(
        &self,
        ctx: &Context<'_>,
        workspace_id: Uuid,
        period: String,
        agent_id: Option<Uuid>,
        model: Option<String>,
    ) -> async_graphql::Result<async_graphql::Json<serde_json::Value>> {
        crate::cost::report(
            &engine(ctx).db,
            workspace_id,
            &period,
            agent_id,
            model.as_deref(),
        )
        .await
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
    async fn save_cost_settings(
        &self,
        ctx: &Context<'_>,
        workspace_id: Uuid,
        api_budget_usd: Option<f64>,
        success_target_percent: Option<f64>,
    ) -> async_graphql::Result<bool> {
        crate::cost::save_settings(
            &engine(ctx).db,
            workspace_id,
            api_budget_usd,
            success_target_percent,
        )
        .await
    }

    async fn save_cost_plan(
        &self,
        ctx: &Context<'_>,
        workspace_id: Uuid,
        connector_id: String,
        label: String,
        monthly_fee_usd: f64,
        allowance_tokens: Option<i64>,
    ) -> async_graphql::Result<bool> {
        crate::cost::save_plan(
            &engine(ctx).db,
            workspace_id,
            &connector_id,
            &label,
            monthly_fee_usd,
            allowance_tokens,
        )
        .await
    }

    async fn create_workspace(
        &self,
        ctx: &Context<'_>,
        name: String,
        prefix: Option<String>,
    ) -> async_graphql::Result<Workspace> {
        engine(ctx)
            .create_workspace(&name, prefix.as_deref())
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))
    }

    /// Renaming a workspace or changing its prefix also rewrites the keys of the
    /// issues filed with no project, because the prefix is part of the key.
    async fn update_workspace(
        &self,
        ctx: &Context<'_>,
        id: Uuid,
        name: Option<String>,
        default_prefix: Option<String>,
    ) -> async_graphql::Result<Workspace> {
        engine(ctx)
            .update_workspace(id, name, default_prefix)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))
    }

    /// The harness and model Lumi answers with, a workspace setting.
    async fn set_workspace_lumi(
        &self,
        ctx: &Context<'_>,
        workspace_id: Uuid,
        harness: String,
        model: String,
    ) -> async_graphql::Result<Workspace> {
        engine(ctx)
            .set_workspace_lumi(workspace_id, &harness, &model)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))
    }

    /// Ask Lumi. The question is stored at once; the answer arrives as its own
    /// message when the runner finishes, so the panel polls `lumiMessages`.
    async fn ask_lumi(
        &self,
        ctx: &Context<'_>,
        workspace_id: Uuid,
        text: String,
        context: Option<String>,
    ) -> async_graphql::Result<LumiMessage> {
        engine(ctx)
            .ask_lumi(workspace_id, &text, context.as_deref())
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))
    }

    /// Run the ticked items of a proposal. Indexes are positions in the
    /// proposal's `items` array.
    async fn apply_lumi_proposal(
        &self,
        ctx: &Context<'_>,
        message_id: Uuid,
        items: Vec<i32>,
    ) -> async_graphql::Result<LumiMessage> {
        let items: Vec<usize> = items.iter().map(|i| *i as usize).collect();
        engine(ctx)
            .apply_lumi_proposal(message_id, &items)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))
    }

    async fn dismiss_lumi_proposal(
        &self,
        ctx: &Context<'_>,
        message_id: Uuid,
    ) -> async_graphql::Result<LumiMessage> {
        engine(ctx)
            .dismiss_lumi_proposal(message_id)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))
    }

    async fn rename_project(
        &self,
        ctx: &Context<'_>,
        id: Uuid,
        name: String,
    ) -> async_graphql::Result<Project> {
        engine(ctx)
            .rename_project(id, &name)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))
    }

    async fn create_project(
        &self,
        ctx: &Context<'_>,
        workspace_id: Uuid,
        name: String,
        prefix: Option<String>,
    ) -> async_graphql::Result<Project> {
        let engine = engine(ctx);
        let id = Uuid::now_v7();
        // The prefix is what an issue is called by, so it is a decision made when
        // the project is made — with the project's own name as the first guess.
        let prefix = match prefix {
            Some(raw) => Engine::normalise_prefix(&raw)
                .map_err(|e| async_graphql::Error::new(e.to_string()))?,
            None => Engine::derive_prefix(&name),
        };
        let taken: Option<(String,)> =
            sqlx::query_as("select id from projects where workspace_id = ? and prefix = ?")
                .bind(workspace_id)
                .bind(&prefix)
                .fetch_optional(&engine.db.pool)
                .await?;
        if taken.is_some() {
            return Err(async_graphql::Error::new(format!(
                "the prefix {prefix} is already used by another project in this workspace"
            )));
        }
        sqlx::query(
            "insert into projects (id, workspace_id, name, prefix, created_at) values (?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(&name)
        .bind(&prefix)
        .bind(crate::db::now_rfc3339())
        .execute(&engine.db.pool)
        .await?;
        Ok(sqlx::query_as("select * from projects where id = ?")
            .bind(id)
            .fetch_one(&engine.db.pool)
            .await?)
    }

    async fn set_project_prefix(
        &self,
        ctx: &Context<'_>,
        project_id: Uuid,
        prefix: String,
    ) -> async_graphql::Result<Project> {
        Ok(engine(ctx).set_project_prefix(project_id, &prefix).await?)
    }

    async fn create_milestone(
        &self,
        ctx: &Context<'_>,
        workspace_id: Uuid,
        project_id: Option<Uuid>,
        title: String,
        body: Option<String>,
        target_date: Option<String>,
    ) -> async_graphql::Result<Milestone> {
        Ok(engine(ctx)
            .create_milestone(
                workspace_id,
                project_id,
                title,
                body.unwrap_or_default(),
                target_date,
            )
            .await?)
    }

    async fn update_milestone(
        &self,
        ctx: &Context<'_>,
        id: Uuid,
        title: Option<String>,
        body: Option<String>,
        status: Option<String>,
        target_date: Option<String>,
    ) -> async_graphql::Result<Milestone> {
        Ok(engine(ctx)
            .update_milestone(id, title, body, status, target_date)
            .await?)
    }

    async fn create_skill(
        &self,
        ctx: &Context<'_>,
        workspace_id: Uuid,
        name: String,
        description: Option<String>,
        source: String,
    ) -> async_graphql::Result<Skill> {
        Ok(engine(ctx)
            .create_skill(
                workspace_id,
                &name,
                description.as_deref().unwrap_or(""),
                &source,
            )
            .await?)
    }

    async fn update_skill(
        &self,
        ctx: &Context<'_>,
        id: Uuid,
        name: Option<String>,
        description: Option<String>,
        source: Option<String>,
    ) -> async_graphql::Result<Skill> {
        Ok(engine(ctx)
            .update_skill(
                id,
                name.as_deref(),
                description.as_deref(),
                source.as_deref(),
            )
            .await?)
    }

    async fn delete_skill(&self, ctx: &Context<'_>, id: Uuid) -> async_graphql::Result<bool> {
        Ok(engine(ctx).delete_skill(id).await?)
    }

    /// Replace the skills an agent carries.
    async fn set_agent_skills(
        &self,
        ctx: &Context<'_>,
        agent_id: Uuid,
        skill_ids: Vec<Uuid>,
    ) -> async_graphql::Result<Vec<Skill>> {
        Ok(engine(ctx).set_agent_skills(agent_id, &skill_ids).await?)
    }

    /// The agent's own instructions, appended to the built-in contract.
    async fn set_agent_instructions(
        &self,
        ctx: &Context<'_>,
        agent_id: Uuid,
        agents_md: String,
    ) -> async_graphql::Result<bool> {
        engine(ctx)
            .set_agent_instructions(agent_id, &agents_md)
            .await?;
        Ok(true)
    }

    /// Save the workflow document. The board follows it immediately.
    async fn set_workflow(
        &self,
        ctx: &Context<'_>,
        workspace_id: Uuid,
        source: String,
    ) -> async_graphql::Result<WorkflowDoc> {
        Ok(engine(ctx).set_workflow(workspace_id, &source).await?)
    }

    async fn set_issue_milestone(
        &self,
        ctx: &Context<'_>,
        issue_id: Uuid,
        milestone_id: Option<Uuid>,
    ) -> async_graphql::Result<Issue> {
        Ok(engine(ctx)
            .set_issue_milestone(issue_id, milestone_id)
            .await?)
    }

    /// Files arrive base64-encoded so the mutation stays one JSON body; the server
    /// writes the bytes and keeps the row as the index.
    #[allow(clippy::too_many_arguments)]
    async fn create_artifact(
        &self,
        ctx: &Context<'_>,
        workspace_id: Uuid,
        project_id: Option<Uuid>,
        milestone_id: Option<Uuid>,
        // Set to attach the file to a task; the task's runs then read it.
        issue_id: Option<Uuid>,
        title: Option<String>,
        kind: Option<String>,
        filename: String,
        mime: Option<String>,
        content_base64: String,
    ) -> async_graphql::Result<Artifact> {
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(content_base64.trim())
            .map_err(|e| async_graphql::Error::new(format!("the file is not valid base64: {e}")))?;
        if bytes.len() as u64 > hulahoop_core::ARTIFACT_LIMIT_BYTES {
            return Err(async_graphql::Error::new(
                "that file is larger than the artifact limit (500 MB)",
            ));
        }
        Ok(engine(ctx)
            .store_artifact(
                workspace_id,
                project_id,
                milestone_id,
                issue_id,
                title.unwrap_or_default(),
                kind,
                filename,
                mime,
                &bytes,
            )
            .await?)
    }

    async fn delete_artifact(&self, ctx: &Context<'_>, id: Uuid) -> async_graphql::Result<bool> {
        Ok(engine(ctx).delete_artifact(id).await?)
    }

    /// Store a GitHub app so the loop can commit as it. Replaceable: pasting a
    /// new key is the fix for an app whose key can no longer sign.
    async fn create_github_app(
        &self,
        ctx: &Context<'_>,
        app_id: String,
        private_key: String,
    ) -> async_graphql::Result<Connection> {
        Ok(engine(ctx).set_github_app(&app_id, &private_key).await?)
    }

    /// Forget the GitHub app specifically. Kept beside `disconnectConnection`
    /// because it predates the general form and a stored app is the one case where
    /// "disconnect" also changes how commits are authored.
    async fn disconnect_github_app(&self, ctx: &Context<'_>) -> async_graphql::Result<Connection> {
        Ok(engine(ctx).clear_connection("github").await?)
    }

    /// Store an API key for an agent CLI. The id names the entry, because Claude
    /// Code has two: a plan login and a billed key are separate purchases.
    async fn set_agent_key(
        &self,
        ctx: &Context<'_>,
        id: String,
        key: String,
    ) -> async_graphql::Result<Connection> {
        Ok(engine(ctx).set_agent_key(&id, &key).await?)
    }

    /// Store the credentials file a CLI wrote when it logged in.
    async fn set_agent_subscription(
        &self,
        ctx: &Context<'_>,
        id: String,
        contents: String,
    ) -> async_graphql::Result<Connection> {
        Ok(engine(ctx).set_agent_subscription(&id, &contents).await?)
    }

    /// Store a token a login minted (`claude setup-token`), which travels to a run
    /// as `CLAUDE_CODE_OAUTH_TOKEN` instead of a session file that expires hourly.
    async fn set_agent_token(
        &self,
        ctx: &Context<'_>,
        id: String,
        token: String,
    ) -> async_graphql::Result<Connection> {
        Ok(engine(ctx).set_agent_token(&id, &token).await?)
    }

    /// Forget a credential. Commits go back to the human's identity and a run falls
    /// back to whatever the machine has, which is the only honest thing to do when
    /// a secret is compromised.
    async fn disconnect_connection(
        &self,
        ctx: &Context<'_>,
        id: String,
    ) -> async_graphql::Result<Connection> {
        Ok(engine(ctx).clear_connection(&id).await?)
    }

    /// Attach a repository either from a checkout on this machine or from the
    /// GitHub app's listing. A remote-only repository is cloned by a local runner;
    /// a checkout is used in place, which is what a workspace runner does.
    async fn add_repo(
        &self,
        ctx: &Context<'_>,
        project_id: Uuid,
        name: String,
        path: Option<String>,
        remote_url: Option<String>,
        vcs: Option<String>,
    ) -> async_graphql::Result<Repo> {
        let engine = engine(ctx);
        let path = path
            .map(|value| value.trim().to_string())
            .filter(|v| !v.is_empty());
        let remote_url = remote_url
            .map(|value| value.trim().to_string())
            .filter(|v| !v.is_empty());
        if path.is_none() && remote_url.is_none() {
            return Err(async_graphql::Error::new(
                "attach a repository by path or by its GitHub remote; one of them is required",
            ));
        }
        // Structural contract check before anything is stored when there is a
        // checkout to look at: a repo without flake.nix and a justfile cannot run
        // the loop at all. A remote-only repository is checked by the first clone.
        if let Some(checkout) = &path {
            hulahoop_core::contract::check_files(std::path::Path::new(checkout))
                .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        }

        let id = Uuid::now_v7();
        let vcs = vcs.unwrap_or_else(|| {
            // A mirror is always cloned with jj (`jj git clone --colocate`), so a
            // repository attached by its remote is jj before it exists on disk.
            let checkout_is_jj = path
                .as_ref()
                .map(|checkout| std::path::Path::new(checkout).join(".jj").exists())
                .unwrap_or(false);
            if path.is_none() || checkout_is_jj {
                "jj".to_string()
            } else {
                "git".to_string()
            }
        });
        // The origin remote is what a pull request job pushes to and what a local
        // runner clones. Reading it from the checkout means nobody has to guess.
        let remote_url = match (&path, remote_url) {
            (Some(checkout), None) => crate::vcs_remote(std::path::Path::new(checkout)),
            (_, given) => given,
        };
        sqlx::query(
            "insert into repos (id, project_id, name, path, remote_url, vcs, base_bookmark, contract, contract_version, created_at)
             values (?, ?, ?, ?, ?, ?, 'main', 'nix+just', ?, ?)",
        )
        .bind(id)
        .bind(project_id)
        .bind(&name)
        .bind(&path)
        .bind(&remote_url)
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
        // A new agent reads the workspace's built-in skills from its first turn.
        let _ = engine(ctx).seed_builtin_skills(workspace_id).await;
        // Probe straight away: an agent that has never been used should not make
        // the first task that names it wait for a probe.
        let _ = engine(ctx).probe_everywhere(id).await;
        Ok(sqlx::query_as("select * from agents where id = ?")
            .bind(id)
            .fetch_one(&engine(ctx).db.pool)
            .await?)
    }

    /// Remove an agent. Its runs keep their history; a workflow that names it
    /// refuses to dispatch until another agent is chosen.
    async fn delete_agent(&self, ctx: &Context<'_>, id: Uuid) -> async_graphql::Result<bool> {
        let engine = engine(ctx);
        sqlx::query("delete from agent_preflight where agent_id = ?")
            .bind(id)
            .execute(&engine.db.pool)
            .await?;
        sqlx::query("delete from agent_skills where agent_id = ?")
            .bind(id)
            .execute(&engine.db.pool)
            .await?;
        let done = sqlx::query("delete from agents where id = ?")
            .bind(id)
            .execute(&engine.db.pool)
            .await?;
        Ok(done.rows_affected() > 0)
    }

    async fn update_agent(
        &self,
        ctx: &Context<'_>,
        id: Uuid,
        name: Option<String>,
        adapter: Option<String>,
        model: Option<String>,
        // Which connector the agent signs in with, where its harness offers two.
        connector: Option<String>,
    ) -> async_graphql::Result<Agent> {
        let engine = engine(ctx);
        for (column, value) in [
            ("name", name),
            ("adapter", adapter),
            ("model", model),
            ("connector", connector),
        ] {
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
        // A changed adapter or model makes the old probe meaningless: it answered a
        // question about a command line that no longer exists. Ask it again now,
        // so an agent nobody has used yet is ready before work arrives.
        engine.probe_everywhere(id).await?;
        Ok(sqlx::query_as("select * from agents where id = ?")
            .bind(id)
            .fetch_one(&engine.db.pool)
            .await?)
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
        // Optional: a project or a repository already names the workspace, and the
        // older callers predate the field.
        workspace_id: Option<Uuid>,
        project_id: Option<Uuid>,
        repo_id: Option<Uuid>,
        title: String,
        body: String,
        priority: Option<String>,
        due_date: Option<String>,
        assignee_type: Option<String>,
        assignee_id: Option<String>,
    ) -> async_graphql::Result<Issue> {
        let engine = engine(ctx);
        let priority = crate::engine::priority(priority.as_deref())
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        let due_date = crate::engine::due_date(due_date.as_deref().unwrap_or_default())
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        // A repository always exists, because a run has to know where to work: the
        // caller may name one, a project with a single repository supplies it, and
        // otherwise the workspace's first repository is the honest default.
        let repo_id = match repo_id {
            Some(id) => id,
            None => {
                let row: Option<(Uuid,)> = match project_id {
                    Some(project) => {
                        sqlx::query_as(
                            "select id from repos where project_id = ? order by created_at limit 1",
                        )
                        .bind(project)
                        .fetch_optional(&engine.db.pool)
                        .await?
                    }
                    None => {
                        let workspace: Option<(Uuid,)> = sqlx::query_as(
                            "select r.id from repos r join projects p on p.id = r.project_id where p.workspace_id = ? order by r.created_at limit 1",
                        )
                        .bind(workspace_id)
                        .fetch_optional(&engine.db.pool)
                        .await?;
                        workspace
                    }
                };
                row.map(|(id,)| id)
                    .ok_or_else(|| {
                        async_graphql::Error::new(
                            "no repository is attached yet, and a run needs one. Add a repository on the project first.",
                        )
                    })?
            }
        };
        let workspace_id = match workspace_id {
            Some(id) => id,
            None => {
                let derived: Option<(Uuid,)> = match project_id {
                    Some(project) => {
                        sqlx::query_as("select workspace_id from projects where id = ?")
                            .bind(project)
                            .fetch_optional(&engine.db.pool)
                            .await?
                    }
                    None => {
                        sqlx::query_as(
                            "select p.workspace_id from repos r join projects p on p.id = r.project_id where r.id = ?",
                        )
                        .bind(repo_id)
                        .fetch_optional(&engine.db.pool)
                        .await?
                    }
                };
                match derived.map(|(id,)| id) {
                    Some(id) => id,
                    None => return Err(async_graphql::Error::new(
                        "the issue needs a workspace: pass workspaceId, or file it under a project",
                    )),
                }
            }
        };
        let (number, issue_key) = engine
            .next_issue_key(workspace_id, project_id)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        let id = Uuid::now_v7();
        let now = crate::db::now_rfc3339();
        sqlx::query(
            "insert into issues (id, workspace_id, project_id, repo_id, number, issue_key, title, body, status, priority, due_date, created_at, updated_at)
             values (?, ?, ?, ?, ?, ?, ?, ?, 'backlog', ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(project_id)
        .bind(repo_id)
        .bind(number)
        .bind(&issue_key)
        .bind(&title)
        .bind(&body)
        .bind(priority)
        .bind(due_date)
        .bind(&now)
        .bind(&now)
        .execute(&engine.db.pool)
        .await?;
        if assignee_type.is_some() {
            engine
                .assign_issue(id, assignee_type.as_deref(), assignee_id.as_deref())
                .await
                .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        }
        Ok(engine.issue(id).await?)
    }

    #[allow(clippy::too_many_arguments)]
    async fn update_issue(
        &self,
        ctx: &Context<'_>,
        id: Uuid,
        title: Option<String>,
        body: Option<String>,
        priority: Option<String>,
        // `null` clears the date; leaving the argument out keeps it.
        due_date: MaybeUndefined<String>,
        // `assigneeType: null` clears the assignee; leaving it out keeps it.
        assignee_type: MaybeUndefined<String>,
        assignee_id: Option<String>,
    ) -> async_graphql::Result<Issue> {
        let engine = engine(ctx);
        engine.issue(id).await?;
        let priority = match priority {
            Some(value) => Some(
                crate::engine::priority(Some(&value))
                    .map_err(|e| async_graphql::Error::new(e.to_string()))?
                    .to_string(),
            ),
            None => None,
        };
        match due_date {
            MaybeUndefined::Undefined => {}
            MaybeUndefined::Null => engine.set_due_date(id, "").await?,
            MaybeUndefined::Value(value) => engine
                .set_due_date(id, &value)
                .await
                .map_err(|e| async_graphql::Error::new(e.to_string()))?,
        }
        match assignee_type {
            MaybeUndefined::Undefined => {}
            MaybeUndefined::Null => engine.assign_issue(id, None, None).await?,
            MaybeUndefined::Value(kind) => engine
                .assign_issue(id, Some(&kind), assignee_id.as_deref())
                .await
                .map_err(|e| async_graphql::Error::new(e.to_string()))?,
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
        Ok(engine.issue(id).await?)
    }

    /// The pull request merged, which is what closes a task.
    /// Reads the merge from GitHub and records it: the revision a merge produced
    /// is GitHub's fact, so pressing the button asks GitHub rather than a person.
    async fn merge_issue(&self, ctx: &Context<'_>, issue_id: Uuid) -> async_graphql::Result<Issue> {
        Ok(engine(ctx).merge_issue(issue_id).await?)
    }

    /// Back to To do, for work that is queued again after a merge or an abandon.
    async fn reopen_issue(
        &self,
        ctx: &Context<'_>,
        issue_id: Uuid,
    ) -> async_graphql::Result<Issue> {
        Ok(engine(ctx)
            .transition(issue_id, hulahoop_core::IssueEvent::Reopen)
            .await?)
    }

    /// The move a person makes by hand, as the document declares it: a blocked
    /// task has the exits its workflow lists, and no others.
    async fn move_issue(
        &self,
        ctx: &Context<'_>,
        issue_id: Uuid,
        status: String,
    ) -> async_graphql::Result<Issue> {
        let target = hulahoop_core::IssueStatus::parse(&status)
            .ok_or_else(|| async_graphql::Error::new(format!("`{status}` names no status")))?;
        Ok(engine(ctx).move_declared(issue_id, target).await?)
    }

    /// Backlog and To do are the same task on either side of the human's line.
    async fn plan_issue(
        &self,
        ctx: &Context<'_>,
        issue_id: Uuid,
        planned: bool,
    ) -> async_graphql::Result<Issue> {
        let event = if planned {
            hulahoop_core::IssueEvent::Plan
        } else {
            hulahoop_core::IssueEvent::Unplan
        };
        Ok(engine(ctx).transition(issue_id, event).await?)
    }

    async fn add_comment(
        &self,
        ctx: &Context<'_>,
        issue_id: Uuid,
        body: String,
    ) -> async_graphql::Result<Comment> {
        let engine = engine(ctx);
        // Mentions are acted on after the comment is stored, and each one adds
        // a note of its own, so the comment is fetched by id, not as the newest.
        let id = engine.human_comment(issue_id, &body).await?;
        Ok(
            sqlx::query_as::<_, Comment>("select * from comments where id = ?")
                .bind(id)
                .fetch_one(&engine.db.pool)
                .await?,
        )
    }

    /// Start a turn on a task. `agentId` names who works it; without one the
    /// workflow's queue decides, and only a workflow naming `Any agent` falls back
    /// to the project's bindings.
    async fn start_run(
        &self,
        ctx: &Context<'_>,
        issue_id: Uuid,
        agent_id: Option<Uuid>,
        parent_run_id: Option<Uuid>,
    ) -> async_graphql::Result<Run> {
        Ok(engine(ctx)
            .start_run(issue_id, agent_id, parent_run_id)
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

    /// The base replay reads a report the suite produced. When the report is missing
    /// for a reason of the harness (a `just test` that only wrote its JUnit file after a
    /// green run), asking again costs one suite run and no model call.
    async fn recheck_pull_request(
        &self,
        ctx: &Context<'_>,
        issue_id: Uuid,
    ) -> async_graphql::Result<Issue> {
        let engine = engine(ctx);
        let issue = engine.issue(issue_id).await?;
        let number = issue
            .pr_number
            .ok_or_else(|| async_graphql::Error::new("this issue has no pull request yet"))?;
        // The revision GitHub has to look at is the head the pull request points
        // at, which is the current run's head.
        let sha: Option<String> = match issue.current_run_id {
            Some(run_id) => sqlx::query_scalar::<_, Option<String>>(
                "select head_revision from runs where id = ?",
            )
            .bind(run_id)
            .fetch_optional(&engine.db.pool)
            .await?
            .flatten(),
            None => None,
        };
        engine
            .queue_job(
                issue.repo_id,
                Some(issue_id),
                "watch_checks",
                serde_json::json!({ "number": number, "sha": sha }),
            )
            .await?;
        Ok(engine.issue(issue_id).await?)
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

/// Nested reads stay on one screen's worth of data: the board asks for issues,
/// the detail screen asks for the rest.
#[ComplexObject]
impl Project {
    /// The repositories of this project. A project holds one in v0, but the
    /// screen asks for the list and the spec models a list.
    async fn repos(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<Repo>> {
        Ok(sqlx::query_as::<_, Repo>(
            "select * from repos where project_id = ? order by created_at",
        )
        .bind(self.id)
        .fetch_all(&engine(ctx).db.pool)
        .await?)
    }
}

#[ComplexObject]
impl Agent {
    /// The skills this agent carries, most relevant knowledge first by name.
    async fn skills(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<Skill>> {
        Ok(engine(ctx).agent_skills(self.id).await?)
    }
}

#[ComplexObject]
impl Issue {
    /// The places this task may go, as the workflow document declares them for the
    /// status it sits in. A blocked task shows exactly these exits, so a person
    /// moves it where the document says and nowhere else.
    async fn moves(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<String>> {
        Ok(engine(ctx)
            .workflow_moves(self.workspace_id, self.status_enum())
            .await?
            .into_iter()
            .map(|status| status.as_str().to_string())
            .collect())
    }

    /// Who still owes a turn in this status. Empty means the status is done with
    /// its team, and the next move (or a person) takes over.
    async fn turn_remaining(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<String>> {
        let queue: Vec<String> = serde_json::from_str(&self.turn_queue).unwrap_or_default();
        let served: Vec<String> = serde_json::from_str(&self.turn_served).unwrap_or_default();
        if queue.is_empty() {
            return Ok(Vec::new());
        }
        let mut left = Vec::new();
        for name in queue {
            let found: Option<(uuid::Uuid,)> = sqlx::query_as(
                "select id from agents where workspace_id = ? and lower(replace(name, ' ', '')) = lower(replace(?, ' ', '')) limit 1",
            )
            .bind(self.workspace_id)
            .bind(name.trim_start_matches('@'))
            .fetch_optional(&engine(ctx).db.pool)
            .await?;
            match found {
                Some((id,)) if served.iter().any(|done| *done == id.to_string()) => {}
                Some(_) => left.push(name),
                None => {}
            }
        }
        Ok(left)
    }

    /// The milestone this issue was filed under, if any.
    async fn milestone(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<Milestone>> {
        let Some(id) = self.milestone_id else {
            return Ok(None);
        };
        Ok(
            sqlx::query_as::<_, Milestone>("select * from milestones where id = ?")
                .bind(id)
                .fetch_optional(&engine(ctx).db.pool)
                .await?,
        )
    }

    /// What the task waits for, read from the agent's question. Shown where a
    /// person can act on it, so the drawer never just says "blocked".
    async fn blocked_by(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<String>> {
        if self.status != "blocked" {
            return Ok(None);
        }
        let Some(comment_id) = self.pinned_comment_id else {
            return Ok(None);
        };
        Ok(
            sqlx::query_scalar::<_, String>("select body from comments where id = ?")
                .bind(comment_id)
                .fetch_optional(&engine(ctx).db.pool)
                .await?,
        )
    }

    /// The project's name, so a drawer can show where the task lives in one query.
    async fn project_name(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<String>> {
        let Some(id) = self.project_id else {
            return Ok(None);
        };
        Ok(
            sqlx::query_scalar::<_, String>("select name from projects where id = ?")
                .bind(id)
                .fetch_optional(&engine(ctx).db.pool)
                .await?,
        )
    }

    /// The project's prefix, so a screen can show `BAL` without a second query.
    async fn project_prefix(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<String>> {
        let Some(id) = self.project_id else {
            return Ok(None);
        };
        let row: Option<(Option<String>,)> =
            sqlx::query_as("select prefix from projects where id = ?")
                .bind(id)
                .fetch_optional(&engine(ctx).db.pool)
                .await?;
        Ok(row.and_then(|(prefix,)| prefix))
    }

    /// Who is on the task, for people to read. The workflow decides who
    /// works it; an agent becomes the assignee when it starts a turn.
    async fn assignee(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<Option<crate::tasks::Assignee>> {
        Ok(engine(ctx).assignee(self).await?)
    }

    /// The effort the task has cost so far, in one aggregate over its runs.
    async fn stats(&self, ctx: &Context<'_>) -> async_graphql::Result<crate::tasks::IssueStats> {
        Ok(engine(ctx).issue_stats(self.id).await?)
    }

    /// The files attached to the task; its runs read them.
    async fn files(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<Artifact>> {
        Ok(engine(ctx).issue_files(self.id).await?)
    }

    async fn file_count(&self, ctx: &Context<'_>) -> async_graphql::Result<i64> {
        Ok(
            sqlx::query_scalar("select count(*) from artifacts where issue_id = ?")
                .bind(self.id)
                .fetch_one(&engine(ctx).db.pool)
                .await?,
        )
    }

    async fn runs(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<Run>> {
        Ok(sqlx::query_as::<_, Run>(
            "select * from runs where issue_id = ? order by created_at desc",
        )
        .bind(self.id)
        .fetch_all(&engine(ctx).db.pool)
        .await?)
    }

    /// The run the task is on. A task carries the newest attempt even when no run
    /// is live, so a screen never has to guess which attempt it is reading.
    async fn current_run(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<Run>> {
        if let Some(id) = self.current_run_id {
            let run: Option<Run> = sqlx::query_as("select * from runs where id = ?")
                .bind(id)
                .fetch_optional(&engine(ctx).db.pool)
                .await?;
            if run.is_some() {
                return Ok(run);
            }
        }
        Ok(sqlx::query_as::<_, Run>(
            "select * from runs where issue_id = ? order by created_at desc limit 1",
        )
        .bind(self.id)
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

    /// The thread, newest first unless asked otherwise. Comments written in
    /// the same instant keep the order they were stored in.
    async fn comments(
        &self,
        ctx: &Context<'_>,
        #[graphql(default = true)] newest_first: bool,
    ) -> async_graphql::Result<Vec<Comment>> {
        let order = if newest_first {
            "created_at desc, rowid desc"
        } else {
            "created_at, rowid"
        };
        Ok(sqlx::query_as::<_, Comment>(&format!(
            "select * from comments where issue_id = ? order by {order}"
        ))
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

    /// The run's transcript after `afterSeq`, normalized across harnesses.
    /// Poll with the returned `lastSeq` while `live` is true.
    async fn transcript(
        &self,
        ctx: &Context<'_>,
        #[graphql(default = 0)] after_seq: i64,
    ) -> async_graphql::Result<crate::transcript::Transcript> {
        Ok(engine(ctx).run_transcript(self, after_seq).await?)
    }

    async fn reviews(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<Review>> {
        Ok(sqlx::query_as::<_, Review>(
            "select * from reviews where reviewed_run_id = ? order by created_at desc",
        )
        .bind(self.id)
        .fetch_all(&engine(ctx).db.pool)
        .await?)
    }

    /// Which adapter ran the turn, so a run list can say `pi` or `claude_code`
    /// without a second round trip to the agent.
    async fn adapter(&self, ctx: &Context<'_>) -> async_graphql::Result<String> {
        if let Some(adapter) = &self.adapter_snapshot {
            return Ok(adapter.clone());
        }
        let adapter: Option<String> = sqlx::query_scalar("select adapter from agents where id = ?")
            .bind(self.agent_id)
            .fetch_optional(&engine(ctx).db.pool)
            .await?;
        Ok(adapter.unwrap_or_else(|| "unknown".to_string()))
    }
}
#[ComplexObject]
impl Comment {
    /// Who wrote it: the agent's name, `Lumi`, or `you` for a person.
    async fn author(&self, ctx: &Context<'_>) -> async_graphql::Result<String> {
        Ok(engine(ctx)
            .author_name(&self.author_type, self.author_id.as_deref())
            .await?)
    }

    /// The run that wrote it, for an agent's summary, question or verdict.
    async fn run(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<Run>> {
        let Some(run_id) = self.run_id else {
            return Ok(None);
        };
        Ok(sqlx::query_as::<_, Run>("select * from runs where id = ?")
            .bind(run_id)
            .fetch_optional(&engine(ctx).db.pool)
            .await?)
    }

    /// The agents and Lumi the comment named.
    async fn mentions(&self) -> Vec<crate::mentions::Mention> {
        serde_json::from_str(&self.mentions).unwrap_or_default()
    }
}

#[ComplexObject]
impl Approval {
    /// The gate flags the human acknowledged when approving this diff.
    async fn flags(&self) -> Vec<String> {
        crate::db::json_list(&self.acknowledged_flags)
    }
}
