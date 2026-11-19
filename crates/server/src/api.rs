//! Internal HTTP API. This is the only door between the runner and the
//! database, which is what keeps the server the single SQLite writer.

use crate::engine::{prompt_for, Engine};
use crate::model::*;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use hulahoop_core::runspec::{
    CheckoutInput, CriterionInput, IssueInput, Limits, ParentInput, ReplayInput, RunSpec,
};
use hulahoop_core::{contract, junit, AgentRole};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

pub struct ApiState {
    pub engine: Arc<Engine>,
    pub token: String,
    pub runner_id_default: String,
}

pub fn router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/internal/runs/claim", post(claim_run))
        .route("/internal/runs/{id}/spec", axum::routing::get(get_spec))
        .route("/internal/runs/{id}/heartbeat", post(heartbeat))
        .route("/internal/runs/{id}/events", post(append_event))
        .route("/internal/runs/{id}/complete", post(complete_run))
        .route("/internal/replays/claim", post(claim_replay))
        .route("/internal/replays/{id}/complete", post(complete_replay))
        .route("/internal/jobs/claim", post(claim_job))
        .route("/internal/jobs/{id}/complete", post(complete_job))
        .route("/internal/repos/{id}/lock", post(lock_repo))
        .route("/internal/repos/{id}/unlock", post(unlock_repo))
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
struct ReplayEnvelope {
    replay: Replay,
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
    let run = state
        .engine
        .complete_run(id, body)
        .await
        .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(
        json!({ "run": { "id": run.id.to_string(), "status": run.status } }),
    ))
}

async fn claim_replay(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> ApiResult<Option<ReplayEnvelope>> {
    authorize(&state, &headers).map_err(|s| fail(s, "bad token"))?;
    let replay = state
        .engine
        .claim_replay()
        .await
        .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let Some(replay) = replay else {
        return Ok(Json(None));
    };
    let spec = build_replay_spec(&state.engine, &replay)
        .await
        .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(Some(ReplayEnvelope { replay, spec })))
}

async fn complete_replay(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<crate::engine::ReplayCompletion>,
) -> ApiResult<Value> {
    authorize(&state, &headers).map_err(|s| fail(s, "bad token"))?;
    state
        .engine
        .complete_replay(id, body)
        .await
        .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(json!({ "ok": true })))
}

async fn claim_job(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> ApiResult<Option<JobEnvelope>> {
    authorize(&state, &headers).map_err(|s| fail(s, "bad token"))?;
    let job = state
        .engine
        .claim_job()
        .await
        .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let Some(job) = job else {
        return Ok(Json(None));
    };
    let spec = build_job_spec(&state.engine, &job)
        .await
        .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
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
    let repo: Repo = sqlx::query_as("select * from repos where id = ?")
        .bind(run.repo_id)
        .fetch_one(&engine.db.pool)
        .await?;
    let agent: Agent = sqlx::query_as("select * from agents where id = ?")
        .bind(run.agent_id)
        .fetch_one(&engine.db.pool)
        .await?;
    let criteria = engine.criteria(run.issue_id).await?;
    let project: Project = sqlx::query_as("select * from projects where id = ?")
        .bind(issue.project_id)
        .fetch_one(&engine.db.pool)
        .await?;

    let checkout_path = contract::checkout_dir(
        &engine.db.data_dir,
        &project.id.to_string(),
        &issue.id,
        &run.id.to_string(),
    );

    // The parent run carries the review body the next attempt has to answer.
    let parent = match run.parent_run_id {
        Some(parent_id) => {
            let parent_run: Option<Run> = sqlx::query_as("select * from runs where id = ?")
                .bind(parent_id)
                .fetch_optional(&engine.db.pool)
                .await?;
            match parent_run {
                Some(parent_run) => {
                    let review: Option<Review> = sqlx::query_as(
                        "select * from reviews where reviewed_run_id = ? order by created_at desc limit 1",
                    )
                    .bind(parent_run.id)
                    .fetch_optional(&engine.db.pool)
                    .await?;
                    Some(ParentInput {
                        head_bookmark: parent_run.head_bookmark.unwrap_or_default(),
                        verdict: review
                            .as_ref()
                            .map(|r| r.verdict.clone())
                            .unwrap_or_else(|| "changes_requested".into()),
                        body: review.map(|r| r.body).unwrap_or_default(),
                    })
                }
                None => None,
            }
        }
        None => None,
    };

    let blocked_answer = if let (Some(pinned), true) =
        (issue.pinned_comment_id, issue.status == "blocked")
    {
        let question: Option<(String,)> = sqlx::query_as("select body from comments where id = ?")
            .bind(pinned.to_string())
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

    // Committing is the app's act, so the implementer is the role that gets the
    // app's token. The test author and the reviewer write tests and read a diff;
    // neither pushes as the user, so neither is handed a credential it cannot use.
    let github_token = match run.role_enum() {
        AgentRole::Implementer => engine.github_token().await?,
        AgentRole::TestAuthor | AgentRole::Reviewer => None,
    };

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

    let spec = RunSpec {
        run_id: run.id,
        issue_id: issue.id,
        role: run.role_enum(),
        attempt: run.attempt,
        issue: IssueInput {
            title: issue.title.clone(),
            body: issue.body.clone(),
            priority: issue.priority.clone(),
        },
        criteria: criteria
            .iter()
            .map(|criterion| CriterionInput {
                position: criterion.position,
                text: criterion.text.clone(),
            })
            .collect(),
        repo: hulahoop_core::runspec::RepoInput {
            name: repo.name.clone(),
            path: std::path::PathBuf::from(&repo.path),
            base_bookmark: repo.base_bookmark.clone(),
            tests_bookmark: issue.tests_bookmark.clone(),
        },
        checkout: CheckoutInput {
            path: checkout_path.clone(),
            base_bookmark: run.base_bookmark.clone(),
            frozen_paths: issue.frozen_path_list(),
        },
        parent,
        blocked_answer,
        replays: replay_inputs,
        limits: Limits::default(),
    }
    .truncate_text();

    let input_path = std::path::Path::new(&run.artifact_dir).join("input.json");
    spec.write(&input_path)?;
    let prompt_path = std::path::Path::new(&run.artifact_dir).join("prompt.md");
    std::fs::write(&prompt_path, prompt_for(run.role_enum()))?;

    Ok(json!({
        "run_id": run.id,
        "issue_id": issue.id,
        "attempt": run.attempt,
        "role": run.role_enum().as_str(),
        "adapter": agent.adapter,
        "model": agent.model,
        "prompt": prompt_for(run.role_enum()),
        "prompt_path": prompt_path,
        "input_path": input_path,
        "repo_path": repo.path,
        "vcs": repo.vcs,
        "repo_id": repo.id,
        "checkout_path": checkout_path,
        "base_bookmark": run.base_bookmark,
        "base_revision": run.base_revision,
        "tests_bookmark": issue.tests_bookmark,
        "frozen_paths": issue.frozen_path_list(),
        "github_token": github_token,
        "artifact_dir": run.artifact_dir,
        "data_dir": engine.db.data_dir,
        "caller_role": "run",
        "timeout_minutes": hulahoop_core::AGENT_WALL_CLOCK_MINUTES,
        "diff_line_limit": hulahoop_core::DIFF_LINE_LIMIT,
        "prompt_revision": run.prompt_revision,
    }))
}

async fn build_replay_spec(engine: &Engine, replay: &Replay) -> anyhow::Result<Value> {
    let issue = engine.issue(replay.issue_id).await?;
    let repo: Repo = sqlx::query_as("select * from repos where id = ?")
        .bind(replay.repo_id)
        .fetch_one(&engine.db.pool)
        .await?;
    let project: Project = sqlx::query_as("select * from projects where id = ?")
        .bind(issue.project_id)
        .fetch_one(&engine.db.pool)
        .await?;
    let checkout_path = contract::checkout_dir(
        &engine.db.data_dir,
        &project.id.to_string(),
        &issue.id,
        &replay.id.to_string(),
    );

    // A landing replay runs on the freshly fetched base after a rebase; the base
    // and head replays run on the revision they were queued for.
    // A landing replay hangs off the approval rather than off a run of its own, and
    // the run behind that approval is the one holding the head bookmark.
    let mut head_run_id = replay.run_id.or(replay.candidate_run_id);
    if head_run_id.is_none() {
        if let Some(approval_id) = replay.approval_id {
            head_run_id =
                sqlx::query_scalar::<_, Uuid>("select run_id from approvals where id = ?")
                    .bind(approval_id)
                    .fetch_optional(&engine.db.pool)
                    .await?;
        }
    }
    let head_bookmark = match head_run_id {
        Some(run_id) => {
            let run: Option<Run> = sqlx::query_as("select * from runs where id = ?")
                .bind(run_id)
                .fetch_optional(&engine.db.pool)
                .await?;
            run.and_then(|r| r.head_bookmark)
        }
        None => None,
    };
    let approval: Option<Approval> = match replay.approval_id {
        Some(id) => {
            sqlx::query_as("select * from approvals where id = ?")
                .bind(id)
                .fetch_optional(&engine.db.pool)
                .await?
        }
        None => None,
    };
    let candidate = match replay.candidate_run_id {
        Some(id) => {
            sqlx::query_as::<_, Run>("select * from runs where id = ?")
                .bind(id)
                .fetch_optional(&engine.db.pool)
                .await?
        }
        None => None,
    };

    // The candidate was authored on the base of *its own* run, and the paths to lay
    // back down are the difference from there. Diffing against the repo's current base
    // would turn every commit landed since then into a "change" and lay the old files
    // back over the new ones.
    let candidate_bookmark = candidate
        .as_ref()
        .and_then(|c| c.candidate_bookmark.clone());
    let candidate_revision = candidate.as_ref().and_then(|c| c.head_revision.clone());
    let candidate_base_revision = candidate.as_ref().and_then(|c| c.base_revision.clone());

    Ok(json!({
        "replay_id": replay.id,
        "issue_id": issue.id,
        "attempt": replay.attempt,
        "kind": replay.kind,
        "repo_path": repo.path,
        "repo_id": repo.id,
        "vcs": repo.vcs,
        "checkout_path": checkout_path,
        "base_bookmark": repo.base_bookmark,
        "base_revision": repo.base_revision,
        "tests_bookmark": replay.tests_bookmark,
        "frozen_paths": issue.frozen_path_list(),
        "artifact_dir": replay.artifact_dir,
        "data_dir": engine.db.data_dir,
        "caller_role": "replay",
        "head_bookmark": head_bookmark,
        "candidate_bookmark": candidate_bookmark,
        "candidate_revision": candidate_revision,
        "candidate_base_revision": candidate_base_revision,
        "landing_bookmark": approval.clone().and_then(|a| a.landing_bookmark),
        "approval_head_revision": approval.map(|a| a.head_revision),
        "timeout_minutes": hulahoop_core::REPLAY_WALL_CLOCK_MINUTES,
    }))
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
    let pull_request = if job.kind == "open_pr" {
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
                let criteria = match issue.as_ref() {
                    Some(issue) => engine.criteria(issue.id).await?,
                    None => Vec::new(),
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
                    "head_revision": run.as_ref().and_then(|r| r.head_revision.clone()),
                    "attempt": run.as_ref().map(|r| r.attempt),
                    "branch": run.as_ref().map(|r| format!("hulahoop/issue-{}-attempt-{}", r.issue_id, r.attempt)),
                    "title": issue.as_ref().map(|i| i.title.clone()),
                    "body": issue.as_ref().map(|i| i.body.clone()),
                    "criteria": criteria.iter().map(|c| c.text.clone()).collect::<Vec<_>>(),
                    "review": review,
                    "app_token": engine.github_token().await?,
                }))
            }
            None => None,
        }
    } else {
        None
    };

    Ok(json!({
        "job_id": job.id,
        "kind": job.kind,
        "payload": payload,
        "repo_path": repo.path,
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
