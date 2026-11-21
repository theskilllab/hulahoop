//! Skills: knowledge written once and attached to the agents that need it.

use uuid::Uuid;

use crate::engine::Engine;
use crate::model::Skill;

impl Engine {
    pub async fn skills(&self, workspace_id: Uuid) -> anyhow::Result<Vec<Skill>> {
        Ok(sqlx::query_as::<_, Skill>(
            "select * from skills where workspace_id = ? order by name collate nocase",
        )
        .bind(workspace_id)
        .fetch_all(&self.db.pool)
        .await?)
    }

    pub async fn create_skill(
        &self,
        workspace_id: Uuid,
        name: &str,
        description: &str,
        source: &str,
    ) -> anyhow::Result<Skill> {
        let name = name.trim();
        if name.is_empty() {
            anyhow::bail!("a skill needs a name");
        }
        let id = Uuid::now_v7();
        let now = crate::db::now_rfc3339();
        sqlx::query(
            "insert into skills (id, workspace_id, name, description, source, created_at, updated_at) \
             values (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(name)
        .bind(description.trim())
        .bind(source)
        .bind(&now)
        .bind(&now)
        .execute(&self.db.pool)
        .await?;
        Ok(
            sqlx::query_as::<_, Skill>("select * from skills where id = ?")
                .bind(id)
                .fetch_one(&self.db.pool)
                .await?,
        )
    }

    pub async fn update_skill(
        &self,
        id: Uuid,
        name: Option<&str>,
        description: Option<&str>,
        source: Option<&str>,
    ) -> anyhow::Result<Skill> {
        let skill: Skill = sqlx::query_as("select * from skills where id = ?")
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Issue not found"))?;
        sqlx::query(
            "update skills set name = ?, description = ?, source = ?, updated_at = ? where id = ?",
        )
        .bind(name.map(str::trim).unwrap_or(skill.name.as_str()))
        .bind(
            description
                .map(str::trim)
                .unwrap_or(skill.description.as_str()),
        )
        .bind(source.unwrap_or(skill.source.as_str()))
        .bind(crate::db::now_rfc3339())
        .bind(id)
        .execute(&self.db.pool)
        .await?;
        Ok(
            sqlx::query_as::<_, Skill>("select * from skills where id = ?")
                .bind(id)
                .fetch_one(&self.db.pool)
                .await?,
        )
    }

    pub async fn delete_skill(&self, id: Uuid) -> anyhow::Result<bool> {
        let done = sqlx::query("delete from skills where id = ?")
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        Ok(done.rows_affected() > 0)
    }

    /// The skills an agent carries, in name order.
    pub async fn agent_skills(&self, agent_id: Uuid) -> anyhow::Result<Vec<Skill>> {
        Ok(sqlx::query_as::<_, Skill>(
            "select s.* from skills s join agent_skills a on a.skill_id = s.id \
             where a.agent_id = ? order by s.name collate nocase",
        )
        .bind(agent_id)
        .fetch_all(&self.db.pool)
        .await?)
    }

    /// Replace an agent's skill set. Skills from another workspace are refused,
    /// because a skill is that workspace's knowledge.
    pub async fn set_agent_skills(
        &self,
        agent_id: Uuid,
        skill_ids: &[Uuid],
    ) -> anyhow::Result<Vec<Skill>> {
        let workspace: Option<(Uuid,)> = sqlx::query_as("select id from agents where id = ?")
            .bind(agent_id)
            .fetch_optional(&self.db.pool)
            .await?;
        let Some((workspace_id,)) = workspace else {
            anyhow::bail!("Issue not found");
        };
        sqlx::query("delete from agent_skills where agent_id = ?")
            .bind(agent_id)
            .execute(&self.db.pool)
            .await?;
        for skill_id in skill_ids {
            let owned: Option<(Uuid,)> =
                sqlx::query_as("select id from skills where id = ? and workspace_id = ?")
                    .bind(skill_id)
                    .bind(workspace_id)
                    .fetch_optional(&self.db.pool)
                    .await?;
            if owned.is_none() {
                anyhow::bail!("skill {skill_id} is not in this workspace");
            }
            sqlx::query("insert into agent_skills (agent_id, skill_id) values (?, ?)")
                .bind(agent_id)
                .bind(skill_id)
                .execute(&self.db.pool)
                .await?;
        }
        self.agent_skills(agent_id).await
    }

    pub async fn set_agent_instructions(
        &self,
        agent_id: Uuid,
        agents_md: &str,
    ) -> anyhow::Result<()> {
        sqlx::query("update agents set agents_md = ?, updated_at = ? where id = ?")
            .bind(agents_md)
            .bind(crate::db::now_rfc3339())
            .bind(agent_id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    /// What the agent is told: the role's prompt, then its own instructions, then
    /// the skills it carries. Order matters — the role says what to do, the agent
    /// says how it works, the skills say what it knows.
    pub async fn compose_prompt(
        &self,
        role: hulahoop_core::AgentRole,
        agent: &crate::model::Agent,
    ) -> anyhow::Result<String> {
        let mut prompt = String::from(crate::engine::prompt_for(role));
        if !agent.agents_md.trim().is_empty() {
            prompt.push_str("\n\n---\n\n# How you work\n\n");
            prompt.push_str(agent.agents_md.trim());
            prompt.push('\n');
        }
        let skills = self.agent_skills(agent.id).await?;
        if !skills.is_empty() {
            prompt.push_str("\n\n---\n\n# Skills\n");
            for skill in skills {
                prompt.push_str(&format!("\n## {}\n\n", skill.name));
                if !skill.description.is_empty() {
                    prompt.push_str(skill.description.trim());
                    prompt.push_str("\n\n");
                }
                prompt.push_str(skill.source.trim());
                prompt.push('\n');
            }
        }
        Ok(prompt)
    }

    /// Prompt provenance: the hash now covers the role prompt and everything the
    /// agent was told, so two runs with different instructions never share a id.
    pub fn prompt_revision(&self, prompt: &str) -> String {
        crate::db::sha256_hex(prompt.as_bytes())
    }
}
