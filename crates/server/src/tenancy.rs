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
                    None => Self::normalise_prefix(&project.name).unwrap_or_else(|_| "PRJ".into()),
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
