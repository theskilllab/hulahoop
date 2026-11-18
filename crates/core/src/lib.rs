//! Domain vocabulary shared by the server, the runner, and the CLI.
//!
//! The state machine in this module is the only authority on issue status: the
//! server calls [`next`] for every transition, so a client cannot skip the gate.

pub mod contract;
pub mod diff;
pub mod junit;
pub mod runspec;

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
    Ready => "ready",
    TestsInProgress => "tests_in_progress",
    TestsReady => "tests_ready",
    InProgress => "in_progress",
    InReview => "in_review",
    Merging => "merging",
    Blocked => "blocked",
    Done => "done",
    Cancelled => "cancelled",
});

impl IssueStatus {
    /// States an issue can be in while an agent run may be live.
    pub fn is_active(self) -> bool {
        matches!(
            self,
            IssueStatus::TestsInProgress
                | IssueStatus::TestsReady
                | IssueStatus::InProgress
                | IssueStatus::InReview
                | IssueStatus::Merging
        )
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, IssueStatus::Done | IssueStatus::Cancelled)
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
    ApproveCriteria,
    StartTestAuthor,
    /** An authoring run finished and wrote its candidate bookmark. */
    AuthoringCompleted,
    FreezeSuite,
    FreezeRejected {
        reason: String,
    },
    AuthoringFailed,
    StartImplementer {
        parent_run_id: Option<uuid::Uuid>,
    },
    ImplementerCompleted,
    ImplementerFailed,
    StartReviewer,
    ReviewerApproved,
    ReviewerChangesRequested,
    RequestChanges,
    ApproveIssue,
    LandingRed,
    RecordMerge,
    Blocked,
    Resume,
    CancelRun,
    Abandon,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TransitionError {
    #[error("an issue in `{from}` cannot handle `{event}`")]
    NotAllowed { from: IssueStatus, event: String },
    #[error("`{from}` is terminal")]
    Terminal { from: IssueStatus },
}

/// The issue state machine, transcribed from the table in the product spec.
///
/// `startRun` is the only way into a working state, which is why the same event
/// carries both the "no active run" guard and the role being started.
pub fn next(from: IssueStatus, event: &IssueEvent) -> Result<IssueStatus, TransitionError> {
    use IssueEvent as E;
    use IssueStatus as S;

    if from.is_terminal() {
        return Err(TransitionError::Terminal { from });
    }

    let to = match (from, event) {
        (S::Backlog, E::ApproveCriteria) => S::Ready,
        (S::Ready, E::StartTestAuthor) => S::TestsInProgress,
        (S::TestsInProgress, E::StartTestAuthor) => S::TestsInProgress,
        (S::TestsInProgress, E::AuthoringCompleted) => S::TestsInProgress,
        (S::TestsInProgress, E::FreezeSuite) => S::TestsReady,
        (S::TestsInProgress, E::FreezeRejected { .. }) => S::TestsInProgress,
        (S::TestsInProgress, E::AuthoringFailed) => S::Ready,
        (S::TestsReady, E::StartImplementer { .. }) => S::InProgress,
        (S::TestsReady, E::StartTestAuthor) => S::TestsInProgress,
        (S::InProgress, E::StartImplementer { .. }) => S::InProgress,
        (S::InProgress, E::ImplementerCompleted) => S::InReview,
        (S::InProgress, E::ImplementerFailed) => S::InProgress,
        (S::InProgress, E::StartTestAuthor) => S::TestsInProgress,
        (S::InReview, E::StartReviewer) => S::InReview,
        (S::InReview, E::ReviewerApproved) => S::InReview,
        (S::InReview, E::ReviewerChangesRequested) => S::InProgress,
        (S::InReview, E::RequestChanges) => S::InProgress,
        (S::InReview, E::ApproveIssue) => S::Merging,
        (S::Merging, E::LandingRed) => S::InProgress,
        (S::Merging, E::RecordMerge) => S::Done,
        (_, E::Blocked) if from.is_active() => S::Blocked,
        (S::Blocked, E::Resume) => S::InProgress,
        (_, E::Abandon) => S::Cancelled,
        (_, E::CancelRun) => from,
        (from, event) if from.is_active() => {
            return Err(TransitionError::NotAllowed {
                from,
                event: format!("{event:?}"),
            })
        }
        (from, event) => {
            return Err(TransitionError::NotAllowed {
                from,
                event: format!("{event:?}"),
            })
        }
    };

    Ok(to)
}

/// `resumeIssue` returns to the status the issue left, so the caller has to say
/// what that was. The server passes `issues.status_before_blocked`.
pub fn resume_to(status_before_blocked: Option<IssueStatus>) -> Option<IssueStatus> {
    status_before_blocked.filter(|s| s.is_active())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_cannot_be_skipped() {
        // Approving straight from backlog is impossible: criteria come first.
        assert!(next(IssueStatus::Backlog, &IssueEvent::ApproveIssue).is_err());
        // Implementation cannot start before the suite is frozen.
        assert!(next(
            IssueStatus::TestsInProgress,
            &IssueEvent::StartImplementer {
                parent_run_id: None
            }
        )
        .is_err());
    }

    #[test]
    fn terminal_states_are_terminal() {
        for status in [IssueStatus::Done, IssueStatus::Cancelled] {
            assert_eq!(
                next(status, &IssueEvent::Abandon),
                Err(TransitionError::Terminal { from: status })
            );
        }
    }

    #[test]
    fn rework_paths_converge_on_in_progress() {
        assert_eq!(
            next(IssueStatus::InReview, &IssueEvent::ReviewerChangesRequested).unwrap(),
            IssueStatus::InProgress
        );
        assert_eq!(
            next(IssueStatus::Merging, &IssueEvent::LandingRed).unwrap(),
            IssueStatus::InProgress
        );
        assert_eq!(
            next(IssueStatus::InReview, &IssueEvent::RequestChanges).unwrap(),
            IssueStatus::InProgress
        );
    }

    #[test]
    fn cancel_keeps_the_status_and_abandon_ends_it() {
        assert_eq!(
            next(IssueStatus::InProgress, &IssueEvent::CancelRun).unwrap(),
            IssueStatus::InProgress
        );
        assert_eq!(
            next(IssueStatus::InProgress, &IssueEvent::Abandon).unwrap(),
            IssueStatus::Cancelled
        );
    }

    #[test]
    fn every_active_state_can_block_and_resume() {
        for status in [
            IssueStatus::TestsInProgress,
            IssueStatus::TestsReady,
            IssueStatus::InProgress,
            IssueStatus::InReview,
            IssueStatus::Merging,
        ] {
            assert_eq!(
                next(status, &IssueEvent::Blocked).unwrap(),
                IssueStatus::Blocked
            );
            assert_eq!(
                next(IssueStatus::Blocked, &IssueEvent::Resume).unwrap(),
                IssueStatus::InProgress
            );
            assert_eq!(resume_to(Some(status)), Some(status));
        }
    }

    #[test]
    fn done_only_through_record_merge() {
        let mut status = IssueStatus::Backlog;
        for event in [
            IssueEvent::ApproveCriteria,
            IssueEvent::StartTestAuthor,
            IssueEvent::AuthoringCompleted,
            IssueEvent::FreezeSuite,
            IssueEvent::StartImplementer {
                parent_run_id: None,
            },
            IssueEvent::ImplementerCompleted,
            IssueEvent::StartReviewer,
            IssueEvent::ReviewerApproved,
            IssueEvent::ApproveIssue,
            IssueEvent::RecordMerge,
        ] {
            status = next(status, &event).unwrap();
        }
        assert_eq!(status, IssueStatus::Done);
    }

    #[test]
    fn status_strings_round_trip_for_the_database() {
        for status in [
            IssueStatus::Backlog,
            IssueStatus::Ready,
            IssueStatus::TestsInProgress,
            IssueStatus::TestsReady,
            IssueStatus::InProgress,
            IssueStatus::InReview,
            IssueStatus::Merging,
            IssueStatus::Blocked,
            IssueStatus::Done,
            IssueStatus::Cancelled,
        ] {
            assert_eq!(IssueStatus::parse(status.as_str()), Some(status));
        }
    }
}
