//! Skills: knowledge written once and attached to the agents that need it.

use uuid::Uuid;

use crate::engine::Engine;
use crate::model::{Skill, SkillUsage};

/// The contract every turn has to read: the markers the server parses, the
/// house rules, and how a turn ends. Embedded, because a run must never depend on
/// the checkout's contents.
pub const CONTRACT_SKILL: &str = include_str!("../skills/run-contract.md");
pub const CONTRACT_NAME: &str = "run-contract";

/// Told to every agent whose task has no repository. The run contract covers the
/// same case; this says it for the turn at hand, so nothing hinges on the agent
/// noticing a null in `input.json`.
pub const NO_REPOSITORY: &str = "# This task has no repository

This task belongs to no repository, so there is no checkout. Your working \
directory is an empty scratch directory that is yours for this turn only and is \
deleted when the turn ends. Work there freely. There is nothing to commit, no \
pull request, and no `just test` to run; skip the house rules about the suite. \
Your result is your final message: put the answer, the findings or the \
deliverable in it. If you produce a file a person needs, copy it into \
`$HULAHOOP_ARTIFACT_DIR` (kept with the run) or attach it to the task if your \
tools allow it, name it in the summary, and include its content when it is \
short. End with the \
usual `HULAHOOP_VERDICT:` or `HULAHOOP_QUESTION:` line.";

impl Engine {
    /// Give a workspace the skills it is born with, and give every agent the
    /// contract. Idempotent: it runs at startup and after a workspace is created,
    /// and a second call changes nothing.
    pub async fn seed_builtin_skills(&self, workspace_id: Uuid) -> anyhow::Result<()> {
        let now = crate::db::now_rfc3339();
        let existing: Option<(Uuid,)> =
            sqlx::query_as("select id from skills where workspace_id = ? and name = ? limit 1")
                .bind(workspace_id)
                .bind(CONTRACT_NAME)
                .fetch_optional(&self.db.pool)
                .await?;
        // The file carries a `SKILL.md` front matter so editors understand it; the
        // prompt gets the body alone, because the agent reads markdown, not YAML.
        let (_, description, body) = split_front_matter(
            CONTRACT_NAME,
            "Read before anything else in every turn. How a turn starts, what it may change, and how it ends.",
            CONTRACT_SKILL,
        );
        let id = match existing {
            Some((id,)) => {
                // The contract is the wire format, so the binary's copy wins: an
                // upgrade that adds a marker must reach every existing workspace,
                // and the desktop shows this skill read-only for the same reason.
                sqlx::query(
                    "update skills set description = ?, source = ?, builtin = 1, updated_at = ?
                     where id = ? and (source != ? or builtin != 1 or description != ?)",
                )
                .bind(&description)
                .bind(&body)
                .bind(&now)
                .bind(id)
                .bind(&body)
                .bind(&description)
                .execute(&self.db.pool)
                .await?;
                id
            }
            None => {
                let id = Uuid::now_v7();
                sqlx::query(
                    "insert into skills (id, workspace_id, name, description, source, builtin, created_at, updated_at)
                     values (?, ?, ?, ?, ?, 1, ?, ?)",
                )
                .bind(id)
                .bind(workspace_id)
                .bind(CONTRACT_NAME)
                .bind(&description)
                .bind(&body)
                .bind(&now)
                .bind(&now)
                .execute(&self.db.pool)
                .await?;
                id
            }
        };
        // Every agent reads it, including the ones that existed before it did.
        let agents: Vec<(Uuid,)> = sqlx::query_as("select id from agents where workspace_id = ?")
            .bind(workspace_id)
            .fetch_all(&self.db.pool)
            .await?;
        for (agent_id,) in agents {
            sqlx::query("insert or ignore into agent_skills (agent_id, skill_id) values (?, ?)")
                .bind(agent_id)
                .bind(id)
                .execute(&self.db.pool)
                .await?;
        }
        Ok(())
    }

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
        // A `SKILL.md` pasted whole carries its own name and description. They are
        // the import format: what the front matter says becomes the row, and the
        // body is kept as the markdown alone, so there is one place a skill's name
        // is written and one place it is read from.
        let (name, description, source) = split_front_matter(name, description, source);
        let name = check_name(&name)?;
        let id = Uuid::now_v7();
        let now = crate::db::now_rfc3339();
        sqlx::query(
            "insert into skills (id, workspace_id, name, description, source, created_at, updated_at) \
             values (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(&name)
        .bind(description.trim())
        .bind(source.trim())
        .bind(&now)
        .bind(&now)
        .execute(&self.db.pool)
        .await
        .map_err(|error| {
            // The name is the checkout directory, so a second skill with the same
            // one would overwrite the first's file. Say which name is taken.
            if error.to_string().contains("UNIQUE constraint failed") {
                anyhow::anyhow!("a skill named {name} already exists in this workspace")
            } else {
                error.into()
            }
        })?;
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
        if skill.builtin == 1 {
            anyhow::bail!("the run contract is the server's text, so it is not edited here");
        }
        let (name, description, source) = split_front_matter(
            name.unwrap_or(&skill.name),
            description.unwrap_or(&skill.description),
            source.unwrap_or(&skill.source),
        );
        let name = check_name(&name)?;
        sqlx::query(
            "update skills set name = ?, description = ?, source = ?, updated_at = ? where id = ?",
        )
        .bind(name)
        .bind(description.trim())
        .bind(source.trim())
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
        let workspace: Option<(Uuid,)> =
            sqlx::query_as("select workspace_id from agents where id = ?")
                .bind(agent_id)
                .fetch_optional(&self.db.pool)
                .await?;
        let Some((workspace_id,)) = workspace else {
            anyhow::bail!("no agent {agent_id}");
        };
        // One transaction: a skill from another workspace must not be able to leave
        // an agent carrying nothing at all.
        let mut tx = self.db.pool.begin().await?;
        for skill_id in skill_ids {
            let owned: Option<(Uuid,)> =
                sqlx::query_as("select id from skills where id = ? and workspace_id = ?")
                    .bind(skill_id)
                    .bind(workspace_id)
                    .fetch_optional(&mut *tx)
                    .await?;
            if owned.is_none() {
                anyhow::bail!("skill {skill_id} is not in this workspace");
            }
        }
        sqlx::query("delete from agent_skills where agent_id = ?")
            .bind(agent_id)
            .execute(&mut *tx)
            .await?;
        for skill_id in skill_ids {
            sqlx::query("insert into agent_skills (agent_id, skill_id) values (?, ?)")
                .bind(agent_id)
                .bind(skill_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
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

    /// The skills that have to be written into a checkout before a run.
    pub async fn skill_inputs(
        &self,
        agent_id: Uuid,
    ) -> anyhow::Result<Vec<hulahoop_core::runspec::SkillInput>> {
        Ok(self
            .agent_skills(agent_id)
            .await?
            .into_iter()
            .filter(|skill| skill.builtin == 0)
            .map(|skill| hulahoop_core::runspec::SkillInput {
                name: skill.name,
                description: skill.description,
                body: skill.source,
            })
            .collect())
    }

    /// What the agent is told before it starts.
    ///
    /// The workspace's built-in skills come first and in full: they are the run
    /// contract, and a turn that never reads the markers produces nothing the
    /// server can read back. The agent's own instructions follow, because they say
    /// what this person does rather than what the loop needs. Attached skills are
    /// named by their description only — the runner writes them into the checkout,
    /// and the harness opens the one that fits the work. Inlining every body would
    /// spend the same context on the four skills this task never needed.
    ///
    /// `has_repo` is false for a task with no repository: the agent is told it
    /// works in an empty scratch directory and reports through its summary.
    /// `memory_index` is the rendered memory index, empty when there is none.
    pub async fn compose_prompt(
        &self,
        agent: &crate::model::Agent,
        has_repo: bool,
        memory_index: &str,
    ) -> anyhow::Result<String> {
        let skills = self.agent_skills(agent.id).await?;
        let mut prompt = String::new();
        for skill in skills.iter().filter(|skill| skill.builtin == 1) {
            prompt.push_str(&format!("# {}\n\n", skill.name));
            prompt.push_str(skill.source.trim());
            prompt.push_str("\n\n");
        }
        if !has_repo {
            prompt.push_str(NO_REPOSITORY);
            prompt.push_str("\n\n");
        }
        if !agent.agents_md.trim().is_empty() {
            prompt.push_str("# How you work\n\n");
            prompt.push_str(agent.agents_md.trim());
            prompt.push('\n');
        }
        if !memory_index.is_empty() {
            prompt.push_str("\n# What you already know\n\n");
            prompt.push_str(memory_index);
        }
        let carried: Vec<&Skill> = skills.iter().filter(|skill| skill.builtin == 0).collect();
        if !carried.is_empty() {
            prompt.push_str("\n# Skills you carry\n\n");
            prompt.push_str(
                "Each one is a `SKILL.md` in the checkout. Read the file when its description matches the work; you do not need the others.\n\n",
            );
            for skill in carried {
                prompt.push_str(&format!(
                    "- `{}` — {} (.hulahoop/skills/{}/SKILL.md)\n",
                    skill.name,
                    skill.description.trim(),
                    hulahoop_core::slugify(&skill.name)
                ));
            }
        }
        Ok(prompt)
    }

    /// Which agents carry each of the workspace's skills. The library is only
    /// worth reading if you can see where a skill is already loaded.
    pub async fn skill_usage(&self, workspace_id: Uuid) -> anyhow::Result<Vec<SkillUsage>> {
        let rows: Vec<(Uuid, Uuid, String)> = sqlx::query_as(
            "select a.skill_id, g.id, g.name \
             from agent_skills a \
             join agents g on g.id = a.agent_id \
             join skills s on s.id = a.skill_id \
             where s.workspace_id = ? \
             order by g.name collate nocase",
        )
        .bind(workspace_id)
        .fetch_all(&self.db.pool)
        .await?;
        let mut usage: Vec<SkillUsage> = Vec::new();
        for (skill_id, agent_id, agent_name) in rows {
            match usage.iter_mut().find(|entry| entry.skill_id == skill_id) {
                Some(entry) => {
                    entry.agent_ids.push(agent_id);
                    entry.agent_names.push(agent_name);
                }
                None => usage.push(SkillUsage {
                    skill_id,
                    agent_ids: vec![agent_id],
                    agent_names: vec![agent_name],
                }),
            }
        }
        Ok(usage)
    }

    /// Prompt provenance: the hash now covers the role prompt and everything the
    /// agent was told, so two runs with different instructions never share a id.
    pub fn prompt_revision(&self, prompt: &str) -> String {
        crate::db::sha256_hex(prompt.as_bytes())
    }
}

/// A skill's name is its directory in the checkout, so it has to survive being a
/// path: lowercase, hyphenated, no spaces. Two names that differ only in case are
/// one name, and the index that enforces that says the same.
fn check_name(name: &str) -> anyhow::Result<String> {
    let clean = name.trim().to_lowercase().replace(' ', "-");
    if clean.is_empty() {
        anyhow::bail!("a skill needs a name");
    }
    if !clean
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        anyhow::bail!("a skill name is lowercase words joined by hyphens: {clean}");
    }
    Ok(clean)
}

/// Take a pasted `SKILL.md` apart: the front matter wins over whatever the caller
/// typed, and the body is what the agent reads.
fn split_front_matter(name: &str, description: &str, source: &str) -> (String, String, String) {
    let text = source.trim_start();
    let Some(rest) = text.strip_prefix("---") else {
        return (name.to_string(), description.to_string(), text.to_string());
    };
    let Some(end) = rest.find("\n---") else {
        return (name.to_string(), description.to_string(), text.to_string());
    };
    let front = &rest[..end];
    let mut found_name = None;
    let mut found_description = None;
    for line in front.lines() {
        let (key, value) = match line.split_once(':') {
            Some(pair) => pair,
            None => continue,
        };
        let value = value
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
        match key.trim() {
            "name" => found_name = Some(value),
            "description" => found_description = Some(value),
            _ => {}
        }
    }
    let body = rest[end + 4..].trim().to_string();
    (
        found_name.unwrap_or_else(|| name.to_string()),
        found_description.unwrap_or_else(|| description.to_string()),
        body,
    )
}

#[cfg(test)]
mod skill_markdown_tests {
    use super::{check_name, split_front_matter};

    /// A skill pasted as a whole `SKILL.md` carries its own identity, and the body
    /// the agent reads is the markdown under the front matter.
    #[test]
    fn a_pasted_skill_md_gives_up_its_name_and_description() {
        let (name, description, body) = split_front_matter(
            "",
            "",
            "---\nname: house-style\ndescription: how we write here\n---\n\n# House Style\n\nUse sentences.\n",
        );
        assert_eq!(name, "house-style");
        assert_eq!(description, "how we write here");
        assert_eq!(body, "# House Style\n\nUse sentences.");
    }

    /// The editor's fields are the source when there is no front matter, and a
    /// paragraph that merely starts with dashes is body text.
    #[test]
    fn a_body_without_front_matter_keeps_what_the_editor_says() {
        let (name, description, body) = split_front_matter(
            "house-style",
            "how we write",
            "# House Style\n\nUse sentences.",
        );
        assert_eq!(name, "house-style");
        assert_eq!(description, "how we write");
        assert_eq!(body, "# House Style\n\nUse sentences.");

        let (_, _, body) = split_front_matter("x", "y", "--- not a block\nstill body");
        assert_eq!(body, "--- not a block\nstill body");
    }

    /// The name is a directory in the checkout, so spaces become hyphens and case
    /// folds; anything that cannot be a path is refused rather than slugified into
    /// something surprising.
    #[test]
    fn a_name_is_a_directory() {
        assert_eq!(check_name(" House Style \n").expect("name"), "house-style");
        assert!(check_name("sql/notes").is_err());
        assert!(check_name("  ").is_err());
    }
}
