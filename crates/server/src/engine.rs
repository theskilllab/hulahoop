//! Business rules: every mutation and every runner callback lands here, so the
//! state machine and the gate live in one place.

use crate::connectors::Credential;
use crate::db::{kill_process_group, sha256_hex, Db};
use crate::github::StoredApp;
use crate::model::ConnectorRow;
use crate::model::*;
use anyhow::{anyhow, bail, Result};
use chrono::Utc;
use hulahoop_core::contract;
use hulahoop_core::diff;
use hulahoop_core::{IssueEvent, IssueStatus, RunFlag, RunStatus};
use uuid::Uuid;

/// The provider's own refusal, when the adapter recorded one. pi reports a rejected
/// model as a transcript line and an exit code of 0, which is why this looks at the
/// text instead of the status.
fn adapter_failure(payload: &RunCompletion) -> Option<String> {
    let transcript = payload.transcript.as_deref().unwrap_or_default();
    let marker = "\"finalError\":\"";
    let start = transcript.rfind(marker)? + marker.len();
    let rest = &transcript[start..];
    let end = rest.find("\"}").or_else(|| rest.find('"'))?;
    let reason = rest[..end].trim();
    if reason.is_empty() {
        return None;
    }
    Some(reason.to_string())
}

/// A cleanup of a task that takes no more turns: merged, closed or cancelled.
pub const CLEANUP_FINISHED: &str = "finished";
/// A cleanup of the checkout a task left behind when it changed project.
pub const CLEANUP_MOVED: &str = "moved";
/// How long a cleanup waits after a run of its task was cancelled. The runner
/// learns of the cancel only when the agent dies, and still formats, diffs and
/// reports in the checkout after that.
const CLEANUP_AFTER_CANCEL_SECS: i64 = 5 * 60;

/// The priority ladder, highest first. A fixed set, so a board can sort by it
/// and a filter can name every value.
pub const PRIORITIES: [&str; 4] = ["urgent", "high", "medium", "low"];

/// Read a priority someone typed. Absent means `medium`; `med` is how people
/// shorten it. Anything off the ladder is refused by name rather than stored,
/// because a priority nobody can sort by is noise on the board.
pub fn priority(value: Option<&str>) -> Result<&'static str> {
    let Some(value) = value.map(|v| v.trim().to_ascii_lowercase()) else {
        return Ok("medium");
    };
    let value = if value == "med" {
        "medium".into()
    } else {
        value
    };
    PRIORITIES
        .iter()
        .find(|known| **known == value)
        .copied()
        .ok_or_else(|| {
            anyhow!(
                "priority must be one of {}, not {value:?}",
                PRIORITIES.join(", ")
            )
        })
}

/// Read a due date. Blank clears it; anything else must be a calendar day in
/// `YYYY-MM-DD`, the one format every client and every sort agrees on.
pub fn due_date(value: &str) -> Result<Option<String>> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map(|date| Some(date.format("%Y-%m-%d").to_string()))
        .map_err(|_| anyhow!("a due date is a calendar day like 2026-03-31, not {value:?}"))
}

pub struct Engine {
    pub db: Db,
}

impl Engine {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    // ---------------------------------------------------------------- issues

    pub async fn issue(&self, id: Uuid) -> Result<Issue> {
        sqlx::query_as::<_, Issue>("select * from issues where id = ?")
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await?
            .ok_or_else(|| anyhow!("issue {id} not found"))
    }

    pub async fn transition(&self, issue_id: Uuid, event: IssueEvent) -> Result<Issue> {
        let issue = self.issue(issue_id).await?;
        let from = issue.status_enum();
        let to = hulahoop_core::next(from, &event).map_err(|error| anyhow!(error))?;
        let now = Utc::now().to_rfc3339();

        let status_before = if to == IssueStatus::Blocked {
            Some(from.as_str().to_string())
        } else if from == IssueStatus::Blocked && to != IssueStatus::Blocked {
            None
        } else {
            issue.status_before_blocked.clone()
        };

        sqlx::query(
            "update issues set status = ?, status_before_blocked = ?, updated_at = ? where id = ?",
        )
        .bind(to.as_str())
        .bind(status_before)
        .bind(&now)
        .bind(issue_id)
        .execute(&self.db.pool)
        .await?;

        // A new status means a new round of turns; staying put (`Start`, a
        // self-move) keeps the queue, so the cap counts real turns instead of
        // being reset by every attempt.
        if from != to {
            let _ = self.reset_turns(issue_id, to).await;
        }
        let issue = self.issue(issue_id).await?;
        self.retire_checkout_when_finished(&issue, from).await;
        // A run that failed or was cancelled leaves the task where it is, for a
        // person to decide: starting the next attempt here retried at once, and a
        // broken host (a full disk) burned a run every second.
        if matches!(event, IssueEvent::CancelRun) && from == to {
            return Ok(issue);
        }
        // Moving a task into a status an agent owns is the dispatch: nobody has
        // to press start, and nothing starts twice because the run table allows
        // one live run per issue.
        if let Err(error) = Box::pin(self.dispatch_if_agent_owns(&issue)).await {
            let _ = self
                .comment(
                    issue_id,
                    hulahoop_core::AuthorType::Server,
                    None,
                    &format!("the workflow wants an agent here, but none could start: {error}"),
                    false,
                )
                .await;
        }
        self.issue(issue_id).await
    }

    /// A task that just became Done or Cancelled takes no more turns, so its
    /// checkout goes: a runner job forgets the workspace in the mirror and
    /// deletes the directory, because the runner owns the checkouts. A task
    /// that is reopened before that job ran keeps its checkout. A failure to
    /// queue or drop the job costs disk space and nothing else, so it is
    /// logged, not raised into the move.
    async fn retire_checkout_when_finished(&self, issue: &Issue, from: IssueStatus) {
        let finished = issue.status_enum().is_terminal();
        if from.is_terminal() && !finished {
            if let Err(error) = self.keep_current_checkout(issue).await {
                tracing::warn!(issue = %issue.id, %error, "could not drop the checkout cleanup");
            }
            return;
        }
        if from.is_terminal() || !finished {
            return;
        }
        if let Err(error) = self.queue_cleanup(issue, CLEANUP_FINISHED).await {
            tracing::warn!(issue = %issue.id, %error, "could not queue the checkout cleanup");
        }
    }

    /// The checkout a task with a repository works in, and the project and
    /// repository that place it. A task with no repository works in scratch
    /// directories the runner removes itself.
    async fn task_checkout(
        &self,
        issue: &Issue,
    ) -> Result<Option<(Uuid, Uuid, std::path::PathBuf)>> {
        let Some(repo_id) = issue.repo_id else {
            return Ok(None);
        };
        let project_id = match issue.project_id {
            Some(id) => id,
            None => {
                sqlx::query_scalar::<_, Uuid>("select project_id from repos where id = ?")
                    .bind(repo_id)
                    .fetch_one(&self.db.pool)
                    .await?
            }
        };
        let checkout =
            contract::checkout_dir(&self.db.data_dir, &project_id.to_string(), &issue.id);
        Ok(Some((repo_id, project_id, checkout)))
    }

    /// Queue the job that deletes a task's checkout as `issue` places it.
    /// `reason` is [`CLEANUP_FINISHED`] for a task that takes no more turns, or
    /// [`CLEANUP_MOVED`] for the checkout a task left behind when it changed
    /// project. The runner derives the path from the ids again, so the payload
    /// cannot point it anywhere else.
    pub(crate) async fn queue_cleanup(&self, issue: &Issue, reason: &str) -> Result<Option<Uuid>> {
        let Some((repo_id, project_id, checkout)) = self.task_checkout(issue).await? else {
            return Ok(None);
        };
        let id = self
            .queue_job(
                repo_id,
                Some(issue.id),
                "cleanup",
                serde_json::json!({
                    "checkout": checkout,
                    "project_id": project_id,
                    "issue_id": issue.id,
                    "reason": reason,
                }),
            )
            .await?;
        Ok(Some(id))
    }

    /// Drops any queued cleanup of the checkout the task works in now: the
    /// task was reopened, or moved back to a project it left, and its next
    /// turn needs that checkout.
    pub(crate) async fn keep_current_checkout(&self, issue: &Issue) -> Result<()> {
        let Some((_, _, checkout)) = self.task_checkout(issue).await? else {
            return Ok(());
        };
        sqlx::query(
            "delete from runner_jobs where kind = 'cleanup' and status = 'queued'
               and issue_id = ? and json_extract(payload, '$.checkout') = ?",
        )
        .bind(issue.id)
        .bind(checkout.to_string_lossy().to_string())
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Fill the turn queue for the status a task just entered.
    ///
    /// The document names the team; nothing starts until the queue exists, and the
    /// order in `Users:` *is* the order of turns.
    async fn reset_turns(&self, issue_id: Uuid, status: IssueStatus) -> Result<()> {
        let issue = self.issue(issue_id).await?;
        let agents = self.turn_agents(issue.workspace_id, status).await?;
        sqlx::query(
            "update issues set turn_queue = ?, turn_served = '[]', turns_used = 0 where id = ?",
        )
        .bind(serde_json::json!(agents).to_string())
        .bind(issue_id)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// The next agent owed a turn in this status, if any.
    ///
    /// The queue holds names because that is what the document says; service is
    /// recorded by id, because an agent's display name can be anything while the
    /// row that ran is unambiguous.
    /// A move a person makes, held to the document: only a status the current one
    /// declares is a move, so a stopped task has the exits its workflow names and
    /// nothing else.
    pub async fn move_declared(&self, issue_id: Uuid, target: IssueStatus) -> Result<Issue> {
        let issue = self.issue(issue_id).await?;
        let moves = self
            .workflow_moves(issue.workspace_id, issue.status_enum())
            .await?;
        if !moves.contains(&target) {
            let allowed = match moves.as_slice() {
                [] => "nothing".to_string(),
                moves => moves
                    .iter()
                    .map(|status| status.title())
                    .collect::<Vec<_>>()
                    .join(", "),
            };
            bail!(
                "the document does not move `{}` to {}. It names {}.",
                issue.status_enum().title(),
                target.title(),
                allowed
            );
        }
        self.move_to(issue_id, target).await
    }

    /// The newest head the issue published, whoever published it: the work a
    /// review judged stays the base of the turns that follow it.
    pub(crate) async fn newest_head(&self, issue_id: Uuid) -> Result<Option<String>> {
        Ok(sqlx::query_scalar::<_, String>(
            "select head_bookmark from runs where issue_id = ? and head_bookmark is not null \
             order by created_at desc limit 1",
        )
        .bind(issue_id)
        .fetch_optional(&self.db.pool)
        .await?)
    }

    /// The run under review: the one a status handed over when there is one,
    /// otherwise the newest finished run that published a head. A second turn in
    /// a status reads the work of the turn before it, which is the point of a team
    /// in a column.
    ///
    /// A task with no repository publishes no heads, so its work is the newest
    /// finished turn other than the one doing the judging (`judging`).
    pub async fn run_under_review(
        &self,
        issue: &crate::model::Issue,
        judging: Option<Uuid>,
    ) -> Result<Uuid> {
        if let Some(id) = issue.current_run_id.filter(|id| Some(*id) != judging) {
            return Ok(id);
        }
        let query = if issue.repo_id.is_some() {
            "select id from runs
              where issue_id = ? and status = 'succeeded'
                and head_bookmark is not null and id is not ?
              order by created_at desc limit 1"
        } else {
            "select id from runs
              where issue_id = ? and status = 'succeeded' and id is not ?
              order by created_at desc limit 1"
        };
        sqlx::query_scalar::<_, Uuid>(query)
            .bind(issue.id)
            .bind(judging)
            .fetch_optional(&self.db.pool)
            .await?
            .ok_or_else(|| anyhow!("there is no finished run on this issue to review yet"))
    }

    async fn next_turn_agent(&self, issue: &Issue) -> Result<Option<String>> {
        let queue: Vec<String> = serde_json::from_str(&issue.turn_queue).unwrap_or_default();
        let served: Vec<String> = serde_json::from_str(&issue.turn_served).unwrap_or_default();
        if queue.is_empty() {
            // A task that predates the queue still gets one turn from the document.
            if served.is_empty() {
                return self
                    .workflow_agent(issue.workspace_id, issue.status_enum())
                    .await;
            }
            return Ok(None);
        }
        for name in queue {
            let id = self
                .agent_for_name(issue.workspace_id, Some(&name))
                .await?
                .map(|id| id.to_string());
            match id {
                Some(id) if served.contains(&id) => continue,
                Some(_) => return Ok(Some(name)),
                // A name nobody answers to cannot take a turn.
                None => continue,
            }
        }
        Ok(None)
    }

    /// Record that an agent has had its turn in the current status.
    async fn mark_served(&self, issue: &Issue, agent_id: Uuid) -> Result<()> {
        let mut served: Vec<String> = serde_json::from_str(&issue.turn_served).unwrap_or_default();
        let id = agent_id.to_string();
        if !served.contains(&id) {
            served.push(id);
        }
        sqlx::query("update issues set turn_served = ? where id = ?")
            .bind(serde_json::json!(served).to_string())
            .bind(issue.id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    /// Move a task to a status by name.
    ///
    /// The state machine only knows the shapes it was born with; a workflow is
    /// what decides where a task goes next. Everything a transition does besides
    /// the shape — clearing a block, dispatching whoever owns the new status —
    /// happens here too, because the workflow is the source of truth.
    pub async fn move_to(&self, issue_id: Uuid, target: IssueStatus) -> Result<Issue> {
        let issue_before = self.issue(issue_id).await?;
        if issue_before.status_enum() == target {
            return Ok(issue_before);
        }
        sqlx::query(
            "update issues set status = ?, status_before_blocked = null, updated_at = ? where id = ?",
        )
        .bind(target.as_str())
        .bind(Utc::now().to_rfc3339())
        .bind(issue_id)
        .execute(&self.db.pool)
        .await?;

        let _ = self.reset_turns(issue_id, target).await;
        let issue = self.issue(issue_id).await?;
        self.retire_checkout_when_finished(&issue, issue_before.status_enum())
            .await;
        if let Err(error) = Box::pin(self.dispatch_if_agent_owns(&issue)).await {
            let _ = self
                .comment(
                    issue_id,
                    hulahoop_core::AuthorType::Server,
                    None,
                    &format!("the workflow wants an agent here, but none could start: {error}"),
                    false,
                )
                .await;
        }
        self.issue(issue_id).await
    }

    /// What a run on this status *is*, for the record: a review, or work.
    ///
    /// It no longer decides who may run — the workflow names the agent. It only
    /// tells the engine which shape of completion to expect, because a review
    /// writes a verdict and work opens a pull request.
    /// Start a run when the workflow says an agent owns this status, and only
    /// while the task can still move: a blocked task waits for a person, and a
    /// finished one is done. A section that names an agent in either place would
    /// otherwise start turns on a task nobody can advance.
    pub(crate) async fn dispatch_if_agent_owns(&self, issue: &Issue) -> Result<()> {
        let status = issue.status_enum();
        if status == IssueStatus::Blocked || status.is_terminal() {
            return Ok(());
        }
        let workspace = issue.workspace_id;
        if self.workflow_owner(workspace, issue.status_enum()).await?
            != hulahoop_core::workflow::Owner::Agent
        {
            return Ok(());
        }
        // One live run per issue, and no second reviewer pass for a head that
        // already has a verdict.
        let live: Option<(Uuid,)> = sqlx::query_as(
            "select id from runs where issue_id = ? and status in ('queued', 'running')",
        )
        .bind(issue.id)
        .fetch_optional(&self.db.pool)
        .await?;
        if live.is_some() {
            return Ok(());
        }
        // Who serves the turn is the workflow's decision, and only that: what the
        // turn turns out to be is read from what it does, not from who did it.
        let named = self.next_turn_agent(issue).await?;
        let agent = self.agent_for_name(workspace, named.as_deref()).await?;
        // A head that already carries a verdict is not judged twice: the agent
        // who judged it has nothing to add until the work changes. Anyone else
        // still takes the turn, so a rework asked for after an approval (a
        // person's "request changes") reaches the agent who does the work.
        // With nobody named, there is no telling who would judge, so any verdict
        // on the head holds the turn as before.
        if let Some(head) = issue.current_run_id {
            let reviewed: Option<(Uuid,)> = sqlx::query_as(
                "select reviews.id from reviews join runs on runs.id = reviews.reviewer_run_id
                  where reviews.reviewed_run_id = ? and (? is null or runs.agent_id = ?) limit 1",
            )
            .bind(head)
            .bind(agent)
            .bind(agent)
            .fetch_optional(&self.db.pool)
            .await?;
            if reviewed.is_some() {
                return Ok(());
            }
        }
        // A status that keeps passing the turn around gets fenced: the cap is in
        // the document (`Turns:`), and hitting it is a question for a person.
        let cap = self.turn_cap(workspace, issue.status_enum()).await?;
        if issue.turns_used >= cap {
            let _ = self
                .comment(
                    issue.id,
                    hulahoop_core::AuthorType::Server,
                    None,
                    &format!(
                        "`{}` has taken {used} turns, which is the limit this workflow sets (`Turns: {cap}`). Somebody has to look at it.",
                        issue.status_enum().title(),
                        used = issue.turns_used
                    ),
                    false,
                )
                .await;
            self.move_to(issue.id, IssueStatus::Blocked).await?;
            return Ok(());
        }
        // A queue where everybody has served is the workflow's answer, so an
        // automatic turn stays quiet instead of handing the work to whoever
        // happens to be bound to the project.
        let queued: Vec<String> = serde_json::from_str(&issue.turn_queue).unwrap_or_default();
        if !queued.is_empty() && agent.is_none() {
            return Ok(());
        }

        sqlx::query("update issues set turns_used = turns_used + 1 where id = ?")
            .bind(issue.id)
            .execute(&self.db.pool)
            .await?;
        // The parent is the turn before this one, whoever took it: that is
        // where the message the next agent has to read lives.
        let parent = self.last_finished_run(issue.id).await?;
        self.start_run(issue.id, agent, parent).await?;
        Ok(())
    }

    /// Start the tasks that were waiting for this agent's probe.
    pub async fn retry_owed_turns(&self, repo_id: Uuid, agent_id: Uuid) -> Result<()> {
        let rows: Vec<(Uuid,)> = sqlx::query_as(
            "select id from issues where repo_id = ? and status not in ('backlog', 'blocked', 'done', 'in_review')",
        )
        .bind(repo_id)
        .fetch_all(&self.db.pool)
        .await?;
        for (issue_id,) in rows {
            let issue = match self.issue(issue_id).await {
                Ok(issue) => issue,
                Err(_) => continue,
            };
            // Only a task whose next turn belongs to this agent: a probe passing
            // must not push somebody else's task forward.
            let named = match self.next_turn_agent(&issue).await {
                Ok(named) => named,
                Err(_) => continue,
            };
            let owed = match self
                .agent_for_name(issue.workspace_id, named.as_deref())
                .await
            {
                Ok(owed) => owed,
                Err(_) => continue,
            };
            match owed {
                Some(id) if id == agent_id => {
                    let _ = Box::pin(self.dispatch_if_agent_owns(&issue)).await;
                }
                // `Any agent` means the project's binding, which may be this one.
                None => {
                    let _ = Box::pin(self.dispatch_if_agent_owns(&issue)).await;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Record a run the server could not finish, mark it failed, and refund its turn.
    pub async fn finish_server_error(
        &self,
        issue_id: Uuid,
        agent_id: Uuid,
        error: &str,
        run_id: Uuid,
    ) -> Result<()> {
        let message = format!("the server could not finish this run: {error}");
        let run = self.run(run_id).await?;
        let usage = spent_metrics(std::path::Path::new(&run.artifact_dir), None).await;
        sqlx::query("update runs set status = 'failed', error = ?, finished_at = ? where id = ?")
            .bind(&message)
            .bind(crate::db::now_rfc3339())
            .bind(run_id)
            .execute(&self.db.pool)
            .await?;
        persist_run_metrics(&self.db, &run, &usage, Some(&message)).await?;
        self.comment(
            issue_id,
            hulahoop_core::AuthorType::Server,
            None,
            &format!("{message}\n\nThe turn was given back, so this did not cost the task one of its turns."),
            false,
        )
        .await?;
        self.refund_turn(issue_id, agent_id).await;
        Ok(())
    }

    /// Give a turn back.
    ///
    /// A turn is spent when a run is dispatched. If the server then fails while
    /// finishing that run — a rule that does not hold for this workflow, a column
    /// that is null where a value was assumed — the agent never got to act, so
    /// the turn it spent has to return, or a cap of six becomes a cap of five.
    pub async fn refund_turn(&self, issue_id: Uuid, agent_id: Uuid) {
        let _ = sqlx::query(
            "update issues set turns_used = max(0, turns_used - 1), turn_served = null where id = ?",
        )
        .bind(issue_id)
        .execute(&self.db.pool)
        .await;
        let _ = sqlx::query("update issues set turn_served = ? where id = ? and turn_served = ?")
            .bind(Option::<String>::None)
            .bind(issue_id)
            .bind(agent_id.to_string())
            .execute(&self.db.pool)
            .await;
    }

    /// Queue a probe for this agent against every repository in its workspace.
    ///
    /// A probe answers "can this command line start in this repo", so it belongs
    /// to the moment a command line changes, not to the moment work arrives: an
    /// agent saved with a working harness should never make a task wait on one.
    pub async fn probe_everywhere(&self, agent_id: Uuid) -> Result<usize> {
        let agent = self.agent(agent_id).await?;
        sqlx::query("delete from agent_preflight where agent_id = ?")
            .bind(agent_id)
            .execute(&self.db.pool)
            .await?;
        let repos: Vec<(Uuid,)> = sqlx::query_as(
            "select r.id from repos r join projects p on p.id = r.project_id where p.workspace_id = ?",
        )
        .bind(agent.workspace_id)
        .fetch_all(&self.db.pool)
        .await?;
        for (repo_id,) in &repos {
            let existing: Option<(Uuid,)> = sqlx::query_as(
                "select id from runner_jobs where repo_id = ? and kind = 'preflight' and status in ('queued', 'running') and json_extract(payload, '$.agent_id') = ?",
            )
            .bind(repo_id)
            .bind(agent_id.to_string())
            .fetch_optional(&self.db.pool)
            .await?;
            if existing.is_none() {
                self.queue_job(
                    *repo_id,
                    None,
                    "preflight",
                    serde_json::json!({
                        "agent_id": agent_id,
                        "adapter": agent.adapter,
                        "model": agent.model,
                    }),
                )
                .await?;
            }
        }
        Ok(repos.len())
    }

    /// The run a turn follows: the newest one that is over.
    pub async fn last_finished_run(&self, issue_id: Uuid) -> Result<Option<Uuid>> {
        let row: Option<(Uuid,)> = sqlx::query_as(
            "select id from runs where issue_id = ? and status in ('succeeded', 'failed', 'cancelled', 'timed_out') order by created_at desc limit 1",
        )
        .bind(issue_id)
        .fetch_optional(&self.db.pool)
        .await?;
        Ok(row.map(|(id,)| id))
    }

    /// Resolve the workflow's `Agent:` value to a concrete agent: a name in the
    /// workspace wins, `any` and an empty value fall back to the project binding.
    pub(crate) async fn agent_for_name(
        &self,
        workspace_id: Uuid,
        name: Option<&str>,
    ) -> Result<Option<Uuid>> {
        if let Some(name) = name.map(|name| name.trim().trim_start_matches('@')) {
            if !name.eq_ignore_ascii_case("any") {
                // `@TestAuthor`, `test author` and `TestAuthor` are the same
                // person; spaces and the `@` are how people write, not identity.
                let found: Option<(Uuid,)> = sqlx::query_as(
                    "select id from agents where workspace_id = ? and lower(replace(name, ' ', '')) = lower(replace(?, ' ', '')) limit 1",
                )
                .bind(workspace_id)
                .bind(name)
                .fetch_optional(&self.db.pool)
                .await?;
                if let Some((id,)) = found {
                    return Ok(Some(id));
                }
            }
        }
        Ok(None)
    }

    pub async fn comment(
        &self,
        issue_id: Uuid,
        author: hulahoop_core::AuthorType,
        author_id: Option<&str>,
        body: &str,
        pinned: bool,
    ) -> Result<Uuid> {
        self.comment_on_run(issue_id, author, author_id, body, pinned, None)
            .await
    }

    /// A comment that came out of a run: the run's summary, its question, a
    /// reviewer's rejection, the pull request it opened. Linking the run lets a
    /// reader jump from what was said to the transcript that said it.
    pub async fn comment_on_run(
        &self,
        issue_id: Uuid,
        author: hulahoop_core::AuthorType,
        author_id: Option<&str>,
        body: &str,
        pinned: bool,
        run_id: Option<Uuid>,
    ) -> Result<Uuid> {
        let id = Uuid::now_v7();
        sqlx::query("insert into comments (id, issue_id, author_type, author_id, body, pinned, run_id, created_at) values (?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(id)
            .bind(issue_id)
            .bind(author.as_str())
            .bind(author_id)
            .bind(body)
            .bind(pinned)
            .bind(run_id)
            .bind(crate::db::now_rfc3339())
            .execute(&self.db.pool)
            .await?;
        if pinned {
            sqlx::query("update issues set pinned_comment_id = ? where id = ?")
                .bind(id)
                .bind(issue_id)
                .execute(&self.db.pool)
                .await?;
        }
        Ok(id)
    }

    // ----------------------------------------------------------- connectors

    /// Every provider the screen offers, stored or not. "Available" and
    /// "connected" are two different answers, and only a complete list shows both.
    pub const ENTRIES: [crate::connectors::Entry; 4] = crate::connectors::ENTRIES;

    fn vault(&self) -> Result<crate::vault::Vault> {
        crate::vault::Vault::open(&self.db.data_dir)
    }

    async fn connector_row(&self, provider: &str, kind: &str) -> Result<Option<ConnectorRow>> {
        Ok(sqlx::query_as::<_, ConnectorRow>(
            "select * from connectors where provider = ? and kind = ?",
        )
        .bind(provider)
        .bind(kind)
        .fetch_optional(&self.db.pool)
        .await?)
    }

    /// One field out of the stored payload. Each field is sealed on its own, so
    /// reading the app id never requires opening the private key.
    fn open_field(vault: &crate::vault::Vault, payload: &str, field: &str) -> Option<String> {
        let sealed = Self::payload_field(payload, field)?;
        vault.open_string(&sealed).ok()
    }

    /// One sealed field out of a stored payload. Payloads written before the envelope
    /// was JSON look like `{"app_id":v1:…,"private_key":v1:…}`, with the ciphertexts
    /// unquoted; a GitHub app stored then is still worth reading, and asking the user
    /// to paste a working key again because of a formatting change is a bad trade.
    fn payload_field(payload: &str, field: &str) -> Option<String> {
        if let Ok(map) = serde_json::from_str::<serde_json::Value>(payload) {
            return map.get(field)?.as_str().map(str::to_string);
        }
        let head = format!("\"{field}\":");
        let start = payload.find(&head)? + head.len();
        let rest = &payload[start..];
        let end = rest.find([',', '}']).unwrap_or(rest.len());
        let value = rest[..end].trim().trim_matches('"');
        if value.is_empty() {
            return None;
        }
        Some(value.to_string())
    }

    fn sealed_payload(fields: &[(&str, String)]) -> String {
        let mut map = serde_json::Map::new();
        for (name, value) in fields {
            map.insert(
                (*name).to_string(),
                serde_json::Value::String(value.clone()),
            );
        }
        serde_json::Value::Object(map).to_string()
    }

    /// The stored app's credentials, or `None` when nothing is stored or the
    /// vault cannot open what is stored. An unreadable credential is a reason to
    /// refuse work, not a reason to guess.
    pub(crate) async fn stored_app(&self, provider: &str, kind: &str) -> Result<Option<StoredApp>> {
        let Some(row) = self.connector_row(provider, kind).await? else {
            return Ok(None);
        };
        let vault = self.vault()?;
        let (Some(app_id), Some(private_key)) = (
            Self::open_field(&vault, &row.secret_ciphertext, "app_id"),
            Self::open_field(&vault, &row.secret_ciphertext, "private_key"),
        ) else {
            return Ok(None);
        };
        Ok(Some(StoredApp {
            app_id,
            private_key,
            slug: row.label,
            updated_at: row.updated_at,
        }))
    }

    /// What a run of this adapter authenticates with: a key in the environment, or
    /// the file a login wrote. `connector` is the entry the agent chose, which
    /// matters where a harness offers two ways to pay; an empty one takes whatever
    /// is connected, subscription first.
    pub async fn agent_credential(
        &self,
        adapter: hulahoop_core::Adapter,
        connector: Option<&str>,
    ) -> Result<Credential> {
        let entries = crate::connectors::for_adapter(adapter);
        let chosen = match connector {
            Some(id) if !id.is_empty() => entries.iter().find(|entry| entry.id == id).copied(),
            _ => None,
        };
        let candidates: Vec<&crate::connectors::Entry> = match chosen {
            Some(entry) => vec![entry],
            None => entries,
        };
        for entry in candidates {
            let Some(row) = self.connector_row(entry.provider, entry.kind).await? else {
                continue;
            };
            let vault = self.vault()?;
            let mut credential = Credential::default();
            if let Some(secret) = Self::open_field(&vault, &row.secret_ciphertext, "secret") {
                if let Some(name) = crate::connectors::api_key_env(entry.provider) {
                    credential.env.push((name.to_string(), secret));
                }
            }
            if let Some(token) = Self::open_field(&vault, &row.secret_ciphertext, "token") {
                credential
                    .env
                    .push(("CLAUDE_CODE_OAUTH_TOKEN".to_string(), token));
            }
            if let Some(contents) = Self::open_field(&vault, &row.secret_ciphertext, "contents") {
                if let Some(path) = crate::connectors::credentials_file(entry.provider) {
                    credential.files.push((path.to_string(), contents));
                }
            }
            if !credential.is_empty() {
                return Ok(credential);
            }
        }
        Ok(Credential::default())
    }

    /// Whether a stored secret can still act is the only honest definition of
    /// "connected": an app whose key cannot sign is not the app it claims to be,
    /// and a key the CLI would reject is not a login.
    pub async fn connections(&self) -> Result<Vec<Connection>> {
        let mut connections = Vec::with_capacity(Self::ENTRIES.len());
        for entry in Self::ENTRIES {
            let row = self.connector_row(entry.provider, entry.kind).await?;
            let connected_at = row.as_ref().map(|row| row.updated_at);
            let vault = self.vault()?;
            let (connected, app_id, label) = match &row {
                None => (false, None, None),
                Some(row) => {
                    let app_id = Self::open_field(&vault, &row.secret_ciphertext, "app_id");
                    let private_key =
                        Self::open_field(&vault, &row.secret_ciphertext, "private_key");
                    // Sealed and openable is what every entry can prove locally;
                    // a github app has one more answer, whether the key parses.
                    let openable = app_id.is_some()
                        || private_key.is_some()
                        || Self::open_field(&vault, &row.secret_ciphertext, "secret").is_some()
                        || Self::open_field(&vault, &row.secret_ciphertext, "token").is_some()
                        || Self::open_field(&vault, &row.secret_ciphertext, "contents").is_some();
                    let signs = private_key.as_deref().is_none_or(crate::github::can_sign);
                    (
                        openable && signs,
                        if openable { app_id } else { None },
                        row.label.clone(),
                    )
                }
            };
            connections.push(Connection {
                id: entry.id.to_string(),
                provider: entry.provider.to_string(),
                kind: entry.kind.to_string(),
                group: entry.group.to_string(),
                name: entry.name.to_string(),
                icon: entry.icon.to_string(),
                desc: entry.desc.to_string(),
                enables: entry.enables.to_string(),
                connected,
                app_id,
                label: label.clone(),
                connected_at,
            });
        }
        Ok(connections)
    }

    /// Forget a stored credential. The loop stops acting as that provider, so the
    /// next attempt goes back to what the machine has until one is stored again.
    pub async fn clear_connection(&self, id: &str) -> Result<Connection> {
        let entry = crate::connectors::entry(id)
            .ok_or_else(|| anyhow!("unknown connector `{id}`"))?
            .to_owned();
        sqlx::query("delete from connectors where provider = ? and kind = ?")
            .bind(entry.provider)
            .bind(entry.kind)
            .execute(&self.db.pool)
            .await?;
        Ok(Connection {
            id: entry.id.to_string(),
            provider: entry.provider.to_string(),
            kind: entry.kind.to_string(),
            group: entry.group.to_string(),
            name: entry.name.to_string(),
            icon: entry.icon.to_string(),
            desc: entry.desc.to_string(),
            enables: entry.enables.to_string(),
            connected: false,
            app_id: None,
            label: None,
            connected_at: None,
        })
    }

    async fn write_connector(
        &self,
        provider: &str,
        kind: &str,
        payload: &str,
        label: Option<&str>,
    ) -> Result<()> {
        let now = crate::db::now_rfc3339();
        sqlx::query(
            "insert into connectors (id, provider, kind, secret_ciphertext, label, created_at, updated_at)
             values (?, ?, ?, ?, ?, ?, ?)
             on conflict(provider, kind) do update set
                 secret_ciphertext = excluded.secret_ciphertext,
                 label = excluded.label,
                 updated_at = excluded.updated_at",
        )
        .bind(Uuid::now_v7())
        .bind(provider)
        .bind(kind)
        .bind(payload)
        .bind(label)
        .bind(&now)
        .bind(&now)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Store (or replace) a GitHub app. The key is sealed before it reaches the
    /// database, and the app id with it: both identify the account the loop can
    /// act as, so neither belongs in a page a backup would copy in the clear.
    pub async fn set_github_app(&self, app_id: &str, private_key: &str) -> Result<Connection> {
        let app_id = app_id.trim();
        let private_key = private_key.trim();
        if app_id.is_empty() {
            bail!("a github app needs an app id; GitHub shows it next to the app name");
        }
        if private_key.is_empty() {
            bail!("a github app needs its private key; GitHub generates the .pem once");
        }
        let vault = self.vault()?;
        let payload = Self::sealed_payload(&[
            ("app_id", vault.seal_string(app_id)?),
            ("private_key", vault.seal_string(private_key)?),
        ]);
        self.write_connector(
            crate::github::PROVIDER,
            crate::connectors::KIND_GITHUB_APP,
            &payload,
            None,
        )
        .await?;
        self.refresh_slug(app_id, private_key).await;
        self.connection("github").await
    }

    /// Store an API key for an agent CLI. The key is sealed like every other
    /// secret, and the adapter it belongs to decides where it goes.
    pub async fn set_agent_key(&self, id: &str, key: &str) -> Result<Connection> {
        let entry = crate::connectors::entry(id)
            .ok_or_else(|| anyhow!("unknown connector `{id}`"))?
            .to_owned();
        let key = key.trim();
        if key.is_empty() {
            bail!(
                "a key is required; the CLI uses it as {} credentials",
                entry.provider
            );
        }
        if entry.kind != crate::connectors::KIND_API_KEY {
            bail!("`{}` takes a login, not an API key", entry.name);
        }
        let vault = self.vault()?;
        let payload = Self::sealed_payload(&[("secret", vault.seal_string(key)?)]);
        self.write_connector(entry.provider, entry.kind, &payload, Some("api key"))
            .await?;
        self.connection(entry.id).await
    }

    /// Store the credentials file a CLI wrote when it logged in. The contents are
    /// copied into every run's scratch home, so a subscription is used by a run and
    /// never by the host.
    pub async fn set_agent_subscription(&self, id: &str, contents: &str) -> Result<Connection> {
        let entry = crate::connectors::entry(id)
            .ok_or_else(|| anyhow!("unknown connector `{id}`"))?
            .to_owned();
        let contents = contents.trim();
        if !contents.starts_with('{') {
            bail!("paste the contents of the credentials file, which is JSON starting with `{{`");
        }
        if entry.kind != crate::connectors::KIND_SUBSCRIPTION {
            bail!("`{}` takes an API key, not a login", entry.name);
        }
        let Some(path) = crate::connectors::credentials_file(entry.provider) else {
            bail!("`{}` does not take a credentials file", entry.name);
        };
        let vault = self.vault()?;
        let payload = Self::sealed_payload(&[("contents", vault.seal_string(contents)?)]);
        self.write_connector(entry.provider, entry.kind, &payload, Some(path))
            .await?;
        self.connection(entry.id).await
    }

    /// Store a token a login minted, for the CLIs that accept one in the
    /// environment. `claude setup-token` is the case this exists for: a token that
    /// lasts a year and rotates nothing, unlike the session file it replaces.
    pub async fn set_agent_token(&self, id: &str, token: &str) -> Result<Connection> {
        let entry = crate::connectors::entry(id)
            .ok_or_else(|| anyhow!("unknown connector `{id}`"))?
            .to_owned();
        let token = token.trim();
        if token.len() < 20 {
            bail!("that does not look like a token; `claude setup-token` prints one line");
        }
        if entry.kind != crate::connectors::KIND_SUBSCRIPTION {
            bail!("`{}` takes an API key, not a token", entry.name);
        }
        let vault = self.vault()?;
        let payload = Self::sealed_payload(&[("token", vault.seal_string(token)?)]);
        self.write_connector(entry.provider, entry.kind, &payload, Some("oauth token"))
            .await?;
        self.connection(entry.id).await
    }

    /// One entry as the screen sees it.
    pub async fn connection(&self, id: &str) -> Result<Connection> {
        let all = self.connections().await?;
        all.into_iter()
            .find(|connection| connection.id == id)
            .ok_or_else(|| anyhow!("unknown connector `{id}`"))
    }

    /// Ask GitHub which app this key belongs to and remember the slug. Best
    /// effort: no network keeps the app, and the slug is learned on a later call.
    async fn refresh_slug(&self, app_id: &str, private_key: &str) {
        if !crate::github::can_sign(private_key) {
            return;
        }
        let Ok(jwt) = crate::github::mint_token(app_id, private_key) else {
            return;
        };
        let Ok(http) = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
        else {
            return;
        };
        match crate::github::fetch_slug(&http, &jwt).await {
            Ok(slug) => {
                if let Err(error) =
                    sqlx::query("update connectors set label = ? where provider = ? and kind = ?")
                        .bind(&slug)
                        .bind(crate::github::PROVIDER)
                        .bind(crate::connectors::KIND_GITHUB_APP)
                        .execute(&self.db.pool)
                        .await
                {
                    tracing::warn!(%error, "could not remember the app slug");
                }
            }
            Err(error) => {
                tracing::warn!(%error, "github did not name the app; the slug stays unknown");
            }
        }
    }

    pub async fn commit_identity(&self) -> Result<(Option<String>, Option<String>)> {
        let Some(app) = self
            .stored_app(crate::github::PROVIDER, crate::connectors::KIND_GITHUB_APP)
            .await?
        else {
            return Ok((None, None));
        };
        if !crate::github::can_sign(&app.private_key) {
            return Ok((None, None));
        }
        let slug = match app.slug.clone() {
            Some(slug) => Some(slug),
            // A stored app can predate the slug, and the slug is what makes the
            // commit address a GitHub bot instead of an unlinked string.
            None => {
                self.refresh_slug(&app.app_id, &app.private_key).await;
                self.stored_app(crate::github::PROVIDER, crate::connectors::KIND_GITHUB_APP)
                    .await?
                    .and_then(|app| app.slug)
            }
        };
        Ok((Some(app.app_id.clone()), slug))
    }

    /// The token a run acts as the app with. `None` when no app is stored or the
    /// stored key cannot sign, because handing out a token nobody can verify is
    /// worse than handing out nothing.
    pub async fn github_token(&self) -> Result<Option<String>> {
        let Some(app) = self
            .stored_app(crate::github::PROVIDER, crate::connectors::KIND_GITHUB_APP)
            .await?
        else {
            return Ok(None);
        };
        if !crate::github::can_sign(&app.private_key) {
            return Ok(None);
        }
        crate::github::mint_token(&app.app_id, &app.private_key).map(Some)
    }

    // ------------------------------------------------------------------ runs

    async fn live_run_on_issue(&self, issue_id: Uuid) -> Result<Option<Run>> {
        Ok(sqlx::query_as::<_, Run>(
            "select * from runs where issue_id = ? and status in ('queued','running') limit 1",
        )
        .bind(issue_id)
        .fetch_optional(&self.db.pool)
        .await?)
    }

    async fn live_run_on_repo(&self, repo_id: Uuid) -> Result<Option<Run>> {
        Ok(sqlx::query_as::<_, Run>(
            "select * from runs where repo_id = ? and status in ('queued','running') limit 1",
        )
        .bind(repo_id)
        .fetch_optional(&self.db.pool)
        .await?)
    }

    /// Any agent in this workspace: the one whose last run is oldest, so a status
    /// that says `any` gets a rotation through the roster instead of one
    /// favourite. The workflow names people by name; this is what `any` means
    /// when there is no name to look up.
    pub async fn any_agent(&self, workspace_id: Uuid) -> Result<Option<Uuid>> {
        let row: Option<(Uuid,)> = sqlx::query_as(
            "select a.id from agents a
               left join (select agent_id, max(created_at) as ran_at from runs group by agent_id) r
                 on r.agent_id = a.id
              where a.workspace_id = ?
              order by coalesce(r.ran_at, '') asc, a.name asc
              limit 1",
        )
        .bind(workspace_id)
        .fetch_optional(&self.db.pool)
        .await?;
        Ok(row.map(|(agent_id,)| agent_id))
    }

    pub async fn start_run(
        &self,
        issue_id: Uuid,
        agent_id: Option<Uuid>,
        parent_run_id: Option<Uuid>,
    ) -> Result<Run> {
        let issue = self.issue(issue_id).await?;
        // A task with no repository still runs: the agent works in an empty
        // scratch directory, and its output is what it says and attaches.
        let repo: Option<Repo> = match issue.repo_id {
            Some(repo_id) => Some(
                sqlx::query_as("select * from repos where id = ?")
                    .bind(repo_id)
                    .fetch_one(&self.db.pool)
                    .await?,
            ),
            None => None,
        };

        // The workflow decides who works a status. When it names an agent, that
        // agent runs; `any` means whichever agent in the workspace has waited
        // longest. A status that names nobody starts nothing.
        let named = self.next_turn_agent(&issue).await?;
        let by_name = self
            .agent_for_name(issue.workspace_id, named.as_deref())
            .await?;
        let by_name = match (by_name, named.as_deref()) {
            (None, Some(name)) if name.eq_ignore_ascii_case("any") => {
                self.any_agent(issue.workspace_id).await?
            }
            (found, _) => found,
        };
        let agent_id = match (agent_id, by_name) {
            (Some(id), _) => id,
            (None, Some(id)) => id,
            // Nobody to run. Which of the two reasons it is decides what the
            // person reading the task has to do about it.
            (None, None) => {
                let queued: Vec<String> =
                    serde_json::from_str(&issue.turn_queue).unwrap_or_default();
                if !queued.is_empty() {
                    bail!(
                        "every agent the workflow names for {} has taken a turn there, so nothing starts until a person moves the task.",
                        issue.status_enum().title()
                    );
                }
                match named.as_deref() {
                    Some(name) => bail!(
                        "the workflow names {name} for {}, and no agent in this workspace answers to that name. Rename the agent, or name the agent the document means.",
                        issue.status_enum().title()
                    ),
                    None => bail!(
                        "the workflow names nobody for {}, so nothing starts. Give the status a `Users:` line naming an agent or `human`.",
                        issue.status_enum().title()
                    ),
                }
            }
        };

        // A probe is the only evidence about the CLI, so a run without one would
        // be a bet on an unknown binary. Probes run inside a checkout, so a
        // repo-less task has nothing to probe against and skips the gate.
        let preflight: Option<(i64, Option<String>)> = match issue.repo_id {
            Some(repo_id) => {
                sqlx::query_as(
                    "select ok, detail from agent_preflight where agent_id = ? and repo_id = ?",
                )
                .bind(agent_id)
                .bind(repo_id)
                .fetch_optional(&self.db.pool)
                .await?
            }
            None => Some((1, None)),
        };
        match (preflight, issue.repo_id) {
            (None, Some(repo_id)) => {
                let agent = self.agent(agent_id).await?;
                self.queue_job(
                    repo_id,
                    Some(issue_id),
                    "preflight",
                    serde_json::json!({
                        "agent_id": agent_id,
                        "adapter": agent.adapter,
                        "model": agent.model,
                    }),
                )
                .await?;
                bail!(
                    "agent {agent_id} has not passed preflight on this repo yet. A probe was queued; retry when Settings shows it green."
                );
            }
            (Some((0, detail)), _) => bail!(
                "agent {agent_id} failed its preflight: {}. Fix the adapter setup in Settings, then re-run the probe.",
                detail.unwrap_or_default()
            ),
            _ => {}
        }

        if let Some(live) = self.live_run_on_issue(issue_id).await? {
            bail!("a run is already active on this issue ({})", live.id);
        }
        // Repo-less runs share nothing, so only runs on a repository queue up
        // behind each other.
        let live_on_repo = match issue.repo_id {
            Some(repo_id) => self.live_run_on_repo(repo_id).await?,
            None => None,
        };
        if let Some(live) = live_on_repo {
            bail!(
                "another run is active on this repo ({}). One agent per repo keeps the jj op log and the build cache uncontended; start this run when that one finishes.",
                live.id
            );
        }

        // The cap counts runs since the last human override, so clearing it is a
        // deliberate act rather than a side effect of a new attempt.
        let count = self.runs_since_cap(&issue).await?;
        if count >= hulahoop_core::RUNS_PER_ISSUE {
            bail!(
                "this issue has used {count} of {} runs. Human override required: call clearRunCap to continue.",
                hulahoop_core::RUNS_PER_ISSUE
            );
        }

        // The model is frozen onto the run: a roster edit later must not rewrite
        // what a past run cost.
        let run_agent = self.agent(agent_id).await?;

        // A turn starts where the turn before it ended. The parent is the run the
        // workflow handed over from, so its head is the base; with no parent the
        // repository's own base bookmark is. A turn that reads and changes nothing
        // publishes nothing, so the next turn in line still starts from the last
        // head that carried a change. A repo-less run has no bookmarks at all.
        let repo_base = repo
            .as_ref()
            .map(|repo| repo.base_bookmark.clone())
            .unwrap_or_default();
        let base_bookmark = match parent_run_id {
            Some(parent_id) => {
                let parent: Run = sqlx::query_as("select * from runs where id = ?")
                    .bind(parent_id)
                    .fetch_optional(&self.db.pool)
                    .await?
                    .ok_or_else(|| anyhow!("parent run {parent_id} not found"))?;
                match parent.head_bookmark.clone() {
                    Some(head) => head,
                    // A review publishes nothing, and neither does a turn that only
                    // asked. The work still sits on the newest head the issue has, so
                    // a later turn starts there instead of on the repository's base.
                    None => self
                        .newest_head(issue_id)
                        .await?
                        .unwrap_or_else(|| repo_base.clone()),
                }
            }
            None => repo_base.clone(),
        };
        let attempt = issue.attempt + 1;

        let run_id = Uuid::now_v7();
        let artifact_dir = contract::run_artifact_dir(&self.db.data_dir, &run_id);
        std::fs::create_dir_all(&artifact_dir)?;
        // The revision names what the agent was told: the binary's version and the
        // instructions it carried. A roster edit that rewrites them changes it.
        let prompt_revision = format!(
            "{}+{}",
            env!("CARGO_PKG_VERSION"),
            sha256_hex(run_agent.agents_md.as_bytes())
        );

        let price = crate::models::price_per_token(&run_agent.model);
        sqlx::query(
            "insert into runs (id, issue_id, repo_id, agent_id, model, adapter_snapshot, connector_id,
                price_in_per_million, price_out_per_million, attempt, base_bookmark, base_revision,
                prompt_revision, artifact_dir, parent_run_id, status, created_at)
             values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'queued', ?)",
        )
        .bind(run_id)
        .bind(issue_id)
        .bind(issue.repo_id)
        .bind(agent_id)
        .bind(&run_agent.model)
        .bind(&run_agent.adapter)
        .bind(&run_agent.connector)
        .bind(price.map(|(input, _)| input * 1_000_000.0))
        .bind(price.map(|(_, output)| output * 1_000_000.0))
        .bind(attempt)
        .bind(&base_bookmark)
        // The base is a revision expression, not a frozen hash: a turn starts on the
        // bookmark its predecessor published, and resolving it here would mean a
        // second process to ask jj. Storing the bookmark keeps the run on the head
        // that was reviewed, even after another attempt moves it. A repo-less run
        // has no revision to start from.
        .bind(repo.as_ref().map(|_| &base_bookmark))
        .bind(&prompt_revision)
        .bind(artifact_dir.display().to_string())
        // The parent is how the next turn reads the one before it (a review's
        // findings, a handover); dropping it left reworks without the review.
        .bind(parent_run_id)
        .bind(crate::db::now_rfc3339())
        .execute(&self.db.pool)
        .await?;

        // The agent that picks a task up assigns itself, so the board shows who
        // has it. This is a label for people: dispatch reads WORKFLOWS.md, never
        // the assignee, and a person may reassign at any time.
        sqlx::query(
            "update issues set run_count = run_count + 1, attempt = ?, assignee_type = 'agent', assignee_id = ?, updated_at = ? where id = ?",
        )
        .bind(issue.attempt + 1)
        .bind(agent_id.to_string())
        .bind(crate::db::now_rfc3339())
        .bind(issue_id)
        .execute(&self.db.pool)
        .await?;

        // Starting a run never moves the task. An agent works *in* the status it
        // was dispatched for — `To do` stays `To do` while the Architect plans —
        // and the move happens when the run finishes and the workflow hands over.
        // The old pipeline advanced on Start; with turns that would skip the rest
        // of a status's team.
        self.run(run_id).await
    }

    pub async fn agent(&self, id: Uuid) -> Result<crate::model::Agent> {
        sqlx::query_as::<_, crate::model::Agent>("select * from agents where id = ?")
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await?
            .ok_or_else(|| anyhow!("agent {id} not found"))
    }

    pub async fn run(&self, id: Uuid) -> Result<Run> {
        sqlx::query_as::<_, Run>("select * from runs where id = ?")
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await?
            .ok_or_else(|| anyhow!("run {id} not found"))
    }

    pub async fn cancel_run(&self, run_id: Uuid) -> Result<Run> {
        let run = self.run(run_id).await?;
        if !run.status_enum().is_live() {
            bail!("run {run_id} is already {}", run.status_enum());
        }
        if let Some(pgid) = run.pgid {
            kill_process_group(pgid as i32);
        }
        // The stream the run already wrote is the only record of what it spent. A
        // cancelled run reports it instead of reading as a run that spent nothing.
        let usage = spent_metrics(std::path::Path::new(&run.artifact_dir), None).await;
        let (tokens_in, tokens_out) = (usage.input, usage.output);
        sqlx::query("update runs set status = 'cancelled', finished_at = ?, error = 'cancelled by a human', tokens_in = ?, tokens_out = ? where id = ?")
            .bind(crate::db::now_rfc3339())
            .bind(tokens_in)
            .bind(tokens_out)
            .bind(run_id)
            .execute(&self.db.pool)
            .await?;
        persist_run_metrics(&self.db, &run, &usage, Some("cancelled by a human")).await?;
        // Cancelling a run leaves the issue where it was: a misclick costs one
        // run, never the issue.
        self.transition(run.issue_id, IssueEvent::CancelRun)
            .await
            .ok();
        self.run(run_id).await
    }

    // ------------------------------------------------------------------ gate

    pub async fn resume_issue(&self, issue_id: Uuid, answer: &str) -> Result<Issue> {
        let issue = self.issue(issue_id).await?;
        if issue.status_enum() != IssueStatus::Blocked {
            bail!("the issue is {}, not blocked", issue.status_enum());
        }
        let question = match issue.pinned_comment_id {
            Some(id) => {
                let row: Option<(String,)> =
                    sqlx::query_as("select body from comments where id = ?")
                        .bind(id)
                        .fetch_optional(&self.db.pool)
                        .await?;
                row.map(|(body,)| body).unwrap_or_default()
            }
            None => String::new(),
        };
        let body = if question.is_empty() {
            format!("Answer: {answer}")
        } else {
            format!("Answer to \"{question}\":\n\n{answer}")
        };
        self.comment(
            issue_id,
            hulahoop_core::AuthorType::Human,
            None,
            &body,
            false,
        )
        .await?;
        // The question stays pinned until a turn carries it: the pin is what puts
        // the pair in the next run's input, so it retires with the run that reads
        // it (`build_run_spec`), not with the answer that is only half of it.
        let restored = hulahoop_core::resume_to(
            issue
                .status_before_blocked
                .as_deref()
                .and_then(IssueStatus::parse),
        )
        .ok_or_else(|| anyhow!("this issue has no status to return to"))?;
        sqlx::query("update issues set status = ?, status_before_blocked = null, updated_at = ? where id = ?")
            .bind(restored.as_str())
            .bind(crate::db::now_rfc3339())
            .bind(issue_id)
            .execute(&self.db.pool)
            .await?;
        // An answer is a person's act, the same as a move: the status the task
        // returns to brings its own round of turns, so the queue belongs to that
        // status again and the question reaches the agent that started it.
        let _ = self.reset_turns(issue_id, restored).await;
        let issue = self.issue(issue_id).await?;
        if let Err(error) = Box::pin(self.dispatch_if_agent_owns(&issue)).await {
            let _ = self
                .comment(
                    issue_id,
                    hulahoop_core::AuthorType::Server,
                    None,
                    &format!("the answered turn could not start: {error}"),
                    false,
                )
                .await;
        }
        self.issue(issue_id).await
    }

    // ------------------------------------------------------------- runner api

    pub async fn queue_job(
        &self,
        repo_id: Uuid,
        issue_id: Option<Uuid>,
        kind: &str,
        payload: serde_json::Value,
    ) -> Result<Uuid> {
        let id = Uuid::now_v7();
        sqlx::query(
            "insert into runner_jobs (id, repo_id, issue_id, kind, payload, status, created_at)
             values (?, ?, ?, ?, ?, 'queued', ?)",
        )
        .bind(id)
        .bind(repo_id)
        .bind(issue_id)
        .bind(kind)
        .bind(payload.to_string())
        .bind(crate::db::now_rfc3339())
        .execute(&self.db.pool)
        .await?;
        Ok(id)
    }

    pub async fn claim_run(&self, runner_id: &str) -> Result<Option<Run>> {
        let candidate: Option<Run> = sqlx::query_as(
            "select * from runs where status = 'queued' order by created_at limit 1",
        )
        .fetch_optional(&self.db.pool)
        .await?;
        let Some(run) = candidate else {
            return Ok(None);
        };
        let now = Utc::now().to_rfc3339();
        let updated = sqlx::query(
            "update runs set status = 'running', runner_id = ?, claimed_at = ?, started_at = ?, last_heartbeat_at = ? where id = ? and status = 'queued'",
        )
        .bind(runner_id)
        .bind(&now)
        .bind(&now)
        .bind(&now)
        .bind(run.id)
        .execute(&self.db.pool)
        .await?;
        if updated.rows_affected() == 0 {
            return Ok(None);
        }
        self.run(run.id).await.map(Some)
    }

    pub async fn heartbeat(&self, run_id: Uuid, pid: Option<i64>, pgid: Option<i64>) -> Result<()> {
        sqlx::query("update runs set last_heartbeat_at = ?, pid = coalesce(?, pid), pgid = coalesce(?, pgid) where id = ?")
            .bind(crate::db::now_rfc3339())
            .bind(pid)
            .bind(pgid)
            .bind(run_id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    pub async fn append_event(
        &self,
        run_id: Uuid,
        kind: &str,
        payload: serde_json::Value,
    ) -> Result<()> {
        let (next,): (i64,) =
            sqlx::query_as("select coalesce(max(seq), 0) + 1 from run_events where run_id = ?")
                .bind(run_id)
                .fetch_one(&self.db.pool)
                .await?;
        sqlx::query("insert or ignore into run_events (id, run_id, seq, kind, payload, created_at) values (?, ?, ?, ?, ?, ?)")
            .bind(Uuid::now_v7())
            .bind(run_id)
            .bind(next)
            .bind(kind)
            .bind(payload.to_string())
            .bind(crate::db::now_rfc3339())
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    /// Records what a run reported and moves its task on from there.
    pub async fn complete_run(&self, run_id: Uuid, payload: RunCompletion) -> Result<Run> {
        let run = self.run(run_id).await?;
        let issue = self.issue(run.issue_id).await?;
        let status = RunStatus::parse(&payload.status)
            .ok_or_else(|| anyhow!("unknown run status {}", payload.status))?;

        // What the agent said when it finished, trimmed to a length a person will
        // read. Kept on the run, shown as a comment, handed to the next turn.
        let summary: Option<String> = {
            let text: String = payload
                .body
                .trim()
                .chars()
                .take(SUMMARY_CHARS)
                .collect::<String>();
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        };

        // The runner reports the paths it saw; the diff is the fallback because a
        // runner that crashed before naming them still left a patch behind.
        let changed_paths = if payload.changed_paths.is_empty() {
            diff::summarize(&payload.diff).files
        } else {
            payload.changed_paths.clone()
        };
        let verdict = diff::classify(&payload.diff);
        let diff_sha = sha256_hex(payload.diff.as_bytes());
        let artifact_dir = std::path::Path::new(&run.artifact_dir);
        std::fs::create_dir_all(artifact_dir)?;
        std::fs::write(artifact_dir.join("diff.patch"), &payload.diff)?;
        let usage = spent_metrics(artifact_dir, payload.transcript.as_deref()).await;
        let (tokens_in, tokens_out) = (usage.input, usage.output);

        let mut flags = verdict.flags.clone();
        if payload.artifact_limit_hit {
            flags.push(RunFlag::ArtifactLimit);
        }
        let flags_json =
            serde_json::to_string(&flags.iter().map(|f| f.as_str()).collect::<Vec<_>>())?;

        let mut final_status = status;
        let mut error = payload.error.clone();

        // A run that changed nothing did no work. pi exits 0 even when its provider
        // rejected the model, so "the process ended" is not evidence that the turn ran;
        // without this, the issue sits at tests_in_progress offering a suite to freeze
        // that was never written.
        let asked = payload
            .question
            .as_deref()
            .is_some_and(|question| !question.trim().is_empty());
        let answered = payload
            .verdict
            .as_deref()
            .is_some_and(|verdict| !verdict.trim().is_empty());
        // A task with no repository changes no files by design: its output is
        // what it says, so a closing message is the work.
        let reported = run.repo_id.is_none() && summary.is_some();
        if final_status == RunStatus::Succeeded
            && changed_paths.is_empty()
            && !asked
            && !answered
            && !reported
        {
            final_status = RunStatus::Failed;
            error = Some(match adapter_failure(&payload) {
                Some(reason) => format!(
                    "the agent changed no files: {reason}. Nothing was run for this turn, so the issue stays where it is; fix the agent in Settings and retry."
                ),
                None if run.repo_id.is_none() => "the agent said nothing. A task with no repository reports its work in the closing message, so a silent turn did no work; check its transcript in the run log, then retry.".to_string(),
                None => "the agent changed no files and said nothing. A turn that edits nothing did no work; check its transcript in the run log, then retry.".to_string(),
            });
        }
        if final_status == RunStatus::Succeeded && payload.artifact_limit_hit {
            final_status = RunStatus::Failed;
            error = Some(format!(
                "the run wrote more than {} MB of artifacts and was stopped",
                hulahoop_core::ARTIFACT_LIMIT_BYTES / (1024 * 1024)
            ));
        }

        let finished = Utc::now().to_rfc3339();
        sqlx::query(
            "update runs set status = ?, exit_code = ?, error = ?, diff_sha = ?, flags = ?,
                changed_paths = ?, head_revision = coalesce(?, head_revision),
                commit_name = ?, commit_email = ?, tokens_in = ?, tokens_out = ?,
                summary = ?, finished_at = ? where id = ?",
        )
        .bind(final_status.as_str())
        .bind(payload.exit_code)
        .bind(&error)
        .bind(&diff_sha)
        .bind(&flags_json)
        .bind(serde_json::to_string(&changed_paths)?)
        .bind(&payload.commit)
        .bind(&payload.commit_name)
        .bind(&payload.commit_email)
        .bind(tokens_in)
        .bind(tokens_out)
        .bind(summary.as_deref())
        .bind(&finished)
        .bind(run_id)
        .execute(&self.db.pool)
        .await?;
        let category_reason = if final_status == RunStatus::TimedOut {
            Some("timed out")
        } else {
            error.as_deref()
        };
        persist_run_metrics(&self.db, &run, &usage, category_reason).await?;

        // What a turn *was* is read from what it did. A closing message that
        // carries a verdict judged the work before it; anything else is work, and
        // its commit becomes the head the next turn starts from and the pull
        // request carries. Who the agent is decides none of this.
        //
        // A verdict is only a review when there is something behind the turn to
        // judge: the first turn of a status has no work before it, so its closing
        // line is a statement about its own work, and the task moves on.
        let judged = matches!(
            payload.verdict.as_deref(),
            Some("approved") | Some("changes_requested")
        ) && self.run_under_review(&issue, Some(run.id)).await.is_ok();

        // An agent that worked and said nothing leaves the next turn guessing. Its
        // closing message is the handover, so it is shown where a person reads the
        // task and handed to whoever works it next. A judgement is not a handover:
        // it is stored as the review it was, in the words the reviewer wrote.
        if !judged && final_status == RunStatus::Succeeded {
            if let Some(text) = summary.as_deref() {
                self.comment_on_run(
                    run.issue_id,
                    hulahoop_core::AuthorType::Agent,
                    Some(&run.agent_id.to_string()),
                    text,
                    false,
                    Some(run.id),
                )
                .await?;
            }
        }

        if let Some(question) = payload.question.as_deref().filter(|q| !q.trim().is_empty()) {
            self.comment_on_run(
                run.issue_id,
                hulahoop_core::AuthorType::Agent,
                Some(&run.agent_id.to_string()),
                question,
                true,
                Some(run.id),
            )
            .await?;
            self.transition(run.issue_id, IssueEvent::Blocked).await?;
            return self.run(run_id).await;
        }

        if judged {
            if final_status == RunStatus::Succeeded {
                let verdict = self.record_review(&issue, &run, &payload).await?;
                // A rejection keeps the task in its status and starts the round
                // again: the document's first agent reworks, the reviewer judges
                // the new head. `Turns:` is what stops the loop.
                if verdict == "approved" {
                    self.advance_after_turn(run.issue_id, run.agent_id).await?;
                } else {
                    self.hand_back_for_rework(run.issue_id).await?;
                }
            } else {
                // A review that never finished judged nothing, so the task waits
                // for a person or a retry.
                let _ = self.transition(run.issue_id, IssueEvent::CancelRun).await;
            }
        } else {
            match final_status {
                RunStatus::Succeeded => {
                    if payload.commit.is_some() {
                        let bookmark = format!("issue/{}/head/{}", issue.id, run.attempt);
                        // The revision is what the reviewer and the pull request
                        // point at, so it is recorded with the bookmark that names
                        // it. A turn that changed nothing published nothing, and its
                        // base stays the head the work is actually on.
                        sqlx::query("update runs set head_bookmark = ?, head_revision = coalesce(?, head_revision) where id = ?")
                            .bind(&bookmark)
                            .bind(payload.commit.as_deref())
                            .bind(run_id)
                            .execute(&self.db.pool)
                            .await?;
                    }
                    // The newest work is what the next reviewer judges and what a
                    // pull request shows. Without this the task keeps pointing at
                    // the head from its last trip through review: the reviewer
                    // would judge stale work, or be held back as having judged it.
                    if payload.commit.is_some() || issue.repo_id.is_none() {
                        sqlx::query("update issues set current_run_id = ? where id = ?")
                            .bind(run_id)
                            .bind(run.issue_id)
                            .execute(&self.db.pool)
                            .await?;
                    }
                    // Where the work goes next is the document's decision, not a
                    // constant in here: take the status's first move. A move into
                    // review is what earns a pull request — everything else is just
                    // the next step of the flow, and the run that lands there is the
                    // one a reviewer will be shown.
                    // The turn is spent. If the status names another agent who has
                    // not worked it yet, the task stays here and the queue moves on;
                    // only a status nobody is owed a turn in hands the task over.
                    self.advance_after_turn(run.issue_id, run.agent_id).await?;
                }
                _ => {
                    // A failed attempt leaves the task where it is so the human, or
                    // a retry, decides what happens next.
                    let _ = self.transition(run.issue_id, IssueEvent::CancelRun).await;
                }
            }
        }

        self.run(run_id).await
    }

    /// Runs the task has used since a person last allowed more. Every run counts,
    /// failed ones too, across every agent and status.
    pub async fn runs_since_cap(&self, issue: &Issue) -> Result<i64> {
        let since = issue
            .run_cap_cleared_at
            .map(|t| t.to_rfc3339())
            .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string());
        let (count,): (i64,) =
            sqlx::query_as("select count(*) from runs where issue_id = ? and created_at >= ?")
                .bind(issue.id)
                .bind(&since)
                .fetch_one(&self.db.pool)
                .await?;
        Ok(count)
    }

    /// A person allows another round of runs. The count starts again, and the
    /// agent the workflow was waiting on starts now instead of on the next
    /// event, which may never come.
    pub async fn clear_run_cap(&self, issue_id: Uuid) -> Result<Issue> {
        let now = crate::db::now_rfc3339();
        sqlx::query("update issues set run_cap_cleared_at = ?, updated_at = ? where id = ?")
            .bind(&now)
            .bind(&now)
            .bind(issue_id)
            .execute(&self.db.pool)
            .await?;
        let issue = self.issue(issue_id).await?;
        Box::pin(self.dispatch_if_agent_owns(&issue)).await?;
        self.issue(issue_id).await
    }

    /// A review asked for changes: everybody in the status is owed a turn again,
    /// in the document's order, and the first of them starts now.
    async fn hand_back_for_rework(&self, issue_id: Uuid) -> Result<()> {
        sqlx::query("update issues set turn_served = '[]' where id = ?")
            .bind(issue_id)
            .execute(&self.db.pool)
            .await?;
        let issue = self.issue(issue_id).await?;
        if let Err(error) = Box::pin(self.dispatch_if_agent_owns(&issue)).await {
            let _ = self
                .comment(
                    issue_id,
                    hulahoop_core::AuthorType::Server,
                    None,
                    &format!(
                        "the review asked for changes, but the rework could not start: {error}"
                    ),
                    false,
                )
                .await;
        }
        Ok(())
    }

    /// What happens when a turn is spent and the work is not blocked: the status
    /// hands over to the next agent it names, or to the first move the document
    /// declares out of it. A handover into review is what earns a pull request,
    /// and the reviewer is shown the run that put it there.
    async fn advance_after_turn(&self, issue_id: Uuid, agent_id: Uuid) -> Result<()> {
        let issue = self.issue(issue_id).await?;
        let _ = self.mark_served(&issue, agent_id).await;
        let issue = self.issue(issue_id).await?;
        if let Some(next) = self.next_turn_agent(&issue).await? {
            if let Err(error) = Box::pin(self.dispatch_if_agent_owns(&issue)).await {
                let _ = self
                    .comment(
                        issue.id,
                        hulahoop_core::AuthorType::Server,
                        None,
                        &format!("the next agent could not start: {error}"),
                        false,
                    )
                    .await;
                return Ok(());
            }
            self.block_if_stalled(issue_id, agent_id, &next).await?;
            return Ok(());
        }
        let target = self
            .workflow_handoff(issue.workspace_id, issue.status_enum())
            .await?
            .unwrap_or(IssueStatus::InReview);
        if target == IssueStatus::InReview {
            // A repo-less task publishes no heads; its newest finished turn is
            // the work a reviewer reads.
            let query = if issue.repo_id.is_some() {
                "update issues set current_run_id = (select id from runs where issue_id = ? and status = 'succeeded' and head_bookmark is not null order by created_at desc limit 1) where id = ?"
            } else {
                "update issues set current_run_id = (select id from runs where issue_id = ? and status = 'succeeded' order by created_at desc limit 1) where id = ?"
            };
            sqlx::query(query)
                .bind(issue_id)
                .bind(issue_id)
                .execute(&self.db.pool)
                .await?;
        }
        let issue = self.move_to(issue_id, target).await?;
        if issue.status_enum() == IssueStatus::InReview {
            self.ensure_pull_request(&issue).await?;
        }
        Ok(())
    }

    /// A handover that started nobody leaves the task idle in a status agents
    /// own, with nothing on the board to say so. Waiting helps nobody, so the
    /// task goes to a person with the reason. The usual case: a rework that
    /// changed nothing, handing the reviewer the head it already judged.
    async fn block_if_stalled(&self, issue_id: Uuid, agent_id: Uuid, next: &str) -> Result<()> {
        let issue = self.issue(issue_id).await?;
        if issue.status_enum() == IssueStatus::Blocked || issue.status_enum().is_terminal() {
            return Ok(());
        }
        let live: Option<(Uuid,)> = sqlx::query_as(
            "select id from runs where issue_id = ? and status in ('queued', 'running')",
        )
        .bind(issue_id)
        .fetch_optional(&self.db.pool)
        .await?;
        if live.is_some() {
            return Ok(());
        }
        let finished: String = sqlx::query_scalar("select name from agents where id = ?")
            .bind(agent_id)
            .fetch_optional(&self.db.pool)
            .await?
            .unwrap_or_else(|| "The last agent".to_string());
        let next = next.trim().trim_start_matches('@');
        let judged = match (
            issue.current_run_id,
            self.agent_for_name(issue.workspace_id, Some(next)).await?,
        ) {
            (Some(head), Some(judge)) => sqlx::query_scalar::<_, String>(
                "select reviews.verdict from reviews join runs on runs.id = reviews.reviewer_run_id
                  where reviews.reviewed_run_id = ? and runs.agent_id = ?
                  order by reviews.created_at desc limit 1",
            )
            .bind(head)
            .bind(judge)
            .fetch_optional(&self.db.pool)
            .await?,
            _ => None,
        };
        let reason = match judged.as_deref() {
            Some("changes_requested") => format!(
                "{finished} finished without changing the work {next} already reviewed, so {next} has nothing new to judge and its change request is still open."
            ),
            Some(_) => format!(
                "{finished} finished without changing the work {next} already reviewed, so {next} has nothing new to judge."
            ),
            None => format!("The workflow names {next} next, but no turn started."),
        };
        self.comment(
            issue_id,
            hulahoop_core::AuthorType::Server,
            None,
            &format!("{reason} Somebody has to look at it."),
            false,
        )
        .await?;
        self.move_to(issue_id, IssueStatus::Blocked).await?;
        Ok(())
    }

    /// The pull request the work needs, opened once.
    ///
    /// A review that approves is the moment the work is ready for a person to read
    /// on GitHub, so approval opens the pull request even when the task stays in its
    /// column. The turn named is the newest one that published a head: a reviewer
    /// publishes none, and naming it asks the runner to push a branch that does not
    /// exist.
    async fn ensure_pull_request(&self, issue: &Issue) -> Result<()> {
        // A task with no repository has nothing to push and nowhere to open a
        // pull request; the transition is the whole handover.
        let Some(repo_id) = issue.repo_id else {
            return Ok(());
        };
        if issue.pr_number.is_some() {
            return Ok(());
        }
        let opened: i64 = sqlx::query_scalar(
            "select count(*) from runner_jobs
              where issue_id = ? and kind = 'open_pr'
                and status in ('queued', 'running', 'succeeded')",
        )
        .bind(issue.id)
        .fetch_one(&self.db.pool)
        .await?;
        if opened > 0 {
            return Ok(());
        }
        let head: Option<Uuid> = sqlx::query_scalar(
            "select id from runs
              where issue_id = ? and status = 'succeeded' and head_bookmark is not null
              order by created_at desc limit 1",
        )
        .bind(issue.id)
        .fetch_optional(&self.db.pool)
        .await?;
        let Some(run_id) = head else {
            self.comment(
                issue.id,
                hulahoop_core::AuthorType::Server,
                None,
                "no turn published a head, so there is nothing to open a pull request from.",
                false,
            )
            .await?;
            return Ok(());
        };
        self.queue_job(
            repo_id,
            Some(issue.id),
            "open_pr",
            serde_json::json!({ "run_id": run_id }),
        )
        .await?;
        Ok(())
    }

    /// The agent whose head became the pull request links it on the task, with
    /// what it did, so a person reading the thread finds the PR where the work
    /// was discussed. Posted once per pull request number.
    async fn announce_pull_request(
        &self,
        issue_id: Uuid,
        run_id: Option<Uuid>,
        number: i64,
        url: Option<&str>,
    ) -> Result<()> {
        let headline = match url {
            Some(url) => format!("Opened pull request #{number}: {url}"),
            None => format!("Opened pull request #{number}"),
        };
        let already: i64 = sqlx::query_scalar(
            "select count(*) from comments where issue_id = ? and body like ? || '%'",
        )
        .bind(issue_id)
        .bind(format!("Opened pull request #{number}"))
        .fetch_one(&self.db.pool)
        .await?;
        if already > 0 {
            return Ok(());
        }
        let run = match run_id {
            Some(id) => self.run(id).await.ok(),
            None => None,
        };
        let summary = run
            .as_ref()
            .and_then(|run| run.summary.as_deref())
            .map(str::trim)
            .filter(|summary| !summary.is_empty());
        let body = match summary {
            Some(summary) => format!("{headline}\n\n{summary}"),
            None => headline,
        };
        match run {
            Some(run) => {
                self.comment_on_run(
                    issue_id,
                    hulahoop_core::AuthorType::Agent,
                    Some(&run.agent_id.to_string()),
                    &body,
                    false,
                    Some(run.id),
                )
                .await?;
            }
            None => {
                self.comment(
                    issue_id,
                    hulahoop_core::AuthorType::Server,
                    None,
                    &body,
                    false,
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn record_review(
        &self,
        issue: &Issue,
        run: &Run,
        payload: &RunCompletion,
    ) -> Result<String> {
        // A reviewer that says nothing reviewed nothing; treat silence as a
        // request for changes so the issue cannot advance on a missing verdict.
        let verdict = payload
            .verdict
            .clone()
            .unwrap_or_else(|| "changes_requested".to_string());
        let reviewed_run_id = self.run_under_review(issue, Some(run.id)).await?;

        sqlx::query(
            "insert into reviews (id, issue_id, reviewed_run_id, reviewer_run_id, verdict, body, created_at)
             values (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::now_v7())
        .bind(issue.id)
        .bind(reviewed_run_id)
        .bind(run.id)
        .bind(&verdict)
        .bind(&payload.body)
        .bind(crate::db::now_rfc3339())
        .execute(&self.db.pool)
        .await?;

        // The pull request exists before the review does, so the verdict is posted
        // where a human reads it, not only inside Hulahoop.
        if let (Some(number), Some(repo_id)) = (issue.pr_number, issue.repo_id) {
            self.queue_job(
                repo_id,
                Some(issue.id),
                "review_pr",
                serde_json::json!({
                    "number": number,
                    "verdict": verdict,
                    "body": payload.body,
                }),
            )
            .await?;
        }

        if verdict == "approved" {
            // Approval is advice the human reads; it does not move the task, and it
            // does put the work where the person reads it: the pull request.
            self.ensure_pull_request(issue).await?;
        } else {
            self.comment_on_run(
                issue.id,
                hulahoop_core::AuthorType::Agent,
                Some(&run.agent_id.to_string()),
                &format!("Reviewer requested changes:\n\n{}", payload.body),
                false,
                Some(run.id),
            )
            .await?;
            let _ = self.transition(issue.id, IssueEvent::RequestChanges).await;
        }
        Ok(verdict)
    }

    /// Records what a runner job reported and applies what its kind changes: a
    /// fetch moves the repository's base revision, a pull request job stores the
    /// number it opened, and so on. A cleanup has nothing to record.
    pub async fn complete_job(
        &self,
        job_id: Uuid,
        payload: serde_json::Value,
        error: Option<String>,
    ) -> Result<()> {
        let job: RunnerJob = sqlx::query_as("select * from runner_jobs where id = ?")
            .bind(job_id)
            .fetch_optional(&self.db.pool)
            .await?
            .ok_or_else(|| anyhow!("job {job_id} not found"))?;

        let status = if error.is_some() {
            "failed"
        } else {
            "succeeded"
        };
        let mut tx = self.db.pool.begin().await?;
        let updated = sqlx::query("update runner_jobs set status = ?, result = ?, error = ?, finished_at = ? where id = ? and status = 'running'")
            .bind(status)
            .bind(payload.to_string())
            .bind(&error)
            .bind(crate::db::now_rfc3339())
            .bind(job_id)
            .execute(&mut *tx)
            .await?;
        if updated.rows_affected() == 0 {
            return Ok(());
        }
        if job.kind == "lumi" {
            let job_payload: serde_json::Value =
                serde_json::from_str(&job.payload).unwrap_or(serde_json::Value::Null);
            self.lumi_complete(job_id, &job_payload, &payload, error.as_deref(), &mut tx)
                .await?;
        }
        tx.commit().await?;

        match job.kind.as_str() {
            "fetch" => {
                if let (Some(revision), Some(job_repo)) = (
                    payload.get("base_revision").and_then(|v| v.as_str()),
                    job.repo_id,
                ) {
                    // A fetch never moves the base back. A remote that is
                    // behind the base the runner compared with keeps it, and so
                    // does a base that changed while the fetch ran (a landing
                    // recorded in between): the fetch only knew the older one.
                    let behind = payload
                        .get("behind")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let compared_with = payload
                        .get("compared_with")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();
                    sqlx::query(
                        "update repos set base_revision = case
                           when ? or coalesce(base_revision, '') != ? then base_revision
                           else ? end,
                           fetched_at = ? where id = ?",
                    )
                    .bind(behind)
                    .bind(compared_with)
                    .bind(revision)
                    .bind(crate::db::now_rfc3339())
                    .bind(job_repo)
                    .execute(&self.db.pool)
                    .await?;
                }
            }
            "land" => self.landed(&job, &payload, error.as_deref()).await?,
            "preflight" => {
                if let (Some(agent_id), Some(ok), Some(detail), Some(job_repo)) = (
                    payload
                        .get("agent_id")
                        .and_then(|v| v.as_str())
                        .and_then(|raw| Uuid::parse_str(raw).ok()),
                    payload.get("ok").and_then(|v| v.as_bool()),
                    payload.get("detail").and_then(|v| v.as_str()),
                    job.repo_id,
                ) {
                    sqlx::query(
                        "insert into agent_preflight (agent_id, repo_id, ok, detail, checked_at)
                         values (?, ?, ?, ?, ?)
                         on conflict(agent_id, repo_id) do update set ok = excluded.ok, detail = excluded.detail, checked_at = excluded.checked_at",
                    )
                    .bind(agent_id)
                    .bind(job_repo)
                    .bind(ok)
                    .bind(detail)
                    .bind(crate::db::now_rfc3339())
                    .execute(&self.db.pool)
                    .await?;
                    // A task that was waiting on this probe can start now. Without
                    // this, an agent saved a minute after a task was planned leaves
                    // that task sitting in a status it is supposed to work.
                    if ok {
                        self.retry_owed_turns(job_repo, agent_id).await?;
                    }
                }
            }
            "open_pr" => {
                // The pull request exists on GitHub's side, so its number comes back
                // from the runner: without it the task cannot link the pull request a
                // person reads, and a merge has to fall back to the run's own head.
                if let (Some(number), Some(issue_id)) = (
                    payload.get("pr_number").and_then(|v| v.as_i64()),
                    job.issue_id,
                ) {
                    let before = self.issue(issue_id).await?.pr_number;
                    sqlx::query(
                        "update issues set pr_number = ?, pr_url = ?, pr_branch = ? where id = ?",
                    )
                    .bind(number)
                    .bind(payload.get("pr_url").and_then(|v| v.as_str()))
                    .bind(payload.get("pr_branch").and_then(|v| v.as_str()))
                    .bind(issue_id)
                    .execute(&self.db.pool)
                    .await?;
                    // A rework pushes to the same pull request; only a new one
                    // is news worth a comment.
                    if before != Some(number) {
                        let run_id = payload
                            .get("run_id")
                            .and_then(|v| v.as_str())
                            .and_then(|raw| Uuid::parse_str(raw).ok());
                        let url = payload.get("pr_url").and_then(|v| v.as_str());
                        self.announce_pull_request(issue_id, run_id, number, url)
                            .await?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Records how a landing (a merge without a pull request) went. Once the
    /// remote has the merged head it becomes the repository's base, so the next
    /// task starts on top of it. A landing that failed leaves the base where it
    /// was and says why on the task, since the merge is then only in Hulahoop.
    async fn landed(
        &self,
        job: &RunnerJob,
        payload: &serde_json::Value,
        error: Option<&str>,
    ) -> Result<()> {
        let revision = payload.get("base_revision").and_then(|v| v.as_str());
        match (error, revision, job.repo_id) {
            (None, Some(revision), Some(repo_id)) => {
                sqlx::query("update repos set base_revision = ? where id = ?")
                    .bind(revision)
                    .bind(repo_id)
                    .execute(&self.db.pool)
                    .await?;
            }
            _ => {
                if let Some(issue_id) = job.issue_id {
                    let reason = error.unwrap_or("the runner reported no landed revision");
                    let body = format!(
                        "The merge could not be pushed to the repository: {reason}. \
                         The base branch there is unchanged; land the work by hand or rework the task."
                    );
                    self.comment(
                        issue_id,
                        hulahoop_core::AuthorType::Server,
                        None,
                        &body,
                        false,
                    )
                    .await?;
                }
            }
        }
        Ok(())
    }

    pub async fn claim_job(&self, lumi: bool) -> Result<Option<RunnerJob>> {
        let candidate: Option<RunnerJob> = sqlx::query_as(
            // A cleanup waits while a run of its task is still live or was just
            // cancelled, so a turn never loses its checkout halfway; and the
            // cleanup of a finished task waits for the task to be finished.
            // Forgetting a mirror waits for every live run of the repository,
            // whose checkouts are workspaces of it.
            "select * from runner_jobs where status = 'queued' and (kind = 'lumi') = ?
               and not (kind = 'forget_mirror' and exists (
                 select 1 from runs where runs.repo_id = runner_jobs.repo_id
                   and runs.status in ('queued', 'running')))
               and not (kind = 'cleanup' and (
                 exists (
                   select 1 from runs where runs.issue_id = runner_jobs.issue_id
                     and (runs.status in ('queued', 'running')
                       or (runs.status = 'cancelled' and runs.finished_at > ?)))
                 or (json_extract(payload, '$.reason') is not ?
                   and not exists (
                     select 1 from issues where issues.id = runner_jobs.issue_id
                       and issues.status in ('done', 'cancelled')))))
             order by created_at limit 1",
        )
        .bind(lumi)
        .bind((Utc::now() - chrono::Duration::seconds(CLEANUP_AFTER_CANCEL_SECS)).to_rfc3339())
        .bind(CLEANUP_MOVED)
        .fetch_optional(&self.db.pool)
        .await?;
        let Some(job) = candidate else {
            return Ok(None);
        };
        let updated = sqlx::query(
            "update runner_jobs set status = 'running', claimed_at = ? where id = ? and status = 'queued'",
        )
        .bind(crate::db::now_rfc3339())
        .bind(job.id)
        .execute(&self.db.pool)
        .await?;
        if updated.rows_affected() == 0 {
            return Ok(None);
        }
        sqlx::query_as::<_, RunnerJob>("select * from runner_jobs where id = ?")
            .bind(job.id)
            .fetch_optional(&self.db.pool)
            .await
            .map_err(Into::into)
    }

    // ------------------------------------------------------------ housekeeping

    /// A crashed runner cannot acknowledge its claimed question. Allow the
    /// harness's ten-minute wall clock plus two minutes for setup and transport.
    pub async fn reap_lumi_jobs(&self) -> Result<()> {
        let cutoff = (Utc::now() - chrono::Duration::minutes(12)).to_rfc3339();
        let stale: Vec<(Uuid,)> = sqlx::query_as(
            "select id from runner_jobs where kind = 'lumi' and status = 'running' and claimed_at < ?",
        )
        .bind(cutoff)
        .fetch_all(&self.db.pool)
        .await?;
        for (id,) in stale {
            self.complete_job(
                id,
                serde_json::Value::Null,
                Some("the runner stopped before Lumi finished answering".into()),
            )
            .await?;
        }
        Ok(())
    }

    /// A runner that stops heartbeating leaves runs `running` forever. The server
    /// is the one process that sees every heartbeat, so it declares them dead.
    pub async fn reap_dead_runs(&self) -> Result<Vec<Run>> {
        let threshold = Utc::now() - chrono::Duration::seconds(hulahoop_core::HEARTBEAT_DEAD_SECS);
        let dead: Vec<Run> = sqlx::query_as(
            "select * from runs where status = 'running' and (last_heartbeat_at is null or last_heartbeat_at < ?)",
        )
        .bind(threshold.to_rfc3339())
        .fetch_all(&self.db.pool)
        .await?;
        for run in &dead {
            if let Some(pgid) = run.pgid {
                kill_process_group(pgid as i32);
            }
            // A runner that died after streaming its transcript still spent what
            // the stream says, so the row carries those tokens rather than zero.
            let usage = spent_metrics(std::path::Path::new(&run.artifact_dir), None).await;
            let (tokens_in, tokens_out) = (usage.input, usage.output);
            sqlx::query("update runs set status = 'failed', finished_at = ?, error = 'the runner stopped heartbeating, so the run was declared dead', tokens_in = ?, tokens_out = ? where id = ?")
                .bind(crate::db::now_rfc3339())
                .bind(tokens_in)
                .bind(tokens_out)
                .bind(run.id)
                .execute(&self.db.pool)
                .await?;
            persist_run_metrics(
                &self.db,
                run,
                &usage,
                Some("the runner stopped heartbeating"),
            )
            .await?;
        }
        Ok(dead)
    }
}

/// The tokens a run's adapter reported into its `agent.jsonl`, summed over the
/// run's finished messages. The transcript the run already wrote holds the whole
/// stream, so a runner that could only send the tail is still counted from its
/// file; an `agent.jsonl` the runner opened but never filled falls back to that
/// tail. Every path that finishes a run reads through here, so a cancelled or
/// reaped run reports what it spent too.
#[cfg(test)]
mod lumi_claim_tests {
    use super::*;

    #[tokio::test]
    async fn lumi_lane_claims_questions_while_normal_job_is_queued() {
        let directory = std::env::temp_dir().join(format!("hulahoop-claims-{}", Uuid::now_v7()));
        let db = Db::open(&directory).await.unwrap();
        let workspace = db.seed_local().await.unwrap();
        let project = Uuid::now_v7();
        let repo = Uuid::now_v7();
        sqlx::query(
            "insert into projects (id, workspace_id, name, created_at) values (?,?, 'Claims',?)",
        )
        .bind(project)
        .bind(workspace)
        .bind(crate::db::now_rfc3339())
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query("insert into repos (id, project_id, name, remote_url, contract_version, created_at) values (?,?, 'repo','/tmp/repo','1',?)")
            .bind(repo).bind(project).bind(crate::db::now_rfc3339())
            .execute(&db.pool).await.unwrap();
        let engine = Engine::new(db);
        let normal = engine
            .queue_job(repo, None, "fetch", serde_json::json!({}))
            .await
            .unwrap();
        let lumi = engine
            .queue_job(
                repo,
                None,
                "lumi",
                serde_json::json!({"workspace_id": workspace.to_string()}),
            )
            .await
            .unwrap();
        assert_eq!(engine.claim_job(true).await.unwrap().unwrap().id, lumi);
        assert_eq!(engine.claim_job(false).await.unwrap().unwrap().id, normal);
        assert!(engine.claim_job(true).await.unwrap().is_none());
        sqlx::query("update runner_jobs set claimed_at = ? where id = ?")
            .bind((Utc::now() - chrono::Duration::minutes(13)).to_rfc3339())
            .bind(lumi)
            .execute(&engine.db.pool)
            .await
            .unwrap();
        engine.reap_lumi_jobs().await.unwrap();
        engine
            .complete_job(lumi, serde_json::json!({"reply": "late reply"}), None)
            .await
            .unwrap();
        let (count,): (i64,) =
            sqlx::query_as("select count(*) from lumi_messages where job_id = ?")
                .bind(lumi)
                .fetch_one(&engine.db.pool)
                .await
                .unwrap();
        assert_eq!(count, 1);
        let (status,): (String,) = sqlx::query_as("select status from runner_jobs where id = ?")
            .bind(lumi)
            .fetch_one(&engine.db.pool)
            .await
            .unwrap();
        assert_eq!(status, "failed");
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[cfg(test)]
async fn spent_tokens(artifact_dir: &std::path::Path, tail: Option<&str>) -> (i64, i64) {
    let usage = spent_metrics(artifact_dir, tail).await;
    (usage.input, usage.output)
}

async fn spent_metrics(
    artifact_dir: &std::path::Path,
    tail: Option<&str>,
) -> hulahoop_core::usage::Metrics {
    let written = tokio::fs::read_to_string(artifact_dir.join("agent.jsonl"))
        .await
        .unwrap_or_default();
    let stream = if written.trim().is_empty() {
        tail.unwrap_or_default().to_string()
    } else {
        written
    };
    tokio::task::spawn_blocking(move || hulahoop_core::usage::metrics(&stream))
        .await
        .unwrap_or_default()
}

async fn persist_run_metrics(
    db: &Db,
    run: &Run,
    usage: &hulahoop_core::usage::Metrics,
    error: Option<&str>,
) -> Result<()> {
    let estimated = if usage.measured
        && usage
            .actual_model
            .as_deref()
            .is_none_or(|actual| actual == run.model)
    {
        run.price_in_per_million
            .zip(run.price_out_per_million)
            .map(|(input, output)| {
                (usage.input as f64 * input + usage.output as f64 * output) / 1_000_000.0
            })
    } else {
        None
    };
    let never_started = run.started_at.is_none() && !usage.measured;
    let cost = if never_started {
        Some(0.0)
    } else {
        usage.reported_cost_usd.or(estimated)
    };
    let source = if never_started {
        Some("never_started")
    } else if usage.reported_cost_usd.is_some() {
        Some("reported")
    } else if estimated.is_some() {
        Some("catalogue_estimate_excludes_cache")
    } else {
        None
    };
    let cash = if never_started {
        Some(0.0)
    } else {
        match run.billing_source.as_deref() {
            // Only an adapter-reported charge is actual API cash. Catalogue
            // estimates remain API-equivalent until the provider confirms them.
            Some("api") => usage.reported_cost_usd,
            Some("subscription") => Some(0.0),
            _ => None,
        }
    };
    let failure = error.map(|reason| {
        if reason.contains("timed out") {
            "timeout"
        } else if reason.contains("heartbeating") {
            "runner"
        } else if reason.contains("frozen test") {
            "frozen_tests"
        } else if reason.contains("provider") || reason.contains("model") {
            "provider"
        } else if reason.contains("artifact") {
            "artifact_limit"
        } else {
            "other"
        }
    });
    sqlx::query("update runs set tokens_in = ?, tokens_out = ?, cache_read_tokens = ?, cache_write_tokens = ?, model_turns = ?, tool_calls = ?, actual_model = ?, api_equivalent_usd = ?, actual_cost_usd = ?, cost_source = ?, failure_category = ? where id = ?")
        .bind(usage.input).bind(usage.output)
        .bind(usage.measured.then_some(usage.cache_read.unwrap_or(0)))
        .bind(usage.measured.then_some(usage.cache_write.unwrap_or(0)))
        .bind(usage.turns).bind(usage.tool_calls).bind(&usage.actual_model)
        .bind(cost).bind(cash).bind(source).bind(failure).bind(run.id)
        .execute(&db.pool).await?;
    Ok(())
}

/// The completion payload a runner posts. `diff` is the unified diff the runner
/// captured, because the server is the only process that writes artifacts.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RunCompletion {
    pub status: String,
    #[serde(default)]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub diff: String,
    #[serde(default)]
    pub changed_paths: Vec<String>,
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub question: Option<String>,
    #[serde(default)]
    pub verdict: Option<String>,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub artifact_limit_hit: bool,
    /// The last lines the agent wrote, so a failure can quote the adapter instead of
    /// saying "it exited".
    #[serde(default)]
    pub transcript: Option<String>,
    /// Who the runner wrote into the commit. Reported so a run that fell back to
    /// the human's identity is visible instead of inferred.
    #[serde(default)]
    pub commit_name: Option<String>,
    #[serde(default)]
    pub commit_email: Option<String>,
}

/// Twelve characters of a revision, which is what a human reads in a comment.
/// The prompt files are embedded so a run never depends on the checkout's
/// contents, and the revision is a hash of exactly what was sent.
/// How much of a closing message is kept. Enough for a plan; small enough that a
/// runaway transcript cannot become the task's record.
const SUMMARY_CHARS: usize = 8_000;

#[cfg(test)]
mod spent_tokens_tests {
    use super::*;

    fn artifact_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("hulahoop-spent-tokens-{tag}-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("create artifact dir");
        dir
    }

    #[tokio::test]
    async fn a_finished_run_reports_the_usage_in_its_stream() {
        let dir = artifact_dir("usage");
        let stream = [
            r#"{"type":"message_update","usage":{"input":100,"output":10}}"#,
            r#"{"type":"message_update","usage":{"input":180,"output":30}}"#,
            r#"{"type":"message_end","message":{"usage":{"input":180,"output":30}}}"#,
        ]
        .join("\n");
        std::fs::write(dir.join("agent.jsonl"), stream).expect("write stream");
        assert_eq!(spent_tokens(&dir, None).await, (180, 30));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_run_with_no_stream_spent_nothing_recorded() {
        let dir = artifact_dir("empty");
        assert_eq!(spent_tokens(&dir, None).await, (0, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_runner_that_could_only_send_the_tail_is_counted_from_it() {
        let dir = artifact_dir("tail");
        let tail = r#"{"type":"message_end","message":{"usage":{"input":42,"output":7}}}"#;
        assert_eq!(spent_tokens(&dir, Some(tail)).await, (42, 7));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod adapter_failure_tests {
    use super::*;

    fn completion(transcript: &str) -> RunCompletion {
        RunCompletion {
            status: "succeeded".into(),
            exit_code: Some(0),
            error: None,
            diff: String::new(),
            changed_paths: Vec::new(),
            commit: None,
            question: None,
            verdict: None,
            body: String::new(),
            artifact_limit_hit: false,
            transcript: Some(transcript.to_string()),
            commit_name: None,
            commit_email: None,
        }
    }

    #[test]
    fn the_provider_refusal_is_quoted_instead_of_exiting_quietly() {
        let payload = completion(
            r#"{"type":"auto_retry_end","success":false,"attempt":3,"finalError":"400: {\"type\":\"server_error\",\"message\":\"Model deepseek-4.1-flash isn't included in OpenCode Go.\"}"}"#,
        );
        let said = adapter_failure(&payload).expect("a reason");
        assert!(said.contains("isn't included in OpenCode Go"), "got {said}");
    }

    #[test]
    fn a_quiet_transcript_has_no_reason_to_quote() {
        assert!(
            adapter_failure(&completion(r#"{"type":"agent_settled","aborted":false}"#)).is_none()
        );
    }
}

#[cfg(test)]
mod rework_tests {
    use super::Engine;
    use crate::db::Db;
    use chrono::{Duration, Utc};
    use uuid::Uuid;

    const DOC: &str = "# Workflows\n\nTurns: 6\n\n## Backlog\n\nUsers: human\n\nNext: To do\n\n## To do\n\nUsers: human\n\nNext: In progress\n\n## In progress\n\nUsers: @Developer, @Reviewer\n\nNext: In progress - @Reviewer asks @Developer to work on it again\n\nNext: In review\n\n## Blocked\n\nUsers: human\n\nNext: In progress\n\n## In review\n\nUsers: human\n\nNext: Done\n\n## Done\n\nUsers: human\n\nNext: none\n";

    /// A review that asks for changes is the reviewer handing the task back:
    /// the round starts again from the first agent the status names.
    #[tokio::test]
    async fn changes_requested_hands_the_task_back_to_the_first_agent() {
        let directory = std::env::temp_dir().join(format!("hulahoop-rework-{}", Uuid::now_v7()));
        let db = Db::open(&directory).await.unwrap();
        let workspace = db.seed_local().await.unwrap();
        let (project, repo, developer, reviewer, issue) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        let now = Utc::now() - Duration::seconds(3);
        sqlx::query(
            "insert into projects (id, workspace_id, name, created_at) values (?,?, 'Rework',?)",
        )
        .bind(project)
        .bind(workspace)
        .bind(now)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query("insert into repos (id, project_id, name, remote_url, contract_version, created_at) values (?,?, 'repo','/tmp/repo','1',?)")
            .bind(repo).bind(project).bind(now).execute(&db.pool).await.unwrap();
        for (id, name) in [(developer, "Developer"), (reviewer, "Reviewer")] {
            sqlx::query("insert into agents (id, workspace_id, name, adapter, model, created_at, updated_at) values (?,?,?,'pi','model-x',?,?)")
                .bind(id).bind(workspace).bind(name).bind(now).bind(now).execute(&db.pool).await.unwrap();
        }
        let served = serde_json::json!([developer.to_string()]).to_string();
        sqlx::query("insert into issues (id, workspace_id, project_id, repo_id, issue_key, number, title, status, turn_queue, turn_served, turns_used, created_at, updated_at) values (?,?,?,?, 'RWK-1',1,'Task','in_progress','[\"Developer\",\"Reviewer\"]',?,2,?,?)")
            .bind(issue).bind(workspace).bind(project).bind(repo).bind(served)
            .bind(now).bind(now).execute(&db.pool).await.unwrap();

        let engine = Engine::new(db);
        engine.set_workflow(workspace, DOC).await.unwrap();
        engine.hand_back_for_rework(issue).await.unwrap();

        let task = engine.issue(issue).await.unwrap();
        assert_eq!(task.turn_served, "[]");
        assert_eq!(task.status, "in_progress");
        assert_eq!(
            engine.next_turn_agent(&task).await.unwrap().as_deref(),
            Some("Developer")
        );
        // The rework was dispatched: the turn is counted against `Turns:`.
        assert_eq!(task.turns_used, 3);

        let _ = std::fs::remove_dir_all(directory);
    }

    /// A rework that changes nothing hands the reviewer the head it already
    /// judged. Nobody can take that turn, so the task goes to a person with the
    /// reason instead of idling in a status agents own.
    #[tokio::test]
    async fn a_rework_without_changes_blocks_the_task() {
        let directory = std::env::temp_dir().join(format!("hulahoop-stall-{}", Uuid::now_v7()));
        let db = Db::open(&directory).await.unwrap();
        let workspace = db.seed_local().await.unwrap();
        let (project, repo, developer, reviewer, issue) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        let (head, review_run, rework) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let now = Utc::now() - Duration::seconds(3);
        sqlx::query(
            "insert into projects (id, workspace_id, name, created_at) values (?,?, 'Stall',?)",
        )
        .bind(project)
        .bind(workspace)
        .bind(now)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query("insert into repos (id, project_id, name, remote_url, contract_version, created_at) values (?,?, 'repo','/tmp/repo','1',?)")
            .bind(repo).bind(project).bind(now).execute(&db.pool).await.unwrap();
        for (id, name) in [(developer, "Developer"), (reviewer, "Reviewer")] {
            sqlx::query("insert into agents (id, workspace_id, name, adapter, model, created_at, updated_at) values (?,?,?,'pi','model-x',?,?)")
                .bind(id).bind(workspace).bind(name).bind(now).bind(now).execute(&db.pool).await.unwrap();
        }
        sqlx::query("insert into issues (id, workspace_id, project_id, repo_id, issue_key, number, title, status, turn_queue, turn_served, turns_used, created_at, updated_at) values (?,?,?,?, 'STL-1',1,'Task','in_progress','[\"Developer\",\"Reviewer\"]','[]',3,?,?)")
            .bind(issue).bind(workspace).bind(project).bind(repo)
            .bind(now).bind(now).execute(&db.pool).await.unwrap();
        for (id, agent, status, bookmark) in [
            (head, developer, "succeeded", Some("issue/x/head/1")),
            (review_run, reviewer, "succeeded", None),
            (rework, developer, "running", None),
        ] {
            sqlx::query("insert into runs (id, issue_id, repo_id, agent_id, attempt, base_bookmark, prompt_revision, artifact_dir, status, model, head_bookmark, created_at) values (?,?,?,?,1,'main','prompt','/tmp',?,'model-x',?,?)")
                .bind(id).bind(issue).bind(repo).bind(agent).bind(status).bind(bookmark).bind(now)
                .execute(&db.pool).await.unwrap();
        }
        sqlx::query("insert into reviews (id, issue_id, reviewed_run_id, reviewer_run_id, verdict, body, created_at) values (?,?,?,?,'changes_requested','Fix the date field.',?)")
            .bind(Uuid::now_v7()).bind(issue).bind(head).bind(review_run).bind(now)
            .execute(&db.pool).await.unwrap();
        sqlx::query("update issues set current_run_id = ? where id = ?")
            .bind(head)
            .bind(issue)
            .execute(&db.pool)
            .await
            .unwrap();

        let engine = Engine::new(db);
        engine.set_workflow(workspace, DOC).await.unwrap();
        let unchanged: super::RunCompletion = serde_json::from_value(serde_json::json!({
            "status": "succeeded",
            "body": "The work was already in place.",
            "verdict": "done",
        }))
        .unwrap();
        engine.complete_run(rework, unchanged).await.unwrap();

        let task = engine.issue(issue).await.unwrap();
        assert_eq!(task.status, "blocked");
        let said: String = sqlx::query_scalar(
            "select body from comments where issue_id = ? and author_type = 'server' order by created_at desc limit 1",
        )
        .bind(issue)
        .fetch_one(&engine.db.pool)
        .await
        .unwrap();
        assert!(
            said.contains("Developer finished without changing the work Reviewer already reviewed"),
            "got {said}"
        );
        assert!(said.contains("change request is still open"), "got {said}");

        let _ = std::fs::remove_dir_all(directory);
    }

    /// A person asking for changes after the reviewer approved sends the task
    /// back to the developer, even though the head already carries a verdict:
    /// only the reviewer who gave it is held back.
    #[tokio::test]
    async fn requesting_changes_after_an_approval_starts_the_developer() {
        let directory =
            std::env::temp_dir().join(format!("hulahoop-human-rework-{}", Uuid::now_v7()));
        let db = Db::open(&directory).await.unwrap();
        let workspace = db.seed_local().await.unwrap();
        let (developer, reviewer, issue, head, review_run) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        let now = Utc::now() - Duration::seconds(3);
        for (id, name) in [(developer, "Developer"), (reviewer, "Reviewer")] {
            sqlx::query("insert into agents (id, workspace_id, name, adapter, model, created_at, updated_at) values (?,?,?,'pi','model-x',?,?)")
                .bind(id).bind(workspace).bind(name).bind(now).bind(now).execute(&db.pool).await.unwrap();
        }
        sqlx::query("insert into issues (id, workspace_id, project_id, repo_id, issue_key, number, title, status, created_at, updated_at) values (?,?,null,null,'RWK-2',2,'Task','in_review',?,?)")
            .bind(issue).bind(workspace).bind(now).bind(now).execute(&db.pool).await.unwrap();
        for (id, agent) in [(head, developer), (review_run, reviewer)] {
            sqlx::query("insert into runs (id, issue_id, repo_id, agent_id, attempt, base_bookmark, prompt_revision, artifact_dir, status, model, created_at) values (?,?,null,?,1,'main','prompt','/tmp','succeeded','model-x',?)")
                .bind(id).bind(issue).bind(agent).bind(now).execute(&db.pool).await.unwrap();
        }
        sqlx::query("insert into reviews (id, issue_id, reviewed_run_id, reviewer_run_id, verdict, body, created_at) values (?,?,?,?,'approved','Looks good.',?)")
            .bind(Uuid::now_v7()).bind(issue).bind(head).bind(review_run).bind(now)
            .execute(&db.pool).await.unwrap();
        sqlx::query("update issues set current_run_id = ? where id = ?")
            .bind(head)
            .bind(issue)
            .execute(&db.pool)
            .await
            .unwrap();

        let engine = Engine::new(db);
        engine.set_workflow(workspace, DOC).await.unwrap();
        engine
            .transition(issue, hulahoop_core::IssueEvent::RequestChanges)
            .await
            .unwrap();

        let agent: Uuid = sqlx::query_scalar(
            "select agent_id from runs where issue_id = ? and status = 'queued'",
        )
        .bind(issue)
        .fetch_one(&engine.db.pool)
        .await
        .unwrap();
        assert_eq!(agent, developer);

        // The rework finishes: the reviewer judges it, the newest work, and is
        // not held back by the verdict it gave the old head.
        let rework: Uuid =
            sqlx::query_scalar("select id from runs where issue_id = ? and status = 'queued'")
                .bind(issue)
                .fetch_one(&engine.db.pool)
                .await
                .unwrap();
        let completion: super::RunCompletion = serde_json::from_value(serde_json::json!({
            "status": "succeeded",
            "body": "Rebased on main.",
        }))
        .unwrap();
        engine.complete_run(rework, completion).await.unwrap();
        let task = engine.issue(issue).await.unwrap();
        assert_eq!(task.current_run_id, Some(rework));
        let agent: Uuid = sqlx::query_scalar(
            "select agent_id from runs where issue_id = ? and status = 'queued'",
        )
        .bind(issue)
        .fetch_one(&engine.db.pool)
        .await
        .unwrap();
        assert_eq!(agent, reviewer);

        // The reviewer asks for changes: the developer's next turn names the
        // review as its parent, so the findings reach the run that fixes them.
        let review: Uuid =
            sqlx::query_scalar("select id from runs where issue_id = ? and status = 'queued'")
                .bind(issue)
                .fetch_one(&engine.db.pool)
                .await
                .unwrap();
        let verdict: super::RunCompletion = serde_json::from_value(serde_json::json!({
            "status": "succeeded",
            "body": "Fix the target date field.",
            "verdict": "changes_requested",
        }))
        .unwrap();
        engine.complete_run(review, verdict).await.unwrap();
        let (agent, parent): (Uuid, Option<Uuid>) = sqlx::query_as(
            "select agent_id, parent_run_id from runs where issue_id = ? and status = 'queued'",
        )
        .bind(issue)
        .fetch_one(&engine.db.pool)
        .await
        .unwrap();
        assert_eq!(agent, developer);
        assert_eq!(parent, Some(review));

        let _ = std::fs::remove_dir_all(directory);
    }
}

#[cfg(test)]
mod repo_less_tests {
    use super::{Engine, RunCompletion};
    use crate::db::Db;
    use chrono::{Duration, Utc};
    use uuid::Uuid;

    const DOC: &str = "# Workflows\n\nTurns: 6\n\n## Backlog\n\nUsers: human\n\nNext: In progress\n\n## In progress\n\nUsers: @Developer\n\nNext: Done\n\n## Blocked\n\nUsers: human\n\nNext: In progress\n\n## Done\n\nUsers: human\n\nNext: none\n";

    /// A workspace with one agent and no repository anywhere, and the
    /// workflow above.
    async fn fixture(tag: &str) -> (std::path::PathBuf, Engine, Uuid, Uuid) {
        let directory =
            std::env::temp_dir().join(format!("hulahoop-repo-less-{tag}-{}", Uuid::now_v7()));
        let db = Db::open(&directory).await.unwrap();
        let workspace = db.seed_local().await.unwrap();
        let developer = Uuid::now_v7();
        let now = Utc::now() - Duration::seconds(3);
        sqlx::query("insert into agents (id, workspace_id, name, adapter, model, created_at, updated_at) values (?,?,'Developer','pi','model-x',?,?)")
            .bind(developer).bind(workspace).bind(now).bind(now).execute(&db.pool).await.unwrap();
        let engine = Engine::new(db);
        engine.set_workflow(workspace, DOC).await.unwrap();
        (directory, engine, workspace, developer)
    }

    /// A task with no project and no repository, in progress.
    async fn task(engine: &Engine, workspace: Uuid, number: i64) -> Uuid {
        let issue = Uuid::now_v7();
        let now = Utc::now();
        sqlx::query("insert into issues (id, workspace_id, project_id, repo_id, issue_key, number, title, status, created_at, updated_at) values (?,?,null,null,?,?,'Task','in_progress',?,?)")
            .bind(issue).bind(workspace).bind(format!("TSK-{number}")).bind(number)
            .bind(now).bind(now).execute(&engine.db.pool).await.unwrap();
        issue
    }

    fn finished(body: &str, question: Option<&str>, verdict: Option<&str>) -> RunCompletion {
        serde_json::from_value(serde_json::json!({
            "status": "succeeded",
            "body": body,
            "question": question,
            "verdict": verdict,
        }))
        .unwrap()
    }

    async fn jobs(engine: &Engine) -> i64 {
        sqlx::query_scalar("select count(*) from runner_jobs where kind != 'preflight'")
            .fetch_one(&engine.db.pool)
            .await
            .unwrap()
    }

    /// A failed run leaves the task for a person: nothing retries on its own,
    /// so a broken host cannot burn through the task's runs.
    #[tokio::test]
    async fn a_failed_run_does_not_start_another() {
        let (directory, engine, workspace, developer) = fixture("no-retry").await;
        let issue = task(&engine, workspace, 1).await;
        let run = engine
            .start_run(issue, Some(developer), None)
            .await
            .unwrap();
        let failed: RunCompletion = serde_json::from_value(serde_json::json!({
            "status": "failed",
            "error": "No space left on device",
        }))
        .unwrap();
        engine.complete_run(run.id, failed).await.unwrap();

        let live: i64 = sqlx::query_scalar(
            "select count(*) from runs where issue_id = ? and status in ('queued', 'running')",
        )
        .bind(issue)
        .fetch_one(&engine.db.pool)
        .await
        .unwrap();
        assert_eq!(live, 0);
        assert_eq!(engine.issue(issue).await.unwrap().status, "in_progress");

        let _ = std::fs::remove_dir_all(directory);
    }

    /// The run limit stops every agent; allowing more runs resets the count
    /// and starts the agent the workflow was waiting on.
    #[tokio::test]
    async fn allowing_more_runs_restarts_the_waiting_agent() {
        let (directory, engine, workspace, developer) = fixture("run-cap").await;
        let issue = task(&engine, workspace, 1).await;
        let earlier = Utc::now() - Duration::seconds(30);
        for _ in 0..hulahoop_core::RUNS_PER_ISSUE {
            sqlx::query("insert into runs (id, issue_id, repo_id, agent_id, attempt, base_bookmark, prompt_revision, artifact_dir, status, model, created_at) values (?,?,null,?,1,'main','prompt','/tmp','failed','model-x',?)")
                .bind(Uuid::now_v7()).bind(issue).bind(developer).bind(earlier)
                .execute(&engine.db.pool).await.unwrap();
        }
        let task_row = engine.issue(issue).await.unwrap();
        assert_eq!(
            engine.runs_since_cap(&task_row).await.unwrap(),
            hulahoop_core::RUNS_PER_ISSUE
        );
        let refused = engine.start_run(issue, Some(developer), None).await;
        assert!(refused.unwrap_err().to_string().contains("clearRunCap"));

        let task_row = engine.clear_run_cap(issue).await.unwrap();
        // The waiting agent started: its run is the first of the new round.
        assert_eq!(engine.runs_since_cap(&task_row).await.unwrap(), 1);
        let (status,): (String,) = sqlx::query_as(
            "select status from runs where issue_id = ? order by created_at desc limit 1",
        )
        .bind(issue)
        .fetch_one(&engine.db.pool)
        .await
        .unwrap();
        assert_eq!(status, "queued");

        let _ = std::fs::remove_dir_all(directory);
    }

    /// Repo-less runs need no preflight and take no repository slot: two
    /// tasks each have a live run at once, and the issue still holds one.
    #[tokio::test]
    async fn two_repo_less_tasks_run_at_the_same_time() {
        let (directory, engine, workspace, developer) = fixture("parallel").await;
        let first = task(&engine, workspace, 1).await;
        let second = task(&engine, workspace, 2).await;

        let a = engine
            .start_run(first, Some(developer), None)
            .await
            .unwrap();
        let b = engine
            .start_run(second, Some(developer), None)
            .await
            .unwrap();
        assert_eq!(a.repo_id, None);
        assert_eq!(b.repo_id, None);
        assert_eq!(a.base_revision, None);
        let preflights: i64 =
            sqlx::query_scalar("select count(*) from runner_jobs where kind = 'preflight'")
                .fetch_one(&engine.db.pool)
                .await
                .unwrap();
        assert_eq!(preflights, 0, "a repo-less run probes no repository");
        assert!(
            engine
                .start_run(first, Some(developer), None)
                .await
                .is_err(),
            "one live run per task still holds"
        );

        let _ = std::fs::remove_dir_all(directory);
    }

    /// A repo-less turn that ends `HULAHOOP_VERDICT: done` moves the task the
    /// way the workflow says, with no pull request and no merge on the way.
    #[tokio::test]
    async fn a_repo_less_turn_moves_the_task_without_a_merge() {
        let (directory, engine, workspace, developer) = fixture("done").await;
        let issue = task(&engine, workspace, 1).await;
        let run = engine
            .start_run(issue, Some(developer), None)
            .await
            .unwrap();

        let run = engine
            .complete_run(
                run.id,
                finished(
                    "Wrote the report.\n\nHULAHOOP_VERDICT: done",
                    None,
                    Some("done"),
                ),
            )
            .await
            .unwrap();
        assert_eq!(run.status, "succeeded", "no changed files is normal here");

        let task = engine.issue(issue).await.unwrap();
        assert_eq!(task.status, "done");
        assert_eq!(task.pr_number, None);
        assert_eq!(jobs(&engine).await, 0, "no pull request, no checks");
        let summary: Option<Uuid> = sqlx::query_scalar(
            "select run_id from comments where issue_id = ? and author_type = 'agent'",
        )
        .bind(issue)
        .fetch_optional(&engine.db.pool)
        .await
        .unwrap()
        .flatten();
        assert_eq!(summary, Some(run.id), "the summary is the run's output");

        let _ = std::fs::remove_dir_all(directory);
    }

    /// A question from a repo-less run blocks the task, as any run's would.
    #[tokio::test]
    async fn a_repo_less_question_blocks_the_task() {
        let (directory, engine, workspace, developer) = fixture("question").await;
        let issue = task(&engine, workspace, 1).await;
        let run = engine
            .start_run(issue, Some(developer), None)
            .await
            .unwrap();

        engine
            .complete_run(
                run.id,
                finished("Which quarter?", Some("Which quarter?"), None),
            )
            .await
            .unwrap();

        assert_eq!(engine.issue(issue).await.unwrap().status, "blocked");
        assert_eq!(jobs(&engine).await, 0);

        let _ = std::fs::remove_dir_all(directory);
    }

    /// A repo-less run that says nothing produced nothing: still a failure.
    #[tokio::test]
    async fn a_silent_repo_less_run_fails() {
        let (directory, engine, workspace, developer) = fixture("silent").await;
        let issue = task(&engine, workspace, 1).await;
        let run = engine
            .start_run(issue, Some(developer), None)
            .await
            .unwrap();

        let run = engine
            .complete_run(run.id, finished("", None, None))
            .await
            .unwrap();
        assert_eq!(run.status, "failed");

        let _ = std::fs::remove_dir_all(directory);
    }

    /// Merging a repo-less task is only the transition: nothing to merge.
    #[tokio::test]
    async fn merging_a_repo_less_task_only_closes_it() {
        let (directory, engine, workspace, _) = fixture("merge").await;
        let issue = task(&engine, workspace, 1).await;
        sqlx::query("update issues set status = 'in_review' where id = ?")
            .bind(issue)
            .execute(&engine.db.pool)
            .await
            .unwrap();

        let task = engine.merge_issue(issue).await.unwrap();
        assert_eq!(task.status, "done");
        assert!(task.merged_at.is_some());
        assert_eq!(jobs(&engine).await, 0);

        let _ = std::fs::remove_dir_all(directory);
    }
}

#[cfg(test)]
mod pull_request_comment_tests {
    use super::Engine;
    use crate::db::Db;
    use chrono::{Duration, Utc};
    use uuid::Uuid;

    /// When the runner reports the pull request it opened, the task gets an
    /// agent comment naming it, carrying the run's summary, linked to the run.
    #[tokio::test]
    async fn an_opened_pull_request_is_announced_on_the_run() {
        let directory = std::env::temp_dir().join(format!("hulahoop-pr-{}", Uuid::now_v7()));
        let db = Db::open(&directory).await.unwrap();
        let workspace = db.seed_local().await.unwrap();
        let (project, repo, agent, issue, run) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        let now = Utc::now() - Duration::seconds(3);
        sqlx::query(
            "insert into projects (id, workspace_id, name, created_at) values (?,?, 'PR',?)",
        )
        .bind(project)
        .bind(workspace)
        .bind(now)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query("insert into repos (id, project_id, name, remote_url, contract_version, created_at) values (?,?, 'repo','/tmp/repo','1',?)")
            .bind(repo).bind(project).bind(now).execute(&db.pool).await.unwrap();
        sqlx::query("insert into agents (id, workspace_id, name, adapter, model, created_at, updated_at) values (?,?,'Developer','pi','model-x',?,?)")
            .bind(agent).bind(workspace).bind(now).bind(now).execute(&db.pool).await.unwrap();
        sqlx::query("insert into issues (id, workspace_id, project_id, repo_id, issue_key, number, title, status, created_at, updated_at) values (?,?,?,?, 'PR-1',1,'Task','in_review',?,?)")
            .bind(issue).bind(workspace).bind(project).bind(repo).bind(now).bind(now).execute(&db.pool).await.unwrap();
        sqlx::query("insert into runs (id, issue_id, repo_id, agent_id, attempt, base_bookmark, prompt_revision, artifact_dir, status, model, summary, created_at) values (?,?,?,?,1,'main','prompt','/tmp','succeeded','model-x','Added the button.',?)")
            .bind(run).bind(issue).bind(repo).bind(agent).bind(now).execute(&db.pool).await.unwrap();

        let engine = Engine::new(db);
        let job = engine
            .queue_job(
                repo,
                Some(issue),
                "open_pr",
                serde_json::json!({ "run_id": run.to_string() }),
            )
            .await
            .unwrap();
        sqlx::query("update runner_jobs set status = 'running' where id = ?")
            .bind(job)
            .execute(&engine.db.pool)
            .await
            .unwrap();
        engine
            .complete_job(
                job,
                serde_json::json!({
                    "pr_number": 7,
                    "pr_url": "https://github.com/acme/repo/pull/7",
                    "pr_branch": "issue/7",
                    "run_id": run.to_string(),
                }),
                None,
            )
            .await
            .unwrap();

        let (body, author, linked): (String, String, Option<Uuid>) = sqlx::query_as(
            "select body, author_type, run_id from comments where issue_id = ? and body like 'Opened pull request%'",
        )
        .bind(issue)
        .fetch_one(&engine.db.pool)
        .await
        .unwrap();
        assert!(body.starts_with("Opened pull request #7: https://github.com/acme/repo/pull/7"));
        assert!(body.contains("Added the button."), "{body}");
        assert_eq!(author, "agent");
        assert_eq!(linked, Some(run));
        assert_eq!(engine.issue(issue).await.unwrap().pr_number, Some(7));

        let _ = std::fs::remove_dir_all(directory);
    }

    /// The summary an agent closes its turn with is posted as its comment and
    /// carries the run it came from, so the thread links back to the run.
    #[tokio::test]
    async fn a_summary_comment_carries_its_run() {
        let directory = std::env::temp_dir().join(format!("hulahoop-summary-{}", Uuid::now_v7()));
        let db = Db::open(&directory).await.unwrap();
        let workspace = db.seed_local().await.unwrap();
        let (project, repo, agent, issue, run) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        let now = Utc::now() - Duration::seconds(3);
        sqlx::query(
            "insert into projects (id, workspace_id, name, created_at) values (?,?, 'Sum',?)",
        )
        .bind(project)
        .bind(workspace)
        .bind(now)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query("insert into repos (id, project_id, name, remote_url, contract_version, created_at) values (?,?, 'repo','/tmp/repo','1',?)")
            .bind(repo).bind(project).bind(now).execute(&db.pool).await.unwrap();
        sqlx::query("insert into agents (id, workspace_id, name, adapter, model, created_at, updated_at) values (?,?,'Developer','pi','model-x',?,?)")
            .bind(agent).bind(workspace).bind(now).bind(now).execute(&db.pool).await.unwrap();
        sqlx::query("insert into issues (id, workspace_id, project_id, repo_id, issue_key, number, title, status, created_at, updated_at) values (?,?,?,?, 'SUM-1',1,'Task','in_progress',?,?)")
            .bind(issue).bind(workspace).bind(project).bind(repo).bind(now).bind(now).execute(&db.pool).await.unwrap();
        let artifacts = directory.join("summary-artifacts");
        sqlx::query("insert into runs (id, issue_id, repo_id, agent_id, attempt, base_bookmark, prompt_revision, artifact_dir, status, model, created_at, started_at) values (?,?,?,?,1,'main','prompt',?,'running','model-x',?,?)")
            .bind(run).bind(issue).bind(repo).bind(agent)
            .bind(artifacts.to_string_lossy().to_string()).bind(now).bind(now)
            .execute(&db.pool).await.unwrap();

        let engine = Engine::new(db);
        let completion: super::RunCompletion = serde_json::from_value(serde_json::json!({
            "status": "succeeded",
            "body": "Added the button.\n\nHULAHOOP_VERDICT: done",
            "verdict": "done",
            "changed_paths": ["src/button.rs"],
            "commit": "abc123",
        }))
        .unwrap();
        engine.complete_run(run, completion).await.unwrap();

        let (body, linked): (String, Option<Uuid>) = sqlx::query_as(
            "select body, run_id from comments where issue_id = ? and author_type = 'agent' order by created_at limit 1",
        )
        .bind(issue)
        .fetch_one(&engine.db.pool)
        .await
        .unwrap();
        assert!(body.contains("Added the button."), "{body}");
        assert_eq!(linked, Some(run));

        let _ = std::fs::remove_dir_all(directory);
    }
}

#[cfg(test)]
mod checkout_lifecycle_tests {
    use super::Engine;
    use crate::db::Db;
    use chrono::Utc;
    use hulahoop_core::IssueEvent;
    use uuid::Uuid;

    /// A task in review on a repository with one published head, ready to merge.
    async fn task_in_review(name: &str) -> (Engine, Uuid, Uuid, Uuid) {
        let directory = std::env::temp_dir().join(format!("hulahoop-{name}-{}", Uuid::now_v7()));
        let db = Db::open(&directory).await.unwrap();
        let workspace = db.seed_local().await.unwrap();
        let (project, repo, agent, issue, run) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        let now = Utc::now();
        sqlx::query(
            "insert into projects (id, workspace_id, name, created_at) values (?,?, 'Keep',?)",
        )
        .bind(project)
        .bind(workspace)
        .bind(now)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query("insert into repos (id, project_id, name, remote_url, contract_version, created_at) values (?,?, 'repo','/tmp/repo','1',?)")
            .bind(repo).bind(project).bind(now).execute(&db.pool).await.unwrap();
        sqlx::query("insert into agents (id, workspace_id, name, adapter, model, created_at, updated_at) values (?,?,'Developer','pi','model-x',?,?)")
            .bind(agent).bind(workspace).bind(now).bind(now).execute(&db.pool).await.unwrap();
        sqlx::query("insert into issues (id, workspace_id, project_id, repo_id, issue_key, number, title, status, created_at, updated_at) values (?,?,?,?, 'KEP-1',1,'Task','in_review',?,?)")
            .bind(issue).bind(workspace).bind(project).bind(repo).bind(now).bind(now).execute(&db.pool).await.unwrap();
        sqlx::query("insert into runs (id, issue_id, repo_id, agent_id, attempt, base_bookmark, prompt_revision, artifact_dir, status, model, head_revision, created_at) values (?,?,?,?,1,'main','prompt','/tmp','succeeded','model-x','0123456789abcdef0123456789abcdef01234567',?)")
            .bind(run).bind(issue).bind(repo).bind(agent).bind(now).execute(&db.pool).await.unwrap();
        (Engine::new(db), project, repo, issue)
    }

    async fn jobs(engine: &Engine, kind: &str) -> Vec<(Option<Uuid>, Option<Uuid>, String)> {
        sqlx::query_as(
            "select repo_id, issue_id, payload from runner_jobs where kind = ? order by created_at",
        )
        .bind(kind)
        .fetch_all(&engine.db.pool)
        .await
        .unwrap()
    }

    const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

    async fn base_revision(engine: &Engine, repo: Uuid) -> Option<String> {
        sqlx::query_scalar("select base_revision from repos where id = ?")
            .bind(repo)
            .fetch_one(&engine.db.pool)
            .await
            .unwrap()
    }

    /// Claims queued jobs until one of `kind` comes up.
    async fn claim(engine: &Engine, kind: &str) -> Uuid {
        loop {
            let job = engine.claim_job(false).await.unwrap().expect(kind);
            if job.kind == kind {
                return job.id;
            }
        }
    }

    /// A merge without a pull request lands the head on the remote's base
    /// branch through a runner, and deletes the task's checkout, which no turn
    /// will use again. The base only moves once the landing succeeded.
    #[tokio::test]
    async fn a_merge_without_a_pull_request_lands_and_cleans_up() {
        let (engine, project, repo, issue) = task_in_review("merge-cleanup").await;
        let merged = engine.merge_issue(issue).await.unwrap();
        assert_eq!(merged.status, "done");

        assert!(jobs(&engine, "fetch").await.is_empty());
        let lands = jobs(&engine, "land").await;
        assert_eq!(lands.len(), 1);
        assert_eq!((lands[0].0, lands[0].1), (Some(repo), Some(issue)));
        let payload: serde_json::Value = serde_json::from_str(&lands[0].2).unwrap();
        assert_eq!(payload["revision"].as_str(), Some(HEAD));
        assert_eq!(base_revision(&engine, repo).await, None);

        let land = claim(&engine, "land").await;
        engine
            .complete_job(land, serde_json::json!({ "base_revision": HEAD }), None)
            .await
            .unwrap();
        assert_eq!(base_revision(&engine, repo).await.as_deref(), Some(HEAD));

        let cleanups = jobs(&engine, "cleanup").await;
        assert_eq!(cleanups.len(), 1);
        let (cleanup_repo, cleanup_issue, payload) = &cleanups[0];
        assert_eq!((*cleanup_repo, *cleanup_issue), (Some(repo), Some(issue)));
        let payload: serde_json::Value = serde_json::from_str(payload).unwrap();
        let expected = hulahoop_core::contract::checkout_dir(
            &engine.db.data_dir,
            &project.to_string(),
            &issue,
        );
        assert_eq!(payload["checkout"].as_str(), expected.to_str());
    }

    /// A landing that fails leaves the base alone and says why on the task.
    #[tokio::test]
    async fn a_failed_landing_is_reported_on_the_task() {
        let (engine, _, repo, issue) = task_in_review("land-failed").await;
        engine.merge_issue(issue).await.unwrap();
        let land = claim(&engine, "land").await;
        engine
            .complete_job(
                land,
                serde_json::json!({}),
                Some("main could not move forward".into()),
            )
            .await
            .unwrap();
        assert_eq!(base_revision(&engine, repo).await, None);
        let bodies: Vec<String> =
            sqlx::query_scalar("select body from comments where issue_id = ? order by created_at")
                .bind(issue)
                .fetch_all(&engine.db.pool)
                .await
                .unwrap();
        assert!(
            bodies
                .iter()
                .any(|body| body.contains("main could not move forward")),
            "{bodies:?}"
        );
    }

    /// A fetch never moves the base back: neither when the remote is behind the
    /// base it was compared with, nor when the base changed (a landing) while
    /// the fetch was out.
    #[tokio::test]
    async fn a_fetch_never_replaces_a_newer_base() {
        let (engine, _, repo, _) = task_in_review("fetch-newer").await;
        let fetch = engine
            .queue_job(repo, None, "fetch", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(claim(&engine, "fetch").await, fetch);
        sqlx::query("update repos set base_revision = ? where id = ?")
            .bind(HEAD)
            .bind(repo)
            .execute(&engine.db.pool)
            .await
            .unwrap();
        engine
            .complete_job(
                fetch,
                serde_json::json!({ "base_revision": "older", "behind": false, "compared_with": "" }),
                None,
            )
            .await
            .unwrap();
        assert_eq!(base_revision(&engine, repo).await.as_deref(), Some(HEAD));

        let behind = engine
            .queue_job(repo, None, "fetch", serde_json::json!({}))
            .await
            .unwrap();
        claim(&engine, "fetch").await;
        engine
            .complete_job(
                behind,
                serde_json::json!({ "base_revision": "older", "behind": true, "compared_with": HEAD }),
                None,
            )
            .await
            .unwrap();
        assert_eq!(base_revision(&engine, repo).await.as_deref(), Some(HEAD));

        let ahead = engine
            .queue_job(repo, None, "fetch", serde_json::json!({}))
            .await
            .unwrap();
        claim(&engine, "fetch").await;
        engine
            .complete_job(
                ahead,
                serde_json::json!({ "base_revision": "newer", "behind": false, "compared_with": HEAD }),
                None,
            )
            .await
            .unwrap();
        assert_eq!(base_revision(&engine, repo).await.as_deref(), Some("newer"));
    }

    /// Reopening a finished task keeps its checkout: the cleanup still queued
    /// for it is dropped before a runner deletes what the next turn needs.
    #[tokio::test]
    async fn reopening_a_task_drops_its_queued_cleanup() {
        let (engine, _, _, issue) = task_in_review("reopen-cleanup").await;
        engine.transition(issue, IssueEvent::Abandon).await.unwrap();
        assert_eq!(jobs(&engine, "cleanup").await.len(), 1);
        engine.transition(issue, IssueEvent::Reopen).await.unwrap();
        assert!(jobs(&engine, "cleanup").await.is_empty());
    }

    /// A cancelled task loses its checkout too, but only once no run of it is
    /// live: a turn never has its checkout deleted under it.
    #[tokio::test]
    async fn a_cancelled_task_is_cleaned_up_after_its_last_run() {
        let (engine, _, _, issue) = task_in_review("cancel-cleanup").await;
        sqlx::query("update runs set status = 'running' where issue_id = ?")
            .bind(issue)
            .execute(&engine.db.pool)
            .await
            .unwrap();
        let cancelled = engine.transition(issue, IssueEvent::Abandon).await.unwrap();
        assert_eq!(cancelled.status, "cancelled");
        assert_eq!(jobs(&engine, "cleanup").await.len(), 1);
        assert!(engine.claim_job(false).await.unwrap().is_none());

        sqlx::query("update runs set status = 'cancelled' where issue_id = ?")
            .bind(issue)
            .execute(&engine.db.pool)
            .await
            .unwrap();
        let claimed = engine.claim_job(false).await.unwrap().expect("cleanup");
        assert_eq!(claimed.kind, "cleanup");
        // Moving a finished task again queues nothing more.
        assert!(engine.transition(issue, IssueEvent::Abandon).await.is_err());
        assert_eq!(jobs(&engine, "cleanup").await.len(), 1);
    }
}
