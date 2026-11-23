//! Domain vocabulary shared by the server, the runner, and the CLI.
//!
//! The state machine in this module is the only authority on issue status: the
//! server calls [`next`] for every transition, so a client cannot skip the gate.

pub mod contract;
pub mod diff;
pub mod junit;
pub mod runspec;
pub mod usage;
pub mod workflow;

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

    /// How the status is written for people.
    pub fn title(self) -> &'static str {
        match self {
            IssueStatus::Backlog => "Backlog",
            IssueStatus::Todo => "To do",
            IssueStatus::InProgress => "In progress",
            IssueStatus::InReview => "In review",
            IssueStatus::Blocked => "Blocked",
            IssueStatus::Done => "Done",
            IssueStatus::Cancelled => "Cancelled",
        }
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

/// The author name a run's commits carry: the app's slug and the agent's own
/// name, so `jj log` answers "who wrote this" without a role anywhere. The team
/// a task passes through is named in the workflow, and those names are the ones
/// the commits should carry.
pub fn commit_name(app_slug: Option<&str>, agent: Option<&str>) -> String {
    let app = app_slug.map(str::trim).filter(|value| !value.is_empty());
    match (app, agent.map(slugify).filter(|value| !value.is_empty())) {
        (Some(app), Some(agent)) => format!("{app}-{agent}"),
        (Some(app), None) => app.to_string(),
        (None, Some(agent)) => agent,
        (None, None) => "hulahoop".to_string(),
    }
}

/// The address GitHub rewrites into a link to the app's bot user.
///
/// The form is `<app id>+<slug>[-<agent>][bot]@users.noreply.github.com`; with
/// any other address the commit shows up unattributed, which reads as if a human
/// wrote it. Without an app and an agent there is nothing honest to build, so a
/// non-routable address stands in.
pub fn commit_email(app_id: Option<&str>, app_slug: Option<&str>, agent: Option<&str>) -> String {
    let id = app_id.map(str::trim).filter(|value| !value.is_empty());
    let app = app_slug.map(str::trim).filter(|value| !value.is_empty());
    let agent = agent.map(slugify).filter(|value| !value.is_empty());
    match (id, app, agent) {
        (Some(id), Some(app), Some(agent)) => {
            format!("{id}+{app}-{agent}[bot]@users.noreply.github.com")
        }
        (Some(id), Some(app), None) => format!("{id}+{app}[bot]@users.noreply.github.com"),
        _ => "hulahoop@hulahoop.invalid".to_string(),
    }
}

/// A display name reduced to something a git identity can hold: lowercase
/// ASCII words joined by single dashes. Anything a name cannot carry is dropped,
/// so `Ada Lovelace (plans)` still yields `ada-lovelace-plans`.
pub fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut gap = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            if gap && !out.is_empty() {
                out.push('-');
            }
            gap = false;
            out.extend(ch.to_lowercase());
        } else {
            gap = true;
        }
    }
    out.trim_matches('-').to_string()
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

#[cfg(test)]
mod identity_tests {
    use super::*;

    #[test]
    fn a_name_becomes_a_slug_a_git_identity_can_hold() {
        assert_eq!(slugify("Architect"), "architect");
        assert_eq!(slugify("Test Author"), "test-author");
        assert_eq!(slugify("Ada Lovelace (plans)"), "ada-lovelace-plans");
        assert_eq!(slugify("  Review   Bot  "), "review-bot");
    }

    #[test]
    fn the_commit_is_named_after_the_agent_not_a_role() {
        assert_eq!(
            commit_name(Some("hulahoop-dev"), Some("Test Author")),
            "hulahoop-dev-test-author"
        );
        assert_eq!(
            commit_email(Some("5834164"), Some("hulahoop-dev"), Some("Architect")),
            "5834164+hulahoop-dev-architect[bot]@users.noreply.github.com"
        );
    }

    #[test]
    fn without_an_agent_name_the_app_stands_alone() {
        assert_eq!(commit_name(Some("hulahoop-dev"), None), "hulahoop-dev");
        assert_eq!(commit_name(None, Some("Architect")), "architect");
        assert_eq!(commit_name(None, None), "hulahoop");
        assert_eq!(
            commit_email(None, None, Some("Architect")),
            "hulahoop@hulahoop.invalid"
        );
        assert_eq!(
            commit_email(Some("5834164"), Some("hulahoop-dev"), None),
            "5834164+hulahoop-dev[bot]@users.noreply.github.com"
        );
    }
}
