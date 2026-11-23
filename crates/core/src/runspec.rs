//! `input.json`: the only channel task data arrives through.
//!
//! The server writes one file per run and passes its path in `HULAHOOP_INPUT`.
//! Nothing about the issue travels in argv or in the environment, because both
//! leak into `ps` and into the agent's own tool output.

use crate::DIFF_LINE_LIMIT;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Inputs are bounded: an issue body is not a place to paste a repository.
pub const MAX_TEXT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunSpec {
    pub run_id: Uuid,
    pub issue_id: Uuid,
    pub attempt: i64,
    pub issue: IssueInput,
    pub criteria: Vec<CriterionInput>,
    pub repo: RepoInput,
    pub checkout: CheckoutInput,
    pub parent: Option<ParentInput>,
    pub blocked_answer: Option<BlockedAnswer>,
    pub replays: Vec<ReplayInput>,
    pub limits: Limits,
    /// The skills this agent carries, so the runner can write them into the
    /// checkout where the harness looks for them.
    #[serde(default)]
    pub skills: Vec<SkillInput>,
}

/// One skill, ready to be written as `SKILL.md`. The body is the whole file as
/// the workspace keeps it; the runner adds nothing to it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillInput {
    pub name: String,
    pub description: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssueInput {
    pub title: String,
    pub body: String,
    pub priority: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CriterionInput {
    pub position: i64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoInput {
    pub name: String,
    /// Absent when the repository was attached by its remote and never checked out
    /// on this machine; a runner then clones instead of borrowing.
    pub path: Option<PathBuf>,
    pub base_bookmark: String,
    pub tests_bookmark: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckoutInput {
    pub path: PathBuf,
    pub base_bookmark: String,
    /// Files the suite is frozen over. The agent must not touch them.
    pub frozen_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ParentInput {
    pub head_bookmark: String,
    pub verdict: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlockedAnswer {
    pub question: String,
    pub answer: String,
}

/// What the reviewer is told about the replays, so it can quote them instead of
/// re-running the suite itself.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplayInput {
    pub kind: String,
    pub status: String,
    pub report_path: Option<PathBuf>,
    pub failed: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Limits {
    pub wall_clock_minutes: i64,
    pub diff_line_limit: i64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            wall_clock_minutes: crate::AGENT_WALL_CLOCK_MINUTES,
            diff_line_limit: DIFF_LINE_LIMIT,
        }
    }
}

impl RunSpec {
    /// Trim every free-text field to the cap, so a 200 KB body cannot become a
    /// 200 KB prompt. Truncation is recorded in the text itself: an agent that
    /// sees an ellipsis knows the input was cut.
    pub fn truncate_text(mut self) -> Self {
        self.issue.title = truncate(&self.issue.title);
        self.issue.body = truncate(&self.issue.body);
        for criterion in &mut self.criteria {
            criterion.text = truncate(&criterion.text);
        }
        if let Some(parent) = &mut self.parent {
            parent.body = truncate(&parent.body);
        }
        if let Some(answer) = &mut self.blocked_answer {
            answer.question = truncate(&answer.question);
            answer.answer = truncate(&answer.answer);
        }
        self
    }

    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(self)?;
        std::fs::write(path, json)
    }

    pub fn read(path: &Path) -> std::io::Result<Self> {
        let bytes = std::fs::read(path)?;
        serde_json::from_slice(&bytes)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }
}

fn truncate(text: &str) -> String {
    if text.len() <= MAX_TEXT_BYTES {
        return text.to_string();
    }
    let mut end = MAX_TEXT_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n\n[truncated at {MAX_TEXT_BYTES} bytes]", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(body: &str) -> RunSpec {
        RunSpec {
            run_id: Uuid::nil(),
            issue_id: Uuid::nil(),
            attempt: 1,
            issue: IssueInput {
                title: "Add multiplication".into(),
                body: body.into(),
                priority: "high".into(),
            },
            criteria: vec![CriterionInput {
                position: 1,
                text: "mul(2,3) returns 6".into(),
            }],
            repo: RepoInput {
                name: "fixture".into(),
                path: Some("/tmp/fixture".into()),
                base_bookmark: "main".into(),
                tests_bookmark: Some("tests/abc".into()),
            },
            checkout: CheckoutInput {
                path: "/tmp/checkout".into(),
                base_bookmark: "tests/abc".into(),
                frozen_paths: vec!["tests/test_calc.py".into()],
            },
            parent: None,
            blocked_answer: None,
            replays: vec![],
            limits: Limits::default(),
            skills: vec![],
        }
    }

    #[test]
    fn round_trips_through_a_file() {
        let dir = std::env::temp_dir().join(format!("hulahoop-spec-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("input.json");
        let original = spec("body");
        original.write(&path).unwrap();
        assert_eq!(RunSpec::read(&path).unwrap(), original);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn oversized_bodies_are_truncated_and_marked() {
        let big = "x".repeat(MAX_TEXT_BYTES * 2);
        let trimmed = spec(&big).truncate_text();
        assert!(trimmed.issue.body.len() <= MAX_TEXT_BYTES + 64);
        assert!(trimmed.issue.body.contains("truncated at"));
    }
}
