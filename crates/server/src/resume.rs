//! Turns a task is owed but never got.
//!
//! A turn starts when something happens to its task: a run finishing, a move,
//! a mention. A dispatch that was refused or lost, or a server that went down
//! between a run ending and the next one starting, leaves the task idle in a
//! status agents own with nothing left to wake it. The sweep here looks for
//! those tasks once at startup and then every minute, and starts the turn the
//! workflow still owes, exactly as a finished turn would have.

use anyhow::Result;
use chrono::{Duration, Utc};
use uuid::Uuid;

use crate::engine::Engine;

/// How often the sweep looks again.
pub const RESUME_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

/// A task touched more recently than this is left to the path that touched it:
/// a run that just finished is still being handed over, and a sweep that got
/// there first would start the agent that has just been served a second time.
const SETTLE_SECS: i64 = 60;

/// What the reaper writes on a run whose runner went quiet. A run that ended
/// this way was cut short by the host, not judged by its agent.
const LOST_RUN: &str = "the runner stopped heartbeating%";

impl Engine {
    /// Start every turn the workflows owe and nothing is serving. Returns the
    /// tasks it started a turn for.
    ///
    /// A task whose newest run failed, timed out or was cancelled waits for a
    /// person, as it does everywhere else: starting another attempt by itself
    /// is how a broken task burns its turns. The one exception is a run the
    /// host lost (the runner stopped heartbeating, which is what a restart
    /// looks like). That run is tried once more, with its turn given back; a
    /// second loss in a row waits for a person too.
    pub async fn resume_owed_turns(&self) -> Result<Vec<Uuid>> {
        let settled = (Utc::now() - Duration::seconds(SETTLE_SECS)).to_rfc3339();
        // Statuses a person owns are filtered by `dispatch_if_agent_owns`; the
        // ones that can never take a turn are left out here.
        let candidates: Vec<(Uuid,)> = sqlx::query_as(
            "select i.id from issues i
              where i.status not in ('blocked', 'done', 'cancelled')
                and i.updated_at < ?
                and not exists (select 1 from runs r where r.issue_id = i.id and r.status in ('queued', 'running'))
                and not exists (select 1 from runs r where r.issue_id = i.id and coalesce(r.finished_at, r.created_at) >= ?)
              order by i.updated_at",
        )
        .bind(&settled)
        .bind(&settled)
        .fetch_all(&self.db.pool)
        .await?;

        let mut started = Vec::new();
        for (issue_id,) in candidates {
            match self.resume_one(issue_id).await {
                Ok(true) => started.push(issue_id),
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(%issue_id, %error, "could not start a turn the task is owed");
                }
            }
        }
        Ok(started)
    }

    async fn resume_one(&self, issue_id: Uuid) -> Result<bool> {
        let issue = self.issue(issue_id).await?;
        // Only a turn the workflow names: with nobody named, a dispatch falls
        // back to the project's binding, which is a guess the sweep must not make.
        if self.next_turn_agent(&issue).await?.is_none() {
            return Ok(false);
        }
        if self.runs_since_cap(&issue).await? >= hulahoop_core::RUNS_PER_ISSUE {
            return Ok(false);
        }

        let newest: Vec<(Uuid, String, bool)> = sqlx::query_as(
            "select agent_id, status, coalesce(error, '') like ? from runs
              where issue_id = ? order by created_at desc limit 2",
        )
        .bind(LOST_RUN)
        .bind(issue_id)
        .fetch_all(&self.db.pool)
        .await?;
        let lost = match newest.first() {
            None => false,
            Some((_, status, _)) if status == "succeeded" => false,
            Some((_, status, lost)) if status == "failed" && *lost => {
                let lost_twice = newest
                    .get(1)
                    .is_some_and(|(_, status, lost)| status == "failed" && *lost);
                if lost_twice {
                    return Ok(false);
                }
                true
            }
            Some(_) => return Ok(false),
        };

        if lost {
            // The lost run spent a turn its agent never got to finish.
            let (agent_id, _, _) = newest[0];
            self.refund_turn(issue_id, agent_id).await;
        }
        let before = self.live_run_id(issue_id).await?;
        let issue = self.issue(issue_id).await?;
        Box::pin(self.dispatch_if_agent_owns(&issue)).await?;
        let after = self.live_run_id(issue_id).await?;
        let resumed = after.is_some() && after != before;
        if resumed {
            tracing::info!(issue = %issue.issue_key, lost, "started a turn the task was owed");
        }
        Ok(resumed)
    }

    async fn live_run_id(&self, issue_id: Uuid) -> Result<Option<Uuid>> {
        let row: Option<(Uuid,)> = sqlx::query_as(
            "select id from runs where issue_id = ? and status in ('queued', 'running') limit 1",
        )
        .bind(issue_id)
        .fetch_optional(&self.db.pool)
        .await?;
        Ok(row.map(|(id,)| id))
    }

    /// The sweep's loop: once right away, so a restart picks up where the last
    /// process left off, then every [`RESUME_EVERY`].
    pub async fn resume_loop(self: std::sync::Arc<Self>) {
        let mut ticker = tokio::time::interval(RESUME_EVERY);
        loop {
            ticker.tick().await;
            if let Err(error) = self.resume_owed_turns().await {
                tracing::warn!(%error, "looking for owed turns failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::db::Db;
    use crate::engine::Engine;
    use chrono::{DateTime, Duration, Utc};
    use uuid::Uuid;

    const DOC: &str = "# Workflows\n\nTurns: 6\n\n## Backlog\n\nUsers: human\n\nNext: To do\n\n## To do\n\nUsers: human\n\nNext: In progress\n\n## In progress\n\nUsers: @Developer, @Reviewer\n\nNext: In review\n\n## Blocked\n\nUsers: human\n\nNext: In progress\n\n## In review\n\nUsers: human\n\nNext: Done\n\n## Done\n\nUsers: human\n\nNext: none\n";

    struct Fixture {
        engine: Engine,
        directory: std::path::PathBuf,
        repo: Uuid,
        developer: Uuid,
        issue: Uuid,
    }

    /// A task in progress, owed the Developer's turn, last touched `idle` ago.
    async fn fixture(idle: Duration) -> Fixture {
        let directory = std::env::temp_dir().join(format!("hulahoop-resume-{}", Uuid::now_v7()));
        let db = Db::open(&directory).await.unwrap();
        let workspace = db.seed_local().await.unwrap();
        let (project, repo, developer, reviewer, issue) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        let then = Utc::now() - idle;
        sqlx::query(
            "insert into projects (id, workspace_id, name, created_at) values (?,?, 'Resume',?)",
        )
        .bind(project)
        .bind(workspace)
        .bind(then)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query("insert into repos (id, project_id, name, remote_url, contract_version, created_at) values (?,?, 'repo','/tmp/repo','1',?)")
            .bind(repo).bind(project).bind(then).execute(&db.pool).await.unwrap();
        for (id, name) in [(developer, "Developer"), (reviewer, "Reviewer")] {
            sqlx::query("insert into agents (id, workspace_id, name, adapter, model, created_at, updated_at) values (?,?,?,'pi','model-x',?,?)")
                .bind(id).bind(workspace).bind(name).bind(then).bind(then).execute(&db.pool).await.unwrap();
            sqlx::query("insert into agent_preflight (agent_id, repo_id, ok, detail, checked_at) values (?,?,1,'ok',?)")
                .bind(id).bind(repo).bind(then).execute(&db.pool).await.unwrap();
        }
        sqlx::query("insert into issues (id, workspace_id, project_id, repo_id, issue_key, number, title, status, turn_queue, turn_served, turns_used, created_at, updated_at) values (?,?,?,?, 'RES-1',1,'Task','in_progress','[\"Developer\",\"Reviewer\"]','[]',1,?,?)")
            .bind(issue).bind(workspace).bind(project).bind(repo).bind(then).bind(then)
            .execute(&db.pool).await.unwrap();
        let engine = Engine::new(db);
        engine.set_workflow(workspace, DOC).await.unwrap();
        // Setting the workflow is not a touch of the task.
        sqlx::query("update issues set updated_at = ? where id = ?")
            .bind(then)
            .bind(issue)
            .execute(&engine.db.pool)
            .await
            .unwrap();
        Fixture {
            engine,
            directory,
            repo,
            developer,
            issue,
        }
    }

    impl Fixture {
        async fn finished_run(&self, at: DateTime<Utc>, status: &str, error: Option<&str>) {
            sqlx::query("insert into runs (id, issue_id, repo_id, agent_id, attempt, base_bookmark, prompt_revision, artifact_dir, status, error, model, created_at, finished_at) values (?,?,?,?,1,'main','prompt','/tmp',?,?,'model-x',?,?)")
                .bind(Uuid::now_v7()).bind(self.issue).bind(self.repo).bind(self.developer)
                .bind(status).bind(error).bind(at).bind(at)
                .execute(&self.engine.db.pool).await.unwrap();
        }

        async fn live_runs(&self) -> i64 {
            let (count,): (i64,) = sqlx::query_as(
                "select count(*) from runs where issue_id = ? and status in ('queued', 'running')",
            )
            .bind(self.issue)
            .fetch_one(&self.engine.db.pool)
            .await
            .unwrap();
            count
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    const LOST: &str = "the runner stopped heartbeating, so the run was declared dead";

    /// The HUL-5 case: a task owed a turn that nothing started.
    #[tokio::test]
    async fn an_owed_turn_nobody_started_starts() {
        let task = fixture(Duration::minutes(10)).await;
        assert_eq!(
            task.engine.resume_owed_turns().await.unwrap(),
            vec![task.issue]
        );
        assert_eq!(task.live_runs().await, 1);
        // Once started, the next sweep leaves it alone.
        assert!(task.engine.resume_owed_turns().await.unwrap().is_empty());
        assert_eq!(task.live_runs().await, 1);
    }

    /// A task touched a moment ago belongs to whatever touched it.
    #[tokio::test]
    async fn a_task_just_touched_is_left_alone() {
        let task = fixture(Duration::seconds(5)).await;
        assert!(task.engine.resume_owed_turns().await.unwrap().is_empty());
        assert_eq!(task.live_runs().await, 0);
    }

    /// A failure is for a person to look at; starting again by itself is how a
    /// broken task burns its turns.
    #[tokio::test]
    async fn a_failed_run_waits_for_a_person() {
        let task = fixture(Duration::minutes(10)).await;
        task.finished_run(
            Utc::now() - Duration::minutes(5),
            "failed",
            Some("the agent exited with 1"),
        )
        .await;
        assert!(task.engine.resume_owed_turns().await.unwrap().is_empty());
        assert_eq!(task.live_runs().await, 0);
    }

    /// A run the host lost (a restart) is tried once more with its turn given
    /// back; a second loss in a row waits for a person.
    #[tokio::test]
    async fn a_run_lost_to_a_restart_is_tried_once_more() {
        let task = fixture(Duration::minutes(10)).await;
        task.finished_run(Utc::now() - Duration::minutes(5), "failed", Some(LOST))
            .await;
        assert_eq!(
            task.engine.resume_owed_turns().await.unwrap(),
            vec![task.issue]
        );
        assert_eq!(task.live_runs().await, 1);
        // The lost turn came back before the new one was spent.
        assert_eq!(task.engine.issue(task.issue).await.unwrap().turns_used, 1);

        let again = fixture(Duration::minutes(10)).await;
        again
            .finished_run(Utc::now() - Duration::minutes(6), "failed", Some(LOST))
            .await;
        again
            .finished_run(Utc::now() - Duration::minutes(5), "failed", Some(LOST))
            .await;
        assert!(again.engine.resume_owed_turns().await.unwrap().is_empty());
        assert_eq!(again.live_runs().await, 0);
    }
}
