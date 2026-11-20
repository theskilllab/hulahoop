//! Domain vocabulary shared by the server, the runner, and the CLI.
//!
//! The state machine in this module is the only authority on issue status: the
//! server calls [`next`] for every transition, so a client cannot skip the gate.

pub mod contract;
pub mod diff;
pub mod junit;
pub mod runspec;
pub mod usage;

use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

/// Recorded in `repos.contract_version` and shown in the migration message.
pub const CONTRACT_VERSION: &str = "26.11";

/// Default caps. Every one of these is a constant in code, never a setting.
pub const HEARTBEAT_INTERVAL_SECS: i64 = 5;
pub const HEARTBEAT_DEAD_SECS: i64 = 30;
pub const AGENT_WALL_CLOCK_MINUTES: i64 = 30;
pub const SETUP_WALL_CLOCK_MINUTES: i64 = 20;
pub const REPLAY_WALL_CLOCK_MINUTES: i64 = 15;
pub const RUNS_PER_ISSUE: i64 = 12;
pub const ARTIFACT_LIMIT_BYTES: u64 = 500 * 1024 * 1024;
pub const DIFF_LINE_LIMIT: i64 = 2000;
pub const LOG_ROTATE_BYTES: u64 = 50 * 1024 * 1024;
pub const GRAPHQL_TIMEOUT_SECS: i64 = 30;
pub const SQLITE_BUSY_TIMEOUT_MS: i64 = 5000;

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $s:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $s),+
                }
            }

            pub fn parse(value: &str) -> Option<Self> {
                match value {
                    $($s => Some($name::$variant)),+,
                    _ => None,
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let raw = String::deserialize(d)?;
                Self::parse(&raw).ok_or_else(|| {
                    serde::de::Error::custom(format!("unknown {}: {raw}", stringify!($name)))
                })
            }
        }
    };
}

string_enum!(IssueStatus {
    Backlog => "backlog",
    Todo => "todo",
    InProgress => "in_progress",
    InReview => "in_review",
    Blocked => "blocked",
    Done => "done",
    Cancelled => "cancelled",
});

impl IssueStatus {
    /// States where an agent run may be live or queued behind the issue.
    pub fn is_active(self) -> bool {
        matches!(
            self,
            IssueStatus::Todo | IssueStatus::InProgress | IssueStatus::InReview
        )
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, IssueStatus::Done | IssueStatus::Cancelled)
    }

    /// The six columns a board shows, in the order work moves through them.
    pub fn board() -> [IssueStatus; 6] {
        [
            IssueStatus::Backlog,
            IssueStatus::Todo,
            IssueStatus::InProgress,
            IssueStatus::Blocked,
            IssueStatus::InReview,
            IssueStatus::Done,
        ]
    }
}

string_enum!(RunStatus {
    Queued => "queued",
    Running => "running",
    Succeeded => "succeeded",
    Failed => "failed",
    Cancelled => "cancelled",
    TimedOut => "timed_out",
});

impl RunStatus {
    pub fn is_live(self) -> bool {
        matches!(self, RunStatus::Queued | RunStatus::Running)
    }
}

string_enum!(ReplayStatus {
    Queued => "queued",
    Running => "running",
    Succeeded => "succeeded",
    Failed => "failed",
    TimedOut => "timed_out",
});

string_enum!(AgentRole {
    TestAuthor => "test_author",
    Implementer => "implementer",
    Reviewer => "reviewer",
});

impl AgentRole {
    pub const ALL: [AgentRole; 3] = [
        AgentRole::TestAuthor,
        AgentRole::Implementer,
        AgentRole::Reviewer,
    ];

    /// The author name a commit made by this role carries. One name per role is
    /// what makes "who wrote this" answerable from `jj log` alone, and the
    /// `.invalid` domain keeps the address obviously non-routable.
    pub fn commit_name(self) -> &'static str {
        match self {
            AgentRole::TestAuthor => "hulahoop-test-author",
            AgentRole::Implementer => "hulahoop-implementer",
            AgentRole::Reviewer => "hulahoop-reviewer",
        }
    }

    pub fn commit_email(self) -> &'static str {
        match self {
            AgentRole::TestAuthor => "hulahoop-test-author@hulahoop.invalid",
            AgentRole::Implementer => "hulahoop-implementer@hulahoop.invalid",
            AgentRole::Reviewer => "hulahoop-reviewer@hulahoop.invalid",
        }
    }

    /// The role's half of a name, so one app can still write as three roles.
    pub fn name_suffix(self) -> &'static str {
        match self {
            AgentRole::TestAuthor => "test-author",
            AgentRole::Implementer => "implementer",
            AgentRole::Reviewer => "reviewer",
        }
    }

    /// The author name, prefixed with the app's slug when there is one.
    ///
    /// Nothing here is a constant the user has to edit: rename the app, paste the
    /// new key, and the commits name themselves after it.
    pub fn app_commit_name(self, slug: Option<&str>) -> String {
        match slug.map(str::trim).filter(|slug| !slug.is_empty()) {
            Some(slug) => format!("{slug}-{}", self.name_suffix()),
            None => self.commit_name().to_string(),
        }
    }

    /// The address GitHub rewrites into a link to the app's bot user.
    ///
    /// The form is `<app id>+<slug>[bot]@users.noreply.github.com`; with any other
    /// address the commit shows up unattributed, which reads as if a human wrote
    /// it. Without both halves there is nothing honest to build, so the role's own
    /// non-routable address stands in and the panel says so.
    pub fn app_commit_email(self, app_id: Option<&str>, slug: Option<&str>) -> String {
        match (app_id, slug) {
            (Some(app_id), Some(slug)) if !app_id.trim().is_empty() && !slug.trim().is_empty() => {
                format!(
                    "{}+{}[bot]@users.noreply.github.com",
                    app_id.trim(),
                    slug.trim()
                )
            }
            _ => self.commit_email().to_string(),
        }
    }
}

string_enum!(Adapter {
    Pi => "pi",
    ClaudeCode => "claude_code",
    Fake => "fake",
});

string_enum!(ReplayKind {
    Base => "base",
    Head => "head",
    Landing => "landing",
});

string_enum!(RunFlag {
    TouchedContract => "touched_contract",
    LargeDiff => "large_diff",
    ArtifactLimit => "artifact_limit",
});

string_enum!(AuthorType {
    Human => "human",
    Agent => "agent",
    Server => "server",
});

string_enum!(Vcs {
    Jj => "jj",
    Git => "git",
});

string_enum!(CheckoutState {
    Creating => "creating",
    Ready => "ready",
    Cleaning => "cleaning",
    Removed => "removed",
});

string_enum!(OwnerKind {
    Run => "run",
    Replay => "replay",
    Preflight => "preflight",
});

/// Everything that can move an issue, as a value. Keeping events as data lets
/// the table below stay the single place a transition is decided.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum IssueEvent {
    /** Backlog is yours; To do is where the team picks work up. */
    Plan,
    Unplan,
    /** An agent takes the task, or starts another attempt at it. */
    Start,
    /** The agent finished and handed the work over for review. */
    Submit,
    RequestChanges,
    /** The pull request merged, which is what closes a task. */
    Merge,
    Blocked,
    Resume,
    Reopen,
    CancelRun,
    Abandon,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TransitionError {
    #[error("a task in `{from}` cannot handle `{event}`")]
    NotAllowed { from: IssueStatus, event: String },
    #[error("`{from}` is terminal, so it has to be reopened first")]
    Terminal { from: IssueStatus },
}

/// The task state machine: six statuses, six moves.
///
/// Backlog and To do are the human's line. To do is the queue an agent reads, so
/// moving a task there is what asks for work; from To do the agent ever only goes
/// forward, to review, and the merge closes it.
pub fn next(from: IssueStatus, event: &IssueEvent) -> Result<IssueStatus, TransitionError> {
    use IssueEvent as E;
    use IssueStatus as S;

    let to = match (from, event) {
        (S::Backlog, E::Plan) => S::Todo,
        (S::Todo, E::Unplan) => S::Backlog,
        (S::Todo, E::Start) => S::InProgress,
        (S::InProgress, E::Start) => S::InProgress,
        (S::InProgress, E::Submit) => S::InReview,
        (S::InReview, E::RequestChanges) => S::InProgress,
        (S::InReview, E::Merge) => S::Done,
        (S::Blocked, E::Resume) => S::InProgress,
        (_, E::Reopen) if from.is_terminal() => S::Todo,
        (_, E::Blocked) if from.is_active() || from == S::Backlog => S::Blocked,
        (_, E::Abandon) if !from.is_terminal() => S::Cancelled,
        (_, E::CancelRun) if from.is_active() => from,
        (_, _) if from.is_terminal() => return Err(TransitionError::Terminal { from }),
        (from, event) => {
            return Err(TransitionError::NotAllowed {
                from,
                event: format!("{event:?}"),
            })
        }
    };

    Ok(to)
}

/// `resumeIssue` returns to the status the task left, so the caller has to say
/// what that was. The server passes `issues.status_before_blocked`.
pub fn resume_to(status_before_blocked: Option<IssueStatus>) -> Option<IssueStatus> {
    status_before_blocked.filter(|s| s.is_active() || *s == IssueStatus::Backlog)
}

#[cfg(test)]
mod tests {
    use super::*;
    use IssueEvent as E;
    use IssueStatus as S;

    #[test]
    fn the_team_only_works_forward() {
        assert_eq!(next(S::Todo, &E::Start).unwrap(), S::InProgress);
        assert_eq!(next(S::InProgress, &E::Submit).unwrap(), S::InReview);
        assert_eq!(next(S::InReview, &E::Merge).unwrap(), S::Done);
        // No shortcut from the queue straight to review.
        assert!(next(S::Todo, &E::Submit).is_err());
        assert!(next(S::Backlog, &E::Start).is_err());
    }

    #[test]
    fn backlog_is_the_human_line() {
        assert_eq!(next(S::Backlog, &E::Plan).unwrap(), S::Todo);
        assert_eq!(next(S::Todo, &E::Unplan).unwrap(), S::Backlog);
    }

    #[test]
    fn review_can_send_work_back() {
        assert_eq!(
            next(S::InReview, &E::RequestChanges).unwrap(),
            S::InProgress
        );
    }

    #[test]
    fn blocked_and_resumed_from_every_working_state() {
        for status in [S::Todo, S::InProgress, S::InReview] {
            assert_eq!(next(status, &E::Blocked).unwrap(), S::Blocked);
            assert_eq!(resume_to(Some(status)), Some(status));
        }
        assert_eq!(next(S::Blocked, &E::Resume).unwrap(), S::InProgress);
    }

    #[test]
    fn terminal_states_need_reopening() {
        for status in [S::Done, S::Cancelled] {
            assert_eq!(
                next(status, &E::Abandon),
                Err(TransitionError::Terminal { from: status })
            );
            assert_eq!(next(status, &E::Reopen).unwrap(), S::Todo);
        }
    }

    #[test]
    fn abandoning_ends_a_live_task() {
        assert_eq!(next(S::InProgress, &E::Abandon).unwrap(), S::Cancelled);
        assert_eq!(next(S::InProgress, &E::CancelRun).unwrap(), S::InProgress);
    }

    #[test]
    fn the_board_shows_six_columns() {
        assert_eq!(IssueStatus::board().len(), 6);
        assert!(IssueStatus::board().contains(&S::Blocked));
        assert!(!IssueStatus::board().contains(&S::Cancelled));
    }
}
