//! Internal HTTP API. This is the only door between the runner and the
//! database, which is what keeps the server the single SQLite writer.

use crate::engine::Engine;
use crate::model::*;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use hulahoop_core::junit;
use hulahoop_core::runspec::{
    CheckoutInput, CommentInput, FileInput, IssueInput, Limits, ParentInput, ReplayInput, RunSpec,
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

/// What a local runner clones. The stored remote wins; the constant is the floor
/// so a repository attached before remotes were read still has somewhere to come
/// from.
pub(crate) fn clone_url(repo: &Repo) -> Option<String> {
    if let Some(remote) = repo.remote_url.as_deref() {
        return Some(remote.to_string());
    }
    // With no remote, the checkout on this machine is the source: cloning a path
    // needs no network and produces the same history. A repository attached from
    // GitHub has the remote and no path; one attached by path may have neither.
    repo.path
        .as_deref()
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
}

/// Where a run finds the repository it works from. A repository attached by path
/// borrows that checkout; one attached by its remote gets a mirror the runner owns,
/// so the loop never works inside a checkout the user is using. The mirror also
/// keeps a private clone possible: the server mints a token scoped to that one
/// repository and hands it to the runner.
fn host_path(repo: &Repo, data_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    if let Some(path) = repo
        .path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
    {
        return Some(std::path::PathBuf::from(path));
    }
    repo.remote_url
        .as_ref()
        .map(|_| data_dir.join("mirrors").join(repo.id.to_string()))
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

fn is_mirror(repo: &Repo) -> bool {
    !repo
        .path
        .as_deref()
        .map(str::trim)
        .map(|path| !path.is_empty())
        .unwrap_or(false)
        && repo.remote_url.is_some()
}

async fn clone_token(engine: &Engine, repo: &Repo) -> Option<String> {
    let (owner, name) = crate::github::split_remote(repo.remote_url.as_deref()?)?;
    let app = engine
        .stored_app(crate::github::PROVIDER, crate::connectors::KIND_GITHUB_APP)
        .await
        .ok()
        .flatten()?;
    let jwt = crate::github::mint_token(&app.app_id, &app.private_key).ok()?;
    crate::github::installation_token(&reqwest::Client::new(), &jwt, &owner, &name)
        .await
        .ok()
}

/// Runs either borrow the repository at its path or produce a checkout of their
/// own. Single decision point, because a runner that mixes the two is a runner
/// that leaves half a clone behind.
fn runner_mode() -> &'static str {
    match std::env::var("HULAHOOP_RUNNER_MODE").as_deref() {
        Ok("local") => "local",
        _ => "workspace",
    }
}

/// Replays read the loop's own bookmarks (`tests/<issue>/…`), which never leave the
/// host repository, so they always run in the borrowed one.
fn checkout_for(
    data_dir: &std::path::Path,
    project: &uuid::Uuid,
    issue: &uuid::Uuid,
    owner: &uuid::Uuid,
) -> std::path::PathBuf {
    if runner_mode() == "local" {
        hulahoop_core::contract::workspace_checkout_dir(
            data_dir,
            &project.to_string(),
            issue,
            owner,
        )
    } else {
        hulahoop_core::contract::checkout_dir(
            data_dir,
            &project.to_string(),
            issue,
            &owner.to_string(),
        )
    }
}

pub fn router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/internal/runs/claim", post(claim_run))
        .route("/internal/runs/{id}/spec", axum::routing::get(get_spec))
        .route("/internal/runs/{id}/heartbeat", post(heartbeat))
        .route("/internal/runs/{id}/events", post(append_event))
        .route("/internal/runs/{id}/complete", post(complete_run))
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
    let expected = state.token.as_bytes();
    let provided = provided.as_bytes();
    let mut diff = (expected.len() ^ provided.len()) as u8;
    for index in 0..expected.len().max(provided.len()) {
        let a = *expected.get(index).unwrap_or(&0);
        let b = *provided.get(index).unwrap_or(&0);
        diff |= a ^ b;
    }
    if diff == 0 {
        Ok(())
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<Value>)>;

fn fail(status: StatusCode, message: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": message.to_string() })))
}

#[derive(Deserialize)]
struct ClaimRequest {
    #[serde(default)]
    runner_id: Option<String>,
}

#[derive(Serialize)]
struct RunEnvelope {
    run: Run,
    spec: Value,
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
        .claim_run(&runner_id)
        .await
        .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let Some(run) = run else {
        return Ok(Json(None));
    };
    let spec = build_run_spec(&state.engine, &run)
        .await
        .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(Some(RunEnvelope { run, spec })))
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
    state
        .engine
        .heartbeat(id, body.pid, body.pgid)
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
    agent.connector = run.connector_id.clone();
    let checkout_path = match &repo {
        Some(repo) => {
            // A task attached to a repository before projects were required may
            // still have none; the repository's own project names the checkout.
            let project: Project = sqlx::query_as("select * from projects where id = ?")
                .bind(issue.project_id.unwrap_or(repo.project_id))
                .fetch_one(&engine.db.pool)
                .await?;
            checkout_for(&engine.db.data_dir, &project.id, &issue.id, &run.id)
        }
        None => hulahoop_core::contract::scratch_dir(&engine.db.data_dir, &run.id),
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
            .agent_credential(adapter, agent.connector.as_deref())
            .await?;
        let has_api_key = credential
            .env
            .iter()
            .any(|(key, _)| key == "ANTHROPIC_API_KEY" || key == "PI_API_KEY");
        let source = if has_api_key {
            "api"
        } else if !credential.is_empty() && adapter == hulahoop_core::Adapter::ClaudeCode {
            "subscription"
        } else {
            // Pi login may bill a provider directly; inherited runner credentials
            // can also supersede a supplied login. Preserve this uncertainty.
            "unknown"
        };
        let selected = match (adapter, source) {
            (hulahoop_core::Adapter::ClaudeCode, "api") => Some("claude-api"),
            (hulahoop_core::Adapter::ClaudeCode, "subscription") => Some("claude-sub"),
            (hulahoop_core::Adapter::Pi, "api") => Some("pi"),
            _ => None,
        };
        if run
            .billing_source
            .as_deref()
            .is_some_and(|previous| previous != source)
        {
            anyhow::bail!("run credential billing source changed after claim");
        }
        if run.billing_source.is_none() {
            sqlx::query("update runs set billing_source = ?, connector_id = coalesce(connector_id, ?) where id = ? and status = 'running' and billing_source is null")
                .bind(source)
                .bind(selected)
                .bind(run.id)
                .execute(&engine.db.pool)
                .await?;
        }
        serde_json::json!({
            "env": credential.env,
            "files": credential.files,
        })
    };
    // Committing is the app's act, so every turn gets the app's token: a turn that
    // changed nothing pushes nothing, and nothing else may decide that in advance.
    let github_token = engine.github_token().await?;

    let replays: Vec<Replay> =
        sqlx::query_as("select * from replays where issue_id = ? order by created_at desc limit 4")
            .bind(issue.id)
            .fetch_all(&engine.db.pool)
            .await?;
    let replay_inputs: Vec<ReplayInput> = replays
        .iter()
        .map(|replay| {
            let report_path = std::path::Path::new(&replay.artifact_dir).join("junit.xml");
            let failed = std::fs::read_to_string(&report_path)
                .map(|xml| junit::parse(&xml).failed_names())
                .unwrap_or_default();
            ReplayInput {
                kind: replay.kind.clone(),
                status: replay.status.clone(),
                report_path: report_path.exists().then_some(report_path),
                failed,
            }
        })
        .collect();

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
            path: repo.path.as_ref().map(std::path::PathBuf::from),
            base_bookmark: repo.base_bookmark.clone(),
        }),
        checkout: CheckoutInput {
            path: checkout_path.clone(),
            base_bookmark: run.base_bookmark.clone(),
        },
        parent,
        blocked_answer,
        replays: replay_inputs,
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
    let (repo_path, repo_mirror, repo_clone_token, vcs, repo_id, repo_clone_url) = match &repo {
        Some(repo) => (
            json!(host_path(repo, &engine.db.data_dir)),
            json!(is_mirror(repo)),
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
        "repo_mirror": repo_mirror,
        "clone_token": repo_clone_token,
        "vcs": vcs,
        "repo_id": repo_id,
        "checkout": checkout_mode,
        "checkout_path": checkout_path,
        "runner_mode": runner_mode(),
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
        // The runner names the commit after the task; without the title every head
        // read "implement: the issue".
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
    let mut repo: Repo = sqlx::query_as("select * from repos where id = ?")
        .bind(job.repo_id)
        .fetch_one(&engine.db.pool)
        .await?;
    // A repository attached before the column existed has no remote, and a pull
    // request job cannot work without one. Reading it now costs a subprocess and
    // saves a migration that would have to guess.
    if repo.remote_url.is_none() {
        let found = repo
            .path
            .as_deref()
            .and_then(|path| crate::vcs_remote(std::path::Path::new(path)));
        if let Some(remote) = found {
            sqlx::query("update repos set remote_url = ? where id = ?")
                .bind(&remote)
                .bind(repo.id)
                .execute(&engine.db.pool)
                .await?;
            repo.remote_url = Some(remote);
        }
    }
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
        let credential = engine.agent_credential(adapter, None).await?;
        Some(json!({ "env": credential.env, "files": credential.files }))
    } else {
        None
    };

    Ok(json!({
        "job_id": job.id,
        "credential": credential,
        "kind": job.kind,
        "payload": payload,
        "repo_path": host_path(&repo, &engine.db.data_dir),
        "repo_mirror": is_mirror(&repo),
        "clone_token": clone_token(engine, &repo).await,
        "repo_id": repo.id,
        "vcs": repo.vcs,
        "remote_url": repo.remote_url,
        "checkout_path": checkout_path,
        "artifact_dir": engine.db.data_dir.join("jobs").join(job.id.to_string()),
        "data_dir": engine.db.data_dir,
        "base_bookmark": repo.base_bookmark,
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
        sqlx::query("insert into repos (id, project_id, name, path, contract_version, created_at) values (?,?, 'repo','/tmp','1',?)")
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
        sqlx::query("insert into repos (id, project_id, name, path, contract_version, created_at) values (?,?, 'repo','/tmp','1',?)")
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
        for key in [
            "repo_path",
            "repo_mirror",
            "clone_url",
            "clone_token",
            "repo_id",
            "vcs",
        ] {
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
