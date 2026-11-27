//! GraphQL surface. Resolvers stay thin: every rule lives in [`crate::engine`],
//! so a mutation and a runner callback cannot disagree about the gate.

use async_graphql::{
    ComplexObject, Context, MaybeUndefined, Object, Schema, SimpleObject, Subscription, ID,
};
use uuid::Uuid;

use crate::engine::Engine;
use crate::memory_notes::Memory;
use crate::model::*;
use crate::pulls::{
    DiffSide, GithubAccount, MergeMethod, PullRequest, PullRequestComment, PullRequestDetail,
    PullRequestFilter, ReviewEvent, RunDiff,
};
use crate::workflows::WorkflowDoc;

pub type HulahoopSchema = Schema<Query, Mutation, SubscriptionRoot>;

pub fn build_schema(engine: std::sync::Arc<Engine>) -> HulahoopSchema {
    Schema::build(Query, Mutation, SubscriptionRoot)
        .data(engine)
        .limit_depth(12)
        .limit_complexity(2000)
        .finish()
}

/// What a WebSocket whose `connection_init` carried the wrong token is told.
pub const BAD_TOKEN: &str = "bad token";
/// `graphql-transport-ws`'s "Forbidden": the token did not match, and trying
/// again with the same one will not change that.
pub const CLOSE_FORBIDDEN: u16 = 4403;
/// `graphql-transport-ws`'s "Connection initialisation timeout".
pub const CLOSE_INIT_TIMEOUT: u16 = 4408;
/// How long a socket may stay open without a successful `connection_init`.
pub const WS_INIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The GraphQL WebSocket, which carries subscriptions. Speaks both
/// `graphql-transport-ws` and the older `graphql-ws`. The client proves it holds
/// the server token in `connection_init` (`{ "token": "…" }`); without it the
/// connection closes before any subscription starts: 4403 for a wrong token,
/// 4408 when no `connection_init` was accepted within [`WS_INIT_TIMEOUT`], and
/// 1011 for a subscription sent before it.
pub async fn serve_websocket(
    schema: HulahoopSchema,
    token: std::sync::Arc<str>,
    headers: axum::http::HeaderMap,
    upgrade: axum::extract::WebSocketUpgrade,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    use futures::{SinkExt, StreamExt};
    let protocol = headers
        .get(axum::http::header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())
        .and_then(|protocols| {
            protocols.split(',').find_map(|p| {
                p.trim()
                    .parse::<async_graphql::http::WebSocketProtocols>()
                    .ok()
            })
        });
    let Some(protocol) = protocol else {
        return axum::http::StatusCode::BAD_REQUEST.into_response();
    };
    upgrade
        .protocols(async_graphql::http::ALL_WEBSOCKET_PROTOCOLS)
        .on_upgrade(move |socket| async move {
            use axum::extract::ws::{CloseFrame, Message};
            let (mut sink, stream) = socket.split();
            let input = stream
                .take_while(|message| futures::future::ready(message.is_ok()))
                .filter_map(|message| {
                    futures::future::ready(match message {
                        Ok(message @ (Message::Text(_) | Message::Binary(_))) => {
                            Some(message.into_data())
                        }
                        _ => None,
                    })
                });
            let output = socket_messages(schema, token, input, protocol, WS_INIT_TIMEOUT);
            futures::pin_mut!(output);
            while let Some(message) = output.next().await {
                let message = match message {
                    async_graphql::http::WsMessage::Text(text) => Message::Text(text.into()),
                    async_graphql::http::WsMessage::Close(code, reason) => {
                        Message::Close(Some(CloseFrame {
                            code,
                            reason: reason.into(),
                        }))
                    }
                };
                if sink.send(message).await.is_err() {
                    break;
                }
            }
        })
}

/// The protocol side of [`serve_websocket`], over any stream of client frames,
/// so it can be driven without a socket. `init_timeout` is how long the client
/// has to get a `connection_init` accepted.
pub fn socket_messages<S>(
    schema: HulahoopSchema,
    token: std::sync::Arc<str>,
    input: S,
    protocol: async_graphql::http::WebSocketProtocols,
    init_timeout: std::time::Duration,
) -> impl futures::Stream<Item = async_graphql::http::WsMessage> + Send + 'static
where
    S: futures::Stream + Send + Unpin + 'static,
    S::Item: AsRef<[u8]>,
{
    use async_graphql::http::WsMessage;
    use futures::StreamExt;
    let inner = async_graphql::http::WebSocket::new(schema, input, protocol)
        .on_connection_init(move |payload: serde_json::Value| async move {
            let provided = payload["token"].as_str().unwrap_or_default();
            if crate::api::same_token(&token, provided) {
                Ok(async_graphql::Data::default())
            } else {
                Err(async_graphql::Error::new(BAD_TOKEN))
            }
        })
        .boxed();
    let deadline = Some(tokio::time::Instant::now() + init_timeout);
    futures::stream::unfold(
        (inner, deadline, false),
        |(mut inner, mut deadline, done)| async move {
            if done {
                return None;
            }
            let message = match deadline {
                Some(at) => tokio::select! {
                    message = inner.next() => message?,
                    _ = tokio::time::sleep_until(at) => {
                        let close = WsMessage::Close(
                            CLOSE_INIT_TIMEOUT,
                            "Connection initialisation timeout".to_string(),
                        );
                        return Some((close, (inner, None, true)));
                    }
                },
                None => inner.next().await?,
            };
            let message = match message {
                WsMessage::Text(text) => {
                    let acked = serde_json::from_str::<serde_json::Value>(&text)
                        .is_ok_and(|value| value["type"] == "connection_ack");
                    if acked {
                        deadline = None;
                    }
                    WsMessage::Text(text)
                }
                // async-graphql closes a refused init with 1002; the client is
                // told the protocol's own "forbidden" instead.
                WsMessage::Close(1002, reason) if reason == BAD_TOKEN => {
                    WsMessage::Close(CLOSE_FORBIDDEN, reason)
                }
                other => other,
            };
            let closing = matches!(message, WsMessage::Close(..));
            Some((message, (inner, deadline, closing)))
        },
    )
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

    /// One OAuth sign-in as it stands; the Connectors screen polls this while
    /// the person signs in. `null` once it is forgotten.
    async fn sign_in(
        &self,
        ctx: &Context<'_>,
        sign_in_id: ID,
    ) -> async_graphql::Result<Option<crate::signin::SignIn>> {
        Ok(crate::signin::status(engine(ctx), &sign_in_id)?)
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
    /// Pi also lists the models of every connected connector.
    async fn models(
        &self,
        ctx: &Context<'_>,
        harness: String,
    ) -> async_graphql::Result<Vec<crate::models::ModelOption>> {
        if harness != "pi" {
            return Ok(crate::models::catalogue(&harness));
        }
        let engine = engine(ctx);
        let connected = engine.connected_pi_providers().await?;
        Ok(crate::models::pi_catalogue(&engine.pi_bin, &connected).await)
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
        #[graphql(default)] project_ids: Vec<Uuid>,
        #[graphql(default = false)] no_project: bool,
    ) -> async_graphql::Result<i64> {
        Ok(engine(ctx)
            .done_issue_count(workspace_id, &project_ids, no_project)
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

    /// What the workspace's agents remember, newest change first. `scope` and
    /// `query` narrow the list; archived notes show only on request.
    async fn memories(
        &self,
        ctx: &Context<'_>,
        workspace_id: Uuid,
        scope: Option<String>,
        query: Option<String>,
        include_archived: Option<bool>,
    ) -> async_graphql::Result<Vec<Memory>> {
        Ok(engine(ctx)
            .memories(
                workspace_id,
                scope.as_deref(),
                query.as_deref(),
                include_archived.unwrap_or(false),
            )
            .await?)
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

    /// How many runs share the host and the room the runner last reported;
    /// null when no runner has reported in the last thirty seconds.
    async fn runner_capacity(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<Option<crate::capacity::RunnerCapacity>> {
        Ok(engine(ctx).runner_capacity().await?)
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

    /// The person's GitHub account, when they gave a token.
    async fn github_account(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<Option<GithubAccount>> {
        Ok(engine(ctx).github_account().await?)
    }

    /// Pull requests in the linked repositories, from the server's cache.
    async fn pull_requests(
        &self,
        ctx: &Context<'_>,
        #[graphql(default_with = "PullRequestFilter::Open")] filter: PullRequestFilter,
    ) -> async_graphql::Result<Vec<PullRequest>> {
        Ok(engine(ctx).pull_requests(filter).await?)
    }

    async fn open_pull_request_count(&self, ctx: &Context<'_>) -> async_graphql::Result<i64> {
        Ok(engine(ctx).open_pull_request_count().await?)
    }

    async fn pull_requests_synced_at(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<Option<chrono::DateTime<chrono::Utc>>> {
        Ok(engine(ctx).pull_requests_synced_at().await?)
    }

    async fn pull_request_sync_problems(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<Vec<String>> {
        Ok(engine(ctx).pull_request_sync_problems().await?)
    }

    /// One pull request, read live from GitHub.
    async fn pull_request(
        &self,
        ctx: &Context<'_>,
        repo_id: Uuid,
        number: i64,
    ) -> async_graphql::Result<Option<PullRequestDetail>> {
        Ok(engine(ctx).pull_request_detail(repo_id, number).await?)
    }
}

/// Named `Subscription` in the schema, like the contract; the Rust name only
/// avoids clashing with the derive.
pub struct SubscriptionRoot;

#[Subscription(name = "Subscription")]
impl SubscriptionRoot {
    /// A run's working diff: the current one at once, then every change, and
    /// the final one with `live: false` before the stream ends.
    async fn run_diff(
        &self,
        ctx: &Context<'_>,
        run_id: Uuid,
    ) -> async_graphql::Result<impl futures::Stream<Item = RunDiff>> {
        let engine = ctx.data_unchecked::<std::sync::Arc<Engine>>();
        Ok(engine.watch_diff(run_id).await?)
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
    /// Checks a GitHub token and keeps it sealed. Fails when GitHub does not
    /// say whose it is.
    async fn set_github_token(
        &self,
        ctx: &Context<'_>,
        token: String,
    ) -> async_graphql::Result<GithubAccount> {
        Ok(engine(ctx).set_github_token(&token).await?)
    }

    async fn clear_github_token(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        Ok(engine(ctx).clear_github_token().await?)
    }

    /// Syncs every linked repository now and answers when it finished.
    async fn sync_pull_requests(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<chrono::DateTime<chrono::Utc>> {
        Ok(engine(ctx).sync_pull_requests(None).await?)
    }

    #[allow(clippy::too_many_arguments)]
    async fn add_pull_request_comment(
        &self,
        ctx: &Context<'_>,
        repo_id: Uuid,
        number: i64,
        path: String,
        line: i64,
        side: DiffSide,
        body: String,
    ) -> async_graphql::Result<PullRequestComment> {
        Ok(engine(ctx)
            .add_pull_request_comment(repo_id, number, &path, line, side, &body)
            .await?)
    }

    async fn delete_pull_request_comment(
        &self,
        ctx: &Context<'_>,
        id: async_graphql::ID,
    ) -> async_graphql::Result<bool> {
        Ok(engine(ctx).delete_pull_request_comment(&id).await?)
    }

    async fn review_pull_request(
        &self,
        ctx: &Context<'_>,
        repo_id: Uuid,
        number: i64,
        event: ReviewEvent,
        body: Option<String>,
    ) -> async_graphql::Result<PullRequestDetail> {
        Ok(engine(ctx)
            .review_pull_request(repo_id, number, event, body.as_deref())
            .await?)
    }

    /// Merges on GitHub, pinned to `head_sha`: the head the person reviewed.
    /// Left out, the cached head is used.
    async fn merge_pull_request(
        &self,
        ctx: &Context<'_>,
        repo_id: Uuid,
        number: i64,
        method: MergeMethod,
        head_sha: Option<String>,
    ) -> async_graphql::Result<PullRequest> {
        Ok(engine(ctx)
            .merge_pull_request(repo_id, number, method, head_sha.as_deref())
            .await?)
    }

    /// Deletes the branch a merged pull request left behind, on GitHub, with
    /// the person's token.
    async fn delete_pull_request_branch(
        &self,
        ctx: &Context<'_>,
        repo_id: Uuid,
        number: i64,
    ) -> async_graphql::Result<PullRequestDetail> {
        Ok(engine(ctx)
            .delete_pull_request_branch(repo_id, number)
            .await?)
    }

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
        color: Option<String>,
        repos: Option<Vec<NewRepoInput>>,
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
        let color = match color {
            Some(color) => {
                Engine::check_project_color(&color)?;
                color
            }
            None => engine.next_project_color(workspace_id).await?.to_string(),
        };
        let repos = repos
            .unwrap_or_default()
            .into_iter()
            .map(|repo| {
                Ok((
                    repo.name,
                    remote_url_to_clone(&repo.remote_url)?,
                    repo.base_bookmark,
                ))
            })
            .collect::<async_graphql::Result<Vec<_>>>()?;
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
        // The project and its repositories save together or not at all.
        let mut tx = engine.db.pool.begin().await?;
        sqlx::query(
            "insert into projects (id, workspace_id, name, prefix, color, created_at) values (?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(&name)
        .bind(&prefix)
        .bind(&color)
        .bind(crate::db::now_rfc3339())
        .execute(&mut *tx)
        .await?;
        let mut repo_ids = Vec::new();
        for (repo_name, remote_url, base_bookmark) in &repos {
            let repo_id = Uuid::now_v7();
            sqlx::query(
                "insert into repos (id, project_id, name, remote_url, vcs, base_bookmark, contract, contract_version, created_at)
                 values (?, ?, ?, ?, 'jj', ?, 'nix+just', ?, ?)",
            )
            .bind(repo_id)
            .bind(id)
            .bind(repo_name)
            .bind(remote_url)
            .bind(base_bookmark.as_deref().unwrap_or("main"))
            .bind(hulahoop_core::CONTRACT_VERSION)
            .bind(crate::db::now_rfc3339())
            .execute(&mut *tx)
            .await?;
            repo_ids.push(repo_id);
        }
        tx.commit().await?;
        for repo_id in repo_ids {
            engine
                .queue_job(repo_id, None, "fetch", serde_json::json!({}))
                .await?;
        }
        Ok(sqlx::query_as("select * from projects where id = ?")
            .bind(id)
            .fetch_one(&engine.db.pool)
            .await?)
    }

    async fn update_project(
        &self,
        ctx: &Context<'_>,
        id: Uuid,
        name: Option<String>,
        color: Option<String>,
    ) -> async_graphql::Result<Project> {
        engine(ctx)
            .update_project(id, name.as_deref(), color.as_deref())
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))
    }

    /// Take a repository off its project. Refused while open tasks use it.
    async fn unlink_repo(&self, ctx: &Context<'_>, id: Uuid) -> async_graphql::Result<Project> {
        engine(ctx)
            .unlink_repo(id)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))
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

    /// Write a memory by hand. `scope_ref` is the project id for `project`, a
    /// name for `agent` and `person`, and empty for `workspace`.
    #[allow(clippy::too_many_arguments)]
    async fn create_memory(
        &self,
        ctx: &Context<'_>,
        workspace_id: Uuid,
        scope: String,
        scope_ref: Option<String>,
        kind: String,
        title: String,
        body: String,
    ) -> async_graphql::Result<Memory> {
        Ok(engine(ctx)
            .create_memory(
                workspace_id,
                &scope,
                scope_ref.as_deref().unwrap_or(""),
                &kind,
                &title,
                &body,
            )
            .await?)
    }

    async fn update_memory(
        &self,
        ctx: &Context<'_>,
        id: Uuid,
        kind: Option<String>,
        title: Option<String>,
        body: Option<String>,
    ) -> async_graphql::Result<Memory> {
        Ok(engine(ctx)
            .edit_memory(id, kind.as_deref(), title.as_deref(), body.as_deref())
            .await?)
    }

    /// Archive a memory (`archived: true`) or bring it back.
    async fn archive_memory(
        &self,
        ctx: &Context<'_>,
        id: Uuid,
        archived: bool,
    ) -> async_graphql::Result<Memory> {
        Ok(engine(ctx).archive_memory(id, archived).await?)
    }

    async fn forget_memory(&self, ctx: &Context<'_>, id: Uuid) -> async_graphql::Result<bool> {
        Ok(engine(ctx).forget_memory(id).await?)
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

    /// A document artifact: type, Markdown content, author, owned by a task or a
    /// milestone. Add its files with `addArtifactFile`.
    #[allow(clippy::too_many_arguments)]
    async fn create_artifact_document(
        &self,
        ctx: &Context<'_>,
        workspace_id: Uuid,
        title: String,
        #[graphql(name = "type")] artifact_type: String,
        content: Option<String>,
        issue_id: Option<Uuid>,
        milestone_id: Option<Uuid>,
        author: Option<String>,
        author_is_agent: Option<bool>,
    ) -> async_graphql::Result<Artifact> {
        Ok(engine(ctx)
            .create_artifact_document(
                workspace_id,
                title,
                artifact_type,
                content.unwrap_or_default(),
                issue_id,
                milestone_id,
                author.unwrap_or_else(|| "You".into()),
                author_is_agent.unwrap_or(false),
            )
            .await?)
    }

    async fn update_artifact(
        &self,
        ctx: &Context<'_>,
        id: Uuid,
        title: Option<String>,
        #[graphql(name = "type")] artifact_type: Option<String>,
        content: Option<String>,
    ) -> async_graphql::Result<Artifact> {
        Ok(engine(ctx)
            .update_artifact(id, title, artifact_type, content)
            .await?)
    }

    /// Change what the artifact belongs to: exactly one of a task or a milestone.
    async fn move_artifact(
        &self,
        ctx: &Context<'_>,
        id: Uuid,
        issue_id: Option<Uuid>,
        milestone_id: Option<Uuid>,
    ) -> async_graphql::Result<Artifact> {
        Ok(engine(ctx)
            .move_artifact(id, issue_id, milestone_id)
            .await?)
    }

    async fn add_artifact_file(
        &self,
        ctx: &Context<'_>,
        artifact_id: Uuid,
        filename: String,
        mime: Option<String>,
        content_base64: String,
    ) -> async_graphql::Result<ArtifactFile> {
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
            .add_artifact_file(artifact_id, filename, mime, &bytes)
            .await?)
    }

    async fn remove_artifact_file(
        &self,
        ctx: &Context<'_>,
        id: Uuid,
    ) -> async_graphql::Result<bool> {
        Ok(engine(ctx).remove_artifact_file(id).await?)
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

    /// Start an OAuth sign-in for a subscription entry (`claude-sub`,
    /// `codex-sub`). Answers once there is a link or a code to show.
    async fn start_sign_in(
        &self,
        ctx: &Context<'_>,
        id: String,
    ) -> async_graphql::Result<crate::signin::SignIn> {
        let engine = ctx.data_unchecked::<std::sync::Arc<Engine>>();
        Ok(crate::signin::start(engine, &id).await?)
    }

    /// Pass the code the provider showed after the person signed in.
    async fn submit_sign_in_code(
        &self,
        ctx: &Context<'_>,
        sign_in_id: ID,
        code: String,
    ) -> async_graphql::Result<crate::signin::SignIn> {
        Ok(crate::signin::submit_code(engine(ctx), &sign_in_id, &code).await?)
    }

    /// Stop a sign-in; `false` when it had already ended.
    async fn cancel_sign_in(
        &self,
        ctx: &Context<'_>,
        sign_in_id: ID,
    ) -> async_graphql::Result<bool> {
        Ok(crate::signin::cancel(engine(ctx), &sign_in_id)?)
    }

    /// Say which plan a connected subscription is on; `null` clears it. A plan
    /// with a published fee fills in Cost control's plan for every workspace.
    async fn set_connector_plan(
        &self,
        ctx: &Context<'_>,
        id: String,
        plan: Option<String>,
    ) -> async_graphql::Result<Connection> {
        Ok(engine(ctx).set_connector_plan(&id, plan.as_deref()).await?)
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

    /// Attach a repository by its url. Hulahoop clones it into a mirror under
    /// its data directory and every task works in a checkout of that mirror, so
    /// a checkout on this machine is never borrowed. Any url `jj git clone`
    /// accepts works, a local path included; pull requests need a GitHub one.
    async fn add_repo(
        &self,
        ctx: &Context<'_>,
        project_id: Uuid,
        name: String,
        remote_url: String,
        vcs: Option<String>,
    ) -> async_graphql::Result<Repo> {
        let engine = engine(ctx);
        let remote_url = remote_url_to_clone(&remote_url)?;

        let id = Uuid::now_v7();
        // A mirror is always cloned with jj (`jj git clone --colocate`), so a
        // repository is jj before it exists on disk.
        let vcs = vcs.unwrap_or_else(|| "jj".to_string());
        sqlx::query(
            "insert into repos (id, project_id, name, remote_url, vcs, base_bookmark, contract, contract_version, created_at)
             values (?, ?, ?, ?, ?, 'main', 'nix+just', ?, ?)",
        )
        .bind(id)
        .bind(project_id)
        .bind(&name)
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

    /// Rename a repository, change its base bookmark, or point it at another
    /// url. A new url forgets the mirror of the old one, so the next job clones
    /// the new url, and forgets the base learned from the old one.
    async fn update_repo(
        &self,
        ctx: &Context<'_>,
        id: Uuid,
        name: Option<String>,
        base_bookmark: Option<String>,
        remote_url: Option<String>,
    ) -> async_graphql::Result<Repo> {
        let engine = engine(ctx);
        if let Some(remote_url) = remote_url {
            let remote_url = remote_url_to_clone(&remote_url)?;
            let current: Option<String> =
                sqlx::query_scalar("select remote_url from repos where id = ?")
                    .bind(id)
                    .fetch_one(&engine.db.pool)
                    .await?;
            if current.as_deref() != Some(remote_url.as_str()) {
                sqlx::query(
                    "update repos set remote_url = ?, base_revision = null, fetched_at = null
                     where id = ?",
                )
                .bind(&remote_url)
                .bind(id)
                .execute(&engine.db.pool)
                .await?;
                engine
                    .queue_job(id, None, "forget_mirror", serde_json::json!({}))
                    .await?;
                engine
                    .queue_job(id, None, "fetch", serde_json::json!({}))
                    .await?;
            }
        }
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
        // Which connector the agent signs in with; none means automatic.
        connector: Option<String>,
    ) -> async_graphql::Result<Agent> {
        let id = Uuid::now_v7();
        let now = crate::db::now_rfc3339();
        let connector = connector.filter(|id| !id.is_empty());
        sqlx::query(
            "insert into agents (id, workspace_id, name, adapter, model, connector, created_at, updated_at)
             values (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(&name)
        .bind(&adapter)
        .bind(&model)
        .bind(&connector)
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
        // Optional; it has to be a milestone of the same workspace.
        milestone_id: Option<Uuid>,
    ) -> async_graphql::Result<Issue> {
        let engine = engine(ctx);
        let priority = crate::engine::priority(priority.as_deref())
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        let due_date = crate::engine::due_date(due_date.as_deref().unwrap_or_default())
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        // The repository follows the project: a caller may name one, a project
        // supplies its first, and a task with no project (or a project with no
        // repository) has none. Its runs work in an empty scratch directory.
        let repo_id = match (repo_id, project_id) {
            (Some(id), _) => Some(id),
            (None, Some(project)) => first_repo_of(&engine.db.pool, project).await?,
            (None, None) => None,
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
                    None => match repo_id {
                        Some(repo_id) => {
                            sqlx::query_as(
                                "select p.workspace_id from repos r join projects p on p.id = r.project_id where r.id = ?",
                            )
                            .bind(repo_id)
                            .fetch_optional(&engine.db.pool)
                            .await?
                        }
                        None => None,
                    },
                };
                match derived.map(|(id,)| id) {
                    Some(id) => id,
                    None => return Err(async_graphql::Error::new(
                        "the issue needs a workspace: pass workspaceId, or file it under a project",
                    )),
                }
            }
        };
        if let Some(milestone_id) = milestone_id {
            let milestone = engine.milestone(milestone_id).await?;
            if milestone.workspace_id != workspace_id {
                return Err(async_graphql::Error::new(
                    "that milestone belongs to another workspace",
                ));
            }
        }
        let (number, issue_key) = engine
            .next_issue_key(workspace_id, project_id)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        let id = Uuid::now_v7();
        let now = crate::db::now_rfc3339();
        sqlx::query(
            "insert into issues (id, workspace_id, project_id, repo_id, milestone_id, number, issue_key, title, body, status, priority, due_date, created_at, updated_at)
             values (?, ?, ?, ?, ?, ?, ?, ?, ?, 'backlog', ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(project_id)
        .bind(repo_id)
        .bind(milestone_id)
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
        // A project brings its first repository; `null` takes the task out of
        // any project and leaves it repo-less. Leaving it out keeps both. Only
        // while no run is live, because a run is working in the old place.
        project_id: MaybeUndefined<Uuid>,
    ) -> async_graphql::Result<Issue> {
        let engine = engine(ctx);
        let issue = engine.issue(id).await?;
        let project = match project_id {
            MaybeUndefined::Undefined => None,
            MaybeUndefined::Null => Some(None),
            MaybeUndefined::Value(project) => Some(Some(project)),
        };
        if let Some(project) = project {
            if project != issue.project_id {
                set_issue_project(engine, &issue, project).await?;
            }
        }
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
    /// A task with no repository just closes.
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
        Ok(engine(ctx).request_changes(issue_id, &body).await?)
    }

    /// Asks the runner to read the pull request's checks again at the current
    /// head. Costs one GitHub call and no model call.
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
        let repo_id = issue.repo_id.ok_or_else(|| {
            async_graphql::Error::new("this issue has no repository, so it has no pull request")
        })?;
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
                repo_id,
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
        Ok(engine(ctx).clear_run_cap(issue_id).await?)
    }
}

/// A repository to link in the same save as `createProject`.
#[derive(async_graphql::InputObject)]
struct NewRepoInput {
    name: String,
    remote_url: String,
    base_bookmark: Option<String>,
}

/// Nested reads stay on one screen's worth of data: the board asks for issues,
/// the detail screen asks for the rest.
#[ComplexObject]
impl Artifact {
    async fn files(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<ArtifactFile>> {
        Ok(engine(ctx).artifact_files(self.id).await?)
    }
}

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

    /// Runs used since a person last allowed more; at `runLimit` no agent starts
    /// until somebody calls `clearRunCap`.
    async fn runs_since_cap(&self, ctx: &Context<'_>) -> async_graphql::Result<i64> {
        Ok(engine(ctx).runs_since_cap(self).await?)
    }

    async fn run_limit(&self) -> i64 {
        hulahoop_core::RUNS_PER_ISSUE
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
        Ok(sqlx::query_scalar(
            "select count(*) from artifacts where issue_id = ? and filename <> ''",
        )
        .bind(self.id)
        .fetch_one(&engine(ctx).db.pool)
        .await?)
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
    /// A live run comes first: it is what a person can watch or cancel.
    async fn current_run(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<Run>> {
        let live: Option<Run> = sqlx::query_as(
            "select * from runs where issue_id = ? and status in ('queued', 'running')
              order by created_at desc limit 1",
        )
        .bind(self.id)
        .fetch_optional(&engine(ctx).db.pool)
        .await?;
        if live.is_some() {
            return Ok(live);
        }
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

/// The repository a task in this project works in: the project's first, or
/// none when the project has no repository yet.
async fn first_repo_of(pool: &sqlx::SqlitePool, project_id: Uuid) -> sqlx::Result<Option<Uuid>> {
    sqlx::query_scalar("select id from repos where project_id = ? order by created_at limit 1")
        .bind(project_id)
        .fetch_optional(pool)
        .await
}

/// The url a repository is cloned from, checked before it reaches
/// `jj git clone`: a web, ssh, git or file url, an absolute path, or the
/// scp-like `user@host:path` form. Anything starting with `-` would be read as
/// an option, so it is refused even where the clone separates it with `--`.
fn remote_url_to_clone(raw: &str) -> async_graphql::Result<String> {
    let url = raw.trim();
    if url.is_empty() {
        return Err(async_graphql::Error::new(
            "attach a repository by its url; Hulahoop clones it from there",
        ));
    }
    let schemes = ["http://", "https://", "ssh://", "git://", "file://"];
    let with_scheme = schemes
        .iter()
        .any(|scheme| url.len() > scheme.len() && url.to_ascii_lowercase().starts_with(scheme));
    let scp_like = url.split_once(':').is_some_and(|(login, path)| {
        login
            .split_once('@')
            .is_some_and(|(user, host)| !user.is_empty() && !host.is_empty() && !host.contains('/'))
            && !path.is_empty()
    });
    let plain = !url.starts_with('-') && !url.chars().any(char::is_control);
    if plain && (with_scheme || std::path::Path::new(url).is_absolute() || scp_like) {
        Ok(url.to_string())
    } else {
        Err(async_graphql::Error::new(format!(
            "`{url}` is not a repository url Hulahoop can clone: use https://, ssh://, git:// or file://, an absolute path, or user@host:path"
        )))
    }
}

/// Moves a task into a project (taking its first repository) or out of every
/// project (leaving it repo-less). Refused while a run is live: that run is
/// working in the old place and would finish against the new one. The
/// checkout the task had in the old place is deleted by a runner job, and one
/// still queued for the place it moves back to is dropped.
async fn set_issue_project(
    engine: &Engine,
    issue: &Issue,
    project_id: Option<Uuid>,
) -> async_graphql::Result<()> {
    let pool = &engine.db.pool;
    let live: i64 = sqlx::query_scalar(
        "select count(*) from runs where issue_id = ? and status in ('queued', 'running')",
    )
    .bind(issue.id)
    .fetch_one(pool)
    .await?;
    if live > 0 {
        return Err(async_graphql::Error::new(
            "a run is live on this task; cancel it or let it finish before changing the project",
        ));
    }
    let repo_id = match project_id {
        Some(project) => {
            let workspace: Option<Uuid> =
                sqlx::query_scalar("select workspace_id from projects where id = ?")
                    .bind(project)
                    .fetch_optional(pool)
                    .await?;
            if workspace != Some(issue.workspace_id) {
                return Err(async_graphql::Error::new(
                    "that project is not in this task's workspace",
                ));
            }
            first_repo_of(pool, project).await?
        }
        None => None,
    };
    sqlx::query("update issues set project_id = ?, repo_id = ?, updated_at = ? where id = ?")
        .bind(project_id)
        .bind(repo_id)
        .bind(crate::db::now_rfc3339())
        .bind(issue.id)
        .execute(pool)
        .await?;
    if let Err(error) = engine
        .queue_cleanup(issue, crate::engine::CLEANUP_MOVED)
        .await
    {
        tracing::warn!(issue = %issue.id, %error, "could not queue the cleanup of the old checkout");
    }
    let moved = engine.issue(issue.id).await?;
    if let Err(error) = engine.keep_current_checkout(&moved).await {
        tracing::warn!(issue = %issue.id, %error, "could not keep the checkout of the new place");
    }
    Ok(())
}

#[cfg(test)]
mod repo_less_tests {
    use super::build_schema;
    use crate::db::Db;
    use crate::engine::Engine;
    use chrono::Utc;
    use std::sync::Arc;
    use uuid::Uuid;

    async fn run(schema: &super::HulahoopSchema, query: String) -> serde_json::Value {
        let response = schema.execute(query).await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        response.data.into_json().unwrap()
    }

    /// A task filed with no project has no repository; one filed under a
    /// project takes that project's repository; moving a task between the two
    /// takes or drops the repository with it.
    #[tokio::test]
    async fn the_repository_follows_the_project() {
        let directory = std::env::temp_dir().join(format!("hulahoop-gql-{}", Uuid::now_v7()));
        let db = Db::open(&directory).await.unwrap();
        let workspace = db.seed_local().await.unwrap();
        let (project, repo) = (Uuid::now_v7(), Uuid::now_v7());
        let now = Utc::now();
        sqlx::query(
            "insert into projects (id, workspace_id, name, created_at) values (?,?, 'App',?)",
        )
        .bind(project)
        .bind(workspace)
        .bind(now)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query("insert into repos (id, project_id, name, remote_url, contract_version, created_at) values (?,?, 'repo','/tmp/repo','1',?)")
            .bind(repo).bind(project).bind(now).execute(&db.pool).await.unwrap();
        let schema = build_schema(Arc::new(Engine::new(db)));

        let loose = run(
            &schema,
            format!(r#"mutation {{ createIssue(workspaceId: "{workspace}", title: "Research", body: "") {{ id repoId projectId }} }}"#),
        )
        .await;
        assert!(loose["createIssue"]["repoId"].is_null());
        assert!(loose["createIssue"]["projectId"].is_null());

        let filed = run(
            &schema,
            format!(r#"mutation {{ createIssue(projectId: "{project}", title: "Button", body: "") {{ id repoId }} }}"#),
        )
        .await;
        assert_eq!(filed["createIssue"]["repoId"], repo.to_string());

        let id = loose["createIssue"]["id"].as_str().unwrap().to_string();
        let moved = run(
            &schema,
            format!(r#"mutation {{ updateIssue(id: "{id}", projectId: "{project}") {{ repoId projectId }} }}"#),
        )
        .await;
        assert_eq!(moved["updateIssue"]["repoId"], repo.to_string());
        assert_eq!(moved["updateIssue"]["projectId"], project.to_string());

        let cleared = run(
            &schema,
            format!(
                r#"mutation {{ updateIssue(id: "{id}", projectId: null) {{ repoId projectId }} }}"#
            ),
        )
        .await;
        assert!(cleared["updateIssue"]["repoId"].is_null());
        assert!(cleared["updateIssue"]["projectId"].is_null());

        let untouched = run(
            &schema,
            format!(r#"mutation {{ updateIssue(id: "{id}", title: "Research, again") {{ repoId title }} }}"#),
        )
        .await;
        assert!(untouched["updateIssue"]["repoId"].is_null());

        let _ = std::fs::remove_dir_all(directory);
    }

    /// The New task sheet can file a task under a milestone, but only one of
    /// the task's own workspace.
    #[tokio::test]
    async fn a_new_task_can_start_in_a_milestone_of_its_workspace() {
        let directory = std::env::temp_dir().join(format!("hulahoop-gql-{}", Uuid::now_v7()));
        let db = Db::open(&directory).await.unwrap();
        let workspace = db.seed_local().await.unwrap();
        let engine = Arc::new(Engine::new(db));
        let ours = engine
            .create_milestone(workspace, None, "Beta".into(), String::new(), None)
            .await
            .unwrap();
        let other = engine
            .create_workspace("Elsewhere", Some("ELS"))
            .await
            .unwrap();
        let theirs = engine
            .create_milestone(other.id, None, "Launch".into(), String::new(), None)
            .await
            .unwrap();
        let schema = build_schema(engine);

        let filed = run(
            &schema,
            format!(
                r#"mutation {{ createIssue(workspaceId: "{workspace}", title: "Docs", body: "", milestoneId: "{}") {{ milestoneId }} }}"#,
                ours.id
            ),
        )
        .await;
        assert_eq!(filed["createIssue"]["milestoneId"], ours.id.to_string());

        let loose = run(
            &schema,
            format!(r#"mutation {{ createIssue(workspaceId: "{workspace}", title: "Notes", body: "") {{ milestoneId }} }}"#),
        )
        .await;
        assert!(loose["createIssue"]["milestoneId"].is_null());

        let refused = schema
            .execute(format!(
                r#"mutation {{ createIssue(workspaceId: "{workspace}", title: "Leak", body: "", milestoneId: "{}") {{ id }} }}"#,
                theirs.id
            ))
            .await;
        assert_eq!(refused.errors.len(), 1);
        assert!(refused.errors[0].message.contains("another workspace"));

        let _ = std::fs::remove_dir_all(directory);
    }
}

#[cfg(test)]
mod repo_url_tests {
    use super::{build_schema, remote_url_to_clone};
    use crate::db::Db;
    use crate::engine::Engine;
    use chrono::Utc;
    use std::sync::Arc;
    use uuid::Uuid;

    #[test]
    fn only_urls_jj_can_clone_are_accepted() {
        for url in [
            "https://github.com/acme/app.git",
            "http://git.local/app",
            "ssh://git@github.com/acme/app",
            "git://git.local/app",
            "file:///srv/app",
            "/srv/repos/app",
            "git@github.com:acme/app.git",
            "  https://github.com/acme/app  ",
        ] {
            assert!(remote_url_to_clone(url).is_ok(), "{url}");
        }
        for url in [
            "",
            "   ",
            "-uhack",
            "--upload-pack=touch /tmp/x",
            "relative/path",
            "github.com:acme/app",
            "@host:path",
            "user@:path",
            "user@host:",
            "https://",
            "ftp://host/app",
            "https://host/app\nmore",
        ] {
            assert!(remote_url_to_clone(url).is_err(), "{url:?}");
        }
    }

    async fn workspace_with_repo(
        name: &str,
    ) -> (Arc<Engine>, Uuid, Uuid, Uuid, std::path::PathBuf) {
        let directory = std::env::temp_dir().join(format!("hulahoop-{name}-{}", Uuid::now_v7()));
        let db = Db::open(&directory).await.unwrap();
        let workspace = db.seed_local().await.unwrap();
        let (project, repo) = (Uuid::now_v7(), Uuid::now_v7());
        let now = Utc::now();
        sqlx::query(
            "insert into projects (id, workspace_id, name, created_at) values (?,?, 'App',?)",
        )
        .bind(project)
        .bind(workspace)
        .bind(now)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query("insert into repos (id, project_id, name, remote_url, base_revision, contract_version, created_at) values (?,?, 'repo','/srv/old','abc','1',?)")
            .bind(repo).bind(project).bind(now).execute(&db.pool).await.unwrap();
        (
            Arc::new(Engine::new(db)),
            workspace,
            project,
            repo,
            directory,
        )
    }

    async fn job_kinds(engine: &Engine) -> Vec<(String, String)> {
        sqlx::query_as("select kind, payload from runner_jobs order by created_at")
            .fetch_all(&engine.db.pool)
            .await
            .unwrap()
    }

    /// A new url forgets the old mirror and the base learned from it, and
    /// fetches again; the same url, or one that is not a url, changes nothing.
    #[tokio::test]
    async fn a_new_url_forgets_the_mirror() {
        let (engine, _, project, repo, directory) = workspace_with_repo("repo-url").await;
        let schema = build_schema(engine.clone());

        let refused = schema
            .execute(format!(
                r#"mutation {{ updateRepo(id: "{repo}", remoteUrl: "--upload-pack=x") {{ id }} }}"#
            ))
            .await;
        assert_eq!(refused.errors.len(), 1);
        let refused = schema
            .execute(format!(
                r#"mutation {{ addRepo(projectId: "{project}", name: "bad", remoteUrl: "-uhack") {{ id }} }}"#
            ))
            .await;
        assert_eq!(refused.errors.len(), 1);
        let same = schema
            .execute(format!(
                r#"mutation {{ updateRepo(id: "{repo}", remoteUrl: "/srv/old") {{ id }} }}"#
            ))
            .await;
        assert!(same.errors.is_empty(), "{:?}", same.errors);
        assert!(job_kinds(&engine).await.is_empty());

        let changed = schema
            .execute(format!(
                r#"mutation {{ updateRepo(id: "{repo}", remoteUrl: "https://github.com/acme/app") {{ remoteUrl baseRevision }} }}"#
            ))
            .await;
        assert!(changed.errors.is_empty(), "{:?}", changed.errors);
        let data = changed.data.into_json().unwrap();
        assert_eq!(
            data["updateRepo"]["remoteUrl"],
            "https://github.com/acme/app"
        );
        assert!(data["updateRepo"]["baseRevision"].is_null());
        let kinds: Vec<String> = job_kinds(&engine)
            .await
            .into_iter()
            .map(|(kind, _)| kind)
            .collect();
        assert_eq!(kinds, ["forget_mirror", "fetch"]);

        let _ = std::fs::remove_dir_all(directory);
    }

    /// A task that moves to another project leaves its checkout behind, and a
    /// runner job deletes it; moving back before that keeps it.
    #[tokio::test]
    async fn moving_a_task_cleans_up_its_old_checkout() {
        let (engine, workspace, project, _, directory) = workspace_with_repo("move-cleanup").await;
        let other = Uuid::now_v7();
        sqlx::query(
            "insert into projects (id, workspace_id, name, created_at) values (?,?, 'Other',?)",
        )
        .bind(other)
        .bind(workspace)
        .bind(Utc::now())
        .execute(&engine.db.pool)
        .await
        .unwrap();
        sqlx::query("insert into repos (id, project_id, name, remote_url, contract_version, created_at) values (?,?, 'other','/srv/other','1',?)")
            .bind(Uuid::now_v7()).bind(other).bind(Utc::now()).execute(&engine.db.pool).await.unwrap();
        let schema = build_schema(engine.clone());
        let filed = schema
            .execute(format!(
                r#"mutation {{ createIssue(projectId: "{project}", title: "Button", body: "") {{ id }} }}"#
            ))
            .await;
        let id = filed.data.into_json().unwrap()["createIssue"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let issue = Uuid::parse_str(&id).unwrap();

        let moved = schema
            .execute(format!(
                r#"mutation {{ updateIssue(id: "{id}", projectId: "{other}") {{ id }} }}"#
            ))
            .await;
        assert!(moved.errors.is_empty(), "{:?}", moved.errors);
        let jobs = job_kinds(&engine).await;
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].0, "cleanup");
        let payload: serde_json::Value = serde_json::from_str(&jobs[0].1).unwrap();
        let old = hulahoop_core::contract::checkout_dir(
            &engine.db.data_dir,
            &project.to_string(),
            &issue,
        );
        assert_eq!(payload["checkout"].as_str(), old.to_str());
        assert_eq!(payload["reason"], crate::engine::CLEANUP_MOVED);

        let back = schema
            .execute(format!(
                r#"mutation {{ updateIssue(id: "{id}", projectId: "{project}") {{ id }} }}"#
            ))
            .await;
        assert!(back.errors.is_empty(), "{:?}", back.errors);
        let jobs = job_kinds(&engine).await;
        assert_eq!(jobs.len(), 1, "{jobs:?}");
        let payload: serde_json::Value = serde_json::from_str(&jobs[0].1).unwrap();
        assert_ne!(payload["checkout"].as_str(), old.to_str());

        let _ = std::fs::remove_dir_all(directory);
    }
}
