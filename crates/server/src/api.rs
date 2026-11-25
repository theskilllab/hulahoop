//! Internal HTTP API. This is the only door between the runner and the
//! database, which is what keeps the server the single SQLite writer.

use crate::engine::Engine;
use crate::model::*;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use hulahoop_core::runspec::{
    CheckoutInput, CommentInput, FileInput, IssueInput, Limits, ParentInput, RunSpec,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

pub struct ApiState {
    pub engine: Arc<Engine>,
    pub token: String,
    pub runner_id_default: String,
}

/// The url a repository is cloned from. Every repository works from a clone
/// Hulahoop owns, so one without a url cannot run; the runner says so when it
/// is handed `null` here.
pub(crate) fn clone_url(repo: &Repo) -> Option<String> {
    repo.remote_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_string)
}

/// Where a run finds the repository it works from: the mirror under the data
/// directory, cloned from the repository's url and fetched by the runner. A
/// checkout the user works in is never touched. The mirror also keeps a private
/// clone possible: the server mints a token scoped to that one repository and
/// hands it to the runner.
fn host_path(repo: &Repo, data_dir: &std::path::Path) -> std::path::PathBuf {
    hulahoop_core::contract::mirror_dir(data_dir, &repo.id)
}

/// A task title as a commit subject: its first line, at most 72 characters.
fn commit_title(title: &str) -> String {
    title
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .chars()
        .take(72)
        .collect()
}

async fn clone_token(engine: &Engine, repo: &Repo) -> Option<String> {
    let (owner, name) = crate::github::split_remote(repo.remote_url.as_deref()?)?;
    let app = engine
        .stored_app(crate::github::PROVIDER, crate::connectors::KIND_GITHUB_APP)
        .await
        .ok()
        .flatten()?;
    let jwt = crate::github::mint_token(&app.app_id, &app.private_key).ok()?;
    crate::github::installation_token(&crate::github::http_client(), &jwt, &owner, &name)
        .await
        .ok()
}

pub fn router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/internal/runs/claim", post(claim_run))
        .route("/internal/runner/capacity", post(report_capacity))
        .route("/internal/runs/{id}/spec", axum::routing::get(get_spec))
        .route("/internal/runs/{id}/heartbeat", post(heartbeat))
        .route("/internal/runs/{id}/events", post(append_event))
        // A live diff is capped by the runner at a byte limit; the body limit is
        // said here so the two cannot drift apart into a silent 413.
        .route(
            "/internal/runs/{id}/diff",
            post(live_diff).layer(axum::extract::DefaultBodyLimit::max(
                hulahoop_core::LIVE_DIFF_BODY_LIMIT,
            )),
        )
        .route("/internal/runs/{id}/complete", post(complete_run))
        .route(
            "/internal/runs/{id}/credential",
            post(refreshed_run_credential),
        )
        .route(
            "/internal/jobs/{id}/credential",
            post(refreshed_job_credential),
        )
        .route("/internal/jobs/claim", post(claim_job))
        .route("/internal/jobs/{id}/complete", post(complete_job))
        .route("/internal/repos/{id}/lock", post(lock_repo))
        .route("/internal/repos/{id}/unlock", post(unlock_repo))
        // The desktop fetches artifact bytes with its bearer token, so a download is
        // an authenticated read rather than a public URL.
        .route("/artifacts/{id}", axum::routing::get(artifact_bytes))
        .with_state(state)
}

fn authorize(state: &ApiState, headers: &HeaderMap) -> Result<(), StatusCode> {
    let provided = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    // Constant-time compare: the token is the only thing between a run and the
    // ability to claim arbitrary work.
    if same_token(&state.token, provided) {
        Ok(())
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

/// Compares a presented token with the server's in constant time, so how long
/// a wrong guess takes says nothing about how close it was. Shared by the
/// runner API and the GraphQL WebSocket's `connection_init`.
pub(crate) fn same_token(expected: &str, provided: &str) -> bool {
    let expected = expected.as_bytes();
    let provided = provided.as_bytes();
    let mut diff = u8::from(expected.len() != provided.len());
    for index in 0..expected.len().max(provided.len()) {
        let a = *expected.get(index).unwrap_or(&0);
        let b = *provided.get(index).unwrap_or(&0);
        diff |= a ^ b;
    }
    diff == 0
}

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<Value>)>;

fn fail(status: StatusCode, message: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": message.to_string() })))
}

#[derive(Deserialize)]
struct ClaimRequest {
    #[serde(default)]
    runner_id: Option<String>,
    /// The room the runner has for another run. A runner that sends none
    /// takes the oldest queued run, as before runs shared the host.
    #[serde(default)]
    capacity: Option<crate::capacity::Capacity>,
}

#[derive(Serialize)]
struct RunEnvelope {
    run: Run,
    spec: Value,
    /// What the run is expected to take, so the runner keeps that much room
    /// for it while it runs.
    need: crate::capacity::Need,
}

#[derive(Deserialize)]
struct CapacityReport {
    capacity: crate::capacity::Capacity,
}

/// The room a runner has while it takes nothing: every slot is taken, so it
/// does not claim, but the Tasks page and the waiting runs still hear from it.
async fn report_capacity(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(body): Json<CapacityReport>,
) -> ApiResult<Value> {
    authorize(&state, &headers).map_err(|s| fail(s, "bad token"))?;
    state
        .engine
        .report_capacity(&body.capacity)
        .await
        .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(json!({ "ok": true })))
}
#[derive(Serialize)]
struct JobEnvelope {
    job: RunnerJob,
    spec: Value,
}

async fn claim_run(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(body): Json<ClaimRequest>,
) -> ApiResult<Option<RunEnvelope>> {
    authorize(&state, &headers).map_err(|s| fail(s, "bad token"))?;
    let runner_id = body
        .runner_id
        .unwrap_or_else(|| state.runner_id_default.clone());
    let run = state
        .engine
        .claim_run(&runner_id, body.capacity)
        .await
        .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let Some((run, need)) = run else {
        return Ok(Json(None));
    };
    let spec = build_run_spec(&state.engine, &run)
        .await
        .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(Some(RunEnvelope { run, spec, need })))
}

async fn get_spec(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<Value> {
    authorize(&state, &headers).map_err(|s| fail(s, "bad token"))?;
    let run = state
        .engine
        .run(id)
        .await
        .map_err(|e| fail(StatusCode::NOT_FOUND, e))?;
    if !run.status_enum().is_live() {
        return Err(fail(StatusCode::CONFLICT, "run has already finished"));
    }
    let spec = build_run_spec(&state.engine, &run)
        .await
        .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(spec))
}

#[derive(Deserialize)]
struct HeartbeatRequest {
    pid: Option<i64>,
    pgid: Option<i64>,
}

async fn heartbeat(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<HeartbeatRequest>,
) -> ApiResult<Value> {
    authorize(&state, &headers).map_err(|s| fail(s, "bad token"))?;
    // The run's status goes back, so a runner stops a run the server has
    // cancelled or declared dead instead of working on in its checkout.
    let status = state
        .engine
        .heartbeat(id, body.pid, body.pgid)
        .await
        .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(json!({ "ok": true, "status": status })))
}

#[derive(Deserialize)]
struct LiveDiffRequest {
    patch: String,
}

/// The working diff of a run in progress, reported by the runner whenever it
/// changed. Kept beside the run's artifacts and passed on to watchers.
async fn live_diff(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<LiveDiffRequest>,
) -> ApiResult<Value> {
    authorize(&state, &headers).map_err(|s| fail(s, "bad token"))?;
    state
        .engine
        .record_live_diff(id, &body.patch)
        .await
        .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct EventRequest {
    kind: String,
    payload: Value,
}

async fn append_event(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<EventRequest>,
) -> ApiResult<Value> {
    authorize(&state, &headers).map_err(|s| fail(s, "bad token"))?;
    state
        .engine
        .append_event(id, &body.kind, body.payload)
        .await
        .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(json!({ "ok": true })))
}

async fn complete_run(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<crate::engine::RunCompletion>,
) -> ApiResult<Value> {
    authorize(&state, &headers).map_err(|s| fail(s, "bad token"))?;
    let run = match state.engine.complete_run(id, body).await {
        Ok(run) => run,
        Err(error) => {
            // The agent did its part; the server did not. Record what happened,
            // give the turn back so the next attempt is not charged for it, and
            // answer 200 so the runner stops retrying a request that cannot work.
            if let Ok(failed) = state.engine.run(id).await {
                let _ = state
                    .engine
                    .finish_server_error(failed.issue_id, failed.agent_id, &error.to_string(), id)
                    .await;
            }
            return Ok(Json(serde_json::json!({ "run": serde_json::Value::Null })));
        }
    };
    Ok(Json(
        json!({ "run": { "id": run.id.to_string(), "status": run.status } }),
    ))
}

/// An OAuth login the harness refreshed during a run, handed back by the
/// runner. The entry is a secret: it is sealed and never logged.
#[derive(Deserialize)]
struct RefreshedCredential {
    connector: String,
    row: Uuid,
    entry: Value,
}

/// A run hands back only while it runs, and only the login it was issued at
/// claim (`runs.credential_refresh`): the connector and the row both match.
async fn refreshed_run_credential(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<RefreshedCredential>,
) -> ApiResult<Value> {
    authorize(&state, &headers).map_err(|s| fail(s, "bad token"))?;
    let issued: Option<(String, Option<String>)> =
        sqlx::query_as("select status, credential_refresh from runs where id = ?")
            .bind(id)
            .fetch_optional(&state.engine.db.pool)
            .await
            .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let Some((status, issued)) = issued else {
        return Err(fail(StatusCode::NOT_FOUND, "no run with that id"));
    };
    store_refreshed(&state.engine, &body, status == "running", issued).await
}

/// A Lumi job hands back on the same terms as a run.
async fn refreshed_job_credential(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<RefreshedCredential>,
) -> ApiResult<Value> {
    authorize(&state, &headers).map_err(|s| fail(s, "bad token"))?;
    let issued: Option<(String, String, Option<String>)> =
        sqlx::query_as("select kind, status, credential_refresh from runner_jobs where id = ?")
            .bind(id)
            .fetch_optional(&state.engine.db.pool)
            .await
            .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let Some((_, status, issued)) = issued.filter(|(kind, _, _)| kind == "lumi") else {
        return Err(fail(StatusCode::NOT_FOUND, "no Lumi job with that id"));
    };
    store_refreshed(&state.engine, &body, status == "running", issued).await
}

/// Store a handed-back login when its owner is still running and it is the
/// login the owner was issued; otherwise answer `{stored: false}`.
async fn store_refreshed(
    engine: &Engine,
    body: &RefreshedCredential,
    running: bool,
    issued: Option<String>,
) -> ApiResult<Value> {
    let issued = issued.and_then(|issued| serde_json::from_str::<Value>(&issued).ok());
    let matches = issued.is_some_and(|issued| {
        issued["connector"].as_str() == Some(body.connector.as_str())
            && issued["row"].as_str() == Some(body.row.to_string().as_str())
    });
    if !running || !matches {
        return Ok(Json(json!({ "stored": false })));
    }
    let stored = engine
        .store_refreshed_oauth(&body.connector, body.row, &body.entry)
        .await
        .map_err(|e| fail(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!({ "stored": stored })))
}

/// What a hand-back must match: the connector and row of the login a run or a
/// Lumi job was given, or null when it was given nothing to refresh.
fn issued_refresh(credential: &crate::connectors::Credential) -> Option<String> {
    credential
        .refresh
        .as_ref()
        .map(|refresh| json!({ "connector": refresh.connector, "row": refresh.row }).to_string())
}

#[derive(Deserialize)]
struct ClaimJobRequest {
    #[serde(default)]
    lumi: bool,
}

async fn claim_job(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(request): Json<ClaimJobRequest>,
) -> ApiResult<Option<JobEnvelope>> {
    authorize(&state, &headers).map_err(|s| fail(s, "bad token"))?;
    let job = state
        .engine
        .claim_job(request.lumi)
        .await
        .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let Some(job) = job else {
        return Ok(Json(None));
    };
    let spec = match build_job_spec(&state.engine, &job).await {
        Ok(spec) => spec,
        Err(_) => {
            // Claiming already moved the job to running. Complete it so Lumi's
            // question gets an answer even when credential resolution fails.
            state
                .engine
                .complete_job(
                    job.id,
                    Value::Null,
                    Some(
                        "Runner job setup failed; check the repository and model connector".into(),
                    ),
                )
                .await
                .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
            return Ok(Json(None));
        }
    };
    Ok(Json(Some(JobEnvelope { job, spec })))
}

#[derive(Deserialize)]
struct JobCompleteRequest {
    #[serde(default)]
    result: Value,
    #[serde(default)]
    error: Option<String>,
}

async fn complete_job(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<JobCompleteRequest>,
) -> ApiResult<Value> {
    authorize(&state, &headers).map_err(|s| fail(s, "bad token"))?;
    state
        .engine
        .complete_job(id, body.result, body.error)
        .await
        .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct LockRequest {
    holder: String,
    #[serde(default)]
    ttl_seconds: Option<i64>,
}

async fn lock_repo(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<LockRequest>,
) -> ApiResult<Value> {
    authorize(&state, &headers).map_err(|s| fail(s, "bad token"))?;
    let ttl = body.ttl_seconds.unwrap_or(600);
    let now = chrono::Utc::now();
    let expires = now + chrono::Duration::seconds(ttl);
    let existing: Option<(String, String)> =
        sqlx::query_as("select holder, expires_at from repo_locks where repo_id = ?")
            .bind(id)
            .fetch_optional(&state.engine.db.pool)
            .await
            .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    if let Some((holder, expires_at)) = existing {
        let expired = chrono::DateTime::parse_from_rfc3339(&expires_at)
            .map(|t| t < now)
            .unwrap_or(true);
        if !expired && holder != body.holder {
            return Err(fail(
                StatusCode::CONFLICT,
                format!("repo is locked by {holder} until {expires_at}"),
            ));
        }
    }
    sqlx::query(
        "insert into repo_locks (repo_id, holder, acquired_at, expires_at) values (?, ?, ?, ?)
         on conflict(repo_id) do update set holder = excluded.holder, acquired_at = excluded.acquired_at, expires_at = excluded.expires_at",
    )
    .bind(id)
    .bind(&body.holder)
    .bind(now.to_rfc3339())
    .bind(expires.to_rfc3339())
    .execute(&state.engine.db.pool)
    .await
    .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(
        json!({ "ok": true, "expires_at": expires.to_rfc3339() }),
    ))
}

async fn unlock_repo(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<Value> {
    authorize(&state, &headers).map_err(|s| fail(s, "bad token"))?;
    sqlx::query("delete from repo_locks where repo_id = ?")
        .bind(id)
        .execute(&state.engine.db.pool)
        .await
        .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(json!({ "ok": true })))
}

/// Everything the runner needs to execute one run, plus the `input.json` the
/// server writes for the agent. Building it here means the runner never reads
/// the database.
async fn build_run_spec(engine: &Engine, run: &Run) -> anyhow::Result<Value> {
    let issue = engine.issue(run.issue_id).await?;
    // A run on a task with no repository has no checkout: the runner gives it an
    // empty scratch directory instead.
    let repo: Option<Repo> = match run.repo_id {
        Some(repo_id) => Some(
            sqlx::query_as("select * from repos where id = ?")
                .bind(repo_id)
                .fetch_one(&engine.db.pool)
                .await?,
        ),
        None => None,
    };
    let mut agent: Agent = sqlx::query_as("select * from agents where id = ?")
        .bind(run.agent_id)
        .fetch_one(&engine.db.pool)
        .await?;
    // The queued run owns the execution identity and its price snapshot.
    // Agent edits after dispatch affect only future runs.
    agent.adapter = run.adapter_snapshot.clone().unwrap_or(agent.adapter);
    agent.model = run.model.clone();
    // The choice made when the run was queued; `connector_id` becomes the
    // connector actually used once the run is claimed. Runs queued before the
    // choice had its own column still read it from `connector_id`.
    agent.connector = run.chosen_connector.clone().or(run.connector_id.clone());
    let (checkout_project, checkout_path) = match &repo {
        Some(repo) => {
            // A task attached to a repository before projects were required may
            // still have none; the repository's own project names the checkout.
            let project: Project = sqlx::query_as("select * from projects where id = ?")
                .bind(issue.project_id.unwrap_or(repo.project_id))
                .fetch_one(&engine.db.pool)
                .await?;
            // One checkout per task, shared by its turns. The runner derives the
            // same path from the project and the task itself; this one is for
            // the record.
            let path = hulahoop_core::contract::checkout_dir(
                &engine.db.data_dir,
                &project.id.to_string(),
                &issue.id,
            );
            (Some(project.id), path)
        }
        None => (
            None,
            hulahoop_core::contract::scratch_dir(&engine.db.data_dir, &run.id),
        ),
    };

    // The parent run carries the review body the next attempt has to answer.
    let parent = match run.parent_run_id {
        Some(parent_id) => {
            let parent_run: Option<Run> = sqlx::query_as("select * from runs where id = ?")
                .bind(parent_id)
                .fetch_optional(&engine.db.pool)
                .await?;
            match parent_run {
                Some(parent_run) => {
                    // The parent is usually the reviewer's own run (the turn
                    // before this one), so the review it wrote counts first; a
                    // review of the parent's work is the older way in.
                    let review: Option<Review> = sqlx::query_as(
                        "select * from reviews where reviewer_run_id = ? or reviewed_run_id = ?
                          order by reviewer_run_id = ? desc, created_at desc limit 1",
                    )
                    .bind(parent_run.id)
                    .bind(parent_run.id)
                    .bind(parent_run.id)
                    .fetch_optional(&engine.db.pool)
                    .await?;
                    // A reviewer's verdict is the handover when there is one. Any
                    // other turn hands over through the message it left behind, so
                    // the next agent reads what was decided instead of guessing.
                    let (verdict, body) = match review {
                        Some(review) => (review.verdict, review.body),
                        None => (
                            "handover".to_string(),
                            parent_run.summary.clone().unwrap_or_default(),
                        ),
                    };
                    // A review publishes no head; the work it read is the
                    // newest head the task has.
                    let head_bookmark = match parent_run.head_bookmark {
                        Some(head) => head,
                        None => engine.newest_head(issue.id).await?.unwrap_or_default(),
                    };
                    Some(ParentInput {
                        head_bookmark,
                        verdict,
                        body,
                    })
                }
                None => None,
            }
        }
        None => None,
    };

    // A question stays pinned until a run carries it. The status a task returns to
    // after an answer is the one it left, so the pin — not the status — is what
    // says this turn has a question and an answer to read.
    let blocked_answer = if let Some(pinned) = issue.pinned_comment_id {
        // The id is bound as a uuid, not as its text: sqlite stores these as blobs,
        // so a string never matches and the question reads as missing.
        let question: Option<(String,)> = sqlx::query_as("select body from comments where id = ?")
            .bind(pinned)
            .fetch_optional(&engine.db.pool)
            .await?;
        let answer: Option<(String,)> = sqlx::query_as(
            "select body from comments where issue_id = ? and author_type = 'human' order by created_at desc limit 1",
        )
        .bind(issue.id)
        .fetch_optional(&engine.db.pool)
        .await?;
        match (question, answer) {
            (Some((question,)), Some((answer,))) => {
                Some(hulahoop_core::runspec::BlockedAnswer { question, answer })
            }
            _ => None,
        }
    } else {
        None
    };

    // The pair is handed over once. A run that re-reads its spec after a runner
    // restart is rebuilt without it, which is the price of keeping the question in
    // the comments a person reads rather than on the run row.
    if blocked_answer.is_some() {
        sqlx::query("update issues set pinned_comment_id = null where id = ?")
            .bind(issue.id)
            .execute(&engine.db.pool)
            .await?;
    }

    // Committing is the app's act, so the run gets the app's token. An agent that
    // cannot log in produces nothing, so the adapter's own credential always
    // travels too: a token for the app, a login for the harness.
    let credential = {
        let adapter =
            hulahoop_core::Adapter::parse(&agent.adapter).unwrap_or(hulahoop_core::Adapter::Pi);
        let credential = engine
            .agent_credential(adapter, agent.connector.as_deref(), Some(&agent.model))
            .await?;
        record_credential(engine, run, &credential).await?;
        credential.to_spec()
    };
    // Committing is the app's act, so every turn gets the app's token: a turn that
    // changed nothing pushes nothing, and nothing else may decide that in advance.
    let github_token = engine.github_token().await?;

    let skills = engine.skill_inputs(agent.id).await?;
    // The runner writes these into the checkout before the agent starts; a skill
    // that only exists inside input.json is advertised in the prompt but absent
    // from the path the prompt names.
    let skill_files = serde_json::to_value(&skills)?;

    let spec = RunSpec {
        run_id: run.id,
        issue_id: issue.id,
        attempt: run.attempt,
        issue: IssueInput {
            title: issue.title.clone(),
            body: issue.body.clone(),
            priority: issue.priority.clone(),
        },
        repo: repo.as_ref().map(|repo| hulahoop_core::runspec::RepoInput {
            name: repo.name.clone(),
            base_bookmark: repo.base_bookmark.clone(),
        }),
        checkout: CheckoutInput {
            path: checkout_path.clone(),
            base_bookmark: run.base_bookmark.clone(),
        },
        parent,
        blocked_answer,
        limits: Limits::default(),
        skills,
        files: run_files(engine, issue.id).await?,
        comments: run_comments(engine, issue.id, run.id).await?,
    }
    .truncate_text();

    let input_path = std::path::Path::new(&run.artifact_dir).join("input.json");
    spec.write(&input_path)?;
    let prompt_path = std::path::Path::new(&run.artifact_dir).join("prompt.md");
    let prompt = engine.compose_prompt(&agent, repo.is_some()).await?;
    std::fs::write(&prompt_path, &prompt)?;

    // What the runner reads to find the repository. A repo-less run sends nulls
    // and `checkout: "none"`, which tells the runner to skip the clone, the
    // workspace, the checks, the diff and the push.
    let (repo_path, repo_clone_token, vcs, repo_id, repo_clone_url) = match &repo {
        Some(repo) => (
            json!(host_path(repo, &engine.db.data_dir)),
            json!(clone_token(engine, repo).await),
            json!(repo.vcs),
            json!(repo.id),
            json!(clone_url(repo)),
        ),
        None => (
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
        ),
    };
    let checkout_mode = if repo.is_some() { "repo" } else { "none" };

    Ok(json!({
        "run_id": run.id,
        "issue_id": issue.id,
        "attempt": run.attempt,
        "adapter": agent.adapter,
        "model": agent.model,
        "agent_name": agent.name,
        "prompt": prompt,
        "prompt_path": prompt_path,
        "input_path": input_path,
        "repo_path": repo_path,
        "clone_token": repo_clone_token,
        "vcs": vcs,
        "repo_id": repo_id,
        "checkout": checkout_mode,
        "checkout_path": checkout_path,
        "checkout_project_id": checkout_project,
        "clone_url": repo_clone_url,
        "base_bookmark": run.base_bookmark,
        "base_revision": run.base_revision,
        // The bookmark the task lands on. A rework starts from the previous head,
        // and the runner moves that work onto the newest base before the turn.
        "repo_base_bookmark": repo.as_ref().map(|repo| repo.base_bookmark.clone()),
        "github_token": github_token,
        // What this adapter authenticates with: a key in the environment or the
        // credentials file the CLI wrote. Empty when nothing is stored, so the run
        // falls back to whatever the machine's own login gives it.
        "credential": credential,
        "artifact_dir": run.artifact_dir,
        "data_dir": engine.db.data_dir,
        "skills": skill_files,
        // A commit is named by the subject its turn gave (`HULAHOOP_COMMIT:`);
        // the task's title is the fallback for a turn that gave none.
        "issue": { "key": issue.issue_key, "title": commit_title(&issue.title) },
        // The connector, opened server-side, travels only on this authenticated
        // channel: the runner needs it to act, the desktop never sees it.
        "caller_role": "run",
        // The address GitHub rewrites into a link to the bot user needs both
        // halves; either missing means the commit keeps the plain app address.
        "commit_app_id": engine.commit_identity().await?.0,
        "commit_app_slug": engine.commit_identity().await?.1,
        "commit_agent_name": agent.name,
        "timeout_minutes": hulahoop_core::AGENT_WALL_CLOCK_MINUTES,
        "diff_line_limit": hulahoop_core::DIFF_LINE_LIMIT,
        "prompt_revision": engine.prompt_revision(&prompt),
    }))
}

/// Write down what a run was given, once, at its first claim: the billing a
/// cost report reads, the connector its plan usage counts against (none when
/// the run fell back to the host login), and the credential the run page
/// shows. A re-read spec must resolve to the same
/// billing, or the run would be billed as one thing and paid as another.
async fn record_credential(
    engine: &Engine,
    run: &Run,
    credential: &crate::connectors::Credential,
) -> anyhow::Result<()> {
    let billing = credential.billing();
    if run
        .billing_source
        .as_deref()
        .is_some_and(|previous| previous != billing)
    {
        anyhow::bail!("run credential billing source changed after claim");
    }
    // Every claim issues the login it builds, so only that one can be handed
    // back: a re-claim after the row was replaced issues the new row.
    sqlx::query("update runs set credential_refresh = ? where id = ?")
        .bind(issued_refresh(credential))
        .bind(run.id)
        .execute(&engine.db.pool)
        .await?;
    if run.billing_source.is_some() {
        return Ok(());
    }
    let used = credential
        .entry
        .filter(|_| !credential.is_empty())
        .map(|entry| entry.id);
    sqlx::query(
        "update runs set billing_source = ?, connector_id = ?, credential_source = ?
          where id = ? and status = 'running' and billing_source is null",
    )
    .bind(billing)
    .bind(used)
    .bind(credential.source())
    .bind(run.id)
    .execute(&engine.db.pool)
    .await?;
    Ok(())
}

/// The task's files, as paths into the artifact store the runner can read.
async fn run_files(engine: &Engine, issue_id: Uuid) -> anyhow::Result<Vec<FileInput>> {
    Ok(engine
        .issue_files(issue_id)
        .await?
        .iter()
        .map(|artifact| FileInput {
            id: artifact.id,
            title: artifact.title.clone(),
            filename: artifact.filename.clone(),
            path: engine.artifact_path(artifact),
        })
        .collect())
}

/// What people and Lumi said since the previous turn on this task started.
/// Agent comments are left out: the parent run already hands its message
/// over, and server notes describe the app, not the work. The most recent
/// comments win when there are many, and they read oldest first.
pub(crate) async fn run_comments(
    engine: &Engine,
    issue_id: Uuid,
    run_id: Uuid,
) -> anyhow::Result<Vec<CommentInput>> {
    let since: Option<String> = sqlx::query_scalar(
        "select started_at from runs
          where issue_id = ? and id <> ? and started_at is not null
          order by julianday(started_at) desc limit 1",
    )
    .bind(issue_id)
    .bind(run_id)
    .fetch_optional(&engine.db.pool)
    .await?
    .flatten();
    let rows: Vec<(String, Option<String>, String, String)> = sqlx::query_as(
        "select author_type, author_id, body, created_at from comments
          where issue_id = ?
            and (author_type = 'human' or author_id = 'lumi')
            and julianday(created_at) > coalesce(julianday(?), 0)
          order by julianday(created_at) desc, rowid desc limit ?",
    )
    .bind(issue_id)
    .bind(since)
    .bind(hulahoop_core::runspec::MAX_RUN_COMMENTS as i64)
    .fetch_all(&engine.db.pool)
    .await?;
    let mut comments = Vec::with_capacity(rows.len());
    for (author_type, author_id, body, created_at) in rows.into_iter().rev() {
        comments.push(CommentInput {
            author: engine
                .author_name(&author_type, author_id.as_deref())
                .await?,
            body,
            created_at,
        });
    }
    Ok(comments)
}

async fn build_job_spec(engine: &Engine, job: &RunnerJob) -> anyhow::Result<Value> {
    let repo: Repo = sqlx::query_as("select * from repos where id = ?")
        .bind(job.repo_id)
        .fetch_one(&engine.db.pool)
        .await?;
    let checkout_path = engine
        .db
        .data_dir
        .join("checkouts")
        .join("jobs")
        .join(job.id.to_string());
    let payload: Value = serde_json::from_str(&job.payload).unwrap_or(Value::Null);

    // A pull request needs the story a reviewer reads: what the issue asked for,
    // what the suite proves, and what the reviewer said, plus the app token that
    // lets the push and the API call be made as the app.
    let pull_request = if job.kind == "watch_checks" {
        Some(json!({
            "number": payload.get("number"),
            "sha": payload.get("sha"),
            "app_token": engine.github_token().await?,
        }))
    } else if job.kind == "review_pr" {
        Some(json!({
            "number": payload.get("number"),
            "verdict": payload.get("verdict"),
            "body": payload.get("body"),
            "app_token": engine.github_token().await?,
        }))
    } else if job.kind == "open_pr" {
        let run_id = payload
            .get("run_id")
            .and_then(|v| v.as_str())
            .and_then(|id| Uuid::parse_str(id).ok());
        match run_id {
            Some(run_id) => {
                let run: Option<Run> = sqlx::query_as("select * from runs where id = ?")
                    .bind(run_id)
                    .fetch_optional(&engine.db.pool)
                    .await?;
                let issue: Option<crate::model::Issue> = match run.as_ref() {
                    Some(run) => {
                        sqlx::query_as("select * from issues where id = ?")
                            .bind(run.issue_id)
                            .fetch_optional(&engine.db.pool)
                            .await?
                    }
                    None => None,
                };
                let review: Option<String> = match run.as_ref() {
                    Some(run) => {
                        sqlx::query_scalar::<_, String>(
                            "select body from reviews where reviewed_run_id = ? order by created_at desc limit 1",
                        )
                        .bind(run.id)
                        .fetch_optional(&engine.db.pool)
                        .await?
                    }
                    None => None,
                };
                Some(json!({
                    "run_id": run_id,
                    "number": issue.as_ref().and_then(|i| i.pr_number),
                    "branch": issue
                        .as_ref()
                        .and_then(|i| i.pr_branch.clone())
                        .or_else(|| {
                            run.as_ref().map(|r| {
                                format!("hulahoop/issue-{}-attempt-{}", r.issue_id, r.attempt)
                            })
                        }),
                    "head_revision": run.as_ref().and_then(|r| r.head_revision.clone()),
                    "attempt": run.as_ref().map(|r| r.attempt),

                    "title": issue.as_ref().map(|i| i.title.clone()),
                    "body": issue.as_ref().map(|i| i.body.clone()),
                    "summary": run.as_ref().and_then(|r| r.summary.clone()),
                    "review": review,
                    "app_token": engine.github_token().await?,
                }))
            }
            None => None,
        }
    } else {
        None
    };

    // Lumi uses the same stored connector resolution as a normal agent run.
    // Credentials are added to the runner spec at claim time, never persisted in
    // the job payload (which also contains the person's question).
    let credential = if job.kind == "lumi" {
        let adapter =
            hulahoop_core::Adapter::parse(payload["adapter"].as_str().unwrap_or_default())
                .ok_or_else(|| anyhow::anyhow!("unknown Lumi adapter"))?;
        let credential = engine
            .agent_credential(adapter, None, payload["model"].as_str())
            .await?;
        sqlx::query("update runner_jobs set credential_refresh = ? where id = ?")
            .bind(issued_refresh(&credential))
            .bind(job.id)
            .execute(&engine.db.pool)
            .await?;
        Some(credential.to_spec())
    } else {
        None
    };

    Ok(json!({
        "job_id": job.id,
        "credential": credential,
        "kind": job.kind,
        "payload": payload,
        "repo_path": host_path(&repo, &engine.db.data_dir),
        "clone_url": clone_url(&repo),
        "clone_token": clone_token(engine, &repo).await,
        "repo_id": repo.id,
        "vcs": repo.vcs,
        "remote_url": repo.remote_url,
        "checkout_path": checkout_path,
        "artifact_dir": engine.db.data_dir.join("jobs").join(job.id.to_string()),
        "data_dir": engine.db.data_dir,
        "base_bookmark": repo.base_bookmark,
        // A fetch never moves the base back behind the revision recorded here: a
        // merge without a pull request records its head before the remote has it.
        "base_revision": repo.base_revision,
        "issue_id": job.issue_id,
        "caller_role": "job",
        "pull_request": pull_request,
    }))
}

/// The bytes behind an artifact row. The token travels in the query string because
/// a browser download cannot set a header, and the file is served as an attachment.
async fn artifact_bytes(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<Uuid>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::to_string)
        .or_else(|| params.get("token").cloned())
        .unwrap_or_default();
    if authorize(&state, &{
        let mut map = HeaderMap::new();
        if let Ok(value) = HeaderValue::from_str(&format!("Bearer {presented}")) {
            map.insert(header::AUTHORIZATION, value);
        }
        map
    })
    .is_err()
    {
        return (StatusCode::UNAUTHORIZED, "the token is missing or wrong").into_response();
    }
    let Ok(artifact) = state.engine.artifact(id).await else {
        return (StatusCode::NOT_FOUND, "no such artifact").into_response();
    };
    let path = state.engine.artifact_path(&artifact);
    let Ok(bytes) = std::fs::read(&path) else {
        return (
            StatusCode::GONE,
            format!("{} is registered but its file is gone", artifact.filename),
        )
            .into_response();
    };
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        artifact
            .mime
            .parse()
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    if let Ok(value) = HeaderValue::from_str(&format!(
        "attachment; filename=\"{}\"",
        artifact.filename.replace('"', "")
    )) {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    response
}

#[cfg(test)]
mod claim_spec_tests {
    use super::build_run_spec;
    use crate::db::Db;
    use crate::engine::Engine;
    use crate::model::Run;
    use chrono::{Duration, Utc};
    use uuid::Uuid;

    /// The runner writes skill files from the claim spec, not from input.json:
    /// a skill missing there is advertised in the prompt but absent on disk.
    #[tokio::test]
    async fn claim_spec_carries_the_agent_skills() {
        let directory = std::env::temp_dir().join(format!("hulahoop-claim-{}", Uuid::now_v7()));
        let db = Db::open(&directory).await.unwrap();
        let workspace = db.seed_local().await.unwrap();
        let (project, repo, agent, issue, run_id) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        let now = Utc::now() - Duration::seconds(3);
        sqlx::query(
            "insert into projects (id, workspace_id, name, created_at) values (?,?, 'Claim',?)",
        )
        .bind(project)
        .bind(workspace)
        .bind(now)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query("insert into repos (id, project_id, name, remote_url, contract_version, created_at) values (?,?, 'repo','/tmp/repo','1',?)")
            .bind(repo).bind(project).bind(now).execute(&db.pool).await.unwrap();
        sqlx::query("insert into agents (id, workspace_id, name, adapter, model, created_at, updated_at) values (?,?, 'builder','pi','model-x',?,?)")
            .bind(agent).bind(workspace).bind(now).bind(now).execute(&db.pool).await.unwrap();
        sqlx::query("insert into issues (id, workspace_id, project_id, repo_id, issue_key, number, title, status, created_at, updated_at) values (?,?,?,?, 'CLM-1',1,'Task','in_progress',?,?)")
            .bind(issue).bind(workspace).bind(project).bind(repo)
            .bind(now).bind(now).execute(&db.pool).await.unwrap();
        let artifacts = directory.join("claim-artifacts");
        std::fs::create_dir_all(&artifacts).unwrap();
        sqlx::query("insert into runs (id, issue_id, repo_id, agent_id, attempt, base_bookmark, prompt_revision, artifact_dir, status, model, created_at) values (?,?,?,?,1,'main','prompt',?,'running','model-x',?)")
            .bind(run_id).bind(issue).bind(repo).bind(agent)
            .bind(artifacts.to_string_lossy().to_string()).bind(now)
            .execute(&db.pool).await.unwrap();

        let engine = Engine::new(db);
        let skill = engine
            .create_skill(
                workspace,
                "hulahoop-context",
                "Read at the start of every run.",
                "# Hulahoop context\nThe product's objects, explained.",
            )
            .await
            .unwrap();
        engine.set_agent_skills(agent, &[skill.id]).await.unwrap();

        let run: Run = sqlx::query_as("select * from runs where id = ?")
            .bind(run_id)
            .fetch_one(&engine.db.pool)
            .await
            .unwrap();
        let spec = build_run_spec(&engine, &run).await.unwrap();

        assert_eq!(spec["issue"]["title"], "Task");
        let skills = spec["skills"].as_array().expect("claim spec names skills");
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0]["name"], "hulahoop-context");
        assert!(skills[0]["body"]
            .as_str()
            .unwrap()
            .contains("Hulahoop context"));

        let _ = std::fs::remove_dir_all(directory);
    }

    /// A turn reads the task's files and what people said since the last turn
    /// started; older comments and agent comments stay out of its input.
    #[tokio::test]
    async fn run_input_carries_files_and_recent_comments() {
        let directory = std::env::temp_dir().join(format!("hulahoop-input-{}", Uuid::now_v7()));
        let db = Db::open(&directory).await.unwrap();
        let workspace = db.seed_local().await.unwrap();
        let (project, repo, agent, issue, previous, run_id) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        let now = Utc::now() - Duration::minutes(10);
        sqlx::query(
            "insert into projects (id, workspace_id, name, created_at) values (?,?, 'Input',?)",
        )
        .bind(project)
        .bind(workspace)
        .bind(now)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query("insert into repos (id, project_id, name, remote_url, contract_version, created_at) values (?,?, 'repo','/tmp/repo','1',?)")
            .bind(repo).bind(project).bind(now).execute(&db.pool).await.unwrap();
        sqlx::query("insert into agents (id, workspace_id, name, adapter, model, created_at, updated_at) values (?,?, 'builder','pi','model-x',?,?)")
            .bind(agent).bind(workspace).bind(now).bind(now).execute(&db.pool).await.unwrap();
        sqlx::query("insert into issues (id, workspace_id, project_id, repo_id, issue_key, number, title, status, created_at, updated_at) values (?,?,?,?, 'INP-1',1,'Task','in_progress',?,?)")
            .bind(issue).bind(workspace).bind(project).bind(repo)
            .bind(now).bind(now).execute(&db.pool).await.unwrap();
        let artifacts = directory.join("input-artifacts");
        std::fs::create_dir_all(&artifacts).unwrap();
        let started = now + Duration::minutes(2);
        for (id, status, started_at) in [
            (previous, "succeeded", Some(started)),
            (run_id, "running", None),
        ] {
            sqlx::query("insert into runs (id, issue_id, repo_id, agent_id, attempt, base_bookmark, prompt_revision, artifact_dir, status, model, created_at, started_at) values (?,?,?,?,1,'main','prompt',?,?,'model-x',?,?)")
                .bind(id).bind(issue).bind(repo).bind(agent)
                .bind(artifacts.to_string_lossy().to_string()).bind(status).bind(now).bind(started_at)
                .execute(&db.pool).await.unwrap();
        }
        for (minute, author_type, author_id, body) in [
            (1, "human", None, "before the last turn"),
            (3, "human", None, "use the staging config"),
            (4, "agent", Some(agent.to_string()), "I did the thing"),
            (
                5,
                "server",
                Some("lumi".to_string()),
                "Lumi says: check the logs",
            ),
        ] {
            sqlx::query("insert into comments (id, issue_id, author_type, author_id, body, pinned, created_at) values (?,?,?,?,?,0,?)")
                .bind(Uuid::now_v7()).bind(issue).bind(author_type).bind(author_id).bind(body)
                .bind((now + Duration::minutes(minute)).to_rfc3339())
                .execute(&db.pool).await.unwrap();
        }

        let engine = Engine::new(db);
        let file = engine
            .store_artifact(
                workspace,
                Some(project),
                None,
                Some(issue),
                "Spec".into(),
                None,
                "spec.md".into(),
                None,
                b"# Spec",
            )
            .await
            .unwrap();
        let run: Run = sqlx::query_as("select * from runs where id = ?")
            .bind(run_id)
            .fetch_one(&engine.db.pool)
            .await
            .unwrap();
        build_run_spec(&engine, &run).await.unwrap();
        let input: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(artifacts.join("input.json")).unwrap())
                .unwrap();

        let files = input["files"].as_array().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0]["filename"], "spec.md");
        let path = std::path::PathBuf::from(files[0]["path"].as_str().unwrap());
        assert_eq!(path, engine.artifact_path(&file));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "# Spec");

        let comments: Vec<(String, String)> = input["comments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                (
                    c["author"].as_str().unwrap().to_string(),
                    c["body"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert_eq!(
            comments,
            vec![
                ("you".to_string(), "use the staging config".to_string()),
                ("Lumi".to_string(), "Lumi says: check the logs".to_string()),
            ]
        );

        let _ = std::fs::remove_dir_all(directory);
    }

    /// A task with no project has no repository: its claim spec names no
    /// repository, no clone and no checkout to make, points the runner at a
    /// scratch directory, and still carries the agent's skills and a prompt
    /// that says where the work goes.
    #[tokio::test]
    async fn a_repo_less_claim_spec_names_no_repository() {
        let directory =
            std::env::temp_dir().join(format!("hulahoop-claim-scratch-{}", Uuid::now_v7()));
        let db = Db::open(&directory).await.unwrap();
        let workspace = db.seed_local().await.unwrap();
        let (agent, issue, run_id) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let now = Utc::now() - Duration::seconds(3);
        sqlx::query("insert into agents (id, workspace_id, name, adapter, model, created_at, updated_at) values (?,?, 'writer','pi','model-x',?,?)")
            .bind(agent).bind(workspace).bind(now).bind(now).execute(&db.pool).await.unwrap();
        sqlx::query("insert into issues (id, workspace_id, project_id, repo_id, issue_key, number, title, status, created_at, updated_at) values (?,?,null,null, 'TSK-1',1,'Task','in_progress',?,?)")
            .bind(issue).bind(workspace).bind(now).bind(now).execute(&db.pool).await.unwrap();
        let artifacts = directory.join("scratch-artifacts");
        std::fs::create_dir_all(&artifacts).unwrap();
        sqlx::query("insert into runs (id, issue_id, repo_id, agent_id, attempt, base_bookmark, prompt_revision, artifact_dir, status, model, created_at) values (?,?,null,?,1,'','prompt',?,'running','model-x',?)")
            .bind(run_id).bind(issue).bind(agent)
            .bind(artifacts.to_string_lossy().to_string()).bind(now)
            .execute(&db.pool).await.unwrap();

        let engine = Engine::new(db);
        let skill = engine
            .create_skill(
                workspace,
                "report-style",
                "How reports read.",
                "# Report style\nShort sentences.",
            )
            .await
            .unwrap();
        engine.set_agent_skills(agent, &[skill.id]).await.unwrap();

        let run: Run = sqlx::query_as("select * from runs where id = ?")
            .bind(run_id)
            .fetch_one(&engine.db.pool)
            .await
            .unwrap();
        let spec = build_run_spec(&engine, &run).await.unwrap();

        assert_eq!(spec["checkout"], "none");
        for key in ["repo_path", "clone_url", "clone_token", "repo_id", "vcs"] {
            assert!(spec[key].is_null(), "{key} should be null: {}", spec[key]);
        }
        assert_eq!(
            spec["checkout_path"].as_str().unwrap(),
            hulahoop_core::contract::scratch_dir(&engine.db.data_dir, &run_id).to_string_lossy()
        );
        let skills = spec["skills"].as_array().expect("claim spec names skills");
        assert!(skills.iter().any(|s| s["name"] == "report-style"));
        assert!(spec["prompt"]
            .as_str()
            .unwrap()
            .contains("This task has no repository"));

        let input: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(spec["input_path"].as_str().unwrap()).unwrap(),
        )
        .unwrap();
        assert!(input["repo"].is_null(), "{}", input["repo"]);

        let _ = std::fs::remove_dir_all(directory);
    }

    /// A claim records which credential the run used and how it bills, once:
    /// a later claim of the same run keeps what the first one wrote.
    #[tokio::test]
    async fn a_claim_records_the_credential_the_run_used() {
        let directory =
            std::env::temp_dir().join(format!("hulahoop-claim-credential-{}", Uuid::now_v7()));
        let db = Db::open(&directory).await.unwrap();
        let workspace = db.seed_local().await.unwrap();
        let now = Utc::now() - Duration::seconds(3);
        let mut runs = Vec::new();
        for (number, model) in [
            (1, "anthropic/claude-sonnet-4-5"),
            (2, "openai-codex/gpt-5.1"),
        ] {
            let (agent, issue, run) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
            sqlx::query("insert into issues (id, workspace_id, project_id, repo_id, issue_key, number, title, status, created_at, updated_at) values (?,?,null,null,?,?,'Task','in_progress',?,?)")
                .bind(issue).bind(workspace).bind(format!("TSK-{number}")).bind(number).bind(now).bind(now)
                .execute(&db.pool).await.unwrap();
            sqlx::query("insert into agents (id, workspace_id, name, adapter, model, created_at, updated_at) values (?,?,?,'pi',?,?,?)")
                .bind(agent).bind(workspace).bind(model).bind(model).bind(now).bind(now)
                .execute(&db.pool).await.unwrap();
            let artifacts = directory.join(run.to_string());
            std::fs::create_dir_all(&artifacts).unwrap();
            sqlx::query("insert into runs (id, issue_id, repo_id, agent_id, attempt, base_bookmark, prompt_revision, artifact_dir, status, model, created_at) values (?,?,null,?,1,'','prompt',?,'running',?,?)")
                .bind(run).bind(issue).bind(agent)
                .bind(artifacts.to_string_lossy().to_string()).bind(model).bind(now)
                .execute(&db.pool).await.unwrap();
            runs.push(run);
        }
        let engine = Engine::new(db);
        engine
            .set_agent_key("claude-api", "sk-ant-api03-a-test-key")
            .await
            .unwrap();

        let claim = |run_id: Uuid| {
            let engine = &engine;
            async move {
                let run: Run = sqlx::query_as("select * from runs where id = ?")
                    .bind(run_id)
                    .fetch_one(&engine.db.pool)
                    .await
                    .unwrap();
                let spec = build_run_spec(engine, &run).await.unwrap();
                let run: Run = sqlx::query_as("select * from runs where id = ?")
                    .bind(run_id)
                    .fetch_one(&engine.db.pool)
                    .await
                    .unwrap();
                (spec, run)
            }
        };
        let (spec, run) = claim(runs[0]).await;
        assert_eq!(run.credential_source.as_deref(), Some("Claude API key"));
        assert_eq!(run.connector_id.as_deref(), Some("claude-api"));
        assert_eq!(run.billing_source.as_deref(), Some("api"));
        assert!(spec["credential"]["files"]
            .as_array()
            .is_some_and(|files| !files.is_empty()));
        let (_, again) = claim(runs[0]).await;
        assert_eq!(again.credential_source, run.credential_source);

        let (_, run) = claim(runs[1]).await;
        assert_eq!(run.credential_source.as_deref(), Some("host login"));
        assert_eq!(run.billing_source.as_deref(), Some("unknown"));

        let _ = std::fs::remove_dir_all(directory);
    }

    /// A rework's parent is the reviewer's run, which wrote the review but
    /// published nothing: the spec carries that review as the handover.
    #[tokio::test]
    async fn a_rework_reads_the_review_its_parent_wrote() {
        let directory =
            std::env::temp_dir().join(format!("hulahoop-claim-rework-{}", Uuid::now_v7()));
        let db = Db::open(&directory).await.unwrap();
        let workspace = db.seed_local().await.unwrap();
        let (agent, issue, head, review, rework) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        let now = Utc::now() - Duration::seconds(3);
        sqlx::query("insert into agents (id, workspace_id, name, adapter, model, created_at, updated_at) values (?,?, 'Developer','pi','model-x',?,?)")
            .bind(agent).bind(workspace).bind(now).bind(now).execute(&db.pool).await.unwrap();
        sqlx::query("insert into issues (id, workspace_id, project_id, repo_id, issue_key, number, title, status, created_at, updated_at) values (?,?,null,null, 'TSK-1',1,'Task','in_progress',?,?)")
            .bind(issue).bind(workspace).bind(now).bind(now).execute(&db.pool).await.unwrap();
        let artifacts = directory.join("rework-artifacts");
        std::fs::create_dir_all(&artifacts).unwrap();
        for (id, parent, status) in [
            (head, None, "succeeded"),
            (review, None, "succeeded"),
            (rework, Some(review), "running"),
        ] {
            sqlx::query("insert into runs (id, issue_id, repo_id, agent_id, attempt, base_bookmark, prompt_revision, artifact_dir, status, model, parent_run_id, created_at) values (?,?,null,?,1,'','prompt',?,?,'model-x',?,?)")
                .bind(id).bind(issue).bind(agent)
                .bind(artifacts.to_string_lossy().to_string()).bind(status).bind(parent).bind(now)
                .execute(&db.pool).await.unwrap();
        }
        sqlx::query("insert into reviews (id, issue_id, reviewed_run_id, reviewer_run_id, verdict, body, created_at) values (?,?,?,?,'changes_requested','Fix the target date field.',?)")
            .bind(Uuid::now_v7()).bind(issue).bind(head).bind(review).bind(now)
            .execute(&db.pool).await.unwrap();

        let engine = Engine::new(db);
        let run: Run = sqlx::query_as("select * from runs where id = ?")
            .bind(rework)
            .fetch_one(&engine.db.pool)
            .await
            .unwrap();
        let spec = build_run_spec(&engine, &run).await.unwrap();
        let input: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(spec["input_path"].as_str().unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(input["parent"]["verdict"], "changes_requested");
        assert_eq!(input["parent"]["body"], "Fix the target date field.");

        let _ = std::fs::remove_dir_all(directory);
    }
}

#[cfg(test)]
mod hand_back_tests {
    //! The routes a runner hands a refreshed login back through, and what a
    //! claim records for them to check against.

    use super::{
        build_job_spec, build_run_spec, refreshed_job_credential, refreshed_run_credential,
        ApiState, RefreshedCredential,
    };
    use crate::db::Db;
    use crate::engine::Engine;
    use crate::model::{Run, RunnerJob};
    use axum::extract::{Path, State};
    use axum::http::HeaderMap;
    use axum::Json;
    use chrono::{Duration, Utc};
    use serde_json::{json, Value};
    use std::sync::Arc;
    use uuid::Uuid;

    const HOUR_MS: i64 = 60 * 60 * 1000;

    fn login(access: &str, hours: i64) -> Value {
        json!({"type": "oauth", "access": access, "refresh": format!("{access}-r"),
               "expires": Utc::now().timestamp_millis() + hours * HOUR_MS})
    }

    struct Fixture {
        state: Arc<ApiState>,
        directory: std::path::PathBuf,
        workspace: Uuid,
        repo: Uuid,
    }

    async fn fixture() -> Fixture {
        let directory = std::env::temp_dir().join(format!("hulahoop-handback-{}", Uuid::now_v7()));
        let db = Db::open(&directory).await.unwrap();
        let workspace = db.seed_local().await.unwrap();
        let (project, repo) = (Uuid::now_v7(), Uuid::now_v7());
        let now = Utc::now() - Duration::seconds(3);
        sqlx::query(
            "insert into projects (id, workspace_id, name, created_at) values (?,?, 'Back',?)",
        )
        .bind(project)
        .bind(workspace)
        .bind(now)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query("insert into repos (id, project_id, name, remote_url, contract_version, created_at) values (?,?, 'repo','/tmp/repo','1',?)")
            .bind(repo).bind(project).bind(now).execute(&db.pool).await.unwrap();
        let mut engine = Engine::new(db);
        engine.pi_bin = directory.join("no-pi");
        let state = Arc::new(ApiState {
            engine: Arc::new(engine),
            token: "runner-token".into(),
            runner_id_default: "runner".into(),
        });
        Fixture {
            state,
            directory,
            workspace,
            repo,
        }
    }

    fn headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer runner-token".parse().unwrap());
        headers
    }

    /// A running run of a Pi agent on `model`, queued with `chosen` as the
    /// agent's connector.
    async fn running_run(fixture: &Fixture, model: &str, chosen: Option<&str>) -> Uuid {
        let pool = &fixture.state.engine.db.pool;
        let (agent, issue, run) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let now = Utc::now() - Duration::seconds(3);
        let number: i64 = sqlx::query_scalar("select count(*) + 1 from issues")
            .fetch_one(pool)
            .await
            .unwrap();
        sqlx::query("insert into issues (id, workspace_id, project_id, repo_id, issue_key, number, title, status, created_at, updated_at) values (?,?,null,null,?,?,'Task','in_progress',?,?)")
            .bind(issue).bind(fixture.workspace).bind(format!("HB-{number}")).bind(number).bind(now).bind(now)
            .execute(pool).await.unwrap();
        sqlx::query("insert into agents (id, workspace_id, name, adapter, model, connector, created_at, updated_at) values (?,?,?,'pi',?,?,?,?)")
            .bind(agent).bind(fixture.workspace).bind(format!("agent-{number}")).bind(model).bind(chosen).bind(now).bind(now)
            .execute(pool).await.unwrap();
        let artifacts = fixture.directory.join(run.to_string());
        std::fs::create_dir_all(&artifacts).unwrap();
        sqlx::query("insert into runs (id, issue_id, repo_id, agent_id, attempt, base_bookmark, prompt_revision, artifact_dir, status, model, connector_id, chosen_connector, created_at) values (?,?,null,?,1,'','prompt',?,'running',?,?,?,?)")
            .bind(run).bind(issue).bind(agent)
            .bind(artifacts.to_string_lossy().to_string()).bind(model).bind(chosen).bind(chosen).bind(now)
            .execute(pool).await.unwrap();
        run
    }

    async fn claim(fixture: &Fixture, run_id: Uuid) -> (Value, Run) {
        let pool = &fixture.state.engine.db.pool;
        let run: Run = sqlx::query_as("select * from runs where id = ?")
            .bind(run_id)
            .fetch_one(pool)
            .await
            .unwrap();
        let spec = build_run_spec(&fixture.state.engine, &run).await.unwrap();
        let run: Run = sqlx::query_as("select * from runs where id = ?")
            .bind(run_id)
            .fetch_one(pool)
            .await
            .unwrap();
        (spec, run)
    }

    async fn hand_back_run(fixture: &Fixture, run: Uuid, body: Value) -> bool {
        let body: RefreshedCredential = serde_json::from_value(body).unwrap();
        let Json(answer) = refreshed_run_credential(
            State(fixture.state.clone()),
            headers(),
            Path(run),
            Json(body),
        )
        .await
        .unwrap_or_else(|_| panic!("the route refused the request"));
        answer["stored"].as_bool().unwrap()
    }

    async fn stored_access(fixture: &Fixture) -> Value {
        let entry = crate::connectors::entry("codex-sub").unwrap();
        let secrets = fixture.state.engine.secrets(entry).await.unwrap().unwrap();
        serde_json::from_str::<Value>(&secrets.oauth.unwrap()).unwrap()["access"].clone()
    }

    /// A running run hands back only the login it was issued: the same
    /// connector and the same row, and only while it runs.
    #[tokio::test]
    async fn a_run_hands_back_only_the_login_it_was_issued() {
        let fixture = fixture().await;
        let engine = &fixture.state.engine;
        engine
            .set_agent_oauth("codex-sub", &login("given", 5), "signed in")
            .await
            .unwrap();
        engine
            .set_agent_key("opencode-go", "ocg-key")
            .await
            .unwrap();
        let run = running_run(&fixture, "openai-codex/gpt-5.1-codex", None).await;
        let (spec, recorded) = claim(&fixture, run).await;
        let refresh = &spec["credential"]["refresh"];
        assert_eq!(refresh["connector"], "codex-sub");
        assert_eq!(recorded.connector_id.as_deref(), Some("codex-sub"));
        let row = refresh["row"].clone();

        // Another connector, or another row of this one, is not what it was given.
        let other_row = json!(Uuid::now_v7());
        assert!(
            !hand_back_run(
                &fixture,
                run,
                json!({"connector": "opencode-go", "row": row, "entry": login("x", 6)})
            )
            .await
        );
        assert!(
            !hand_back_run(
                &fixture,
                run,
                json!({"connector": "codex-sub", "row": other_row, "entry": login("x", 6)})
            )
            .await
        );
        assert_eq!(stored_access(&fixture).await, "given");

        assert!(
            hand_back_run(
                &fixture,
                run,
                json!({"connector": "codex-sub", "row": row, "entry": login("refreshed", 6)})
            )
            .await
        );
        assert_eq!(stored_access(&fixture).await, "refreshed");

        // Finished runs hand back nothing.
        sqlx::query("update runs set status = 'succeeded' where id = ?")
            .bind(run)
            .execute(&engine.db.pool)
            .await
            .unwrap();
        assert!(
            !hand_back_run(
                &fixture,
                run,
                json!({"connector": "codex-sub", "row": row, "entry": login("late", 7)})
            )
            .await
        );
        assert_eq!(stored_access(&fixture).await, "refreshed");

        // A wrong token is refused outright.
        let body: RefreshedCredential = serde_json::from_value(
            json!({"connector": "codex-sub", "row": row, "entry": login("x", 6)}),
        )
        .unwrap();
        assert!(refreshed_run_credential(
            State(fixture.state.clone()),
            HeaderMap::new(),
            Path(run),
            Json(body)
        )
        .await
        .is_err());
        let _ = std::fs::remove_dir_all(&fixture.directory);
    }

    /// A run that fell back to the host login counts against no connector, and
    /// a re-claim still resolves the agent's own choice.
    #[tokio::test]
    async fn a_host_login_run_records_no_connector_and_keeps_the_choice() {
        let fixture = fixture().await;
        let engine = &fixture.state.engine;
        engine
            .set_agent_key("opencode-go", "ocg-key")
            .await
            .unwrap();
        let run = running_run(&fixture, "anthropic/claude-sonnet-4-5", Some("opencode-go")).await;
        let (spec, recorded) = claim(&fixture, run).await;
        assert_eq!(recorded.credential_source.as_deref(), Some("host login"));
        assert_eq!(recorded.connector_id, None);
        assert_eq!(recorded.chosen_connector.as_deref(), Some("opencode-go"));
        assert!(spec["credential"]["refresh"].is_null());
        let (_, again) = claim(&fixture, run).await;
        assert_eq!(again.connector_id, None);
        assert_eq!(again.billing_source.as_deref(), Some("unknown"));

        // The agent's choice serves this one and is what it records.
        let run = running_run(&fixture, "opencode-go/glm-5", Some("opencode-go")).await;
        let (_, recorded) = claim(&fixture, run).await;
        assert_eq!(recorded.connector_id.as_deref(), Some("opencode-go"));
        let _ = std::fs::remove_dir_all(&fixture.directory);
    }

    async fn job(fixture: &Fixture, kind: &str, payload: Value) -> RunnerJob {
        let pool = &fixture.state.engine.db.pool;
        let id = Uuid::now_v7();
        sqlx::query("insert into runner_jobs (id, repo_id, kind, payload, status, created_at) values (?,?,?,?,'running',?)")
            .bind(id).bind(fixture.repo).bind(kind).bind(payload.to_string()).bind(Utc::now())
            .execute(pool).await.unwrap();
        sqlx::query_as("select * from runner_jobs where id = ?")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn hand_back_job(fixture: &Fixture, job: Uuid, body: Value) -> Result<bool, ()> {
        let body: RefreshedCredential = serde_json::from_value(body).unwrap();
        refreshed_job_credential(
            State(fixture.state.clone()),
            headers(),
            Path(job),
            Json(body),
        )
        .await
        .map(|Json(answer)| answer["stored"].as_bool().unwrap())
        .map_err(|_| ())
    }

    /// Lumi resolves its credential from the model it asks with, and hands
    /// back on a run's terms; no other kind of job hands back at all.
    #[tokio::test]
    async fn a_lumi_job_gets_its_models_login_and_hands_back_only_that() {
        let fixture = fixture().await;
        let engine = &fixture.state.engine;
        engine
            .set_agent_oauth("codex-sub", &login("given", 5), "signed in")
            .await
            .unwrap();
        engine
            .set_agent_key("opencode-go", "ocg-key")
            .await
            .unwrap();

        let codex = job(
            &fixture,
            "lumi",
            json!({"adapter": "pi", "model": "openai-codex/gpt-5.1-codex", "message": "hi"}),
        )
        .await;
        let spec = build_job_spec(engine, &codex).await.unwrap();
        let files = spec["credential"]["files"].as_array().unwrap();
        let auth: Value = serde_json::from_str(files[0][1].as_str().unwrap()).unwrap();
        assert_eq!(
            auth["openai-codex"]["access"], "given",
            "the model's provider decides"
        );
        let row = spec["credential"]["refresh"]["row"].clone();

        let opencode = job(
            &fixture,
            "lumi",
            json!({"adapter": "pi", "model": "opencode-go/glm-5", "message": "hi"}),
        )
        .await;
        let spec = build_job_spec(engine, &opencode).await.unwrap();
        let auth: Value =
            serde_json::from_str(spec["credential"]["files"][0][1].as_str().unwrap()).unwrap();
        assert_eq!(
            auth,
            json!({"opencode-go": {"type": "api_key", "key": "ocg-key"}})
        );
        // Given nothing to refresh, it hands back nothing.
        assert_eq!(
            hand_back_job(
                &fixture,
                opencode.id,
                json!({"connector": "codex-sub", "row": row, "entry": login("x", 6)})
            )
            .await,
            Ok(false)
        );

        assert_eq!(
            hand_back_job(
                &fixture,
                codex.id,
                json!({"connector": "codex-sub", "row": row, "entry": login("refreshed", 6)})
            )
            .await,
            Ok(true)
        );
        assert_eq!(stored_access(&fixture).await, "refreshed");
        sqlx::query("update runner_jobs set status = 'succeeded' where id = ?")
            .bind(codex.id)
            .execute(&engine.db.pool)
            .await
            .unwrap();
        assert_eq!(
            hand_back_job(
                &fixture,
                codex.id,
                json!({"connector": "codex-sub", "row": row, "entry": login("late", 7)})
            )
            .await,
            Ok(false)
        );

        let fetch = job(&fixture, "fetch", json!({})).await;
        assert!(hand_back_job(
            &fixture,
            fetch.id,
            json!({"connector": "codex-sub", "row": row, "entry": login("x", 6)})
        )
        .await
        .is_err());
        let _ = std::fs::remove_dir_all(&fixture.directory);
    }
}
