//! The workflow document of a workspace, and the board it produces.

use async_graphql::SimpleObject;
use hulahoop_core::workflow::{self, Owner, Workflow};
use hulahoop_core::IssueStatus;
use sqlx::Row;
use uuid::Uuid;

use crate::engine::Engine;

/// One section, shaped for the screen.
#[derive(Debug, Clone, SimpleObject)]
pub struct WorkflowSection {
    /// The status this section moves, as a machine name: `in_progress`.
    pub status: String,
    /// The heading as written, so the board shows the team's wording.
    pub title: String,
    /// `human` or `agent`.
    pub owner: String,
    /// The agent name, or `any` when the project's binding decides.
    pub agent: Option<String>,
    /// The status this one leads to, if it leads anywhere.
    pub next: Option<String>,
    /// Why that move happens: the sentence the diagram writes on the arrow.
    pub when: Option<String>,
    pub body: String,
}

/// The document plus what the server understood of it.
#[derive(Debug, Clone, SimpleObject)]
pub struct WorkflowDoc {
    pub source: String,
    pub updated_at: Option<String>,
    pub sections: Vec<WorkflowSection>,
    /// What stops this document from driving the board. Empty means it does.
    pub errors: Vec<String>,
}

fn shape(parsed: &Workflow) -> Vec<WorkflowSection> {
    parsed
        .sections
        .iter()
        .map(|section| WorkflowSection {
            status: section.status.as_str().to_string(),
            title: section.title.clone(),
            owner: section.owner.as_str().to_string(),
            agent: section.agent.clone(),
            next: section.next.map(|s| s.as_str().to_string()),
            when: section.when.clone(),
            body: section.body.clone(),
        })
        .collect()
}

impl Engine {
    /// The workspace's document. A workspace that never saved one reads the
    /// default, so the board is never empty on a fresh install.
    pub async fn workflow(&self, workspace_id: Uuid) -> anyhow::Result<WorkflowDoc> {
        let row = sqlx::query("select source, updated_at from workflows where workspace_id = ?")
            .bind(workspace_id)
            .fetch_optional(&self.db.pool)
            .await?;
        let (source, updated_at) = match row {
            Some(row) => (
                row.get::<String, _>("source"),
                row.get::<Option<String>, _>("updated_at"),
            ),
            None => (workflow::DEFAULT_SOURCE.to_string(), None),
        };
        let parsed = workflow::parse(&source);
        Ok(WorkflowDoc {
            sections: shape(&parsed),
            // A workspace on the default is not "broken": it simply never saved.
            errors: if updated_at.is_some() {
                parsed.errors.clone()
            } else {
                Vec::new()
            },
            source,
            updated_at,
        })
    }

    /// Save a document. A document that does not parse into every status is
    /// refused, because the board would then be lying about where work can go.
    pub async fn set_workflow(
        &self,
        workspace_id: Uuid,
        source: &str,
    ) -> anyhow::Result<WorkflowDoc> {
        let parsed = workflow::parse(source);
        if !parsed.is_complete() {
            anyhow::bail!(
                "this document cannot drive the board yet: {}",
                parsed.errors.join(" ")
            );
        }
        sqlx::query(
            "insert into workflows (workspace_id, source, updated_at) values (?, ?, ?) \
             on conflict (workspace_id) do update set source = excluded.source, \
             updated_at = excluded.updated_at",
        )
        .bind(workspace_id)
        .bind(source)
        .bind(crate::db::now_rfc3339())
        .execute(&self.db.pool)
        .await?;
        self.workflow(workspace_id).await
    }

    /// The status a section leads to, as the machine sees it.
    pub async fn workflow_next(
        &self,
        workspace_id: Uuid,
        status: IssueStatus,
    ) -> anyhow::Result<Option<IssueStatus>> {
        let doc = self.workflow(workspace_id).await?;
        let parsed = workflow::parse(&doc.source);
        Ok(parsed.section(status).and_then(|section| section.next))
    }

    /// The agent name a status dispatches to, when the document names one.
    pub async fn workflow_agent(
        &self,
        workspace_id: Uuid,
        status: IssueStatus,
    ) -> anyhow::Result<Option<String>> {
        let doc = self.workflow(workspace_id).await?;
        let parsed = workflow::parse(&doc.source);
        Ok(parsed.agent_for(status).map(|name| name.to_string()))
    }

    /// Whether a person owns this status. The board uses it to decide whether to
    /// draw an agent working or a person waiting.
    pub async fn workflow_owner(
        &self,
        workspace_id: Uuid,
        status: IssueStatus,
    ) -> anyhow::Result<Owner> {
        let doc = self.workflow(workspace_id).await?;
        let parsed = workflow::parse(&doc.source);
        Ok(parsed
            .section(status)
            .map(|section| section.owner)
            .unwrap_or(Owner::Human))
    }
}
