//! Comments people write on a task, and the `@` mentions in them.
//!
//! A mention is a way to get somebody's attention, never a way around the
//! workflow. `@Developer` on a stuck task gives the Developer the turn
//! WORKFLOWS.md already owes it in that status, and nothing more: an agent the
//! status does not name, a task that is blocked or done, a task with a turn in
//! flight all leave the mention as a note the agent reads next time it works
//! the task. Every mention says, in a short comment, which of those happened,
//! so nobody has to guess why an agent did or did not start.
//!
//! `@Lumi` asks Lumi about the task; the answer comes back as a comment.

use anyhow::Result;
use hulahoop_core::{AuthorType, IssueStatus};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::engine::Engine;
use crate::model::Issue;

/// One person a comment names. `kind` is `agent` or `lumi`; `id` is the
/// agent's uuid, or `lumi`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, async_graphql::SimpleObject)]
pub struct Mention {
    #[serde(rename = "type")]
    #[graphql(name = "type")]
    pub kind: String,
    pub id: String,
    pub name: String,
}

/// How many words after an `@` may make up one name. `@Test Author` is how
/// people write an agent called "Test Author"; three words covers every name a
/// team picks without swallowing the rest of the sentence.
const NAME_WORDS: usize = 3;

/// The words that follow each `@` in a comment, as candidates from longest to
/// shortest. An `@` inside a word (an email address) is not a mention.
pub fn mention_candidates(text: &str) -> Vec<Vec<String>> {
    let chars: Vec<char> = text.chars().collect();
    let mut found = Vec::new();
    for (at, c) in chars.iter().enumerate() {
        if *c != '@' || (at > 0 && chars[at - 1].is_alphanumeric()) {
            continue;
        }
        let mut words = Vec::new();
        let mut index = at + 1;
        while words.len() < NAME_WORDS {
            let start = index;
            while index < chars.len() && is_name_char(chars[index]) {
                index += 1;
            }
            if index == start {
                break;
            }
            words.push(chars[start..index].iter().collect::<String>());
            // Only a single space continues a name; punctuation ends it.
            if index < chars.len() && chars[index] == ' ' {
                index += 1;
            } else {
                break;
            }
        }
        if words.is_empty() {
            continue;
        }
        let candidates = (1..=words.len())
            .rev()
            .map(|n| words[..n].concat())
            .collect();
        found.push(candidates);
    }
    found
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// How names compare: spaces and case are how people write, not identity, the
/// same rule `agent_for_name` uses.
fn normalize(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

impl Engine {
    /// Who wrote a comment, as the thread shows it.
    pub async fn author_name(&self, author_type: &str, author_id: Option<&str>) -> Result<String> {
        if author_type == AuthorType::Human.as_str() {
            return Ok("you".into());
        }
        if author_id == Some("lumi") {
            return Ok("Lumi".into());
        }
        if let Some(id) = author_id.and_then(|raw| Uuid::parse_str(raw).ok()) {
            let name: Option<String> = sqlx::query_scalar("select name from agents where id = ?")
                .bind(id)
                .fetch_optional(&self.db.pool)
                .await?;
            if let Some(name) = name {
                return Ok(name);
            }
        }
        Ok(author_type.to_string())
    }

    /// The agents and Lumi a comment names, each once, in the order written.
    pub async fn parse_mentions(&self, workspace_id: Uuid, text: &str) -> Result<Vec<Mention>> {
        let agents: Vec<(Uuid, String)> =
            sqlx::query_as("select id, name from agents where workspace_id = ?")
                .bind(workspace_id)
                .fetch_all(&self.db.pool)
                .await?;
        let mut mentions: Vec<Mention> = Vec::new();
        for candidates in mention_candidates(text) {
            let found = candidates.iter().find_map(|candidate| {
                let wanted = normalize(candidate);
                if wanted == "lumi" {
                    return Some(Mention {
                        kind: "lumi".into(),
                        id: "lumi".into(),
                        name: "Lumi".into(),
                    });
                }
                agents
                    .iter()
                    .find(|(_, name)| normalize(name) == wanted)
                    .map(|(id, name)| Mention {
                        kind: "agent".into(),
                        id: id.to_string(),
                        name: name.clone(),
                    })
            });
            if let Some(mention) = found {
                if !mentions.iter().any(|m| m.id == mention.id) {
                    mentions.push(mention);
                }
            }
        }
        Ok(mentions)
    }

    /// A person's comment on a task. The comment is stored first, with the
    /// mentions it carries; then each mention is acted on and explained.
    pub async fn human_comment(&self, issue_id: Uuid, body: &str) -> Result<Uuid> {
        let issue = self.issue(issue_id).await?;
        let mentions = self.parse_mentions(issue.workspace_id, body).await?;
        let id = self
            .comment(issue_id, AuthorType::Human, None, body, false)
            .await?;
        if !mentions.is_empty() {
            sqlx::query("update comments set mentions = ? where id = ?")
                .bind(serde_json::to_string(&mentions)?)
                .bind(id)
                .execute(&self.db.pool)
                .await?;
        }
        for mention in &mentions {
            let note = match mention.kind.as_str() {
                "lumi" => self.mention_lumi(&issue, body).await,
                _ => self.mention_agent(issue_id, mention).await?,
            };
            if let Some(note) = note {
                self.comment(issue_id, AuthorType::Server, None, &note, false)
                    .await?;
            }
        }
        Ok(id)
    }

    /// `@Lumi`: queue the question with the task attached. When Lumi cannot
    /// answer at all, the reason is the note, so the person is not left
    /// waiting for a reply that will never come.
    async fn mention_lumi(&self, issue: &Issue, body: &str) -> Option<String> {
        let context = format!("Task {}", issue.issue_key);
        match self
            .ask_lumi_about(issue.workspace_id, body, Some(&context), Some(issue.id))
            .await
        {
            Ok(_) => None,
            Err(error) => Some(format!("Lumi can't answer here: {error}")),
        }
    }

    /// `@Agent`: give the agent the turn the workflow owes it, when the task is
    /// waiting on nobody, and say what happened either way.
    async fn mention_agent(&self, issue_id: Uuid, mention: &Mention) -> Result<Option<String>> {
        let issue = self.issue(issue_id).await?;
        let name = &mention.name;
        let status = issue.status_enum();
        let Ok(agent_id) = Uuid::parse_str(&mention.id) else {
            return Ok(None);
        };

        if let Some(run) = self.active_run(issue_id).await? {
            let note = if run.0 == agent_id {
                format!(
                    "Notified {name}: it is working on this now and will see this on its next turn"
                )
            } else {
                format!("Notified {name}: it will see this on its next turn")
            };
            return Ok(Some(note));
        }
        if status == IssueStatus::Blocked || status.is_terminal() {
            return Ok(Some(format!(
                "Notified {name}: the task is {}, so nothing starts; it will see this when it next works the task",
                status.title()
            )));
        }
        if !self.status_names_agent(&issue, agent_id).await? {
            return Ok(Some(format!(
                "{name} has no turn in {} per WORKFLOWS.md; it will see this when it next works the task",
                status.title()
            )));
        }

        // The status owes this agent a turn: forget that it was served, and
        // let the ordinary dispatch decide, so the turn cap and the queue
        // order still apply.
        let mut served: Vec<String> = serde_json::from_str(&issue.turn_served).unwrap_or_default();
        served.retain(|id| *id != agent_id.to_string());
        sqlx::query("update issues set turn_served = ? where id = ?")
            .bind(serde_json::json!(served).to_string())
            .bind(issue_id)
            .execute(&self.db.pool)
            .await?;
        let issue = self.issue(issue_id).await?;
        if let Err(error) = self.dispatch_if_agent_owns(&issue).await {
            return Ok(Some(format!(
                "Notified {name}, but its turn could not start: {error}"
            )));
        }
        let note = match self.active_run(issue_id).await? {
            Some((started,)) if started == agent_id => {
                format!("Notified {name}: starting its turn")
            }
            Some(_) => format!(
                "Notified {name}: another agent goes first per WORKFLOWS.md; {name} will see this on its turn"
            ),
            None => format!("Notified {name}: it will see this on its next turn"),
        };
        Ok(Some(note))
    }

    async fn active_run(&self, issue_id: Uuid) -> Result<Option<(Uuid,)>> {
        Ok(sqlx::query_as(
            "select agent_id from runs where issue_id = ? and status in ('queued', 'running') limit 1",
        )
        .bind(issue_id)
        .fetch_optional(&self.db.pool)
        .await?)
    }

    /// Whether the task's current status lists this agent in `Users:`. The
    /// queue the status was entered with wins; a task that predates the queue
    /// reads the document.
    async fn status_names_agent(&self, issue: &Issue, agent_id: Uuid) -> Result<bool> {
        let mut names: Vec<String> = serde_json::from_str(&issue.turn_queue).unwrap_or_default();
        if names.is_empty() {
            names = self
                .turn_agents(issue.workspace_id, issue.status_enum())
                .await?;
        }
        for name in names {
            if self.agent_for_name(issue.workspace_id, Some(&name)).await? == Some(agent_id) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_run_longest_first_and_skip_emails() {
        let found = mention_candidates("ping @Test Author, and mail me@example.com or @lumi.");
        assert_eq!(
            found,
            vec![
                vec!["TestAuthor".to_string(), "Test".to_string()],
                vec!["lumi".to_string()],
            ]
        );
        assert!(mention_candidates("@ nobody").is_empty());
    }
}
