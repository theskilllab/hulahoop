//! The task board's reads and the task fields a person sets by hand.
//!
//! The board asks for many tasks at once, so everything here is a single query
//! per question: the list, the count of finished tasks it hides, the effort a
//! task has cost so far. Nothing nested is loaded unless the screen asks.
//!
//! Due date and assignee are information for people. The workflow decides who
//! works a task; an assignee only says who is on it, and an agent becomes the
//! assignee when it starts a turn.

use anyhow::{bail, Result};
use hulahoop_core::IssueStatus;
use uuid::Uuid;

use crate::engine::Engine;
use crate::model::Issue;

/// What a task has cost so far, summed over every run it has had.
#[derive(Debug, Clone, Default, PartialEq, async_graphql::SimpleObject)]
pub struct IssueStats {
    pub runs: i64,
    pub failed_runs: i64,
    pub turns: i64,
    pub tool_calls: i64,
    pub tokens: i64,
    pub api_equivalent_usd: f64,
    /// Wall-clock minutes of finished runs, rounded down.
    pub active_minutes: i64,
}

/// Who a task is assigned to, as a person reads it.
#[derive(Debug, Clone, PartialEq, async_graphql::SimpleObject)]
pub struct Assignee {
    /// `human` or `agent`.
    #[graphql(name = "type")]
    pub kind: String,
    /// The agent's id; `me` for the person using the app.
    pub id: String,
    pub name: String,
}

/// The statuses the Active view hides: the work is over either way.
const FINISHED: [IssueStatus; 2] = [IssueStatus::Done, IssueStatus::Cancelled];

/// The id the local person is stored under. There is one person per install,
/// so the id carries no information; it only marks the assignment as human.
pub const LOCAL_HUMAN: &str = "me";

impl Engine {
    /// A workspace's tasks, newest first, optionally within one project and one
    /// status. Finished tasks are left out unless asked for, because a board
    /// that keeps every finished task forever buries the work in flight.
    pub async fn list_issues(
        &self,
        workspace_id: Uuid,
        project_id: Option<Uuid>,
        status: Option<&str>,
        include_done: bool,
    ) -> Result<Vec<Issue>> {
        let rows = sqlx::query_as::<_, Issue>(
            "select * from issues
              where workspace_id = ?
                and (? is null or project_id = ?)
                and (? is null or status = ?)
                and (? or status not in (?, ?))
              order by created_at desc",
        )
        .bind(workspace_id)
        .bind(project_id)
        .bind(project_id)
        .bind(status)
        .bind(status)
        .bind(include_done)
        .bind(FINISHED[0].as_str())
        .bind(FINISHED[1].as_str())
        .fetch_all(&self.db.pool)
        .await?;
        Ok(rows)
    }

    /// How many finished tasks the Active view is hiding.
    pub async fn done_issue_count(
        &self,
        workspace_id: Uuid,
        project_id: Option<Uuid>,
    ) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "select count(*) from issues
              where workspace_id = ?
                and (? is null or project_id = ?)
                and status in (?, ?)",
        )
        .bind(workspace_id)
        .bind(project_id)
        .bind(project_id)
        .bind(FINISHED[0].as_str())
        .bind(FINISHED[1].as_str())
        .fetch_one(&self.db.pool)
        .await?)
    }

    /// One aggregate over the task's runs. A run that never reported a metric
    /// adds nothing, so legacy rows count as zero rather than unknown.
    pub async fn issue_stats(&self, issue_id: Uuid) -> Result<IssueStats> {
        let row: (i64, i64, i64, i64, i64, f64, f64) = sqlx::query_as(
            "select count(*),
                    coalesce(sum(status = 'failed'), 0),
                    coalesce(sum(model_turns), 0),
                    coalesce(sum(tool_calls), 0),
                    coalesce(sum(tokens_in + tokens_out), 0),
                    coalesce(sum(api_equivalent_usd), 0.0),
                    coalesce(sum(case when started_at is not null and finished_at is not null
                                      then (julianday(finished_at) - julianday(started_at)) * 1440.0
                                 end), 0.0)
               from runs where issue_id = ?",
        )
        .bind(issue_id)
        .fetch_one(&self.db.pool)
        .await?;
        Ok(IssueStats {
            runs: row.0,
            failed_runs: row.1,
            turns: row.2,
            tool_calls: row.3,
            tokens: row.4,
            api_equivalent_usd: row.5,
            active_minutes: row.6.max(0.0).floor() as i64,
        })
    }

    /// The task's assignee with a name a person recognises, or `None`.
    pub async fn assignee(&self, issue: &Issue) -> Result<Option<Assignee>> {
        match issue.assignee_type.as_deref() {
            Some("human") => Ok(Some(Assignee {
                kind: "human".into(),
                id: LOCAL_HUMAN.into(),
                name: "you".into(),
            })),
            Some("agent") => {
                let Some(id) = issue.assignee_id.clone() else {
                    return Ok(None);
                };
                let name = self.author_name("agent", Some(&id)).await?;
                Ok(Some(Assignee {
                    kind: "agent".into(),
                    id,
                    name,
                }))
            }
            _ => Ok(None),
        }
    }

    /// Set or clear the assignee. An empty type clears it; an agent must be
    /// one of the workspace's own.
    pub async fn assign_issue(
        &self,
        issue_id: Uuid,
        kind: Option<&str>,
        id: Option<&str>,
    ) -> Result<()> {
        let issue = self.issue(issue_id).await?;
        let (kind, id) = match kind.map(str::trim).filter(|k| !k.is_empty()) {
            None => (None, None),
            Some("human") => (Some("human"), Some(LOCAL_HUMAN.to_string())),
            Some("agent") => {
                let Some(agent_id) = id.and_then(|raw| Uuid::parse_str(raw.trim()).ok()) else {
                    bail!("an agent assignee needs the agent's id");
                };
                let known: Option<i64> =
                    sqlx::query_scalar("select 1 from agents where id = ? and workspace_id = ?")
                        .bind(agent_id)
                        .bind(issue.workspace_id)
                        .fetch_optional(&self.db.pool)
                        .await?;
                if known.is_none() {
                    bail!("agent {agent_id} is not in this workspace");
                }
                (Some("agent"), Some(agent_id.to_string()))
            }
            Some(other) => bail!("assignee type must be human or agent, not {other:?}"),
        };
        sqlx::query(
            "update issues set assignee_type = ?, assignee_id = ?, updated_at = ? where id = ?",
        )
        .bind(kind)
        .bind(id)
        .bind(crate::db::now_rfc3339())
        .bind(issue_id)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Set or clear the due date. A blank value clears it.
    pub async fn set_due_date(&self, issue_id: Uuid, value: &str) -> Result<()> {
        let due = crate::engine::due_date(value)?;
        sqlx::query("update issues set due_date = ?, updated_at = ? where id = ?")
            .bind(due)
            .bind(crate::db::now_rfc3339())
            .bind(issue_id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::db::Db;
    use crate::engine::Engine;
    use chrono::{Duration, Utc};
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;
    use uuid::Uuid;

    const DOC: &str = "# Workflows\n\nTurns: 6\n\n## Backlog\n\nUsers: human\n\nNext: To do\n\n## To do\n\nUsers: human\n\nNext: In progress\n\n## In progress\n\nUsers: @Developer, @Reviewer\n\nNext: In progress - @Reviewer asks @Developer to work on it again\n\nNext: In review\n\n## Blocked\n\nUsers: human\n\nNext: In progress\n\n## In review\n\nUsers: human\n\nNext: Done\n\n## Done\n\nUsers: human\n\nNext: none\n";

    /// A workspace with one project, one repository, three agents that all
    /// passed preflight, and the workflow above.
    struct Fixture {
        engine: Engine,
        directory: std::path::PathBuf,
        workspace: Uuid,
        project: Uuid,
        repo: Uuid,
        developer: Uuid,
        reviewer: Uuid,
        writer: Uuid,
    }

    impl Fixture {
        async fn new() -> Self {
            let directory = std::env::temp_dir().join(format!("hulahoop-tasks-{}", Uuid::now_v7()));
            let db = Db::open(&directory).await.unwrap();
            let workspace = db.seed_local().await.unwrap();
            let (project, repo) = (Uuid::now_v7(), Uuid::now_v7());
            let (developer, reviewer, writer) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
            let now = Utc::now() - Duration::seconds(30);
            sqlx::query(
                "insert into projects (id, workspace_id, name, created_at) values (?,?, 'Tasks',?)",
            )
            .bind(project)
            .bind(workspace)
            .bind(now)
            .execute(&db.pool)
            .await
            .unwrap();
            sqlx::query("insert into repos (id, project_id, name, remote_url, contract_version, created_at) values (?,?, 'repo','/tmp/repo','1',?)")
                .bind(repo).bind(project).bind(now).execute(&db.pool).await.unwrap();
            for (id, name) in [
                (developer, "Developer"),
                (reviewer, "Reviewer"),
                (writer, "Test Writer"),
            ] {
                sqlx::query("insert into agents (id, workspace_id, name, adapter, model, created_at, updated_at) values (?,?,?,'pi','model-x',?,?)")
                    .bind(id).bind(workspace).bind(name).bind(now).bind(now).execute(&db.pool).await.unwrap();
                sqlx::query("insert into agent_preflight (agent_id, repo_id, ok, detail, checked_at) values (?,?,1,'ok',?)")
                    .bind(id).bind(repo).bind(now).execute(&db.pool).await.unwrap();
            }
            let engine = Engine::new(db);
            engine.set_workflow(workspace, DOC).await.unwrap();
            Self {
                engine,
                directory,
                workspace,
                project,
                repo,
                developer,
                reviewer,
                writer,
            }
        }

        /// A task in `status`, whose turn queue has served `served` already.
        async fn issue(&self, number: i64, status: &str, served: &[Uuid]) -> Uuid {
            let id = Uuid::now_v7();
            let served: Vec<String> = served.iter().map(Uuid::to_string).collect();
            let now = Utc::now() - Duration::seconds(20 - number);
            sqlx::query("insert into issues (id, workspace_id, project_id, repo_id, issue_key, number, title, status, turn_queue, turn_served, turns_used, created_at, updated_at) values (?,?,?,?,?,?,'Task',?,'[\"Developer\",\"Reviewer\"]',?,2,?,?)")
                .bind(id).bind(self.workspace).bind(self.project).bind(self.repo)
                .bind(format!("TSK-{number}")).bind(number).bind(status)
                .bind(serde_json::json!(served).to_string())
                .bind(now).bind(now).execute(&self.engine.db.pool).await.unwrap();
            id
        }

        async fn runs(&self, issue: Uuid) -> Vec<(Uuid, String)> {
            sqlx::query_as("select agent_id, status from runs where issue_id = ?")
                .bind(issue)
                .fetch_all(&self.engine.db.pool)
                .await
                .unwrap()
        }

        async fn bodies(&self, issue: Uuid) -> Vec<String> {
            sqlx::query_scalar("select body from comments where issue_id = ? order by rowid")
                .bind(issue)
                .fetch_all(&self.engine.db.pool)
                .await
                .unwrap()
        }

        fn done(self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    /// Criteria are text a person wrote; the migration that drops the table
    /// keeps that text in the task body, in order, and fixes stray priorities.
    #[tokio::test]
    async fn migration_keeps_criteria_in_the_body() {
        let directory = std::env::temp_dir().join(format!("hulahoop-0022-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&directory).unwrap();
        let options = SqliteConnectOptions::from_str(&format!(
            "sqlite://{}",
            directory.join("db.sqlite").display()
        ))
        .unwrap()
        .create_if_missing(true)
        .foreign_keys(false);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        let mut before = sqlx::migrate!("./migrations");
        before.migrations = before
            .migrations
            .iter()
            .filter(|migration| migration.version < 22)
            .cloned()
            .collect::<Vec<_>>()
            .into();
        before.run(&pool).await.unwrap();

        let (with, without) = (Uuid::now_v7(), Uuid::now_v7());
        for (id, number, priority) in [(with, 1, "med"), (without, 2, "whenever")] {
            sqlx::query("insert into issues (id, workspace_id, project_id, repo_id, issue_key, number, title, body, priority, created_at, updated_at) values (?,?,?,?,?,?,'Task','Do the thing.',?,'2026-01-01','2026-01-01')")
                .bind(id).bind(Uuid::now_v7()).bind(Uuid::now_v7()).bind(Uuid::now_v7())
                .bind(format!("MIG-{number}")).bind(number).bind(priority)
                .execute(&pool).await.unwrap();
        }
        for (position, text) in [(2, "second"), (1, "first")] {
            sqlx::query("insert into criteria (id, issue_id, position, text) values (?,?,?,?)")
                .bind(Uuid::now_v7())
                .bind(with)
                .bind(position)
                .bind(text)
                .execute(&pool)
                .await
                .unwrap();
        }

        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        let rows: Vec<(String, String)> =
            sqlx::query_as("select body, priority from issues order by number")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            rows[0].0,
            "Do the thing.\n\n## Acceptance criteria\n\n- [ ] first\n- [ ] second"
        );
        assert_eq!(rows[1].0, "Do the thing.");
        assert_eq!(rows[0].1, "medium");
        assert_eq!(rows[1].1, "medium");
        let criteria: Option<String> = sqlx::query_scalar(
            "select name from sqlite_master where type = 'table' and name = 'criteria'",
        )
        .fetch_optional(&pool)
        .await
        .unwrap();
        assert!(criteria.is_none());
        pool.close().await;
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn priorities_are_the_four_rungs() {
        assert_eq!(crate::engine::priority(None).unwrap(), "medium");
        assert_eq!(crate::engine::priority(Some("med")).unwrap(), "medium");
        assert_eq!(crate::engine::priority(Some("Urgent")).unwrap(), "urgent");
        let error = crate::engine::priority(Some("p0")).unwrap_err().to_string();
        assert!(error.contains("urgent, high, medium, low"), "{error}");
    }

    #[tokio::test]
    async fn due_dates_set_clear_and_reject_nonsense() {
        let fixture = Fixture::new().await;
        let issue = fixture.issue(1, "todo", &[]).await;
        fixture
            .engine
            .set_due_date(issue, "2026-03-31")
            .await
            .unwrap();
        let task = fixture.engine.issue(issue).await.unwrap();
        assert_eq!(task.due_date.as_deref(), Some("2026-03-31"));
        assert!(fixture
            .engine
            .set_due_date(issue, "31/03/2026")
            .await
            .is_err());
        fixture.engine.set_due_date(issue, "").await.unwrap();
        assert_eq!(fixture.engine.issue(issue).await.unwrap().due_date, None);
        fixture.done();
    }

    /// Agents assign themselves: starting a turn makes the agent the assignee,
    /// whoever was on the task before.
    #[tokio::test]
    async fn starting_a_run_assigns_the_agent() {
        let fixture = Fixture::new().await;
        let issue = fixture.issue(1, "in_progress", &[]).await;
        fixture
            .engine
            .assign_issue(issue, Some("human"), None)
            .await
            .unwrap();
        fixture
            .engine
            .start_run(issue, Some(fixture.developer), None)
            .await
            .unwrap();
        let task = fixture.engine.issue(issue).await.unwrap();
        let assignee = fixture.engine.assignee(&task).await.unwrap().unwrap();
        assert_eq!(assignee.kind, "agent");
        assert_eq!(assignee.id, fixture.developer.to_string());
        assert_eq!(assignee.name, "Developer");
        assert!(fixture
            .engine
            .assign_issue(issue, Some("agent"), Some("not-an-id"))
            .await
            .is_err());
        fixture.done();
    }

    #[tokio::test]
    async fn active_view_hides_finished_tasks_and_counts_them() {
        let fixture = Fixture::new().await;
        let open = fixture.issue(1, "todo", &[]).await;
        fixture.issue(2, "done", &[]).await;
        fixture.issue(3, "cancelled", &[]).await;
        let engine = &fixture.engine;

        let active = engine
            .list_issues(fixture.workspace, None, None, false)
            .await
            .unwrap();
        assert_eq!(active.iter().map(|i| i.id).collect::<Vec<_>>(), vec![open]);
        let all = engine
            .list_issues(fixture.workspace, Some(fixture.project), None, true)
            .await
            .unwrap();
        assert_eq!(all.len(), 3);
        let done = engine
            .list_issues(fixture.workspace, None, Some("done"), true)
            .await
            .unwrap();
        assert_eq!(done.len(), 1);
        assert_eq!(
            engine
                .done_issue_count(fixture.workspace, &[fixture.project], false)
                .await
                .unwrap(),
            2
        );
        fixture.done();
    }

    /// The "+N" toggle follows the project filter: several projects at once,
    /// and "No project" as a choice of its own.
    #[tokio::test]
    async fn done_count_follows_a_multi_project_filter() {
        let fixture = Fixture::new().await;
        let other = Uuid::now_v7();
        sqlx::query("insert into projects (id, workspace_id, name, created_at) values (?,?, 'Other','2026-01-01')")
            .bind(other)
            .bind(fixture.workspace)
            .execute(&fixture.engine.db.pool)
            .await
            .unwrap();
        // One finished task each in the first project, the other, and none;
        // the first project also has a second one, and an open task is ignored.
        let in_first = fixture.issue(1, "done", &[]).await;
        let in_first_too = fixture.issue(2, "cancelled", &[]).await;
        let in_other = fixture.issue(3, "done", &[]).await;
        let loose = fixture.issue(4, "done", &[]).await;
        fixture.issue(5, "todo", &[]).await;
        let _ = (in_first, in_first_too);
        sqlx::query("update issues set project_id = ? where id = ?")
            .bind(other)
            .bind(in_other)
            .execute(&fixture.engine.db.pool)
            .await
            .unwrap();
        sqlx::query("update issues set project_id = null where id = ?")
            .bind(loose)
            .execute(&fixture.engine.db.pool)
            .await
            .unwrap();
        let count = |projects: Vec<Uuid>, none: bool| {
            let engine = &fixture.engine;
            let workspace = fixture.workspace;
            async move {
                engine
                    .done_issue_count(workspace, &projects, none)
                    .await
                    .unwrap()
            }
        };
        assert_eq!(count(vec![], false).await, 4, "no filter counts everything");
        assert_eq!(count(vec![fixture.project], false).await, 2);
        assert_eq!(count(vec![], true).await, 1, "no project only");
        assert_eq!(
            count(vec![fixture.project, other], false).await,
            3,
            "two projects"
        );
        assert_eq!(
            count(vec![other], true).await,
            2,
            "one project plus no project"
        );
        fixture.done();
    }

    #[tokio::test]
    async fn stats_sum_every_run() {
        let fixture = Fixture::new().await;
        let issue = fixture.issue(1, "in_progress", &[]).await;
        for (status, turns, tools, tokens_in, tokens_out, usd, minutes) in [
            ("succeeded", 4, 10, 1000, 200, 0.5, 30),
            ("failed", 1, 2, 100, 50, 0.25, 15),
        ] {
            let started = Utc::now() - Duration::minutes(60);
            sqlx::query("insert into runs (id, issue_id, repo_id, agent_id, attempt, base_bookmark, prompt_revision, artifact_dir, status, model, created_at, started_at, finished_at, model_turns, tool_calls, tokens_in, tokens_out, api_equivalent_usd) values (?,?,?,?,1,'main','prompt','/tmp',?,'model-x',?,?,?,?,?,?,?,?)")
                .bind(Uuid::now_v7()).bind(issue).bind(fixture.repo).bind(fixture.developer)
                .bind(status).bind(started).bind(started).bind(started + Duration::minutes(minutes))
                .bind(turns).bind(tools).bind(tokens_in).bind(tokens_out).bind(usd)
                .execute(&fixture.engine.db.pool).await.unwrap();
        }
        let stats = fixture.engine.issue_stats(issue).await.unwrap();
        assert_eq!(stats.runs, 2);
        assert_eq!(stats.failed_runs, 1);
        assert_eq!(stats.turns, 5);
        assert_eq!(stats.tool_calls, 12);
        assert_eq!(stats.tokens, 1350);
        assert!((stats.api_equivalent_usd - 0.75).abs() < 1e-9);
        assert!(
            (44..=45).contains(&stats.active_minutes),
            "{}",
            stats.active_minutes
        );
        fixture.done();
    }

    /// A mention gives a stuck task the turn its workflow owes the agent.
    #[tokio::test]
    async fn mentioning_a_queued_agent_on_a_stuck_task_starts_its_turn() {
        let fixture = Fixture::new().await;
        let issue = fixture
            .issue(1, "in_progress", &[fixture.developer, fixture.reviewer])
            .await;
        let comment = fixture
            .engine
            .human_comment(issue, "@developer please look at the failing build")
            .await
            .unwrap();

        let runs = fixture.runs(issue).await;
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].0, fixture.developer);
        let bodies = fixture.bodies(issue).await;
        assert_eq!(
            bodies.last().unwrap(),
            "Notified Developer: starting its turn"
        );
        let mentions: String = sqlx::query_scalar("select mentions from comments where id = ?")
            .bind(comment)
            .fetch_one(&fixture.engine.db.pool)
            .await
            .unwrap();
        assert!(mentions.contains("\"Developer\""), "{mentions}");

        // With a run in flight, a second mention starts nothing.
        fixture
            .engine
            .human_comment(issue, "@Reviewer heads up")
            .await
            .unwrap();
        assert_eq!(fixture.runs(issue).await.len(), 1);
        assert_eq!(
            fixture.bodies(issue).await.last().unwrap(),
            "Notified Reviewer: it will see this on its next turn"
        );
        fixture.done();
    }

    /// A mention never goes around the workflow: an agent the status does not
    /// name, or a blocked task, only gets a note.
    #[tokio::test]
    async fn mentions_outside_the_workflow_are_only_noted() {
        let fixture = Fixture::new().await;
        let issue = fixture.issue(1, "in_progress", &[fixture.developer]).await;
        fixture
            .engine
            .human_comment(issue, "@Test Writer can you add coverage?")
            .await
            .unwrap();
        assert!(fixture.runs(issue).await.is_empty());
        assert_eq!(
            fixture.bodies(issue).await.last().unwrap(),
            "Test Writer has no turn in In progress per WORKFLOWS.md; it will see this when it next works the task"
        );

        let blocked = fixture.issue(2, "blocked", &[]).await;
        fixture
            .engine
            .human_comment(blocked, "@Developer the answer is yes")
            .await
            .unwrap();
        assert!(fixture.runs(blocked).await.is_empty());
        assert!(fixture
            .bodies(blocked)
            .await
            .last()
            .unwrap()
            .starts_with("Notified Developer: the task is Blocked"));
        let _ = fixture.writer;
        fixture.done();
    }

    #[tokio::test]
    async fn lumi_without_a_harness_explains_itself() {
        let fixture = Fixture::new().await;
        let issue = fixture.issue(1, "todo", &[]).await;
        fixture
            .engine
            .human_comment(issue, "@Lumi what is left here?")
            .await
            .unwrap();
        let note = fixture.bodies(issue).await.last().unwrap().clone();
        assert!(note.starts_with("Lumi can't answer here:"), "{note}");
        let jobs: i64 = sqlx::query_scalar("select count(*) from runner_jobs where kind = 'lumi'")
            .fetch_one(&fixture.engine.db.pool)
            .await
            .unwrap();
        assert_eq!(jobs, 0);
        fixture.done();
    }
}
