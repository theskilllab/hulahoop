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

impl Engine {
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

    /// Store bytes on disk and a row in the database. The row is the index; the
    /// file is the thing, kept under `artifacts/<id>/<filename>` so a delete is one
    /// directory and two artifacts can share a filename without ever colliding.
    #[allow(clippy::too_many_arguments)]
    pub async fn store_artifact(
        &self,
        workspace_id: Uuid,
        project_id: Option<Uuid>,
        milestone_id: Option<Uuid>,
        title: String,
        kind: Option<String>,
        filename: String,
        mime: Option<String>,
        bytes: &[u8],
    ) -> Result<Artifact> {
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
            "insert into artifacts (id, workspace_id, project_id, milestone_id, title, kind, filename, mime, size, sha256, created_at)
             values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(project_id)
        .bind(milestone_id)
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
