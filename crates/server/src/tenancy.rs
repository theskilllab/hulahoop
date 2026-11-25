//! Projects, milestones and artifacts: the tenancy the loop hangs off.
//!
//! A project carries the prefix an issue is named by; a milestone is a body of work
//! an issue can point at; an artifact is a file, optionally filed under a milestone.
//! They live in their own impl block because none of them touch the state machine.

use crate::db::sha256_hex;
use crate::engine::Engine;
use crate::model::*;
use anyhow::{anyhow, bail, Context, Result};
use uuid::Uuid;

/// The colours a project can wear, in the order a new project picks them.
pub const PROJECT_COLORS: [&str; 8] = [
    "#2f7ad8", "#1f9d7a", "#c2692b", "#6a5acd", "#d4558a", "#b88100", "#d03b3b", "#64748b",
];

impl Engine {
    // ---- github -------------------------------------------------------------

    /// The repositories this app's installation can reach. Read over the app JWT,
    /// because an installation token is already narrowed to one repository.
    pub async fn github_repositories(&self) -> Result<Vec<crate::github::GithubRepo>> {
        let app = self
            .stored_app(crate::github::PROVIDER, crate::connectors::KIND_GITHUB_APP)
            .await?
            .ok_or_else(|| {
                // Two very different situations reach this line, and the user's fix
                // differs: paste a key, or replace a key whose vault will not open.
                anyhow!(
                    "no usable GitHub app is stored. Paste the app id and its private key in Settings (replacing any stored value), install the app on the repositories you want, then try again."
                )
            })?;
        let jwt = crate::github::mint_token(&app.app_id, &app.private_key)?;
        crate::github::list_repositories(&crate::github::http_client(), &jwt).await
    }

    // ---- workspaces ----------------------------------------------------------

    /// A workspace is the top of the tree: projects live in it, and an issue that
    /// belongs to no project is still filed here, keyed by the workspace prefix.
    pub async fn create_workspace(&self, name: &str, prefix: Option<&str>) -> Result<Workspace> {
        let name = name.trim().to_string();
        if name.len() < 2 {
            bail!("a workspace needs a name");
        }
        let prefix = match prefix.map(str::trim).filter(|raw| !raw.is_empty()) {
            Some(raw) => Self::normalise_prefix(raw)?,
            None => Self::derive_prefix(&name),
        };
        let slug = self.unique_slug(&name).await?;
        let id = Uuid::now_v7();
        let now = crate::db::now_rfc3339();
        sqlx::query(
            "insert into workspaces (id, name, slug, default_prefix, created_at) values (?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(&name)
        .bind(&slug)
        .bind(&prefix)
        .bind(&now)
        .execute(&self.db.pool)
        .await?;
        if let Ok(user) = self.db.local_user_id().await {
            sqlx::query("insert or ignore into workspace_members (workspace_id, user_id, role) values (?, ?, 'owner')")
                .bind(id)
                .bind(user)
                .execute(&self.db.pool)
                .await?;
        }
        self.workspace(id).await
    }

    /// `local`, then `local-2`, `local-3`. The slug is what a URL or a log line can
    /// carry, so it stays readable and unique.
    async fn unique_slug(&self, name: &str) -> Result<String> {
        let base: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect();
        let base = base.trim_matches('-').to_string();
        let base = if base.is_empty() {
            "workspace".to_string()
        } else {
            base
        };
        for attempt in 0..50 {
            let candidate = if attempt == 0 {
                base.clone()
            } else {
                format!("{base}-{}", attempt + 1)
            };
            let taken: Option<(String,)> =
                sqlx::query_as("select id from workspaces where slug = ?")
                    .bind(&candidate)
                    .fetch_optional(&self.db.pool)
                    .await?;
            if taken.is_none() {
                return Ok(candidate);
            }
        }
        bail!("could not find a free slug for {name}")
    }

    pub async fn workspace(&self, id: Uuid) -> Result<Workspace> {
        sqlx::query_as::<_, Workspace>("select * from workspaces where id = ?")
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await?
            .ok_or_else(|| anyhow!("workspace {id} not found"))
    }

    pub async fn update_workspace(
        &self,
        id: Uuid,
        name: Option<String>,
        default_prefix: Option<String>,
    ) -> Result<Workspace> {
        let workspace = self.workspace(id).await?;
        let name = match name
            .map(|value| value.trim().to_string())
            .filter(|v| !v.is_empty())
        {
            Some(value) if value.len() >= 2 => value,
            Some(_) => bail!("a workspace needs a name"),
            None => workspace.name.clone(),
        };
        let prefix = match default_prefix
            .map(|value| value.trim().to_string())
            .filter(|v| !v.is_empty())
        {
            Some(raw) => Self::normalise_prefix(&raw)?,
            None => workspace.default_prefix.clone(),
        };
        // A project in this workspace must not already use the new workspace prefix,
        // or two different sets of issues would share one key space.
        let clash: Option<(String,)> =
            sqlx::query_as("select name from projects where workspace_id = ? and prefix = ?")
                .bind(id)
                .bind(&prefix)
                .fetch_optional(&self.db.pool)
                .await?;
        if let Some((project,)) = clash {
            bail!("the project {project} already uses {prefix}; pick another workspace prefix");
        }
        sqlx::query("update workspaces set name = ?, default_prefix = ? where id = ?")
            .bind(&name)
            .bind(&prefix)
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        if prefix != workspace.default_prefix {
            sqlx::query(
                "update issues set issue_key = ? || '-' || number where workspace_id = ? and project_id is null",
            )
            .bind(&prefix)
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        }
        self.workspace(id).await
    }

    /// `(user id, name, email, role)` — a workspace is shared or it is not, but the
    /// list of people is what makes that visible.
    pub async fn workspace_members(&self, workspace_id: Uuid) -> Result<Vec<WorkspaceMember>> {
        Ok(sqlx::query_as::<_, WorkspaceMember>(
            "select m.user_id, u.name, u.email, m.role from workspace_members m join users u on u.id = m.user_id where m.workspace_id = ? order by u.name",
        )
        .bind(workspace_id)
        .fetch_all(&self.db.pool)
        .await?)
    }

    pub fn check_project_color(color: &str) -> Result<()> {
        if !PROJECT_COLORS.contains(&color) {
            bail!("a project colour is one of the eight project colours");
        }
        Ok(())
    }

    /// The first palette colour no project in the workspace wears, or the
    /// palette's next turn once every colour is taken.
    pub async fn next_project_color(&self, workspace_id: Uuid) -> Result<&'static str> {
        let used: Vec<String> =
            sqlx::query_scalar("select color from projects where workspace_id = ?")
                .bind(workspace_id)
                .fetch_all(&self.db.pool)
                .await?;
        Ok(PROJECT_COLORS
            .iter()
            .find(|color| !used.iter().any(|u| u == *color))
            .copied()
            .unwrap_or(PROJECT_COLORS[used.len() % PROJECT_COLORS.len()]))
    }

    /// Change a project's name and/or colour. The prefix has its own mutation.
    pub async fn update_project(
        &self,
        id: Uuid,
        name: Option<&str>,
        color: Option<&str>,
    ) -> Result<Project> {
        if let Some(color) = color {
            Self::check_project_color(color)?;
        }
        if let Some(name) = name {
            self.rename_project(id, name).await?;
        }
        if let Some(color) = color {
            sqlx::query("update projects set color = ? where id = ?")
                .bind(color)
                .bind(id)
                .execute(&self.db.pool)
                .await?;
        }
        Ok(
            sqlx::query_as::<_, Project>("select * from projects where id = ?")
                .bind(id)
                .fetch_one(&self.db.pool)
                .await?,
        )
    }

    /// Unlink a repository from its project. Open tasks still work in it, and
    /// a repository with runs carries their history (deleting it would cascade
    /// to those runs), so either blocks the unlink. Finished tasks without
    /// runs just lose the link.
    pub async fn unlink_repo(&self, repo_id: Uuid) -> Result<Project> {
        let project_id: Uuid = sqlx::query_scalar("select project_id from repos where id = ?")
            .bind(repo_id)
            .fetch_optional(&self.db.pool)
            .await?
            .ok_or_else(|| anyhow!("repository {repo_id} not found"))?;
        let open: i64 = sqlx::query_scalar(
            "select count(*) from issues where repo_id = ? and status not in ('done', 'cancelled')",
        )
        .bind(repo_id)
        .fetch_one(&self.db.pool)
        .await?;
        if open > 0 {
            let (noun, verb) = if open == 1 {
                ("task", "uses")
            } else {
                ("tasks", "use")
            };
            bail!("{open} open {noun} still {verb} this repository. Finish or move them first.");
        }
        let runs: i64 = sqlx::query_scalar("select count(*) from runs where repo_id = ?")
            .bind(repo_id)
            .fetch_one(&self.db.pool)
            .await?;
        if runs > 0 {
            bail!("This repository has run history, which unlinking would delete.");
        }
        let mut tx = self.db.pool.begin().await?;
        sqlx::query("update issues set repo_id = null where repo_id = ?")
            .bind(repo_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("delete from repos where id = ?")
            .bind(repo_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(
            sqlx::query_as::<_, Project>("select * from projects where id = ?")
                .bind(project_id)
                .fetch_one(&self.db.pool)
                .await?,
        )
    }

    pub async fn rename_project(&self, id: Uuid, name: &str) -> Result<Project> {
        let name = name.trim();
        if name.len() < 2 {
            bail!("a project needs a name");
        }
        sqlx::query("update projects set name = ? where id = ?")
            .bind(name)
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        Ok(
            sqlx::query_as::<_, Project>("select * from projects where id = ?")
                .bind(id)
                .fetch_one(&self.db.pool)
                .await?,
        )
    }

    // ---- projects, milestones, artifacts -------------------------------------

    /// `BAL`, not `bal` and not `BAL-15`. Two to five characters, letters and
    /// digits, so a key always reads the same way in a log and a branch name.
    pub fn normalise_prefix(raw: &str) -> Result<String> {
        let cleaned: String = raw
            .trim()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_uppercase();
        if cleaned.len() < 2 || cleaned.len() > 5 {
            bail!("a prefix is two to five letters or digits, like BAL or ATL");
        }
        Ok(cleaned)
    }

    /// The prefix a name suggests: `Local` becomes `LOC`, `Bali` becomes `BAL`.
    /// Used when nobody typed one, so a workspace or project is never keyless.
    pub fn derive_prefix(name: &str) -> String {
        let letters: String = name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_uppercase();
        match letters.len() {
            0 => "WS".into(),
            1..=3 => letters,
            _ => letters[..3].to_string(),
        }
    }

    pub async fn set_project_prefix(&self, project_id: Uuid, prefix: &str) -> Result<Project> {
        let prefix = Self::normalise_prefix(prefix)?;
        let project: Project = sqlx::query_as("select * from projects where id = ?")
            .bind(project_id)
            .fetch_optional(&self.db.pool)
            .await?
            .ok_or_else(|| anyhow!("project {project_id} not found"))?;
        let taken: Option<(String,)> = sqlx::query_as(
            "select id from projects where workspace_id = ? and prefix = ? and id <> ?",
        )
        .bind(project.workspace_id)
        .bind(&prefix)
        .bind(project_id)
        .fetch_optional(&self.db.pool)
        .await?;
        if taken.is_some() {
            bail!("the prefix {prefix} is already used by another project in this workspace");
        }
        // Workspace-level issues carry the workspace prefix, so a project claiming
        // it would mint keys that collide with theirs.
        let workspace: Workspace = sqlx::query_as("select * from workspaces where id = ?")
            .bind(project.workspace_id)
            .fetch_one(&self.db.pool)
            .await?;
        if workspace.default_prefix == prefix {
            bail!(
                "{prefix} is the workspace prefix, used by issues filed with no project. Pick another one."
            );
        }
        sqlx::query("update projects set prefix = ? where id = ?")
            .bind(&prefix)
            .bind(project_id)
            .execute(&self.db.pool)
            .await?;
        // Keys are stored, so renaming a prefix is a decision about the past too:
        // rewrite this project's keys and leave every other project's alone.
        sqlx::query("update issues set issue_key = ? || '-' || number where project_id = ?")
            .bind(&prefix)
            .bind(project_id)
            .execute(&self.db.pool)
            .await?;
        Ok(sqlx::query_as("select * from projects where id = ?")
            .bind(project_id)
            .fetch_one(&self.db.pool)
            .await?)
    }

    /// The next number and the key that carries it, scoped to the project — or to
    /// the workspace's own bucket when the issue belongs to no project.
    pub async fn next_issue_key(
        &self,
        workspace_id: Uuid,
        project_id: Option<Uuid>,
    ) -> Result<(i64, String)> {
        let prefix = match project_id {
            Some(id) => {
                let project: Project = sqlx::query_as("select * from projects where id = ?")
                    .bind(id)
                    .fetch_one(&self.db.pool)
                    .await?;
                match project.prefix {
                    Some(prefix) => prefix,
                    None => Self::derive_prefix(&project.name),
                }
            }
            None => {
                let workspace: Workspace = sqlx::query_as("select * from workspaces where id = ?")
                    .bind(workspace_id)
                    .fetch_one(&self.db.pool)
                    .await?;
                workspace.default_prefix
            }
        };
        let highest: Option<(i64,)> =
            match project_id {
                Some(id) => {
                    sqlx::query_as("select max(number) from issues where project_id = ?")
                        .bind(id)
                        .fetch_optional(&self.db.pool)
                        .await?
                }
                None => sqlx::query_as(
                    "select max(number) from issues where workspace_id = ? and project_id is null",
                )
                .bind(workspace_id)
                .fetch_optional(&self.db.pool)
                .await?,
            };
        let number = highest.map(|(n,)| n).unwrap_or(0) + 1;
        Ok((number, format!("{prefix}-{number}")))
    }

    pub async fn milestone(&self, id: Uuid) -> Result<Milestone> {
        sqlx::query_as::<_, Milestone>("select * from milestones where id = ?")
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await?
            .ok_or_else(|| anyhow!("milestone {id} not found"))
    }

    pub async fn milestones(&self, workspace_id: Uuid) -> Result<Vec<Milestone>> {
        Ok(sqlx::query_as::<_, Milestone>(
            "select * from milestones where workspace_id = ? order by created_at",
        )
        .bind(workspace_id)
        .fetch_all(&self.db.pool)
        .await?)
    }

    pub async fn create_milestone(
        &self,
        workspace_id: Uuid,
        project_id: Option<Uuid>,
        title: String,
        body: String,
        target_date: Option<String>,
    ) -> Result<Milestone> {
        let title = title.trim().to_string();
        if title.len() < 3 {
            bail!("a milestone needs a title, so it can be referred to later");
        }
        let id = Uuid::now_v7();
        let now = crate::db::now_rfc3339();
        sqlx::query(
            "insert into milestones (id, workspace_id, project_id, title, body, status, target_date, created_at, updated_at)
             values (?, ?, ?, ?, ?, 'open', ?, ?, ?)",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(project_id)
        .bind(&title)
        .bind(&body)
        .bind(&target_date)
        .bind(&now)
        .bind(&now)
        .execute(&self.db.pool)
        .await?;
        self.milestone(id).await
    }

    pub async fn update_milestone(
        &self,
        id: Uuid,
        title: Option<String>,
        body: Option<String>,
        status: Option<String>,
        target_date: Option<String>,
    ) -> Result<Milestone> {
        let milestone = self.milestone(id).await?;
        let status = status.unwrap_or(milestone.status);
        if !matches!(status.as_str(), "open" | "closed") {
            bail!("a milestone is open or closed");
        }
        sqlx::query(
            "update milestones set title = ?, body = ?, status = ?, target_date = ?, updated_at = ? where id = ?",
        )
        .bind(title.unwrap_or(milestone.title))
        .bind(body.unwrap_or(milestone.body))
        .bind(&status)
        .bind(target_date.or(milestone.target_date))
        .bind(crate::db::now_rfc3339())
        .bind(id)
        .execute(&self.db.pool)
        .await?;
        self.milestone(id).await
    }

    pub async fn set_issue_milestone(
        &self,
        issue_id: Uuid,
        milestone_id: Option<Uuid>,
    ) -> Result<Issue> {
        if let Some(id) = milestone_id {
            let milestone = self.milestone(id).await?;
            let issue = self.issue(issue_id).await?;
            if milestone.workspace_id != issue.workspace_id {
                bail!("that milestone belongs to another workspace");
            }
        }
        sqlx::query("update issues set milestone_id = ?, updated_at = ? where id = ?")
            .bind(milestone_id)
            .bind(crate::db::now_rfc3339())
            .bind(issue_id)
            .execute(&self.db.pool)
            .await?;
        self.issue(issue_id).await
    }

    pub fn artifact_dir(&self, id: Uuid) -> std::path::PathBuf {
        self.db.data_dir.join("artifacts").join(id.to_string())
    }

    pub async fn artifact(&self, id: Uuid) -> Result<Artifact> {
        sqlx::query_as::<_, Artifact>("select * from artifacts where id = ?")
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await?
            .ok_or_else(|| anyhow!("artifact {id} not found"))
    }

    pub fn artifact_path(&self, artifact: &Artifact) -> std::path::PathBuf {
        self.artifact_dir(artifact.id).join(&artifact.filename)
    }

    pub async fn artifacts(
        &self,
        workspace_id: Uuid,
        milestone_id: Option<Uuid>,
    ) -> Result<Vec<Artifact>> {
        let rows = match milestone_id {
            Some(id) => {
                sqlx::query_as::<_, Artifact>(
                    "select * from artifacts where workspace_id = ? and milestone_id = ? order by created_at desc",
                )
                .bind(workspace_id)
                .bind(id)
                .fetch_all(&self.db.pool)
                .await?
            }
            None => {
                sqlx::query_as::<_, Artifact>(
                    "select * from artifacts where workspace_id = ? order by created_at desc",
                )
                .bind(workspace_id)
                .fetch_all(&self.db.pool)
                .await?
            }
        };
        Ok(rows)
    }

    /// The files attached to one task, oldest first, the order they were added.
    pub async fn issue_files(&self, issue_id: Uuid) -> Result<Vec<Artifact>> {
        Ok(sqlx::query_as::<_, Artifact>(
            "select * from artifacts where issue_id = ? order by created_at, id",
        )
        .bind(issue_id)
        .fetch_all(&self.db.pool)
        .await?)
    }

    /// Store bytes on disk and a row in the database. The row is the index; the
    /// file is the thing, kept under `artifacts/<id>/<filename>` so a delete is one
    /// directory and two artifacts can share a filename without ever colliding.
    #[allow(clippy::too_many_arguments)]
    pub async fn store_artifact(
        &self,
        workspace_id: Uuid,
        project_id: Option<Uuid>,
        milestone_id: Option<Uuid>,
        issue_id: Option<Uuid>,
        title: String,
        kind: Option<String>,
        filename: String,
        mime: Option<String>,
        bytes: &[u8],
    ) -> Result<Artifact> {
        // A file can only be attached to a task of the same workspace; anything
        // else would leak the file into somebody else's run input.
        if let Some(issue_id) = issue_id {
            let issue = self.issue(issue_id).await?;
            if issue.workspace_id != workspace_id {
                return Err(anyhow!("task {issue_id} belongs to another workspace"));
            }
        }
        let title = if title.trim().is_empty() {
            filename.clone()
        } else {
            title.trim().to_string()
        };
        let filename = filename
            .trim()
            .rsplit('/')
            .next()
            .unwrap_or("artifact")
            .replace(['\n', '\r'], "");
        let filename = if filename.is_empty() {
            "artifact".to_string()
        } else {
            filename
        };
        let id = Uuid::now_v7();
        let dir = self.artifact_dir(id);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating the artifact directory {}", dir.display()))?;
        std::fs::write(dir.join(&filename), bytes)
            .with_context(|| format!("writing the artifact into {}", dir.display()))?;
        let kind = kind.unwrap_or_else(|| match filename.rsplit('.').next() {
            Some("png" | "jpg" | "jpeg" | "gif" | "svg" | "webp") => "image".into(),
            Some("zip" | "tar" | "gz" | "tgz") => "archive".into(),
            Some("md" | "txt" | "pdf" | "json" | "yaml" | "yml") => "document".into(),
            _ => "file".into(),
        });
        sqlx::query(
            "insert into artifacts (id, workspace_id, project_id, milestone_id, issue_id, title, kind, filename, mime, size, sha256, created_at)
             values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(project_id)
        .bind(milestone_id)
        .bind(issue_id)
        .bind(&title)
        .bind(&kind)
        .bind(&filename)
        .bind(mime.unwrap_or_else(|| "application/octet-stream".into()))
        .bind(bytes.len() as i64)
        .bind(sha256_hex(bytes))
        .bind(crate::db::now_rfc3339())
        .execute(&self.db.pool)
        .await?;
        self.artifact(id).await
    }

    pub async fn delete_artifact(&self, id: Uuid) -> Result<bool> {
        let artifact = self.artifact(id).await?;
        sqlx::query("delete from artifacts where id = ?")
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        let dir = self.artifact_dir(artifact.id);
        let _ = std::fs::remove_dir_all(&dir);
        Ok(true)
    }
}

impl Engine {
    /// Closes a task that has no repository. There is no code to merge and no
    /// base to move, so the merge is only the transition; `merged_at` still
    /// dates the finish so cost reporting attributes the task's effort.
    async fn close_without_repository(
        &self,
        issue: &crate::model::Issue,
    ) -> anyhow::Result<crate::model::Issue> {
        sqlx::query("update issues set merged_at = ?, updated_at = ? where id = ?")
            .bind(crate::db::now_rfc3339())
            .bind(crate::db::now_rfc3339())
            .bind(issue.id)
            .execute(&self.db.pool)
            .await?;
        self.comment(
            issue.id,
            hulahoop_core::AuthorType::Server,
            None,
            "Closed. This task has no repository, so there is nothing to merge.",
            false,
        )
        .await?;
        self.transition(issue.id, hulahoop_core::IssueEvent::Merge)
            .await
    }

    /// Closes a task whose project has no pull request to read a merge from.
    ///
    /// A repository with no GitHub remote still has a loop in it: the head the
    /// review looked at is what the work became, so it becomes the base and the
    /// merge is recorded against the run's own revision.
    async fn merge_without_pull_request(
        &self,
        issue: &crate::model::Issue,
    ) -> anyhow::Result<crate::model::Issue> {
        let head: Option<(Option<String>,)> = sqlx::query_as(
            "select head_revision from runs where issue_id = ? and head_revision is not null \
             order by created_at desc limit 1",
        )
        .bind(issue.id)
        .fetch_optional(&self.db.pool)
        .await?;
        let revision = head.and_then(|(revision,)| revision).ok_or_else(|| {
            anyhow::anyhow!("nothing was published on this task, so there is nothing to merge yet")
        })?;

        sqlx::query(
            "update issues set merge_revision = ?, merged_at = ?, updated_at = ? where id = ?",
        )
        .bind(&revision)
        .bind(crate::db::now_rfc3339())
        .bind(crate::db::now_rfc3339())
        .bind(issue.id)
        .execute(&self.db.pool)
        .await?;
        // The base only becomes the merged head once the remote has it: a
        // runner moves the base bookmark forward in the mirror and pushes it,
        // and that landing records the new base. Recording it here first would
        // let the next fetch, which still sees the old remote, contradict it.
        if let Some(repo_id) = issue.repo_id {
            self.queue_job(
                repo_id,
                Some(issue.id),
                "land",
                serde_json::json!({ "revision": revision }),
            )
            .await?;
        }
        self.comment(
            issue.id,
            hulahoop_core::AuthorType::Server,
            None,
            &format!(
                "Merged as {}. There is no pull request on this project, so the head the review looked at is pushed to the base branch.",
                &revision[..12.min(revision.len())]
            ),
            false,
        )
        .await?;
        self.transition(issue.id, hulahoop_core::IssueEvent::Merge)
            .await
    }

    /// Records a merge as GitHub describes it.
    ///
    /// The revision a merge produced only exists on GitHub's side, so it is read
    /// from the pull request instead of typed by a person: a hand-pasted hash is
    /// the kind of fact that is right once and stale afterwards.
    pub async fn merge_issue(&self, issue_id: uuid::Uuid) -> anyhow::Result<crate::model::Issue> {
        let issue = self.issue(issue_id).await?;
        let Some(repo_id) = issue.repo_id else {
            return self.close_without_repository(&issue).await;
        };
        let Some(number) = issue.pr_number else {
            // No pull request: either the project has no GitHub remote at all, or
            // the handover never opened one. Both mean there is no GitHub side to
            // read, and the run's own head is the merge.
            return self.merge_without_pull_request(&issue).await;
        };
        let repo: crate::model::Repo = sqlx::query_as("select * from repos where id = ?")
            .bind(repo_id)
            .fetch_one(&self.db.pool)
            .await?;
        let remote = crate::api::clone_url(&repo).ok_or_else(|| {
            anyhow::anyhow!(
                "repository {} has no remote to talk to github through",
                repo.name
            )
        })?;
        let (owner, name) = crate::github::split_remote(&remote)
            .ok_or_else(|| anyhow::anyhow!("{remote} is not a github repository"))?;

        let app = self
            .stored_app(crate::github::PROVIDER, crate::connectors::KIND_GITHUB_APP)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!("no usable GitHub app is stored, so the merge cannot be read back")
            })?;
        let jwt = crate::github::mint_token(&app.app_id, &app.private_key)?;
        let http = crate::github::http_client();
        let token = crate::github::installation_token(&http, &jwt, &owner, &name).await?;
        let state = crate::github::pull_request(&http, &token, &owner, &name, number).await?;

        if !state.merged {
            anyhow::bail!(
                "pull request #{number} is still {} on github. Merge it there first, then press again — the revision comes from github, so nothing has to be typed."
                , state.state
            );
        }
        let merge_revision = state.merge_commit_sha.ok_or_else(|| {
            anyhow::anyhow!("github reports #{number} merged but names no revision")
        })?;
        self.record_github_merge(issue_id, repo_id, &merge_revision, state.merged_at)
            .await
    }

    /// Records a merge GitHub made: the revision it produced becomes the task's
    /// merge and the repository's base, the task says so, closes, and the mirror
    /// fetches. Shared by the Merge button on a task, which reads a merge back
    /// from GitHub, and the one on a pull request, which made it.
    pub(crate) async fn record_github_merge(
        &self,
        issue_id: uuid::Uuid,
        repo_id: uuid::Uuid,
        merge_revision: &str,
        merged_at: Option<String>,
    ) -> anyhow::Result<crate::model::Issue> {
        sqlx::query(
            "update issues set merge_revision = ?, merged_at = ?, updated_at = ? where id = ?",
        )
        .bind(merge_revision)
        .bind(merged_at.unwrap_or_else(crate::db::now_rfc3339))
        .bind(crate::db::now_rfc3339())
        .bind(issue_id)
        .execute(&self.db.pool)
        .await?;
        sqlx::query("update repos set base_revision = ? where id = ?")
            .bind(merge_revision)
            .bind(repo_id)
            .execute(&self.db.pool)
            .await?;
        self.comment(
            issue_id,
            hulahoop_core::AuthorType::Server,
            None,
            &format!(
                "Merged as {} on github. The repository's base is now that revision, so the next task starts from the merge.",
                &merge_revision[..12.min(merge_revision.len())]
            ),
            false,
        )
        .await?;
        let merged = self
            .transition(issue_id, hulahoop_core::IssueEvent::Merge)
            .await?;
        self.fetch_after_merge(Some(repo_id)).await;
        Ok(merged)
    }

    /// A merge moves the base, so the mirror fetches: the next task's checkout
    /// starts from the newest revision instead of the one before the merge. The
    /// merge is already recorded, so a fetch that cannot be queued is logged
    /// rather than reported as a failed merge.
    async fn fetch_after_merge(&self, repo_id: Option<uuid::Uuid>) {
        let Some(repo_id) = repo_id else {
            return;
        };
        if let Err(error) = self
            .queue_job(repo_id, None, "fetch", serde_json::json!({}))
            .await
        {
            tracing::warn!(repo = %repo_id, %error, "could not queue the fetch after a merge");
        }
    }
}
