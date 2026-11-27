//! Long-term memory on the server: saving what agents and people say is worth
//! keeping. Nothing here waits for approval; the rules in `hulahoop_core::memory`
//! (per-run cap, near-duplicates, replaced decisions) keep the store small.

use anyhow::Result;
use hulahoop_core::memory::{
    fts_query, normalize_tokens, note_file_name, one_line, parse_lines, render_index, select_index,
    similarity, IndexCandidate, MemoryLine, MEMORY_ARCHIVE_RUNS, MEMORY_BODY_MAX,
    MEMORY_DEDUPE_MIN, MEMORY_STALE_RUNS, MEMORY_TITLE_MAX,
};
use hulahoop_core::runspec::MemoryNote;
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

/// What a run is told about memory: the index for the prompt and the notes the
/// runner writes into `.hulahoop/memory/`.
#[derive(Debug, Clone, Default)]
pub struct MemoryIndex {
    pub index: String,
    pub notes: Vec<MemoryNote>,
}

/// How many full-text matches count as "relevant to the task".
const MATCH_LIMIT: i64 = 100;

/// A live memory in reach of a run, as the index query returns it.
type Reachable = (i64, Uuid, String, String, String, String, i64, String, i64);

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
            title: clip(&one_line(question), MEMORY_TITLE_MAX),
            body: clip(&one_line(answer), MEMORY_BODY_MAX),
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

    /// The memory index for a run on `issue` by `agent_name`, within `budget`
    /// tokens. A run reaches the workspace's notes, its project's, its own
    /// agent's and the people's; another project's never. Notes idle for
    /// `MEMORY_ARCHIVE_RUNS` runs are archived here, and every note listed counts
    /// as used, which is what keeps it out of the stale tail.
    pub async fn memory_index(
        &self,
        issue: &Issue,
        agent_name: &str,
        budget: usize,
    ) -> Result<MemoryIndex> {
        let project_ref = issue
            .project_id
            .map(|id| id.to_string())
            .unwrap_or_default();
        let rows: Vec<Reachable> = sqlx::query_as(
            "select m.rowid, m.id, m.scope, m.kind, m.title, m.body, m.use_count, m.updated_at,
                (select count(*) from runs r join issues i on i.id = r.issue_id
                  where i.workspace_id = m.workspace_id
                    and r.created_at > coalesce(m.last_used_at, m.created_at)) as idle
             from memories m
             where m.workspace_id = ? and m.replaced_by is null and m.archived_at is null
               and (m.scope in ('workspace', 'person')
                    or (m.scope = 'project' and m.scope_ref = ?)
                    or (m.scope = 'agent' and m.scope_ref = ?))",
        )
        .bind(issue.workspace_id)
        .bind(&project_ref)
        .bind(agent_name)
        .fetch_all(&self.db.pool)
        .await?;
        let matches = self.matching_rows(issue).await?;
        let mut candidates = Vec::new();
        let mut notes = std::collections::HashMap::new();
        for (seq, id, scope, kind, title, body, use_count, updated, idle) in rows {
            if idle >= MEMORY_ARCHIVE_RUNS {
                sqlx::query("update memories set archived_at = ? where id = ?")
                    .bind(crate::db::now_rfc3339())
                    .bind(id)
                    .execute(&self.db.pool)
                    .await?;
                continue;
            }
            let text = format!("# {title}\n\n{scope} {kind}\n\n{body}\n");
            notes.insert(seq, (id, text));
            candidates.push(IndexCandidate {
                seq,
                scope,
                kind,
                title,
                stale: idle >= MEMORY_STALE_RUNS,
                match_rank: matches
                    .iter()
                    .position(|rowid| *rowid == seq)
                    .map(|p| p as i64),
                use_count,
                updated_at: chrono::DateTime::parse_from_rfc3339(&updated)
                    .map_or(0, |at| at.timestamp()),
            });
        }
        let chosen = select_index(candidates, budget);
        let mut listed = Vec::new();
        for candidate in &chosen {
            let (id, text) = notes.remove(&candidate.seq).unwrap_or_default();
            sqlx::query(
                "update memories set last_used_at = ?, use_count = use_count + 1 where id = ?",
            )
            .bind(crate::db::now_rfc3339())
            .bind(id)
            .execute(&self.db.pool)
            .await?;
            listed.push(MemoryNote {
                file: note_file_name(candidate.seq, &candidate.title),
                body: text,
            });
        }
        Ok(MemoryIndex {
            index: render_index(&chosen),
            notes: listed,
        })
    }

    /// Row ids of the notes that match the task's words, best first.
    async fn matching_rows(&self, issue: &Issue) -> Result<Vec<i64>> {
        let Some(query) = fts_query(&format!("{} {}", issue.title, issue.body)) else {
            return Ok(Vec::new());
        };
        Ok(sqlx::query_scalar(
            "select rowid from memories_fts where memories_fts match ? order by rank limit ?",
        )
        .bind(query)
        .bind(MATCH_LIMIT)
        .fetch_all(&self.db.pool)
        .await?)
    }

    /// Saves one memory. A note that nearly repeats a live one in the same
    /// scope updates it; a changed `decision` on the same subject replaces it, and
    /// only another `decision` may do so.
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
                // A live decision is never rewritten by another kind of note.
                if kind == "decision" && new.kind != "decision" {
                    if !changed {
                        return Ok(Saved::Updated(*old));
                    }
                    return Ok(Saved::Added(self.insert_memory(new).await?));
                }
                if changed && kind == "decision" {
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
    use hulahoop_core::memory::estimate_tokens;

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

    async fn finish_as(w: &World, issue: Uuid, status: &str, body: &str) {
        let run = w
            .engine
            .start_run(issue, Some(w.developer), None)
            .await
            .unwrap();
        let done: RunCompletion =
            serde_json::from_value(serde_json::json!({"status": status, "body": body})).unwrap();
        w.engine.complete_run(run.id, done).await.unwrap();
    }

    async fn count(w: &World) -> i64 {
        sqlx::query_scalar("select count(*) from memories")
            .fetch_one(&w.engine.db.pool)
            .await
            .unwrap()
    }

    async fn answer(w: &World, issue: Uuid, question: &str, reply: &str) {
        w.engine
            .comment(
                issue,
                hulahoop_core::AuthorType::Agent,
                None,
                question,
                true,
            )
            .await
            .unwrap();
        w.engine
            .move_to(issue, hulahoop_core::IssueStatus::Blocked)
            .await
            .unwrap();
        w.engine.resume_issue(issue, reply).await.unwrap();
    }

    #[tokio::test]
    async fn a_fact_never_rewrites_a_live_decision() {
        let w = world("decision-kept").await;
        let issue = task(&w, None, 1).await;
        finish(
            &w,
            issue,
            "HULAHOOP_MEMORY: workspace decision database choice — use postgres",
        )
        .await;
        finish(
            &w,
            issue,
            "HULAHOOP_MEMORY: workspace fact database choice — sqlite ships bundled",
        )
        .await;
        let kinds: Vec<(String, String)> =
            sqlx::query_as("select kind, body from memories order by created_at")
                .fetch_all(&w.engine.db.pool)
                .await
                .unwrap();
        assert_eq!(
            kinds,
            vec![
                ("decision".to_string(), "use postgres".to_string()),
                ("fact".to_string(), "sqlite ships bundled".to_string())
            ]
        );
    }

    #[tokio::test]
    async fn an_answer_is_saved_on_one_line_without_control_characters() {
        let w = world("answer-clean").await;
        let p = project(&w, "Alpha").await;
        let issue = task(&w, Some(p), 1).await;
        answer(
            &w,
            issue,
            "Use REST\nor GraphQL?\u{1b}[31m",
            "GraphQL,\n\n  we have\ta schema",
        )
        .await;
        let (title, body): (String, String) = sqlx::query_as("select title, body from memories")
            .fetch_one(&w.engine.db.pool)
            .await
            .unwrap();
        assert_eq!(title, "Use REST or GraphQL?[31m");
        assert_eq!(body, "GraphQL, we have a schema");
    }

    #[tokio::test]
    async fn an_answer_on_a_task_with_no_project_saves_nothing() {
        let w = world("answer-none").await;
        let issue = task(&w, None, 1).await;
        answer(&w, issue, "Which one?", "The first").await;
        assert_eq!(count(&w).await, 0);
    }

    #[tokio::test]
    async fn a_failed_run_saves_no_markers() {
        let w = world("failed").await;
        let issue = task(&w, None, 1).await;
        let line = "HULAHOOP_MEMORY: workspace fact anything — at all";
        finish_as(&w, issue, "failed", line).await;
        finish_as(&w, issue, "cancelled", line).await;
        assert_eq!(count(&w).await, 0);
    }

    #[tokio::test]
    async fn agent_and_person_scopes_resolve_to_a_name() {
        let w = world("names").await;
        let issue = task(&w, None, 1).await;
        finish(
            &w,
            issue,
            "HULAHOOP_MEMORY: agent fact mine — own note\nHULAHOOP_MEMORY: agent:Reviewer fact theirs — other note\nHULAHOOP_MEMORY: person:Ada preference terse — likes short summaries\nHULAHOOP_MEMORY: person fact nobody — whose?",
        )
        .await;
        let rows: Vec<(String, String, String)> =
            sqlx::query_as("select scope, scope_ref, title from memories order by created_at")
                .fetch_all(&w.engine.db.pool)
                .await
                .unwrap();
        assert_eq!(
            rows,
            vec![
                ("agent".into(), "Developer".into(), "mine".into()),
                ("agent".into(), "Reviewer".into(), "theirs".into()),
                ("person".into(), "Ada".into(), "terse".into()),
            ]
        );
    }

    async fn index_for(w: &World, issue: Uuid, budget: usize) -> MemoryIndex {
        let issue = w.engine.issue(issue).await.unwrap();
        w.engine
            .memory_index(&issue, "Developer", budget)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn the_index_never_leaks_another_projects_or_agents_notes() {
        let w = world("index-scope").await;
        let (a, b) = (project(&w, "Alpha").await, project(&w, "Beta").await);
        let (ia, ib) = (task(&w, Some(a), 1).await, task(&w, Some(b), 2).await);
        finish(
            &w,
            ia,
            "HULAHOOP_MEMORY: project fact alpha secret — in alpha",
        )
        .await;
        finish(&w, ib, "HULAHOOP_MEMORY: project fact beta secret — in beta\nHULAHOOP_MEMORY: agent:Reviewer fact reviewer only — nope\nHULAHOOP_MEMORY: workspace fact shared — everywhere").await;
        let index = index_for(&w, ia, 1500).await;
        assert!(index.index.contains("alpha secret"));
        assert!(index.index.contains("shared"));
        assert!(!index.index.contains("beta secret"));
        assert!(!index.index.contains("reviewer only"));
        assert_eq!(index.notes.len(), 2);
        assert!(index.notes.iter().all(|n| !n.body.contains("beta")));
    }

    #[tokio::test]
    async fn a_repo_less_task_gets_workspace_notes() {
        let w = world("index-none").await;
        let issue = task(&w, None, 1).await;
        finish(
            &w,
            issue,
            "HULAHOOP_MEMORY: workspace fact shared — everywhere",
        )
        .await;
        let index = index_for(&w, issue, 1500).await;
        assert!(index.index.contains("shared"));
        assert!(index.notes[0].body.contains("everywhere"));
    }

    #[tokio::test]
    async fn the_index_stays_within_its_budget_and_lists_only_the_notes_it_ships() {
        let w = world("index-budget").await;
        let issue = task(&w, None, 1).await;
        for i in 0..3 {
            finish(&w, issue, &format!("HULAHOOP_MEMORY: workspace fact topic{i} unique{i} — body{i} text{i}\nHULAHOOP_MEMORY: workspace fact other{i} distinct{i} — more{i} words{i}")).await;
        }
        assert_eq!(count(&w).await, 6);
        let budget = 40;
        let index = index_for(&w, issue, budget).await;
        assert!(estimate_tokens(&index.index) <= budget);
        let lines = index.index.lines().filter(|l| l.starts_with("- ")).count();
        assert!(lines > 0 && lines < 6);
        assert_eq!(index.notes.len(), lines);
    }

    #[tokio::test]
    async fn a_replaced_decision_leaves_the_index_and_its_replacement_stays() {
        let w = world("index-replaced").await;
        let issue = task(&w, None, 1).await;
        finish(
            &w,
            issue,
            "HULAHOOP_MEMORY: workspace decision database choice — use postgres",
        )
        .await;
        finish(
            &w,
            issue,
            "HULAHOOP_MEMORY: workspace decision database choice — use sqlite",
        )
        .await;
        let index = index_for(&w, issue, 1500).await;
        assert_eq!(index.notes.len(), 1);
        assert!(index.notes[0].body.contains("use sqlite"));
    }

    #[tokio::test]
    async fn listed_notes_count_as_used() {
        let w = world("index-used").await;
        let issue = task(&w, None, 1).await;
        finish(
            &w,
            issue,
            "HULAHOOP_MEMORY: workspace fact shared — everywhere",
        )
        .await;
        index_for(&w, issue, 1500).await;
        let uses: i64 = sqlx::query_scalar("select use_count from memories")
            .fetch_one(&w.engine.db.pool)
            .await
            .unwrap();
        assert_eq!(uses, 1);
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
