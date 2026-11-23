//! Lumi, the workspace assistant. A question becomes a runner job that runs
//! the workspace's configured harness one-shot; the answer comes back as a
//! `lumi_messages` row, optionally carrying a proposal. Nothing in a proposal
//! runs until a person applies it — that is the contract the panel shows.

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::engine::Engine;
use crate::model::{Issue, LumiMessage, Workspace};

/// The system prompt: what Hulahoop is, what Lumi may propose, and the output
/// contract. Shipped with the server so every workspace speaks the same one.
pub const LUMI_PROMPT: &str = include_str!("lumi.md");

/// How many conversation turns ride along with a question. Enough to follow a
/// thread, small enough that the snapshot stays the biggest part.
const LUMI_HISTORY: i64 = 12;

impl Engine {
    pub async fn lumi_messages(&self, workspace_id: Uuid) -> Result<Vec<LumiMessage>> {
        Ok(sqlx::query_as(
            "select * from lumi_messages where workspace_id = ? order by created_at, id",
        )
        .bind(workspace_id)
        .fetch_all(&self.db.pool)
        .await?)
    }

    pub async fn set_workspace_lumi(
        &self,
        workspace_id: Uuid,
        harness: &str,
        model: &str,
    ) -> Result<Workspace> {
        if hulahoop_core::Adapter::parse(harness).is_none() {
            bail!("{harness} is not a harness this server knows");
        }
        // A Pi model id names its provider with a slash; the Claude CLI has no such
        // notion, so a saved pair like claude_code + anthropic/x can only fail at
        // answer time. Refusing it here points at the real mistake instead.
        if harness == "claude_code" && model.contains('/') {
            bail!("{model} is not a Claude Code model; pick one from its own catalogue");
        }
        sqlx::query("update workspaces set lumi_harness = ?, lumi_model = ? where id = ?")
            .bind(harness)
            .bind(model)
            .bind(workspace_id)
            .execute(&self.db.pool)
            .await?;
        Ok(sqlx::query_as("select * from workspaces where id = ?")
            .bind(workspace_id)
            .fetch_one(&self.db.pool)
            .await?)
    }

    /// Store the question and queue the job that answers it. The job needs a
    /// repository only for a dev shell to find the harness CLI in, so the
    /// workspace's first one serves.
    pub async fn ask_lumi(
        &self,
        workspace_id: Uuid,
        text: &str,
        context: Option<&str>,
    ) -> Result<LumiMessage> {
        self.ask_lumi_about(workspace_id, text, context, None).await
    }

    /// Ask Lumi with a task in hand. `@Lumi` in a task's comments comes here:
    /// the question carries the task itself, and the job remembers the task
    /// so the answer is posted back on it as well as in the panel.
    pub async fn ask_lumi_about(
        &self,
        workspace_id: Uuid,
        text: &str,
        context: Option<&str>,
        issue_id: Option<Uuid>,
    ) -> Result<LumiMessage> {
        let workspace: Workspace = sqlx::query_as("select * from workspaces where id = ?")
            .bind(workspace_id)
            .fetch_one(&self.db.pool)
            .await?;
        let (Some(harness), Some(model)) = (workspace.lumi_harness, workspace.lumi_model) else {
            bail!("Lumi has no harness yet. Pick one under Settings → Workspace.");
        };
        let repo: Option<(Uuid,)> = sqlx::query_as(
            "select r.id from repos r join projects p on p.id = r.project_id
             where p.workspace_id = ? order by r.created_at limit 1",
        )
        .bind(workspace_id)
        .fetch_optional(&self.db.pool)
        .await?;
        let Some((repo_id,)) = repo else {
            bail!("Lumi runs inside a repository's dev shell, and this workspace has none. Add a repository to a project first.");
        };

        let history: Vec<LumiMessage> = sqlx::query_as(
            "select * from (select * from lumi_messages where workspace_id = ?
             order by created_at desc, id desc limit ?) order by created_at, id",
        )
        .bind(workspace_id)
        .bind(LUMI_HISTORY)
        .fetch_all(&self.db.pool)
        .await?;

        let id = Uuid::now_v7();
        sqlx::query(
            "insert into lumi_messages (id, workspace_id, role, body, context, created_at)
             values (?, ?, 'user', ?, ?, ?)",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(text)
        .bind(context)
        .bind(crate::db::now_rfc3339())
        .execute(&self.db.pool)
        .await?;

        let snapshot = self.lumi_snapshot(workspace_id).await?;
        let system_prompt = format!(
            "{LUMI_PROMPT}\n\n## Workspace snapshot\n\n```json\n{}\n```\n",
            serde_json::to_string_pretty(&snapshot)?
        );
        let conversation: Vec<Value> = history
            .iter()
            .map(|m| json!({ "role": m.role, "text": m.body }))
            .collect();
        let mut message = match context {
            Some(context) => format!("Looking at: {context}\n\n{text}"),
            None => text.to_string(),
        };
        if let Some(issue_id) = issue_id {
            message = format!("{}\n\n{message}", self.lumi_task_brief(issue_id).await?);
        }

        self.queue_job(
            repo_id,
            issue_id,
            "lumi",
            json!({
                "workspace_id": workspace_id,
                "adapter": harness,
                "model": model,
                "system_prompt": system_prompt,
                "message": message,
                "history": conversation,
                "issue_id": issue_id,
            }),
        )
        .await?;
        Ok(sqlx::query_as("select * from lumi_messages where id = ?")
            .bind(id)
            .fetch_one(&self.db.pool)
            .await?)
    }

    /// The task a mention came from, written out for Lumi: what it is and what
    /// was said on it lately, newest last, so the reply fits the thread.
    async fn lumi_task_brief(&self, issue_id: Uuid) -> Result<String> {
        let issue = self.issue(issue_id).await?;
        let comments: Vec<(String, Option<String>, String)> = sqlx::query_as(
            "select author_type, author_id, body from (select * from comments where issue_id = ?
             order by created_at desc, id desc limit 10) order by created_at, id",
        )
        .bind(issue_id)
        .fetch_all(&self.db.pool)
        .await?;
        let mut brief = format!(
            "## Task {}: {}\n\nStatus: {}. Priority: {}.\n\n{}\n",
            issue.issue_key,
            issue.title,
            issue.status,
            issue.priority,
            issue.body.trim()
        );
        if !comments.is_empty() {
            brief.push_str("\n### Recent comments\n");
            for (author_type, author_id, body) in comments {
                let who = self.author_name(&author_type, author_id.as_deref()).await?;
                let body: String = body.chars().take(1500).collect();
                brief.push_str(&format!("\n**{who}:** {body}\n"));
            }
        }
        Ok(brief)
    }

    /// What Lumi knows: the open work, the people and agents around it, and the
    /// workflow document. Assembled server-side so the model never touches the
    /// database.
    pub async fn lumi_snapshot(&self, workspace_id: Uuid) -> Result<Value> {
        let issues: Vec<Issue> = sqlx::query_as(
            "select * from issues where workspace_id = ? order by created_at desc limit 200",
        )
        .bind(workspace_id)
        .fetch_all(&self.db.pool)
        .await?;
        let issue_rows: Vec<Value> = issues
            .iter()
            .map(|issue| {
                json!({
                    "key": issue.issue_key,
                    "title": issue.title,
                    "status": issue.status,
                    "priority": issue.priority,
                    "due_date": issue.due_date,
                    "has_description": !issue.body.trim().is_empty(),
                    "milestone_id": issue.milestone_id,
                    "pr_number": issue.pr_number,
                })
            })
            .collect();
        let milestones: Vec<(Uuid, String, String, Option<String>)> = sqlx::query_as(
            "select id, title, status, target_date from milestones where workspace_id = ?",
        )
        .bind(workspace_id)
        .fetch_all(&self.db.pool)
        .await?;
        let agents: Vec<(Uuid, String, String, String)> =
            sqlx::query_as("select id, name, adapter, model from agents where workspace_id = ?")
                .bind(workspace_id)
                .fetch_all(&self.db.pool)
                .await?;
        let mut agent_rows = Vec::new();
        for (id, name, adapter, model) in &agents {
            let last: Option<(String, Option<String>)> = sqlx::query_as(
                "select status, finished_at from runs where agent_id = ? order by created_at desc limit 1",
            )
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await?;
            agent_rows.push(json!({
                "name": name,
                "adapter": adapter,
                "model": model,
                "last_run": last.map(|(status, at)| json!({ "status": status, "finished_at": at })),
            }));
        }
        let workflow = self.workflow(workspace_id).await?;
        let projects: Vec<(String,)> =
            sqlx::query_as("select name from projects where workspace_id = ?")
                .bind(workspace_id)
                .fetch_all(&self.db.pool)
                .await?;
        Ok(json!({
            "projects": projects.iter().map(|(name,)| name).collect::<Vec<_>>(),
            "issues": issue_rows,
            "milestones": milestones.iter().map(|(id, title, status, due)| json!({
                "id": id, "title": title, "status": status, "target_date": due,
            })).collect::<Vec<_>>(),
            "agents": agent_rows,
            "workflow": workflow.source,
        }))
    }

    /// The runner finished a `lumi` job: keep the answer, or say why there is
    /// none. Called from `complete_job`, so a dead harness still answers.
    pub async fn lumi_complete(
        &self,
        job_id: Uuid,
        job_payload: &Value,
        result: &Value,
        error: Option<&str>,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ) -> Result<()> {
        let Some(workspace_id) = job_payload["workspace_id"]
            .as_str()
            .and_then(|raw| Uuid::parse_str(raw).ok())
        else {
            return Ok(());
        };
        let (body, proposal) = match error {
            Some(error) => (format!("Lumi could not answer: {error}"), None),
            None => {
                let reply = result["reply"].as_str().unwrap_or_default().to_string();
                let proposal = result.get("proposal").filter(|p| p.is_object()).cloned();
                if reply.is_empty() && proposal.is_none() {
                    ("Lumi answered with nothing.".to_string(), None)
                } else {
                    (reply, proposal)
                }
            }
        };
        let inserted = sqlx::query(
            "insert into lumi_messages (id, workspace_id, role, body, context, proposal, proposal_state, created_at, job_id)
             values (?, ?, 'lumi', ?, null, ?, ?, ?, ?)
             on conflict do nothing",
        )
        .bind(Uuid::now_v7())
        .bind(workspace_id)
        .bind(&body)
        .bind(proposal.as_ref().map(|p| p.to_string()))
        .bind(proposal.as_ref().map(|_| "open"))
        .bind(crate::db::now_rfc3339())
        .bind(job_id)
        .execute(&mut **tx)
        .await?;

        // A question asked on a task is answered on the task. Only the first
        // reply for a job is posted, so a retried job cannot answer twice. The
        // insert goes through the open transaction: the pool would wait on the
        // write lock this transaction holds.
        let issue_id = job_payload["issue_id"]
            .as_str()
            .and_then(|raw| Uuid::parse_str(raw).ok());
        if let (Some(issue_id), 1) = (issue_id, inserted.rows_affected()) {
            let body = if proposal.is_some() {
                format!("{body}\n\n_Lumi also proposed changes; review them in the Lumi panel._")
            } else {
                body
            };
            sqlx::query(
                "insert into comments (id, issue_id, author_type, author_id, body, pinned, created_at)
                 values (?, ?, 'server', 'lumi', ?, 0, ?)",
            )
            .bind(Uuid::now_v7())
            .bind(issue_id)
            .bind(body)
            .bind(crate::db::now_rfc3339())
            .execute(&mut **tx)
            .await?;
        }
        Ok(())
    }

    /// Run the ticked items of a proposal, in order, and stop at the first
    /// failure so the person sees exactly what happened and what did not.
    pub async fn apply_lumi_proposal(
        &self,
        message_id: Uuid,
        items: &[usize],
    ) -> Result<LumiMessage> {
        let message: LumiMessage = sqlx::query_as("select * from lumi_messages where id = ?")
            .bind(message_id)
            .fetch_one(&self.db.pool)
            .await?;
        if message.proposal_state.as_deref() != Some("open") {
            bail!("this proposal is not open any more");
        }
        let proposal: Value = serde_json::from_str(
            message
                .proposal
                .as_deref()
                .ok_or_else(|| anyhow!("the message carries no proposal"))?,
        )?;
        let all = proposal["items"].as_array().cloned().unwrap_or_default();
        for (applied, index) in items.iter().enumerate() {
            let item = all
                .get(*index)
                .ok_or_else(|| anyhow!("the proposal has no item {index}"))?;
            self.apply_lumi_item(message.workspace_id, item)
                .await
                .map_err(|error| {
                    anyhow!(
                        "“{}” failed: {error}. {applied} earlier change(s) were applied.",
                        item["text"].as_str().unwrap_or("an item")
                    )
                })?;
        }
        sqlx::query("update lumi_messages set proposal_state = 'applied' where id = ?")
            .bind(message_id)
            .execute(&self.db.pool)
            .await?;
        Ok(sqlx::query_as("select * from lumi_messages where id = ?")
            .bind(message_id)
            .fetch_one(&self.db.pool)
            .await?)
    }

    pub async fn dismiss_lumi_proposal(&self, message_id: Uuid) -> Result<LumiMessage> {
        sqlx::query(
            "update lumi_messages set proposal_state = 'dismissed' where id = ? and proposal_state = 'open'",
        )
        .bind(message_id)
        .execute(&self.db.pool)
        .await?;
        Ok(sqlx::query_as("select * from lumi_messages where id = ?")
            .bind(message_id)
            .fetch_one(&self.db.pool)
            .await?)
    }

    async fn lumi_issue(&self, workspace_id: Uuid, args: &Value) -> Result<Issue> {
        let key = args["issue"]
            .as_str()
            .ok_or_else(|| anyhow!("the item names no issue"))?;
        sqlx::query_as("select * from issues where workspace_id = ? and issue_key = ?")
            .bind(workspace_id)
            .bind(key)
            .fetch_optional(&self.db.pool)
            .await?
            .ok_or_else(|| anyhow!("there is no issue {key} in this workspace"))
    }

    /// One proposal item. The vocabulary is deliberately the same set of moves
    /// a person has on the screen — Lumi gets no door a human lacks.
    async fn apply_lumi_item(&self, workspace_id: Uuid, item: &Value) -> Result<()> {
        let args = &item["args"];
        match item["action"].as_str().unwrap_or_default() {
            "create_issue" => {
                let title = args["title"]
                    .as_str()
                    .ok_or_else(|| anyhow!("create_issue needs a title"))?;
                let project: Option<(Uuid,)> = match args["project"].as_str() {
                    Some(name) => {
                        sqlx::query_as(
                            "select id from projects where workspace_id = ? and name = ?",
                        )
                        .bind(workspace_id)
                        .bind(name)
                        .fetch_optional(&self.db.pool)
                        .await?
                    }
                    None => {
                        sqlx::query_as(
                            "select id from projects where workspace_id = ? order by created_at limit 1",
                        )
                        .bind(workspace_id)
                        .fetch_optional(&self.db.pool)
                        .await?
                    }
                };
                let project_id = project.map(|(id,)| id);
                let repo: Option<(Uuid,)> =
                    match project_id {
                        Some(project) => sqlx::query_as(
                            "select id from repos where project_id = ? order by created_at limit 1",
                        )
                        .bind(project)
                        .fetch_optional(&self.db.pool)
                        .await?,
                        None => None,
                    };
                let Some((repo_id,)) = repo else {
                    bail!("the project has no repository, and an issue needs one");
                };
                let priority = crate::engine::priority(args["priority"].as_str())?;
                let (number, issue_key) = self.next_issue_key(workspace_id, project_id).await?;
                let id = Uuid::now_v7();
                let now = crate::db::now_rfc3339();
                sqlx::query(
                    "insert into issues (id, workspace_id, project_id, repo_id, number, issue_key, title, body, status, priority, created_at, updated_at)
                     values (?, ?, ?, ?, ?, ?, ?, ?, 'backlog', ?, ?, ?)",
                )
                .bind(id)
                .bind(workspace_id)
                .bind(project_id)
                .bind(repo_id)
                .bind(number)
                .bind(&issue_key)
                .bind(title)
                .bind(args["body"].as_str().unwrap_or_default())
                .bind(priority)
                .bind(&now)
                .bind(&now)
                .execute(&self.db.pool)
                .await?;
            }
            "update_issue" => {
                let issue = self.lumi_issue(workspace_id, args).await?;
                let priority = match args["priority"].as_str() {
                    Some(value) => Some(crate::engine::priority(Some(value))?),
                    None => None,
                };
                for (column, value) in [
                    ("title", args["title"].as_str()),
                    ("body", args["body"].as_str()),
                    ("priority", priority),
                ] {
                    if let Some(value) = value {
                        sqlx::query(&format!(
                            "update issues set {column} = ?, updated_at = ? where id = ?"
                        ))
                        .bind(value)
                        .bind(crate::db::now_rfc3339())
                        .bind(issue.id)
                        .execute(&self.db.pool)
                        .await?;
                    }
                }
            }
            "plan_issue" => {
                let issue = self.lumi_issue(workspace_id, args).await?;
                self.transition(issue.id, hulahoop_core::IssueEvent::Plan)
                    .await?;
            }
            "move_issue" => {
                let issue = self.lumi_issue(workspace_id, args).await?;
                let target = args["status"]
                    .as_str()
                    .and_then(hulahoop_core::IssueStatus::parse)
                    .ok_or_else(|| anyhow!("move_issue needs a status the board knows"))?;
                self.move_declared(issue.id, target).await?;
            }
            "comment" => {
                let issue = self.lumi_issue(workspace_id, args).await?;
                let body = args["body"]
                    .as_str()
                    .ok_or_else(|| anyhow!("comment needs a body"))?;
                self.comment(
                    issue.id,
                    hulahoop_core::AuthorType::Server,
                    Some("lumi"),
                    body,
                    false,
                )
                .await?;
            }
            "start_run" => {
                let issue = self.lumi_issue(workspace_id, args).await?;
                self.start_run(issue.id, None, None).await?;
            }
            other => bail!("“{other}” is not an action Lumi may take"),
        }
        Ok(())
    }
}

#[cfg(test)]
mod lumi_settings_tests {
    use crate::db::Db;
    use crate::engine::Engine;

    #[tokio::test]
    async fn claude_code_refuses_a_pi_model_id() {
        let directory =
            std::env::temp_dir().join(format!("hulahoop-lumi-set-{}", uuid::Uuid::now_v7()));
        let db = Db::open(&directory).await.unwrap();
        let workspace = db.seed_local().await.unwrap();
        let engine = Engine::new(db);

        let refused = engine
            .set_workspace_lumi(workspace, "claude_code", "anthropic/claude-fable-5-2")
            .await;
        assert!(refused.is_err());
        assert!(refused.unwrap_err().to_string().contains("Claude Code"));

        let saved = engine
            .set_workspace_lumi(workspace, "claude_code", "sonnet")
            .await
            .unwrap();
        assert_eq!(saved.lumi_model.as_deref(), Some("sonnet"));

        let pi = engine
            .set_workspace_lumi(workspace, "pi", "anthropic/claude-fable-5-2")
            .await
            .unwrap();
        assert_eq!(pi.lumi_model.as_deref(), Some("anthropic/claude-fable-5-2"));

        let _ = std::fs::remove_dir_all(directory);
    }
}
