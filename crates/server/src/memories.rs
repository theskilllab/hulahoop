//! Long-term memory on the server: saving what agents and people say is worth
//! keeping. Nothing here waits for approval; the rules in `hulahoop_core::memory`
//! (per-run cap, near-duplicates, replaced decisions) keep the store small.

use anyhow::Result;
use hulahoop_core::memory::{
    normalize_tokens, parse_lines, similarity, MemoryLine, MEMORY_BODY_MAX, MEMORY_DEDUPE_MIN,
    MEMORY_TITLE_MAX,
};
use uuid::Uuid;

use crate::engine::Engine;
use crate::model::{Issue, Run};

/// A memory to save, with whose it is and where it came from.
#[derive(Debug, Clone)]
pub struct NewMemory {
    pub workspace_id: Uuid,
    pub scope: String,
    /// The project id for `project`, the name for `agent` and `person`.
    pub scope_ref: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub source_issue_id: Option<Uuid>,
    pub source_run_id: Option<Uuid>,
    pub source_comment_id: Option<Uuid>,
    pub author: String,
}

/// What saving did, so a caller can tell an added note from an updated one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Saved {
    Added(Uuid),
    Updated(Uuid),
    /// A newer decision took the place of the older one.
    Replaced {
        new: Uuid,
        old: Uuid,
    },
}

type Existing = (Uuid, String, String, String);

impl Engine {
    /// Saves every `HULAHOOP_MEMORY:` line in an agent's closing message. A line
    /// that cannot be saved is logged and skipped; it never fails the turn.
    pub async fn save_run_memories(&self, run: &Run, issue: &Issue, text: &str) -> Result<()> {
        for parsed in parse_lines(text) {
            let saved = match parsed {
                Ok(line) => self.memory_from_line(run, issue, line).await,
                Err(reason) => Err(anyhow::anyhow!(reason)),
            };
            let outcome = match saved {
                Ok(new) => self.save_memory(&new).await,
                Err(error) => Err(error),
            };
            if let Err(error) = outcome {
                tracing::warn!(run_id = %run.id, "dropping a memory: {error:#}");
            }
        }
        Ok(())
    }

    /// A person's answer to a blocked question becomes a `decision` for the
    /// task's project. A task with no project has nowhere to keep it.
    pub async fn remember_answer(
        &self,
        issue: &Issue,
        comment_id: Uuid,
        question: &str,
        answer: &str,
    ) -> Result<()> {
        let Some(project_id) = issue.project_id else {
            return Ok(());
        };
        self.save_memory(&NewMemory {
            workspace_id: issue.workspace_id,
            scope: "project".into(),
            scope_ref: project_id.to_string(),
            kind: "decision".into(),
            title: clip(question.trim(), MEMORY_TITLE_MAX),
            body: clip(answer.trim(), MEMORY_BODY_MAX),
            source_issue_id: Some(issue.id),
            source_run_id: None,
            source_comment_id: Some(comment_id),
            author: "human".into(),
        })
        .await?;
        Ok(())
    }

    async fn memory_from_line(
        &self,
        run: &Run,
        issue: &Issue,
        line: MemoryLine,
    ) -> Result<NewMemory> {
        let agent: String = sqlx::query_scalar("select name from agents where id = ?")
            .bind(run.agent_id)
            .fetch_one(&self.db.pool)
            .await?;
        let (base, named) = match line.scope.split_once(':') {
            Some((base, name)) => (base, Some(name)),
            None => (line.scope.as_str(), None),
        };
        let scope_ref = match (base, named) {
            ("workspace", _) => String::new(),
            ("project", _) => issue
                .project_id
                .map(|id| id.to_string())
                .ok_or_else(|| anyhow::anyhow!("the task belongs to no project"))?,
            ("agent", name) => name.map_or_else(|| agent.clone(), str::to_owned),
            (_, Some(name)) => name.to_owned(),
            _ => anyhow::bail!("a person memory must say whose: `person:<name>`"),
        };
        Ok(NewMemory {
            workspace_id: issue.workspace_id,
            scope: base.into(),
            scope_ref,
            kind: line.kind,
            title: line.title,
            body: line.body,
            source_issue_id: Some(issue.id),
            source_run_id: Some(run.id),
            source_comment_id: None,
            author: agent,
        })
    }

    /// Saves one memory. A note that nearly repeats a live one in the same
    /// scope updates it; a changed `decision` on the same subject replaces it.
    pub async fn save_memory(&self, new: &NewMemory) -> Result<Saved> {
        let live: Vec<Existing> = sqlx::query_as(
            "select id, kind, title, body from memories
             where workspace_id = ? and scope = ? and scope_ref = ?
               and replaced_by is null and archived_at is null",
        )
        .bind(new.workspace_id)
        .bind(&new.scope)
        .bind(&new.scope_ref)
        .fetch_all(&self.db.pool)
        .await?;
        let both = format!("{} {}", new.title, new.body);
        let twin = live.iter().find(|(_, _, title, body)| {
            similarity(title, &new.title) >= MEMORY_DEDUPE_MIN
                || similarity(&format!("{title} {body}"), &both) >= MEMORY_DEDUPE_MIN
        });
        match twin {
            None => Ok(Saved::Added(self.insert_memory(new).await?)),
            Some((old, kind, _, body)) => {
                let changed = normalize_tokens(body) != normalize_tokens(&new.body);
                if changed && kind == "decision" && new.kind == "decision" {
                    let id = self.insert_memory(new).await?;
                    sqlx::query("update memories set replaced_by = ?, updated_at = ? where id = ?")
                        .bind(id)
                        .bind(crate::db::now_rfc3339())
                        .bind(old)
                        .execute(&self.db.pool)
                        .await?;
                    return Ok(Saved::Replaced { new: id, old: *old });
                }
                self.update_memory(*old, new).await?;
                Ok(Saved::Updated(*old))
            }
        }
    }

    async fn insert_memory(&self, new: &NewMemory) -> Result<Uuid> {
        let id = Uuid::now_v7();
        let now = crate::db::now_rfc3339();
        sqlx::query(
            "insert into memories (id, workspace_id, scope, scope_ref, kind, title, body,
                source_issue_id, source_run_id, source_comment_id, author, created_at, updated_at)
             values (?,?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(id)
        .bind(new.workspace_id)
        .bind(&new.scope)
        .bind(&new.scope_ref)
        .bind(&new.kind)
        .bind(&new.title)
        .bind(&new.body)
        .bind(new.source_issue_id)
        .bind(new.source_run_id)
        .bind(new.source_comment_id)
        .bind(&new.author)
        .bind(&now)
        .bind(&now)
        .execute(&self.db.pool)
        .await?;
        Ok(id)
    }

    async fn update_memory(&self, id: Uuid, new: &NewMemory) -> Result<()> {
        sqlx::query(
            "update memories set kind = ?, title = ?, body = ?, source_issue_id = ?,
                source_run_id = ?, source_comment_id = ?, author = ?, updated_at = ?
             where id = ?",
        )
        .bind(&new.kind)
        .bind(&new.title)
        .bind(&new.body)
        .bind(new.source_issue_id)
        .bind(new.source_run_id)
        .bind(new.source_comment_id)
        .bind(&new.author)
        .bind(crate::db::now_rfc3339())
        .bind(id)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }
}

/// `text` cut to `max` characters.
fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::engine::RunCompletion;
    use chrono::Utc;

    const DOC: &str = "# Workflows\n\nTurns: 6\n\n## Backlog\n\nUsers: human\n\nNext: In progress\n\n## In progress\n\nUsers: @Developer\n\nNext: Done\n\n## Blocked\n\nUsers: human\n\nNext: In progress\n\n## Done\n\nUsers: human\n\nNext: none\n";

    struct World {
        dir: std::path::PathBuf,
        engine: Engine,
        workspace: Uuid,
        developer: Uuid,
    }

    impl Drop for World {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    async fn world(tag: &str) -> World {
        let dir = std::env::temp_dir().join(format!("hulahoop-memories-{tag}-{}", Uuid::now_v7()));
        let db = Db::open(&dir).await.unwrap();
        let workspace = db.seed_local().await.unwrap();
        let developer = Uuid::now_v7();
        let now = Utc::now();
        sqlx::query("insert into agents (id, workspace_id, name, adapter, model, created_at, updated_at) values (?,?,'Developer','pi','model-x',?,?)")
            .bind(developer).bind(workspace).bind(now).bind(now).execute(&db.pool).await.unwrap();
        let engine = Engine::new(db);
        engine.set_workflow(workspace, DOC).await.unwrap();
        World {
            dir,
            engine,
            workspace,
            developer,
        }
    }

    /// An in-progress task in a project, with no repository.
    async fn task(w: &World, project: Option<Uuid>, number: i64) -> Uuid {
        let issue = Uuid::now_v7();
        let now = Utc::now();
        sqlx::query("insert into issues (id, workspace_id, project_id, repo_id, issue_key, number, title, status, created_at, updated_at) values (?,?,?,null,?,?,'Task','in_progress',?,?)")
            .bind(issue).bind(w.workspace).bind(project).bind(format!("TSK-{number}")).bind(number)
            .bind(now).bind(now).execute(&w.engine.db.pool).await.unwrap();
        issue
    }

    async fn project(w: &World, name: &str) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query("insert into projects (id, workspace_id, name, created_at) values (?,?,?,?)")
            .bind(id)
            .bind(w.workspace)
            .bind(name)
            .bind(Utc::now())
            .execute(&w.engine.db.pool)
            .await
            .unwrap();
        id
    }

    /// Finishes a turn on `issue` whose closing message is `body`.
    async fn finish(w: &World, issue: Uuid, body: &str) {
        let run = w
            .engine
            .start_run(issue, Some(w.developer), None)
            .await
            .unwrap();
        let done: RunCompletion =
            serde_json::from_value(serde_json::json!({"status": "succeeded", "body": body}))
                .unwrap();
        w.engine.complete_run(run.id, done).await.unwrap();
    }

    async fn titles(w: &World, scope: &str) -> Vec<String> {
        sqlx::query_scalar(
            "select title from memories where scope = ? and replaced_by is null order by created_at",
        )
        .bind(scope)
        .fetch_all(&w.engine.db.pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_marker_in_a_closing_message_is_saved_with_its_source() {
        let w = world("save").await;
        let p = project(&w, "Alpha").await;
        let issue = task(&w, Some(p), 1).await;
        finish(
            &w,
            issue,
            "Done.\nHULAHOOP_MEMORY: project gotcha envrc is blocked — run commands via direnv exec",
        )
        .await;
        let (scope_ref, author, source): (String, String, Option<Uuid>) = sqlx::query_as(
            "select scope_ref, author, source_issue_id from memories where title = 'envrc is blocked'",
        )
        .fetch_one(&w.engine.db.pool)
        .await
        .unwrap();
        assert_eq!(scope_ref, p.to_string());
        assert_eq!(author, "Developer");
        assert_eq!(source, Some(issue));
    }

    #[tokio::test]
    async fn a_near_duplicate_updates_instead_of_adding() {
        let w = world("dedupe").await;
        let issue = task(&w, None, 1).await;
        finish(
            &w,
            issue,
            "HULAHOOP_MEMORY: workspace gotcha envrc is blocked — use direnv exec",
        )
        .await;
        finish(
            &w,
            issue,
            "HULAHOOP_MEMORY: workspace gotcha the envrc is blocked — always use direnv exec",
        )
        .await;
        assert_eq!(titles(&w, "workspace").await.len(), 1);
        let body: String = sqlx::query_scalar("select title from memories")
            .fetch_one(&w.engine.db.pool)
            .await
            .unwrap();
        assert_eq!(body, "the envrc is blocked");
    }

    #[tokio::test]
    async fn unrelated_notes_in_one_scope_are_both_kept() {
        let w = world("distinct").await;
        let issue = task(&w, None, 1).await;
        finish(
            &w,
            issue,
            "HULAHOOP_MEMORY: workspace fact the prototype is in the milestone zip — open it first\nHULAHOOP_MEMORY: workspace fact cache is cleared on deploy — expect a slow first run",
        )
        .await;
        assert_eq!(titles(&w, "workspace").await.len(), 2);
    }

    #[tokio::test]
    async fn a_changed_decision_replaces_the_older_one() {
        let w = world("replace").await;
        let p = project(&w, "Alpha").await;
        let issue = task(&w, Some(p), 1).await;
        finish(
            &w,
            issue,
            "HULAHOOP_MEMORY: project decision database choice — use postgres",
        )
        .await;
        finish(
            &w,
            issue,
            "HULAHOOP_MEMORY: project decision database choice — use sqlite",
        )
        .await;
        let (live, replaced): (i64, i64) = sqlx::query_as(
            "select sum(replaced_by is null), sum(replaced_by is not null) from memories",
        )
        .fetch_one(&w.engine.db.pool)
        .await
        .unwrap();
        assert_eq!((live, replaced), (1, 1));
        let body: String =
            sqlx::query_scalar("select body from memories where replaced_by is null")
                .fetch_one(&w.engine.db.pool)
                .await
                .unwrap();
        assert_eq!(body, "use sqlite");
    }

    #[tokio::test]
    async fn one_run_saves_at_most_the_cap_and_bad_lines_do_not_fail_it() {
        let w = world("cap").await;
        let issue = task(&w, None, 1).await;
        let lines: String = (0..5)
            .map(|i| {
                format!("HULAHOOP_MEMORY: workspace fact topic{i} alpha{i} — body{i} beta{i}\n")
            })
            .collect();
        finish(
            &w,
            issue,
            &format!("{lines}HULAHOOP_MEMORY: project fact no project here — body"),
        )
        .await;
        assert_eq!(
            titles(&w, "workspace").await.len(),
            hulahoop_core::memory::MEMORY_WRITES_PER_RUN
        );
        assert!(titles(&w, "project").await.is_empty());
    }

    #[tokio::test]
    async fn a_project_memory_is_kept_for_its_own_project_only() {
        let w = world("scope").await;
        let (a, b) = (project(&w, "Alpha").await, project(&w, "Beta").await);
        let (ia, ib) = (task(&w, Some(a), 1).await, task(&w, Some(b), 2).await);
        finish(
            &w,
            ia,
            "HULAHOOP_MEMORY: project fact deploy window — fridays",
        )
        .await;
        finish(
            &w,
            ib,
            "HULAHOOP_MEMORY: project fact deploy window — mondays",
        )
        .await;
        // The same words in two projects are two notes, not one update.
        let refs: Vec<String> = sqlx::query_scalar("select scope_ref from memories order by body")
            .fetch_all(&w.engine.db.pool)
            .await
            .unwrap();
        assert_eq!(refs, vec![a.to_string(), b.to_string()]);
    }

    #[tokio::test]
    async fn an_answered_question_is_saved_as_a_decision_for_the_project() {
        let w = world("answer").await;
        let p = project(&w, "Alpha").await;
        let issue = task(&w, Some(p), 1).await;
        let question = w
            .engine
            .comment(
                issue,
                hulahoop_core::AuthorType::Agent,
                None,
                "Use REST or GraphQL?",
                true,
            )
            .await
            .unwrap();
        w.engine
            .move_to(issue, hulahoop_core::IssueStatus::Blocked)
            .await
            .unwrap();
        w.engine
            .resume_issue(issue, "GraphQL, we already have a schema")
            .await
            .unwrap();
        let (kind, scope, comment, source): (String, String, Option<Uuid>, Option<Uuid>) =
            sqlx::query_as("select kind, scope, source_comment_id, source_issue_id from memories")
                .fetch_one(&w.engine.db.pool)
                .await
                .unwrap();
        assert_eq!((kind.as_str(), scope.as_str()), ("decision", "project"));
        assert!(comment.is_some() && comment != Some(question));
        assert_eq!(source, Some(issue));
    }

    #[tokio::test]
    async fn the_full_text_index_follows_the_notes() {
        let w = world("fts").await;
        let issue = task(&w, None, 1).await;
        finish(
            &w,
            issue,
            "HULAHOOP_MEMORY: workspace gotcha envrc blocked — run via direnv",
        )
        .await;
        let hits: i64 = sqlx::query_scalar(
            "select count(*) from memories_fts where memories_fts match '\"envrc\" OR \"nothing\"'",
        )
        .fetch_one(&w.engine.db.pool)
        .await
        .unwrap();
        assert_eq!(hits, 1);
    }
}
