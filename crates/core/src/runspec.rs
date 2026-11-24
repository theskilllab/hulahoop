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

/// A comment is a note, not a document. Anything longer is cut so twenty of
/// them still fit comfortably in a prompt.
pub const MAX_COMMENT_BYTES: usize = 4 * 1024;

/// How many recent comments a run is handed. The oldest drop off first; the
/// agent can always ask for the full thread through the task.
pub const MAX_RUN_COMMENTS: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunSpec {
    pub run_id: Uuid,
    pub issue_id: Uuid,
    pub attempt: i64,
    pub issue: IssueInput,
    /// `None` for a task with no repository: the run works in an empty scratch
    /// directory (`checkout.path`), with nothing to commit and no pull request.
    #[serde(default)]
    pub repo: Option<RepoInput>,
    pub checkout: CheckoutInput,
    pub parent: Option<ParentInput>,
    pub blocked_answer: Option<BlockedAnswer>,
    pub replays: Vec<ReplayInput>,
    pub limits: Limits,
    /// The skills this agent carries, so the runner can write them into the
    /// checkout where the harness looks for them.
    #[serde(default)]
    pub skills: Vec<SkillInput>,
    /// Files attached to the task, as absolute paths the agent can read.
    #[serde(default)]
    pub files: Vec<FileInput>,
    /// What people (and Lumi) said on the task since the previous run started,
    /// oldest first, so a nudge or a clarification reaches the next turn.
    #[serde(default)]
    pub comments: Vec<CommentInput>,
}

/// One file attached to the task. `path` points into the server's artifact
/// store, so it is readable from a runner on the same machine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileInput {
    pub id: Uuid,
    pub title: String,
    pub filename: String,
    pub path: PathBuf,
}

/// One comment on the task, already reduced to who said it and what they said.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommentInput {
    pub author: String,
    pub body: String,
    pub created_at: String,
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
pub struct RepoInput {
    pub name: String,
    /// Absent when the repository was attached by its remote and never checked out
    /// on this machine; a runner then clones instead of borrowing.
    pub path: Option<PathBuf>,
    pub base_bookmark: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckoutInput {
    pub path: PathBuf,
    pub base_bookmark: String,
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
        for comment in &mut self.comments {
            comment.body = truncate_to(&comment.body, MAX_COMMENT_BYTES);
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
    truncate_to(text, MAX_TEXT_BYTES)
}

fn truncate_to(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n\n[truncated at {limit} bytes]", &text[..end])
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
            repo: Some(RepoInput {
                name: "fixture".into(),
                path: Some("/tmp/fixture".into()),
                base_bookmark: "main".into(),
            }),
            checkout: CheckoutInput {
                path: "/tmp/checkout".into(),
                base_bookmark: "main".into(),
            },
            parent: None,
            blocked_answer: None,
            replays: vec![],
            limits: Limits::default(),
            skills: vec![],
            files: vec![FileInput {
                id: Uuid::nil(),
                title: "Spec".into(),
                filename: "spec.md".into(),
                path: "/tmp/artifacts/spec.md".into(),
            }],
            comments: vec![CommentInput {
                author: "you".into(),
                body: "Please keep the API stable.".into(),
                created_at: "2026-01-01T00:00:00Z".into(),
            }],
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
    fn a_repo_less_spec_round_trips_with_no_repository() {
        let dir = std::env::temp_dir().join(format!("hulahoop-spec-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("input.json");
        let mut original = spec("body");
        original.repo = None;
        original.write(&path).unwrap();
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(raw["repo"].is_null());
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

    #[test]
    fn long_comments_are_cut_shorter_than_bodies() {
        let mut long = spec("body");
        long.comments[0].body = "y".repeat(MAX_COMMENT_BYTES * 3);
        let trimmed = long.truncate_text();
        assert!(trimmed.comments[0].body.len() <= MAX_COMMENT_BYTES + 64);
        assert!(trimmed.comments[0].body.contains("truncated at"));
    }
}
