//! What a person does with memories: list and search them, add one, correct
//! one, archive it or forget it. Agents never come through here; they save with
//! `HULAHOOP_MEMORY:` lines (see `memories.rs`).

use anyhow::{bail, Result};
use async_graphql::SimpleObject;
use chrono::{DateTime, Utc};
use hulahoop_core::memory::{
    fts_query, one_line, KINDS, MEMORY_BODY_MAX, MEMORY_TITLE_MAX, SCOPES,
};
use uuid::Uuid;

use crate::engine::Engine;
use crate::memories::{NewMemory, Saved};

/// A memory as the desktop shows it, with the key of the task it came from.
#[derive(Debug, Clone, serde::Serialize, SimpleObject, sqlx::FromRow)]
pub struct Memory {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub scope: String,
    /// The project id for `project`, a name for `agent` and `person`.
    pub scope_ref: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub source_issue_id: Option<Uuid>,
    pub source_issue_key: Option<String>,
    pub source_run_id: Option<Uuid>,
    pub author: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub use_count: i64,
    pub replaced_by: Option<Uuid>,
    pub archived_at: Option<DateTime<Utc>>,
}

const SELECT: &str = "select m.id, m.workspace_id, m.scope, m.scope_ref, m.kind, m.title, m.body,
        m.source_issue_id, i.issue_key as source_issue_key, m.source_run_id, m.author,
        m.created_at, m.updated_at, m.last_used_at, m.use_count, m.replaced_by, m.archived_at
    from memories m left join issues i on i.id = m.source_issue_id";

/// A title or body a person typed, held to the same limits as an agent's.
fn checked(text: &str, what: &str, max: usize) -> Result<String> {
    let text = one_line(text);
    if text.is_empty() {
        bail!("a memory needs a {what}");
    }
    Ok(text.chars().take(max).collect())
}

fn check_kind(kind: &str) -> Result<()> {
    if !KINDS.contains(&kind) {
        bail!("kind must be one of {}", KINDS.join(", "));
    }
    Ok(())
}

impl Engine {
    /// The workspace's live memories, newest change first. `scope` narrows to one
    /// scope and `query` to notes matching those words. Replaced and archived
    /// notes are left out unless `include_archived` is set.
    pub async fn memories(
        &self,
        workspace_id: Uuid,
        scope: Option<&str>,
        query: Option<&str>,
        include_archived: bool,
    ) -> Result<Vec<Memory>> {
        let words = query.and_then(fts_query);
        if query.is_some_and(|q| !q.trim().is_empty()) && words.is_none() {
            return Ok(Vec::new());
        }
        let mut sql = format!("{SELECT} where m.workspace_id = ?");
        if scope.is_some() {
            sql.push_str(" and m.scope = ?");
        }
        if words.is_some() {
            sql.push_str(
                " and m.rowid in (select rowid from memories_fts where memories_fts match ?)",
            );
        }
        if !include_archived {
            sql.push_str(" and m.replaced_by is null and m.archived_at is null");
        }
        sql.push_str(" order by m.updated_at desc");
        let mut rows = sqlx::query_as::<_, Memory>(&sql).bind(workspace_id);
        if let Some(scope) = scope {
            rows = rows.bind(scope);
        }
        if let Some(words) = words {
            rows = rows.bind(words);
        }
        Ok(rows.fetch_all(&self.db.pool).await?)
    }

    async fn memory(&self, id: Uuid) -> Result<Memory> {
        sqlx::query_as::<_, Memory>(&format!("{SELECT} where m.id = ?"))
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Memory not found"))
    }

    /// A person writes a memory. It goes through the same near-duplicate and
    /// replace rules as an agent's, so the note returned may be an older one.
    pub async fn create_memory(
        &self,
        workspace_id: Uuid,
        scope: &str,
        scope_ref: &str,
        kind: &str,
        title: &str,
        body: &str,
    ) -> Result<Memory> {
        if !SCOPES.contains(&scope) {
            bail!("scope must be one of {}", SCOPES.join(", "));
        }
        check_kind(kind)?;
        let scope_ref = scope_ref.trim();
        if scope == "workspace" && !scope_ref.is_empty() {
            bail!("a workspace memory belongs to no one in particular");
        }
        if scope != "workspace" && scope_ref.is_empty() {
            bail!("a {scope} memory needs to say whose it is");
        }
        if scope == "project" {
            let found: Option<Uuid> =
                sqlx::query_scalar("select id from projects where id = ? and workspace_id = ?")
                    .bind(scope_ref)
                    .bind(workspace_id)
                    .fetch_optional(&self.db.pool)
                    .await?;
            if found.is_none() {
                bail!("Project not found");
            }
        }
        let saved = self
            .save_memory(&NewMemory {
                workspace_id,
                scope: scope.into(),
                scope_ref: scope_ref.into(),
                kind: kind.into(),
                title: checked(title, "title", MEMORY_TITLE_MAX)?,
                body: checked(body, "body", MEMORY_BODY_MAX)?,
                source_issue_id: None,
                source_run_id: None,
                source_comment_id: None,
                author: "human".into(),
            })
            .await?;
        match saved {
            Saved::Added(id) | Saved::Updated(id) | Saved::Replaced { new: id, .. } => {
                self.memory(id).await
            }
            Saved::Skipped => bail!("a live decision already says this"),
        }
    }

    /// A person corrects a memory. Its source stays; the author becomes them.
    pub async fn edit_memory(
        &self,
        id: Uuid,
        kind: Option<&str>,
        title: Option<&str>,
        body: Option<&str>,
    ) -> Result<Memory> {
        let old = self.memory(id).await?;
        let kind = kind.unwrap_or(&old.kind);
        check_kind(kind)?;
        let title = checked(title.unwrap_or(&old.title), "title", MEMORY_TITLE_MAX)?;
        let body = checked(body.unwrap_or(&old.body), "body", MEMORY_BODY_MAX)?;
        sqlx::query(
            "update memories set kind = ?, title = ?, body = ?, author = 'human', updated_at = ?
             where id = ?",
        )
        .bind(kind)
        .bind(title)
        .bind(body)
        .bind(crate::db::now_rfc3339())
        .bind(id)
        .execute(&self.db.pool)
        .await?;
        self.memory(id).await
    }

    /// Archive a memory, or bring an archived one back. Archived notes stay on
    /// record but leave the index.
    pub async fn archive_memory(&self, id: Uuid, archived: bool) -> Result<Memory> {
        let at = archived.then(crate::db::now_rfc3339);
        let done = sqlx::query("update memories set archived_at = ? where id = ?")
            .bind(at)
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        if done.rows_affected() == 0 {
            bail!("Memory not found");
        }
        self.memory(id).await
    }

    /// Forget a memory for good.
    pub async fn forget_memory(&self, id: Uuid) -> Result<bool> {
        let done = sqlx::query("delete from memories where id = ?")
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        Ok(done.rows_affected() > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    struct World {
        dir: std::path::PathBuf,
        engine: Engine,
        workspace: Uuid,
    }

    impl Drop for World {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    async fn world() -> World {
        let dir = std::env::temp_dir().join(format!("hulahoop-memory-notes-{}", Uuid::now_v7()));
        let db = Db::open(&dir).await.unwrap();
        let workspace = db.seed_local().await.unwrap();
        World {
            dir,
            engine: Engine::new(db),
            workspace,
        }
    }

    async fn add(w: &World, scope: &str, who: &str, title: &str, body: &str) -> Memory {
        w.engine
            .create_memory(w.workspace, scope, who, "fact", title, body)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_person_adds_lists_and_searches_memories() {
        let w = world().await;
        add(&w, "workspace", "", "envrc blocked", "run through direnv").await;
        add(
            &w,
            "person",
            "Sam",
            "likes short summaries",
            "keep handovers brief",
        )
        .await;
        let all = w
            .engine
            .memories(w.workspace, None, None, false)
            .await
            .unwrap();
        assert_eq!(all.len(), 2);
        let people = w
            .engine
            .memories(w.workspace, Some("person"), None, false)
            .await
            .unwrap();
        assert_eq!(people.len(), 1);
        assert_eq!(people[0].author, "human");
        let found = w
            .engine
            .memories(w.workspace, None, Some("direnv"), false)
            .await
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title, "envrc blocked");
        // Only punctuation is a search that can match nothing, not everything.
        let none = w
            .engine
            .memories(w.workspace, None, Some("!!"), false)
            .await
            .unwrap();
        assert!(none.is_empty());
    }

    #[tokio::test]
    async fn bad_input_is_refused() {
        let w = world().await;
        let create =
            |scope: &'static str, who: &'static str, kind: &'static str, title: &'static str| {
                w.engine
                    .create_memory(w.workspace, scope, who, kind, title, "body")
            };
        assert!(create("galaxy", "", "fact", "t").await.is_err());
        assert!(create("workspace", "", "rumour", "t").await.is_err());
        assert!(create("workspace", "", "fact", "  ").await.is_err());
        assert!(create("person", "", "fact", "t").await.is_err());
        assert!(create("workspace", "x", "fact", "t").await.is_err());
        let missing = Uuid::now_v7().to_string();
        let error = w
            .engine
            .create_memory(w.workspace, "project", &missing, "fact", "t", "b")
            .await;
        assert!(error.is_err());
    }

    #[tokio::test]
    async fn editing_changes_the_text_and_the_full_text_index_follows() {
        let w = world().await;
        let note = add(&w, "workspace", "", "old title", "stale words").await;
        let edited = w
            .engine
            .edit_memory(
                note.id,
                Some("gotcha"),
                Some("new title"),
                Some("fresh words"),
            )
            .await
            .unwrap();
        assert_eq!(
            (edited.kind.as_str(), edited.title.as_str()),
            ("gotcha", "new title")
        );
        let by = |q: &'static str| w.engine.memories(w.workspace, None, Some(q), false);
        assert!(by("stale").await.unwrap().is_empty());
        assert_eq!(by("fresh").await.unwrap().len(), 1);
        assert!(w
            .engine
            .edit_memory(note.id, Some("rumour"), None, None)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn archived_notes_leave_the_list_and_forgotten_ones_are_gone() {
        let w = world().await;
        let note = add(&w, "workspace", "", "kept for now", "something").await;
        w.engine.archive_memory(note.id, true).await.unwrap();
        assert!(w
            .engine
            .memories(w.workspace, None, None, false)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            w.engine
                .memories(w.workspace, None, None, true)
                .await
                .unwrap()
                .len(),
            1
        );
        w.engine.archive_memory(note.id, false).await.unwrap();
        assert_eq!(
            w.engine
                .memories(w.workspace, None, None, false)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(w.engine.forget_memory(note.id).await.unwrap());
        assert!(!w.engine.forget_memory(note.id).await.unwrap());
        assert!(w
            .engine
            .memories(w.workspace, None, None, true)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn lumi_reads_the_same_memories() {
        let w = world().await;
        add(&w, "workspace", "", "envrc blocked", "run through direnv").await;
        let snapshot = w.engine.lumi_snapshot(w.workspace).await.unwrap();
        assert_eq!(snapshot["memories"][0]["title"], "envrc blocked");
    }
}
