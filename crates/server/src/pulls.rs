//! Pull requests: the person's GitHub token, the cached list, review and merge.
//!
//! The list is a cache. The desktop polls every few seconds, and asking GitHub
//! that often would spend the rate limit on nothing, so the server syncs every
//! linked repository in the background and the list reads SQLite. One pull
//! request opened for review is fetched live, because a person deciding whether
//! to merge should see what GitHub sees now.
//!
//! Reviews and merges are made as the person, with their own token, and always
//! on GitHub first: only when GitHub has accepted a review or a merge does the
//! linked task move. A refusal leaves the task exactly where it was and hands
//! GitHub's reason to the person.
//!
//! The working diff of a live run is here too, because it is what "Watch live"
//! on a pull request shows: the runner reports it while the agent works, and a
//! subscription streams it to whoever is watching.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{anyhow, bail, Context as _, Result};
use async_graphql::{ComplexObject, Context, Enum, SimpleObject, ID};
use chrono::{DateTime, Utc};
use hulahoop_core::IssueStatus;
use serde_json::Value;
use uuid::Uuid;

use crate::engine::Engine;
use crate::github::{split_remote, Github};
use crate::model::{Issue, Run};

/// How often the background sync runs.
pub const SYNC_INTERVAL: Duration = Duration::from_secs(120);
/// Closed and merged pull requests stay in the list this long after they close.
const KEEP_CLOSED_DAYS: i64 = 30;
/// CI that has settled is asked again after this long; CI still running is
/// asked on every sync.
const CI_RECHECK_MINUTES: i64 = 10;
/// The `provider` of a person's GitHub token in `personal_tokens`.
const PROVIDER: &str = "github";
/// How many repositories a new token is probed in at once.
const TOKEN_PROBES_AT_ONCE: usize = 4;
/// The `meta` key holding when the last sync finished.
const SYNCED_AT_KEY: &str = "pull_requests_synced_at";

/// Why every review and merge button is off when there is no token.
pub const NO_TOKEN_REASON: &str = "Add your GitHub token in Settings → Personal";
/// GitHub refuses a review from the pull request's own author.
pub const OWN_PULL_REASON: &str = "GitHub does not let you review your own pull request";

// ------------------------------------------------------------------- types

/// The person's GitHub account, as their token showed it when it was checked.
#[derive(Debug, Clone, PartialEq, SimpleObject)]
pub struct GithubAccount {
    pub login: String,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
    /// Enough of the token to recognise it, never enough to use it.
    pub token_hint: String,
    pub validated_at: DateTime<Utc>,
    /// What the token cannot do in the linked repositories; empty when it can
    /// do everything the screen needs.
    pub problems: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum PullRequestFilter {
    Open,
    NeedsMyReview,
    Mine,
    ByAgents,
    FailingCi,
    Merged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum ReviewState {
    Draft,
    ReviewRequested,
    Approved,
    ChangesRequested,
    Merged,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum CiState {
    Passing,
    Failing,
    Running,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum ReviewEvent {
    Approve,
    RequestChanges,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum DiffSide {
    Left,
    Right,
}

impl ReviewState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "DRAFT",
            Self::ReviewRequested => "REVIEW_REQUESTED",
            Self::Approved => "APPROVED",
            Self::ChangesRequested => "CHANGES_REQUESTED",
            Self::Merged => "MERGED",
            Self::Closed => "CLOSED",
        }
    }

    fn parse(text: &str) -> Self {
        match text {
            "DRAFT" => Self::Draft,
            "APPROVED" => Self::Approved,
            "CHANGES_REQUESTED" => Self::ChangesRequested,
            "MERGED" => Self::Merged,
            "CLOSED" => Self::Closed,
            _ => Self::ReviewRequested,
        }
    }
}

impl CiState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Passing => "PASSING",
            Self::Failing => "FAILING",
            Self::Running => "RUNNING",
            Self::None => "NONE",
        }
    }

    fn parse(text: &str) -> Self {
        match text {
            "PASSING" => Self::Passing,
            "FAILING" => Self::Failing,
            "RUNNING" => Self::Running,
            _ => Self::None,
        }
    }
}

impl MergeMethod {
    /// The word GitHub's merge endpoint takes.
    fn as_github(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Squash => "squash",
            Self::Rebase => "rebase",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "merge" => Some(Self::Merge),
            "squash" => Some(Self::Squash),
            "rebase" => Some(Self::Rebase),
            _ => None,
        }
    }
}

impl ReviewEvent {
    fn as_github(self) -> &'static str {
        match self {
            Self::Approve => "APPROVE",
            Self::RequestChanges => "REQUEST_CHANGES",
        }
    }
}

impl DiffSide {
    fn as_github(self) -> &'static str {
        match self {
            Self::Left => "LEFT",
            Self::Right => "RIGHT",
        }
    }

    fn parse(text: &str) -> Self {
        if text.eq_ignore_ascii_case("LEFT") {
            Self::Left
        } else {
            Self::Right
        }
    }
}

/// One pull request in the list, read from the cache.
#[derive(Debug, Clone, SimpleObject)]
#[graphql(complex)]
pub struct PullRequest {
    /// `<repoId>#<number>`.
    pub id: ID,
    pub repo_id: Uuid,
    /// `owner/repo`.
    pub repo_name: String,
    pub number: i64,
    pub title: String,
    pub url: String,
    /// `open` or `closed`.
    pub state: String,
    pub draft: bool,
    pub merged: bool,
    pub author_login: String,
    pub author_avatar_url: Option<String>,
    /// Authored by the GitHub App, or linked to a task an agent works.
    pub by_agent: bool,
    pub head_ref: String,
    pub head_sha: String,
    pub base_ref: String,
    pub additions: i64,
    pub deletions: i64,
    pub changed_files: i64,
    pub review_state: ReviewState,
    pub review_requested_from_me: bool,
    pub ci_state: CiState,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub merged_at: Option<DateTime<Utc>>,
    #[graphql(skip)]
    pub issue_id: Option<Uuid>,
}

#[ComplexObject]
impl PullRequest {
    /// The task this pull request was opened for, if any.
    async fn issue(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<Issue>> {
        match self.issue_id {
            Some(id) => Ok(Some(crate::gql::engine(ctx).issue(id).await?)),
            None => Ok(None),
        }
    }

    /// The linked task's run that is queued or running now, if any: the one
    /// "Watch live" follows.
    async fn live_run(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<Run>> {
        let Some(issue_id) = self.issue_id else {
            return Ok(None);
        };
        Ok(sqlx::query_as::<_, Run>(
            "select * from runs where issue_id = ? and status in ('queued', 'running')
              order by created_at desc limit 1",
        )
        .bind(issue_id)
        .fetch_optional(&crate::gql::engine(ctx).db.pool)
        .await?)
    }
}

#[derive(Debug, Clone, PartialEq, SimpleObject)]
pub struct PullRequestFile {
    pub path: String,
    pub previous_path: Option<String>,
    /// `added`, `removed`, `modified` or `renamed`.
    pub status: String,
    pub additions: i64,
    pub deletions: i64,
    /// Unified hunks; GitHub sends none for binary or very large files.
    pub patch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, SimpleObject)]
pub struct PullRequestComment {
    pub id: ID,
    pub path: String,
    pub line: i64,
    pub side: DiffSide,
    pub body: String,
    /// `you` for a comment kept in Hulahoop while there is no token.
    pub author_login: String,
    pub created_at: DateTime<Utc>,
    /// Saved in Hulahoop and not on GitHub yet.
    pub pending: bool,
    /// Pending, with no token to send it with.
    pub only_in_hulahoop: bool,
}

#[derive(Debug, Clone, PartialEq, SimpleObject)]
pub struct PullRequestReview {
    pub author_login: String,
    pub state: String,
    pub body: Option<String>,
    pub submitted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, SimpleObject)]
pub struct CheckRun {
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub url: Option<String>,
}

/// One pull request opened for review, fetched live from GitHub.
#[derive(Debug, Clone, SimpleObject)]
pub struct PullRequestDetail {
    pub pull_request: PullRequest,
    pub body: Option<String>,
    pub files: Vec<PullRequestFile>,
    pub comments: Vec<PullRequestComment>,
    pub reviews: Vec<PullRequestReview>,
    pub checks: Vec<CheckRun>,
    /// The methods the repository accepts.
    pub merge_methods: Vec<MergeMethod>,
    /// The method used last in this repository, to preselect.
    pub last_merge_method: Option<MergeMethod>,
    pub can_review: bool,
    pub review_blocked_reason: Option<String>,
    pub can_merge: bool,
    pub merge_blocked_reason: Option<String>,
}

/// A run's working diff, as one message of the `runDiff` subscription.
#[derive(Debug, Clone, PartialEq, SimpleObject)]
pub struct RunDiff {
    pub run_id: Uuid,
    pub patch: String,
    /// `sha256` of the patch, so a watcher can tell a change from a repeat.
    pub sha: String,
    /// False once the run has finished: this is its final diff and nothing
    /// more follows.
    pub live: bool,
    pub updated_at: DateTime<Utc>,
}

/// What wakes the background sync before its interval is up, and which
/// repositories it should look at when it does.
#[derive(Default)]
pub struct SyncSignal {
    wake: tokio::sync::Notify,
    pending: Mutex<Pending>,
    /// One sync at a time: the button and the timer would otherwise write the
    /// same rows twice.
    running: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct Pending {
    all: bool,
    repos: Vec<Uuid>,
}

impl SyncSignal {
    /// The repositories asked for since the last sync; `None` means all.
    fn take(&self) -> Option<Vec<Uuid>> {
        let mut pending = match self.pending.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let taken = std::mem::take(&mut *pending);
        if taken.all || taken.repos.is_empty() {
            None
        } else {
            Some(taken.repos)
        }
    }
}

/// What the server remembers about GitHub between calls: installation tokens
/// (GitHub mints them for an hour; minting one per call burns rate limit) and
/// when a rate limit lifts, so the sync stops asking until then.
#[derive(Default)]
pub struct GithubCache {
    app_tokens: Mutex<HashMap<(String, String), (String, std::time::Instant)>>,
    rate_limited_until: Mutex<Option<DateTime<Utc>>>,
}

/// An installation token is good for an hour; ten minutes are left as margin.
const APP_TOKEN_TTL: Duration = Duration::from_secs(50 * 60);

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

impl GithubCache {
    fn app_token(&self, owner: &str, name: &str) -> Option<String> {
        let tokens = locked(&self.app_tokens);
        tokens
            .get(&(owner.to_string(), name.to_string()))
            .filter(|(_, minted)| minted.elapsed() < APP_TOKEN_TTL)
            .map(|(token, _)| token.clone())
    }

    fn keep_app_token(&self, owner: &str, name: &str, token: &str) {
        locked(&self.app_tokens).insert(
            (owner.to_string(), name.to_string()),
            (token.to_string(), std::time::Instant::now()),
        );
    }

    /// When the current rate limit lifts, if one is in force.
    fn rate_limited(&self) -> Option<DateTime<Utc>> {
        let mut until = locked(&self.rate_limited_until);
        if until.is_some_and(|at| at <= Utc::now()) {
            *until = None;
        }
        *until
    }

    fn limit_until(&self, at: DateTime<Utc>) {
        *locked(&self.rate_limited_until) = Some(at);
    }
}

/// The sentence a person reads for a rate limit.
fn rate_limit_reason(until: DateTime<Utc>) -> String {
    format!(
        "GitHub's rate limit was reached; syncing again after {} UTC",
        until.format("%H:%M")
    )
}

/// The review gate of one pull request, with the reason when a button is off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gate {
    pub can_review: bool,
    pub review_blocked_reason: Option<String>,
    pub can_merge: bool,
    pub merge_blocked_reason: Option<String>,
}

/// Which buttons a person gets on a pull request. Pure, so the rules read as a
/// list: no token, then the pull request's own state, then reviews, then CI.
///
/// `mergeable` is GitHub's own conflict check, `None` while it is computing.
/// `task` is why the linked task is not ready to merge, if it is not.
pub fn gate(
    login: Option<&str>,
    pull: &PullRequest,
    mergeable: Option<bool>,
    methods: &[MergeMethod],
    task: Option<&str>,
) -> Gate {
    let closed_reason = if pull.merged {
        Some("This pull request is already merged")
    } else if pull.state != "open" {
        Some("This pull request is closed")
    } else {
        None
    };

    let review_blocked = match login {
        None => Some(NO_TOKEN_REASON),
        Some(_) if closed_reason.is_some() => closed_reason,
        Some(me) if me.eq_ignore_ascii_case(&pull.author_login) => Some(OWN_PULL_REASON),
        Some(_) => None,
    };
    // GitHub never lets an author approve their own pull request, so waiting
    // for that approval would lock them out of merging it for good. Their own
    // pull request merges on CI alone; a branch rule that wants a reviewer is
    // still enforced by GitHub when the merge is sent.
    let own = login.is_some_and(|me| me.eq_ignore_ascii_case(&pull.author_login));

    let merge_blocked = if login.is_none() {
        Some(NO_TOKEN_REASON)
    } else if closed_reason.is_some() {
        closed_reason
    } else if task.is_some() {
        task
    } else if pull.draft {
        Some("This pull request is still a draft")
    } else if pull.review_state == ReviewState::ChangesRequested {
        Some("Changes were requested; it needs an approving review first")
    } else if pull.review_state != ReviewState::Approved && !own {
        Some("Waiting for an approving review")
    } else if pull.ci_state == CiState::Failing {
        Some("CI is failing")
    } else if pull.ci_state == CiState::Running {
        Some("CI is still running")
    } else if mergeable == Some(false) {
        Some("GitHub reports a conflict with the base branch")
    } else if methods.is_empty() {
        Some("The repository allows no merge method")
    } else {
        None
    };

    Gate {
        can_review: review_blocked.is_none(),
        review_blocked_reason: review_blocked.map(str::to_string),
        can_merge: merge_blocked.is_none(),
        merge_blocked_reason: merge_blocked.map(str::to_string),
    }
}

/// The review state GitHub shows: each reviewer counts with their latest
/// review that said something (a plain comment changes nothing), and one
/// request for changes outweighs any number of approvals.
pub fn review_state(pull: &Value, reviews: &[Value]) -> ReviewState {
    if is_merged(pull) {
        return ReviewState::Merged;
    }
    if pull["state"].as_str() == Some("closed") {
        return ReviewState::Closed;
    }
    if pull["draft"].as_bool() == Some(true) {
        return ReviewState::Draft;
    }
    let mut latest: HashMap<String, String> = HashMap::new();
    for review in reviews {
        let state = review["state"].as_str().unwrap_or_default();
        if matches!(state, "COMMENTED" | "PENDING" | "") {
            continue;
        }
        let login = review["user"]["login"].as_str().unwrap_or_default();
        latest.insert(login.to_string(), state.to_string());
    }
    if latest.values().any(|s| s == "CHANGES_REQUESTED") {
        ReviewState::ChangesRequested
    } else if latest.values().any(|s| s == "APPROVED") {
        ReviewState::Approved
    } else {
        ReviewState::ReviewRequested
    }
}

/// Conclusions that make a check a failure. `neutral` and `skipped` pass.
const FAILED_CONCLUSIONS: &[&str] = &[
    "failure",
    "cancelled",
    "timed_out",
    "action_required",
    "startup_failure",
    "stale",
];

/// One CI state out of check runs and the older commit statuses. A commit
/// nothing reported on is `NONE`, which the merge gate lets through.
pub fn ci_state(check_runs: &[Value], combined: &Value) -> CiState {
    let statuses = combined["statuses"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();
    let combined_state = combined["state"].as_str().unwrap_or_default();
    if check_runs.is_empty() && statuses == 0 {
        return CiState::None;
    }
    let failed = check_runs
        .iter()
        .any(|run| FAILED_CONCLUSIONS.contains(&run["conclusion"].as_str().unwrap_or_default()))
        || (statuses > 0 && matches!(combined_state, "failure" | "error"));
    if failed {
        return CiState::Failing;
    }
    let running = check_runs
        .iter()
        .any(|run| run["status"].as_str() != Some("completed"))
        || (statuses > 0 && combined_state == "pending");
    if running {
        CiState::Running
    } else {
        CiState::Passing
    }
}

/// Enough of a token to recognise it: its kind prefix and its last four.
pub fn token_hint(token: &str) -> String {
    const PREFIXES: &[&str] = &["github_pat_", "ghp_", "gho_", "ghu_", "ghs_", "ghr_"];
    let prefix = PREFIXES
        .iter()
        .find(|p| token.starts_with(**p))
        .copied()
        .unwrap_or("");
    let chars: Vec<char> = token.chars().collect();
    let tail: String = if chars.len() >= prefix.len() + 8 {
        chars[chars.len() - 4..].iter().collect()
    } else {
        String::new()
    };
    format!("{prefix}…{tail}")
}

fn is_merged(pull: &Value) -> bool {
    pull["merged"].as_bool() == Some(true) || !pull["merged_at"].is_null()
}

/// A GitHub timestamp in the form the rest of the database uses.
fn timestamp(value: &Value) -> Option<String> {
    value
        .as_str()
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        .map(|at| at.with_timezone(&Utc).to_rfc3339())
}

fn parse_time(value: &Value) -> Option<DateTime<Utc>> {
    value
        .as_str()
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        .map(|at| at.with_timezone(&Utc))
}

fn non_empty(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .map(str::to_string)
}

/// The cached row, as stored.
#[derive(Debug, Clone, sqlx::FromRow)]
struct PullRow {
    repo_id: Uuid,
    number: i64,
    repo_name: String,
    title: String,
    url: String,
    state: String,
    draft: bool,
    merged: bool,
    author_login: String,
    author_avatar_url: Option<String>,
    head_ref: String,
    head_sha: String,
    base_ref: String,
    additions: i64,
    deletions: i64,
    changed_files: i64,
    review_state: String,
    requested_reviewers: String,
    ci_state: String,
    ci_checked_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    merged_at: Option<DateTime<Utc>>,
    issue_id: Option<Uuid>,
}

impl PullRow {
    /// `bot` is the login the App's own pull requests carry (`slug[bot]`).
    fn into_pull(self, me: Option<&str>, bot: Option<&str>) -> PullRequest {
        let requested: Vec<String> =
            serde_json::from_str(&self.requested_reviewers).unwrap_or_default();
        let review_requested_from_me = me
            .map(|me| requested.iter().any(|login| login.eq_ignore_ascii_case(me)))
            .unwrap_or(false);
        PullRequest {
            id: ID(format!("{}#{}", self.repo_id, self.number)),
            repo_id: self.repo_id,
            repo_name: self.repo_name,
            number: self.number,
            title: self.title,
            url: self.url,
            state: self.state,
            draft: self.draft,
            merged: self.merged,
            by_agent: bot.is_some_and(|bot| bot.eq_ignore_ascii_case(&self.author_login))
                || self.issue_id.is_some(),
            author_login: self.author_login,
            author_avatar_url: self.author_avatar_url,
            head_ref: self.head_ref,
            head_sha: self.head_sha,
            base_ref: self.base_ref,
            additions: self.additions,
            deletions: self.deletions,
            changed_files: self.changed_files,
            review_state: ReviewState::parse(&self.review_state),
            review_requested_from_me,
            ci_state: CiState::parse(&self.ci_state),
            created_at: self.created_at,
            updated_at: self.updated_at,
            merged_at: self.merged_at,
            issue_id: self.issue_id,
        }
    }
}

/// Pull requests of repositories still linked to GitHub; an unlinked one's
/// rows wait for the next sync's clean-up, or the repository's deletion.
const LINKED_ONLY: &str = "p.repo_id in (select id from repos where remote_url is not null)";

const PULL_SELECT: &str = "select p.*,
        (select i.id from issues i where i.repo_id = p.repo_id and i.pr_number = p.number
          order by i.created_at desc limit 1) as issue_id
   from pull_requests p";

#[derive(sqlx::FromRow)]
struct TokenRow {
    secret_ciphertext: String,
    login: String,
    name: Option<String>,
    avatar_url: Option<String>,
    token_hint: String,
    problems: String,
    validated_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct CommentRow {
    id: Uuid,
    path: String,
    line: i64,
    side: String,
    body: String,
    created_at: DateTime<Utc>,
}

/// A repository linked to a project that lives on GitHub.
#[derive(Debug, Clone)]
struct LinkedRepo {
    id: Uuid,
    owner: String,
    name: String,
}

impl LinkedRepo {
    fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

// ----------------------------------------------------------------- engine

impl Engine {
    // --------------------------------------------------------- the token

    /// The person's GitHub account, if they gave a token.
    pub async fn github_account(&self) -> Result<Option<GithubAccount>> {
        Ok(self.token_row().await?.map(|row| GithubAccount {
            login: row.login,
            name: row.name,
            avatar_url: row.avatar_url,
            token_hint: row.token_hint,
            validated_at: row.validated_at,
            problems: serde_json::from_str(&row.problems).unwrap_or_default(),
        }))
    }

    /// Checks a token with GitHub, then keeps it sealed. GitHub has to say who
    /// it belongs to, or nothing is stored; what it cannot do in the linked
    /// repositories is stored beside it as plain sentences.
    pub async fn set_github_token(&self, token: &str) -> Result<GithubAccount> {
        let token = token.trim();
        if token.is_empty() {
            bail!("the token is empty");
        }
        let github = Github::new(&self.github_api, token);
        let user = github
            .user()
            .await
            .map_err(|error| anyhow!("GitHub did not accept this token. {error}"))?;
        let login = user["login"]
            .as_str()
            .filter(|login| !login.is_empty())
            .ok_or_else(|| anyhow!("GitHub accepted the token but named no account"))?
            .to_string();

        // A few repositories at a time: one slow repository should not hold the
        // Save button for the length of every other one's probes.
        use futures::StreamExt;
        let probes = futures::stream::iter(self.linked_repos().await?)
            .map(|repo| {
                let github = github.clone();
                async move {
                    let full = repo.full_name();
                    let mut problems = Vec::new();
                    match github.repo(&repo.owner, &repo.name).await {
                        Err(_) => {
                            problems.push(format!("cannot see {full}"));
                            return problems;
                        }
                        Ok(seen) => {
                            let permissions = &seen["permissions"];
                            if permissions.is_object()
                                && permissions["push"].as_bool() != Some(true)
                            {
                                problems.push(format!("cannot merge pull requests in {full}"));
                            }
                        }
                    }
                    if github
                        .can_list_pulls(&repo.owner, &repo.name)
                        .await
                        .is_err()
                    {
                        problems.push(format!("cannot read pull requests in {full}"));
                    }
                    problems
                }
            })
            .buffered(TOKEN_PROBES_AT_ONCE)
            .collect::<Vec<_>>()
            .await;
        let problems: Vec<String> = probes.into_iter().flatten().collect();

        let account = GithubAccount {
            login,
            name: non_empty(&user["name"]),
            avatar_url: non_empty(&user["avatar_url"]),
            token_hint: token_hint(token),
            validated_at: Utc::now(),
            problems,
        };
        let sealed = self.vault()?.seal_string(token)?;
        let user_id = self.db.local_user_id().await?;
        sqlx::query(
            "insert into personal_tokens
               (user_id, provider, secret_ciphertext, login, name, avatar_url, token_hint, problems, validated_at)
             values (?, ?, ?, ?, ?, ?, ?, ?, ?)
             on conflict (user_id, provider) do update set
               secret_ciphertext = excluded.secret_ciphertext, login = excluded.login,
               name = excluded.name, avatar_url = excluded.avatar_url,
               token_hint = excluded.token_hint, problems = excluded.problems,
               validated_at = excluded.validated_at",
        )
        .bind(user_id)
        .bind(PROVIDER)
        .bind(&sealed)
        .bind(&account.login)
        .bind(&account.name)
        .bind(&account.avatar_url)
        .bind(&account.token_hint)
        .bind(serde_json::to_string(&account.problems)?)
        .bind(account.validated_at.to_rfc3339())
        .execute(&self.db.pool)
        .await?;
        // Pending comments keep the name they were written under; with a token
        // they are the person's, not an anonymous "you".
        sqlx::query("update pull_request_comments set author_login = ?")
            .bind(&account.login)
            .execute(&self.db.pool)
            .await?;
        tracing::info!(login = %account.login, "github token stored");
        Ok(account)
    }

    /// Forgets the person's token. Returns whether there was one.
    pub async fn clear_github_token(&self) -> Result<bool> {
        let user_id = self.db.local_user_id().await?;
        let done = sqlx::query("delete from personal_tokens where user_id = ? and provider = ?")
            .bind(user_id)
            .bind(PROVIDER)
            .execute(&self.db.pool)
            .await?;
        Ok(done.rows_affected() > 0)
    }

    async fn token_row(&self) -> Result<Option<TokenRow>> {
        let user_id = self.db.local_user_id().await?;
        Ok(sqlx::query_as::<_, TokenRow>(
            "select secret_ciphertext, login, name, avatar_url, token_hint, problems, validated_at
               from personal_tokens where user_id = ? and provider = ?",
        )
        .bind(user_id)
        .bind(PROVIDER)
        .fetch_optional(&self.db.pool)
        .await?)
    }

    /// The person's token and login, opened from the vault.
    async fn personal_github(&self) -> Result<Option<(Github, String)>> {
        let Some(row) = self.token_row().await? else {
            return Ok(None);
        };
        let token = self.vault()?.open_string(&row.secret_ciphertext)?;
        Ok(Some((Github::new(&self.github_api, &token), row.login)))
    }

    /// A client for reading one repository as the App, with its installation
    /// token for that repository, or none when no App is stored. Tokens are
    /// kept for most of their hour.
    async fn app_github(&self, owner: &str, name: &str) -> Result<Option<Github>> {
        if let Some(token) = self.github_cache.app_token(owner, name) {
            return Ok(Some(Github::new(&self.github_api, &token)));
        }
        let Some(jwt) = self.github_token().await? else {
            return Ok(None);
        };
        let token = crate::github::installation_token_at(
            &crate::github::http_client(),
            &self.github_api,
            &jwt,
            owner,
            name,
        )
        .await?;
        self.github_cache.keep_app_token(owner, name, &token);
        Ok(Some(Github::new(&self.github_api, &token)))
    }

    /// The login the App's own pull requests are authored by, when it is known.
    async fn app_bot_login(&self) -> Option<String> {
        self.stored_app(crate::github::PROVIDER, crate::connectors::KIND_GITHUB_APP)
            .await
            .ok()
            .flatten()
            .and_then(|app| app.slug)
            .map(|slug| format!("{slug}[bot]"))
    }

    /// Runs `read` with the person's token, falling back to the App when there
    /// is no token or GitHub would not show the repository to it.
    async fn read_github<T, F, Fut>(&self, repo: &LinkedRepo, read: F) -> Result<T>
    where
        F: Fn(Github) -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let full = repo.full_name();
        if let Some((github, _)) = self.personal_github().await? {
            match read(github).await {
                Ok(value) => return Ok(value),
                Err(error) if crate::github::GithubError::is_denied(&error) => {
                    tracing::debug!(repo = %full, "the token cannot read this repository: {error:#}");
                    match self.app_github(&repo.owner, &repo.name).await {
                        Ok(Some(app)) => return read(app).await,
                        _ => return Err(error),
                    }
                }
                Err(error) => return Err(error),
            }
        }
        match self.app_github(&repo.owner, &repo.name).await {
            Ok(Some(app)) => read(app).await,
            Ok(None) => bail!("no GitHub access for {full}"),
            Err(error) => bail!("no GitHub access for {full}: {error:#}"),
        }
    }

    /// Repositories linked to a project whose remote is on GitHub.
    async fn linked_repos(&self) -> Result<Vec<LinkedRepo>> {
        let rows: Vec<(Uuid, Option<String>)> = sqlx::query_as(
            "select id, remote_url from repos where remote_url is not null order by created_at",
        )
        .fetch_all(&self.db.pool)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(id, remote)| {
                let (owner, name) = split_remote(remote.as_deref()?)?;
                Some(LinkedRepo { id, owner, name })
            })
            .collect())
    }

    async fn linked_repo(&self, repo_id: Uuid) -> Result<Option<LinkedRepo>> {
        Ok(self
            .linked_repos()
            .await?
            .into_iter()
            .find(|repo| repo.id == repo_id))
    }

    // ---------------------------------------------------------- the list

    /// The cached pull requests a filter selects, most recently updated first.
    ///
    /// Every filter but `MERGED` shows open pull requests. "Needs my review"
    /// is what GitHub asked the person to review, plus agents' pull requests
    /// nobody has reviewed yet, since an app cannot ask for a reviewer.
    pub async fn pull_requests(&self, filter: PullRequestFilter) -> Result<Vec<PullRequest>> {
        let me = self.token_row().await?.map(|row| row.login);
        let bot = self.app_bot_login().await;
        let rows = sqlx::query_as::<_, PullRow>(&format!(
            "{PULL_SELECT} where {LINKED_ONLY} order by p.updated_at desc"
        ))
        .fetch_all(&self.db.pool)
        .await?;
        let pulls = rows
            .into_iter()
            .map(|row| row.into_pull(me.as_deref(), bot.as_deref()));
        Ok(pulls
            .filter(|pull| {
                let open = pull.state == "open";
                match filter {
                    PullRequestFilter::Open => open,
                    PullRequestFilter::NeedsMyReview => {
                        let mine = me
                            .as_deref()
                            .is_some_and(|me| me.eq_ignore_ascii_case(&pull.author_login));
                        open && !pull.draft
                            && !mine
                            && (pull.review_requested_from_me
                                || (pull.by_agent
                                    && pull.review_state == ReviewState::ReviewRequested))
                    }
                    PullRequestFilter::Mine => {
                        open && me
                            .as_deref()
                            .is_some_and(|me| me.eq_ignore_ascii_case(&pull.author_login))
                    }
                    PullRequestFilter::ByAgents => open && pull.by_agent,
                    PullRequestFilter::FailingCi => open && pull.ci_state == CiState::Failing,
                    PullRequestFilter::Merged => pull.merged,
                }
            })
            .collect())
    }

    /// How many cached pull requests are open, for the sidebar.
    pub async fn open_pull_request_count(&self) -> Result<i64> {
        Ok(sqlx::query_scalar(&format!(
            "select count(*) from pull_requests p where p.state = 'open' and {LINKED_ONLY}"
        ))
        .fetch_one(&self.db.pool)
        .await?)
    }

    /// When the last sync finished.
    pub async fn pull_requests_synced_at(&self) -> Result<Option<DateTime<Utc>>> {
        let value: Option<String> = sqlx::query_scalar("select value from meta where key = ?")
            .bind(SYNCED_AT_KEY)
            .fetch_optional(&self.db.pool)
            .await?;
        Ok(value
            .and_then(|text| DateTime::parse_from_rfc3339(&text).ok())
            .map(|at| at.with_timezone(&Utc)))
    }

    /// What the last sync could not do, one sentence per repository.
    pub async fn pull_request_sync_problems(&self) -> Result<Vec<String>> {
        let linked: Vec<Uuid> = self.linked_repos().await?.iter().map(|r| r.id).collect();
        let rows: Vec<(Uuid, String)> = sqlx::query_as(
            "select repo_id, sync_problem from pull_request_repos where sync_problem is not null",
        )
        .fetch_all(&self.db.pool)
        .await?;
        Ok(rows
            .into_iter()
            .filter(|(id, _)| linked.contains(id))
            .map(|(_, problem)| problem)
            .collect())
    }

    async fn load_pull(&self, repo_id: Uuid, number: i64) -> Result<Option<PullRequest>> {
        let me = self.token_row().await?.map(|row| row.login);
        let bot = self.app_bot_login().await;
        let row = sqlx::query_as::<_, PullRow>(&format!(
            "{PULL_SELECT} where p.repo_id = ? and p.number = ?"
        ))
        .bind(repo_id)
        .bind(number)
        .fetch_optional(&self.db.pool)
        .await?;
        Ok(row.map(|row| row.into_pull(me.as_deref(), bot.as_deref())))
    }

    async fn cached_row(&self, repo_id: Uuid, number: i64) -> Result<Option<PullRow>> {
        Ok(sqlx::query_as::<_, PullRow>(&format!(
            "{PULL_SELECT} where p.repo_id = ? and p.number = ?"
        ))
        .bind(repo_id)
        .bind(number)
        .fetch_optional(&self.db.pool)
        .await?)
    }

    // ---------------------------------------------------------- the sync

    /// Asks the background sync to run now. `Some(repo)` narrows it to that
    /// repository; `None` syncs all of them.
    pub fn request_pull_sync(&self, repo_id: Option<Uuid>) {
        {
            let mut pending = match self.pull_sync.pending.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            match repo_id {
                Some(id) => pending.repos.push(id),
                None => pending.all = true,
            }
        }
        self.pull_sync.wake.notify_one();
    }

    /// The background sync: once at start, then every [`SYNC_INTERVAL`] or
    /// sooner when something asks. A failed sync is logged and tried again on
    /// the next round; per-repository problems are recorded, not raised.
    pub async fn pull_sync_loop(self: std::sync::Arc<Self>) {
        let mut only: Option<Vec<Uuid>> = None;
        // A fixed deadline: an early, narrowed sync must not push the next full
        // one back, or a busy server would never sync every repository.
        let mut due = tokio::time::Instant::now();
        loop {
            if only.is_none() {
                due = tokio::time::Instant::now() + SYNC_INTERVAL;
            }
            if let Err(error) = self.sync_pull_requests(only.as_deref()).await {
                tracing::warn!("pull request sync failed: {error:#}");
            }
            only = tokio::select! {
                _ = tokio::time::sleep_until(due) => None,
                _ = self.pull_sync.wake.notified() => self.pull_sync.take(),
            };
        }
    }

    /// Brings the cache in step with GitHub for every linked repository, or the
    /// ones named. One repository GitHub refuses does not stop the others: its
    /// reason is recorded and shown, and the rest sync.
    pub async fn sync_pull_requests(&self, only: Option<&[Uuid]>) -> Result<DateTime<Utc>> {
        let _running = self.pull_sync.running.lock().await;
        if let Some(until) = self.github_cache.rate_limited() {
            bail!("{}", rate_limit_reason(until));
        }
        for repo in self.linked_repos().await? {
            if only.is_some_and(|only| !only.contains(&repo.id)) {
                continue;
            }
            let problem = match self.sync_repo(&repo).await {
                Ok(()) => None,
                Err(error) => {
                    tracing::warn!(repo = %repo.full_name(), "pull request sync: {error:#}");
                    if let Some(until) = crate::github::GithubError::rate_limit(&error) {
                        // Every further call would be refused too: stop here,
                        // say until when, and leave the other repositories as
                        // they were.
                        self.github_cache.limit_until(until);
                        let reason = rate_limit_reason(until);
                        sqlx::query(
                            "insert into pull_request_repos (repo_id, sync_problem) values (?, ?)
                             on conflict (repo_id) do update set sync_problem = excluded.sync_problem",
                        )
                        .bind(repo.id)
                        .bind(&reason)
                        .execute(&self.db.pool)
                        .await?;
                        bail!("{reason}");
                    }
                    Some(format!("{error:#}"))
                }
            };
            sqlx::query(
                "insert into pull_request_repos (repo_id, synced_at, sync_problem) values (?, ?, ?)
                 on conflict (repo_id) do update set synced_at = excluded.synced_at,
                   sync_problem = excluded.sync_problem",
            )
            .bind(repo.id)
            .bind(crate::db::now_rfc3339())
            .bind(problem)
            .execute(&self.db.pool)
            .await?;
        }
        let now = Utc::now();
        sqlx::query(
            "insert into meta (key, value) values (?, ?)
             on conflict (key) do update set value = excluded.value",
        )
        .bind(SYNCED_AT_KEY)
        .bind(now.to_rfc3339())
        .execute(&self.db.pool)
        .await?;
        Ok(now)
    }

    /// One repository: the hundred most recently updated pull requests and
    /// every open one. Reviews are read again when a pull request changed, CI
    /// when its head moved, while it runs, and every few minutes otherwise.
    async fn sync_repo(&self, repo: &LinkedRepo) -> Result<()> {
        let full = repo.full_name();
        let listing = format!("could not list pull requests in {full}");
        // The first listing picks the client: the person's token, or the App
        // when the token cannot see this repository.
        let (github, recent) = self
            .read_github(repo, |github| {
                let (owner, name) = (repo.owner.clone(), repo.name.clone());
                async move {
                    let recent = github.pulls_page(&owner, &name, "all", 1).await?;
                    Ok((github, recent))
                }
            })
            .await
            .context(listing.clone())?;
        let mut seen: HashMap<i64, Value> = HashMap::new();
        let open = github
            .open_pulls(&repo.owner, &repo.name)
            .await
            .context(listing)?;
        for pull in recent.into_iter().chain(open) {
            if let Some(number) = pull["number"].as_i64() {
                seen.insert(number, pull);
            }
        }

        let cached: HashMap<i64, PullRow> =
            sqlx::query_as::<_, PullRow>(&format!("{PULL_SELECT} where p.repo_id = ?"))
                .bind(repo.id)
                .fetch_all(&self.db.pool)
                .await?
                .into_iter()
                .map(|row| (row.number, row))
                .collect();

        let cutoff = Utc::now() - chrono::Duration::days(KEEP_CLOSED_DAYS);
        for (number, listed) in &seen {
            let open = listed["state"].as_str() == Some("open");
            let closed_at = parse_time(&listed["closed_at"]).or(parse_time(&listed["updated_at"]));
            if !open && closed_at.is_some_and(|at| at < cutoff) {
                continue;
            }
            let before = cached.get(number);
            if let Err(error) = self.refresh_pull(&github, repo, listed, before).await {
                if crate::github::GithubError::rate_limit(&error).is_some() {
                    return Err(error);
                }
                tracing::warn!(repo = %full, number, "pull request sync: {error:#}");
            }
        }

        // An open pull request that fell out of both listings closed some time
        // ago; ask for it directly so the cache does not keep it open forever.
        for (number, row) in &cached {
            if row.state != "open" || seen.contains_key(number) {
                continue;
            }
            match github.pull(&repo.owner, &repo.name, *number).await {
                Ok(pull) => {
                    if let Err(error) = self.refresh_pull(&github, repo, &pull, Some(row)).await {
                        tracing::warn!(repo = %full, number, "pull request sync: {error:#}");
                    }
                }
                // Only GitHub saying it is gone removes it; an outage, a
                // timeout or a rate limit keeps the row for the next round.
                Err(error) if crate::github::GithubError::is_gone(&error) => {
                    sqlx::query("delete from pull_requests where repo_id = ? and number = ?")
                        .bind(repo.id)
                        .bind(number)
                        .execute(&self.db.pool)
                        .await?;
                }
                Err(error) => {
                    if crate::github::GithubError::rate_limit(&error).is_some() {
                        return Err(error);
                    }
                    tracing::warn!(repo = %full, number, "pull request sync: {error:#}");
                }
            }
        }

        sqlx::query(
            "delete from pull_requests where repo_id = ? and state = 'closed'
               and coalesce(closed_at, updated_at) < ?",
        )
        .bind(repo.id)
        .bind(cutoff.to_rfc3339())
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Updates one cached pull request from its listing, asking GitHub for
    /// more only where the listing says something changed.
    async fn refresh_pull(
        &self,
        github: &Github,
        repo: &LinkedRepo,
        listed: &Value,
        before: Option<&PullRow>,
    ) -> Result<()> {
        let number = listed["number"].as_i64().unwrap_or_default();
        let open = listed["state"].as_str() == Some("open");
        let updated = parse_time(&listed["updated_at"]);
        let changed = before.is_none_or(|row| Some(row.updated_at) != updated);

        // The listing has no line counts; the pull request itself does.
        let pull = if changed && listed["additions"].is_null() {
            github.pull(&repo.owner, &repo.name, number).await?
        } else {
            listed.clone()
        };

        let head = pull["head"]["sha"].as_str().unwrap_or_default();
        // Reviews do not always move `updated_at`, so they are read again on
        // the same clock as CI, not only when the listing says it changed.
        let stale = before.is_none_or(|row| {
            row.ci_checked_at
                .is_none_or(|at| Utc::now() - at > chrono::Duration::minutes(CI_RECHECK_MINUTES))
        });
        let review = if !open || is_merged(&pull) || pull["draft"].as_bool() == Some(true) {
            review_state(&pull, &[])
        } else if changed || stale {
            let reviews = github.reviews(&repo.owner, &repo.name, number).await?;
            review_state(&pull, &reviews)
        } else {
            before
                .map(|row| ReviewState::parse(&row.review_state))
                .unwrap_or(ReviewState::ReviewRequested)
        };

        let ci_due = stale
            || before.is_some_and(|row| {
                row.head_sha != head || row.ci_state == CiState::Running.as_str()
            });
        let ci = if open && ci_due && !head.is_empty() {
            let runs = github.check_runs(&repo.owner, &repo.name, head).await?;
            let combined = github
                .combined_status(&repo.owner, &repo.name, head)
                .await?;
            Some(ci_state(&runs, &combined))
        } else {
            None
        };
        self.store_pull(repo, &pull, review, ci, before).await
    }

    /// Writes one pull request into the cache. `ci` is `None` when CI was not
    /// asked this time, which keeps what was cached.
    async fn store_pull(
        &self,
        repo: &LinkedRepo,
        pull: &Value,
        review: ReviewState,
        ci: Option<CiState>,
        before: Option<&PullRow>,
    ) -> Result<()> {
        let number = pull["number"]
            .as_i64()
            .ok_or_else(|| anyhow!("GitHub sent a pull request without a number"))?;
        let count = |key: &str, cached: Option<i64>| pull[key].as_i64().or(cached).unwrap_or(0);
        let requested: Vec<String> = pull["requested_reviewers"]
            .as_array()
            .map(|users| {
                users
                    .iter()
                    .filter_map(|user| user["login"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let now = crate::db::now_rfc3339();
        let ci_state = ci
            .or_else(|| before.map(|row| CiState::parse(&row.ci_state)))
            .unwrap_or(CiState::None);
        let ci_checked_at = if ci.is_some() {
            Some(now.clone())
        } else {
            before.and_then(|row| row.ci_checked_at.map(|at| at.to_rfc3339()))
        };
        sqlx::query(
            "insert into pull_requests
               (repo_id, number, repo_name, title, url, state, draft, merged, author_login,
                author_avatar_url, head_ref, head_sha, base_ref, additions, deletions, changed_files,
                review_state, requested_reviewers, ci_state, ci_checked_at, created_at, updated_at,
                merged_at, closed_at, synced_at)
             values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             on conflict (repo_id, number) do update set
               repo_name = excluded.repo_name, title = excluded.title, url = excluded.url,
               state = excluded.state, draft = excluded.draft, merged = excluded.merged,
               author_login = excluded.author_login, author_avatar_url = excluded.author_avatar_url,
               head_ref = excluded.head_ref, head_sha = excluded.head_sha, base_ref = excluded.base_ref,
               additions = excluded.additions, deletions = excluded.deletions,
               changed_files = excluded.changed_files, review_state = excluded.review_state,
               requested_reviewers = excluded.requested_reviewers, ci_state = excluded.ci_state,
               ci_checked_at = excluded.ci_checked_at, created_at = excluded.created_at,
               updated_at = excluded.updated_at, merged_at = excluded.merged_at,
               closed_at = excluded.closed_at, synced_at = excluded.synced_at
             -- A sync that read GitHub before a merge must not write the
             -- pull request back open over the merge recorded after it, and
             -- GitHub's timestamps are whole seconds, so a merge is never undone.
             where excluded.updated_at >= pull_requests.updated_at
               and (excluded.merged = 1 or pull_requests.merged = 0)",
        )
        .bind(repo.id)
        .bind(number)
        .bind(repo.full_name())
        .bind(pull["title"].as_str().unwrap_or_default())
        .bind(pull["html_url"].as_str().unwrap_or_default())
        .bind(pull["state"].as_str().unwrap_or("open"))
        .bind(pull["draft"].as_bool().unwrap_or(false))
        .bind(is_merged(pull))
        .bind(pull["user"]["login"].as_str().unwrap_or_default())
        .bind(non_empty(&pull["user"]["avatar_url"]))
        .bind(pull["head"]["ref"].as_str().unwrap_or_default())
        .bind(pull["head"]["sha"].as_str().unwrap_or_default())
        .bind(pull["base"]["ref"].as_str().unwrap_or_default())
        .bind(count("additions", before.map(|row| row.additions)))
        .bind(count("deletions", before.map(|row| row.deletions)))
        .bind(count("changed_files", before.map(|row| row.changed_files)))
        .bind(review.as_str())
        .bind(serde_json::to_string(&requested)?)
        .bind(ci_state.as_str())
        .bind(ci_checked_at)
        .bind(timestamp(&pull["created_at"]).unwrap_or_else(|| now.clone()))
        .bind(timestamp(&pull["updated_at"]).unwrap_or_else(|| now.clone()))
        .bind(timestamp(&pull["merged_at"]))
        .bind(timestamp(&pull["closed_at"]))
        .bind(&now)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    // -------------------------------------------------------- one review

    /// One pull request, read live from GitHub: its files, comments, reviews
    /// and checks, which merge methods the repository accepts, and which of
    /// the person's buttons are on. Also refreshes the cached row, so the list
    /// agrees with what the person just saw.
    pub async fn pull_request_detail(
        &self,
        repo_id: Uuid,
        number: i64,
    ) -> Result<Option<PullRequestDetail>> {
        let Some(repo) = self.linked_repo(repo_id).await? else {
            return Ok(None);
        };
        let login = self.token_row().await?.map(|row| row.login);
        let (fresh, files, comments) = self
            .read_github(&repo, |github| {
                let (owner, name, login) = (repo.owner.clone(), repo.name.clone(), login.clone());
                let repo = repo.clone();
                async move {
                    futures::try_join!(
                        self.fresh_pull(&github, &repo, number, login.as_deref()),
                        github.pull_files(&owner, &name, number),
                        github.review_comments(&owner, &name, number),
                    )
                }
            })
            .await?;
        let Fresh {
            pull,
            reviews,
            runs,
            combined,
            merge_methods,
            pull_request,
            gate,
        } = fresh;
        let last_merge_method: Option<String> = sqlx::query_scalar(
            "select last_merge_method from pull_request_repos where repo_id = ?",
        )
        .bind(repo_id)
        .fetch_optional(&self.db.pool)
        .await?
        .flatten();

        let mut all_comments: Vec<PullRequestComment> = comments
            .iter()
            // A comment whose line is gone from the current diff (`line` is
            // null: it is outdated) would land on whatever line now carries its
            // old number, so it is left to GitHub's own conversation view.
            .filter(|comment| comment["line"].as_i64().is_some())
            .map(|comment| PullRequestComment {
                id: ID(format!(
                    "github-{}",
                    comment["id"].as_i64().unwrap_or_default()
                )),
                path: comment["path"].as_str().unwrap_or_default().to_string(),
                line: comment["line"].as_i64().unwrap_or(0),
                side: DiffSide::parse(comment["side"].as_str().unwrap_or("RIGHT")),
                body: comment["body"].as_str().unwrap_or_default().to_string(),
                author_login: comment["user"]["login"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                created_at: parse_time(&comment["created_at"]).unwrap_or_else(Utc::now),
                pending: false,
                only_in_hulahoop: false,
            })
            .collect();
        all_comments.extend(
            self.pending_comments(repo_id, number, login.as_deref())
                .await?,
        );

        let mut checks: Vec<CheckRun> = runs
            .iter()
            .map(|run| CheckRun {
                name: run["name"].as_str().unwrap_or_default().to_string(),
                status: run["status"].as_str().unwrap_or_default().to_string(),
                conclusion: non_empty(&run["conclusion"]),
                url: non_empty(&run["html_url"]),
            })
            .collect();
        // Commit statuses read as checks too: one that is pending is still
        // running, any other has settled with its state as the conclusion.
        for status in combined["statuses"].as_array().into_iter().flatten() {
            let state = status["state"].as_str().unwrap_or_default();
            checks.push(CheckRun {
                name: status["context"].as_str().unwrap_or_default().to_string(),
                status: if state == "pending" {
                    "in_progress".to_string()
                } else {
                    "completed".to_string()
                },
                conclusion: (state != "pending").then(|| state.to_string()),
                url: non_empty(&status["target_url"]),
            });
        }

        Ok(Some(PullRequestDetail {
            pull_request,
            body: non_empty(&pull["body"]),
            files: files
                .iter()
                .map(|file| PullRequestFile {
                    path: file["filename"].as_str().unwrap_or_default().to_string(),
                    previous_path: non_empty(&file["previous_filename"]),
                    status: file["status"].as_str().unwrap_or_default().to_string(),
                    additions: file["additions"].as_i64().unwrap_or_default(),
                    deletions: file["deletions"].as_i64().unwrap_or_default(),
                    patch: file["patch"].as_str().map(str::to_string),
                })
                .collect(),
            comments: all_comments,
            reviews: reviews
                .iter()
                .filter(|review| review["state"].as_str() != Some("PENDING"))
                .map(review_out)
                .collect(),
            checks,
            merge_methods,
            last_merge_method: last_merge_method.as_deref().and_then(MergeMethod::parse),
            can_review: gate.can_review,
            review_blocked_reason: gate.review_blocked_reason,
            can_merge: gate.can_merge,
            merge_blocked_reason: gate.merge_blocked_reason,
        }))
    }

    /// The pull request as GitHub has it right now, with what the gate needs:
    /// reviews, CI, the merge methods the repository allows and the linked
    /// task. Refreshes the cached row on the way, so the list agrees.
    async fn fresh_pull(
        &self,
        github: &Github,
        repo: &LinkedRepo,
        number: i64,
        login: Option<&str>,
    ) -> Result<Fresh> {
        let (o, r) = (repo.owner.as_str(), repo.name.as_str());
        let pull = github.pull(o, r, number).await?;
        let head = pull["head"]["sha"].as_str().unwrap_or_default().to_string();
        let (reviews, runs, combined, settings) = futures::try_join!(
            github.reviews(o, r, number),
            github.check_runs(o, r, &head),
            github.combined_status(o, r, &head),
            github.repo(o, r),
        )?;
        let review = review_state(&pull, &reviews);
        let ci = ci_state(&runs, &combined);
        let before = self.cached_row(repo.id, number).await?;
        self.store_pull(repo, &pull, review, Some(ci), before.as_ref())
            .await?;
        let pull_request = self
            .load_pull(repo.id, number)
            .await?
            .ok_or_else(|| anyhow!("pull request #{number} vanished from the cache"))?;

        let mut merge_methods = Vec::new();
        for (key, method) in [
            ("allow_merge_commit", MergeMethod::Merge),
            ("allow_squash_merge", MergeMethod::Squash),
            ("allow_rebase_merge", MergeMethod::Rebase),
        ] {
            // A token that cannot administer the repository may not be told;
            // GitHub's default then is that every method is allowed.
            if settings[key].as_bool().unwrap_or(true) {
                merge_methods.push(method);
            }
        }
        let task = self.task_merge_block(repo.id, number).await?;
        let gate = gate(
            login,
            &pull_request,
            pull["mergeable"].as_bool(),
            &merge_methods,
            task.as_deref(),
        );
        Ok(Fresh {
            pull,
            reviews,
            runs,
            combined,
            merge_methods,
            pull_request,
            gate,
        })
    }

    /// Why the task linked to a pull request is not ready for it to merge:
    /// an agent still working on it, or a task that never reached review.
    /// A task already done is fine, as is a pull request with no task.
    async fn task_merge_block(&self, repo_id: Uuid, number: i64) -> Result<Option<String>> {
        let Some(issue) = self.linked_issue(repo_id, number).await? else {
            return Ok(None);
        };
        if self.live_run_on_issue(issue.id).await?.is_some() {
            return Ok(Some(format!(
                "An agent is still working on {}",
                issue.issue_key
            )));
        }
        Ok(match issue.status_enum() {
            IssueStatus::InReview | IssueStatus::Done => None,
            _ => Some(format!(
                "Move {} to In review before merging",
                issue.issue_key
            )),
        })
    }

    async fn pending_comments(
        &self,
        repo_id: Uuid,
        number: i64,
        login: Option<&str>,
    ) -> Result<Vec<PullRequestComment>> {
        let rows = sqlx::query_as::<_, CommentRow>(
            "select id, path, line, side, body, created_at from pull_request_comments
              where repo_id = ? and number = ? order by created_at, id",
        )
        .bind(repo_id)
        .bind(number)
        .fetch_all(&self.db.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| comment_out(row, login))
            .collect())
    }

    /// Keeps a line comment in Hulahoop until a review sends it.
    #[allow(clippy::too_many_arguments)]
    pub async fn add_pull_request_comment(
        &self,
        repo_id: Uuid,
        number: i64,
        path: &str,
        line: i64,
        side: DiffSide,
        body: &str,
    ) -> Result<PullRequestComment> {
        if body.trim().is_empty() {
            bail!("a comment needs some text");
        }
        if path.trim().is_empty() || line < 1 {
            bail!("a comment belongs on a line of a file");
        }
        if self.linked_repo(repo_id).await?.is_none() {
            bail!("no linked GitHub repository has id {repo_id}");
        }
        let login = self.token_row().await?.map(|row| row.login);
        let row = CommentRow {
            id: Uuid::now_v7(),
            path: path.to_string(),
            line,
            side: side.as_github().to_string(),
            body: body.trim().to_string(),
            created_at: Utc::now(),
        };
        sqlx::query(
            "insert into pull_request_comments
               (id, repo_id, number, path, line, side, body, author_login, created_at)
             values (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(row.id)
        .bind(repo_id)
        .bind(number)
        .bind(&row.path)
        .bind(row.line)
        .bind(&row.side)
        .bind(&row.body)
        .bind(login.as_deref().unwrap_or("you"))
        .bind(row.created_at.to_rfc3339())
        .execute(&self.db.pool)
        .await?;
        Ok(comment_out(row, login.as_deref()))
    }

    /// Drops a comment still pending in Hulahoop. One already on GitHub is
    /// GitHub's to delete.
    pub async fn delete_pull_request_comment(&self, id: &str) -> Result<bool> {
        let Ok(id) = Uuid::parse_str(id) else {
            bail!("only a comment still pending in Hulahoop can be deleted here");
        };
        let done = sqlx::query("delete from pull_request_comments where id = ?")
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Submits a review on GitHub with every pending line comment. Only once
    /// GitHub accepted it are the comments dropped here and, for a request for
    /// changes, the linked task sent back to In progress with what was asked.
    pub async fn review_pull_request(
        &self,
        repo_id: Uuid,
        number: i64,
        event: ReviewEvent,
        body: Option<&str>,
    ) -> Result<PullRequestDetail> {
        let repo = self
            .linked_repo(repo_id)
            .await?
            .ok_or_else(|| anyhow!("no linked GitHub repository has id {repo_id}"))?;
        let Some((github, login)) = self.personal_github().await? else {
            bail!("{NO_TOKEN_REASON}");
        };
        let pending = sqlx::query_as::<_, CommentRow>(
            "select id, path, line, side, body, created_at from pull_request_comments
              where repo_id = ? and number = ? order by created_at, id",
        )
        .bind(repo_id)
        .bind(number)
        .fetch_all(&self.db.pool)
        .await?;
        let note = body.map(str::trim).filter(|note| !note.is_empty());
        if note.is_none() && pending.is_empty() {
            bail!("a review needs a note or at least one line comment");
        }
        let comments: Vec<Value> = pending
            .iter()
            .map(|comment| {
                serde_json::json!({
                    "path": comment.path,
                    "line": comment.line,
                    "side": comment.side,
                    "body": comment.body,
                })
            })
            .collect();

        // The same gate the buttons show, on what GitHub says now.
        let fresh = self
            .fresh_pull(&github, &repo, number, Some(&login))
            .await?;
        if let Some(reason) = fresh.gate.review_blocked_reason {
            bail!("{reason}");
        }

        github
            .create_review(
                &repo.owner,
                &repo.name,
                number,
                event.as_github(),
                note,
                &comments,
            )
            .await?;

        // GitHub has the review. Nothing below may turn that into a failure
        // the person would retry, sending the same review twice: what goes
        // wrong here is logged and the review stands.
        if let Err(error) = self.drop_sent_comments(&pending).await {
            tracing::warn!(
                number,
                "the review was sent, but its comments stay pending here: {error:#}"
            );
        }

        if event == ReviewEvent::RequestChanges {
            if let Some(issue) = self.linked_issue(repo_id, number).await? {
                if issue.status_enum() == IssueStatus::InReview {
                    let mut text = format!("Changes requested on pull request #{number}.");
                    if let Some(note) = note {
                        text.push_str("\n\n");
                        text.push_str(note);
                    }
                    if !pending.is_empty() {
                        text.push_str("\n\nLine comments:");
                        for comment in &pending {
                            text.push_str(&format!(
                                "\n- `{}` line {}: {}",
                                comment.path, comment.line, comment.body
                            ));
                        }
                    }
                    if let Err(error) = self.request_changes(issue.id, &text).await {
                        tracing::warn!(
                            number,
                            "the review was sent, but the task was not sent back: {error:#}"
                        );
                    }
                }
            }
        }

        match self.pull_request_detail(repo_id, number).await {
            Ok(Some(detail)) => Ok(detail),
            outcome => {
                if let Err(error) = &outcome {
                    tracing::warn!(
                        number,
                        "the review was sent, but reading it back failed: {error:#}"
                    );
                }
                Ok(review_sent_detail(fresh))
            }
        }
    }

    /// Drops the comments a review just carried to GitHub, all or none.
    async fn drop_sent_comments(&self, sent: &[CommentRow]) -> Result<()> {
        let mut tx = self.db.pool.begin().await?;
        for comment in sent {
            sqlx::query("delete from pull_request_comments where id = ?")
                .bind(comment.id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Merges on GitHub, then records it: the method becomes the repository's
    /// preselected one, and a linked task in review closes exactly as the
    /// task's own Merge button closes it.
    ///
    /// The gate is checked again here on what GitHub says now, and the merge
    /// is pinned to `head_sha` (the head the person reviewed; the cached one
    /// when not given), so commits pushed since are never merged unseen.
    pub async fn merge_pull_request(
        &self,
        repo_id: Uuid,
        number: i64,
        method: MergeMethod,
        head_sha: Option<&str>,
    ) -> Result<PullRequest> {
        let repo = self
            .linked_repo(repo_id)
            .await?
            .ok_or_else(|| anyhow!("no linked GitHub repository has id {repo_id}"))?;
        let Some((github, login)) = self.personal_github().await? else {
            bail!("{NO_TOKEN_REASON}");
        };
        let seen = match head_sha.map(str::trim).filter(|sha| !sha.is_empty()) {
            Some(sha) => Some(sha.to_string()),
            None => self
                .cached_row(repo_id, number)
                .await?
                .map(|row| row.head_sha)
                .filter(|sha| !sha.is_empty()),
        };
        let fresh = self
            .fresh_pull(&github, &repo, number, Some(&login))
            .await?;
        if let Some(reason) = &fresh.gate.merge_blocked_reason {
            bail!("{reason}");
        }
        if !fresh.merge_methods.contains(&method) {
            bail!(
                "{} does not allow {} merges",
                repo.full_name(),
                method.as_github()
            );
        }
        let sha = seen.unwrap_or_else(|| fresh.pull_request.head_sha.clone());
        if sha != fresh.pull_request.head_sha {
            bail!(
                "New commits were pushed to #{number} since you looked; review them before merging"
            );
        }
        let merged = github
            .merge(&repo.owner, &repo.name, number, method.as_github(), &sha)
            .await?;

        // GitHub merged it. From here on nothing may report a failure, or the
        // person would press Merge again on a merged pull request: what goes
        // wrong is logged, and the next sync catches the cache up.
        if let Err(error) = self
            .after_merge(&repo, &github, number, method, &merged)
            .await
        {
            tracing::warn!(repo = %repo.full_name(), number, "merged on GitHub, but recording it failed: {error:#}");
        }
        match self.load_pull(repo_id, number).await {
            Ok(Some(pull)) if pull.merged => Ok(pull),
            outcome => {
                if let Err(error) = &outcome {
                    tracing::warn!(
                        number,
                        "merged on GitHub, but reading it back failed: {error:#}"
                    );
                }
                let mut pull = fresh.pull_request;
                pull.merged = true;
                pull.state = "closed".to_string();
                pull.review_state = ReviewState::Merged;
                pull.merged_at = Some(Utc::now());
                Ok(pull)
            }
        }
    }

    /// What follows a merge GitHub accepted: the preselected method, the
    /// cached row and the linked task.
    async fn after_merge(
        &self,
        repo: &LinkedRepo,
        github: &Github,
        number: i64,
        method: MergeMethod,
        merged: &Value,
    ) -> Result<()> {
        sqlx::query(
            "insert into pull_request_repos (repo_id, last_merge_method) values (?, ?)
             on conflict (repo_id) do update set last_merge_method = excluded.last_merge_method",
        )
        .bind(repo.id)
        .bind(method.as_github())
        .execute(&self.db.pool)
        .await?;

        let pull = github.pull(&repo.owner, &repo.name, number).await.ok();
        if let Some(pull) = &pull {
            let before = self.cached_row(repo.id, number).await?;
            self.store_pull(repo, pull, review_state(pull, &[]), None, before.as_ref())
                .await?;
        }

        if let Some(issue) = self.linked_issue(repo.id, number).await? {
            if issue.status_enum() == IssueStatus::InReview {
                let revision = merged["sha"]
                    .as_str()
                    .or_else(|| pull.as_ref().and_then(|p| p["merge_commit_sha"].as_str()))
                    .map(str::to_string);
                if let Some(revision) = revision {
                    let merged_at = pull.as_ref().and_then(|p| timestamp(&p["merged_at"]));
                    self.record_github_merge(issue.id, repo.id, &revision, merged_at)
                        .await?;
                }
            }
        }
        Ok(())
    }

    async fn linked_issue(&self, repo_id: Uuid, number: i64) -> Result<Option<Issue>> {
        Ok(sqlx::query_as::<_, Issue>(
            "select * from issues where repo_id = ? and pr_number = ? order by created_at desc limit 1",
        )
        .bind(repo_id)
        .bind(number)
        .fetch_optional(&self.db.pool)
        .await?)
    }

    // ------------------------------------------------------ the live diff

    /// Stores a working diff the runner reported for a live run and tells
    /// whoever is watching. A report for a run that already finished is
    /// dropped: its final diff has been published and nothing follows it.
    pub async fn record_live_diff(&self, run_id: Uuid, patch: &str) -> Result<()> {
        let run = self.run(run_id).await?;
        if !run.status_enum().is_live() {
            return Ok(());
        }
        let dir = Path::new(&run.artifact_dir);
        tokio::fs::create_dir_all(dir).await?;
        tokio::fs::write(dir.join("live.diff.patch"), patch).await?;
        self.publish_diff(run_id);
        Ok(())
    }

    /// Tells every watcher of a run that its diff changed or that it ended.
    /// Only the run's id travels: each watcher reads the diff itself, so the
    /// channel holds no patches and a watcher that fell behind loses nothing.
    /// Every path that ends a run calls this. Nobody watching is fine.
    pub fn publish_diff(&self, run_id: Uuid) {
        let _ = self.diffs.send(run_id);
    }

    /// The diff a watcher starts from: the latest working diff while the run
    /// is live, the final one once it finished. A run that ended without one
    /// (cancelled, declared dead) keeps showing its last working diff.
    pub async fn current_diff(&self, run_id: Uuid) -> Result<RunDiff> {
        let run = self.run(run_id).await?;
        let live = run.status_enum().is_live();
        let dir = Path::new(&run.artifact_dir);
        let mut file = dir.join(if live {
            "live.diff.patch"
        } else {
            "diff.patch"
        });
        if !live && !tokio::fs::try_exists(&file).await.unwrap_or(false) {
            file = dir.join("live.diff.patch");
        }
        let patch = tokio::fs::read_to_string(&file).await.unwrap_or_default();
        let updated_at = tokio::fs::metadata(&file)
            .await
            .and_then(|meta| meta.modified())
            .map(DateTime::<Utc>::from)
            .unwrap_or_else(|_| Utc::now());
        Ok(RunDiff {
            run_id,
            sha: crate::db::sha256_hex(patch.as_bytes()),
            patch,
            live,
            updated_at,
        })
    }

    /// Every diff of one run from now on: the current one first, then each
    /// change, ending after the final one. The run's status is also read on a
    /// slow clock, so a watcher ends even if a run-ending path never said so.
    pub async fn watch_diff(
        self: &std::sync::Arc<Self>,
        run_id: Uuid,
    ) -> Result<impl futures::Stream<Item = RunDiff> + Send + 'static> {
        // Subscribe before reading, so a change between the two is not lost.
        let receiver = self.diffs.subscribe();
        let current = self.current_diff(run_id).await?;
        let state = WatchState {
            engine: self.clone(),
            first: Some(current.clone()),
            receiver,
            last_sha: current.sha,
            done: false,
        };
        Ok(futures::stream::unfold(
            state,
            move |mut state| async move {
                if state.done {
                    return None;
                }
                if let Some(first) = state.first.take() {
                    state.done = !first.live;
                    return Some((first, state));
                }
                loop {
                    let look = tokio::select! {
                        signal = state.receiver.recv() => match signal {
                            Ok(id) => id == run_id,
                            // Fell behind: what was missed is on disk.
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => true,
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                        },
                        _ = tokio::time::sleep(WATCH_BACKSTOP) => true,
                    };
                    if !look {
                        continue;
                    }
                    let Ok(diff) = state.engine.current_diff(run_id).await else {
                        return None;
                    };
                    if diff.live && diff.sha == state.last_sha {
                        continue;
                    }
                    state.last_sha = diff.sha.clone();
                    state.done = !diff.live;
                    return Some((diff, state));
                }
            },
        ))
    }
}

/// How often a diff watcher checks the run itself, whatever it was told.
const WATCH_BACKSTOP: Duration = Duration::from_secs(30);

/// One pull request read fresh from GitHub, with its gate. See
/// [`Engine::fresh_pull`].
struct Fresh {
    pull: Value,
    reviews: Vec<Value>,
    runs: Vec<Value>,
    combined: Value,
    merge_methods: Vec<MergeMethod>,
    pull_request: PullRequest,
    gate: Gate,
}

struct WatchState {
    engine: std::sync::Arc<Engine>,
    first: Option<RunDiff>,
    receiver: tokio::sync::broadcast::Receiver<Uuid>,
    last_sha: String,
    done: bool,
}

/// The detail a sent review answers with when GitHub cannot be read back:
/// what was fetched just before, with the review's comments no longer pending.
fn review_sent_detail(fresh: Fresh) -> PullRequestDetail {
    let Fresh {
        pull,
        reviews,
        merge_methods,
        pull_request,
        gate,
        ..
    } = fresh;
    PullRequestDetail {
        pull_request,
        body: non_empty(&pull["body"]),
        files: Vec::new(),
        comments: Vec::new(),
        reviews: reviews
            .iter()
            .filter(|review| review["state"].as_str() != Some("PENDING"))
            .map(review_out)
            .collect(),
        checks: Vec::new(),
        merge_methods,
        last_merge_method: None,
        can_review: gate.can_review,
        review_blocked_reason: gate.review_blocked_reason,
        can_merge: gate.can_merge,
        merge_blocked_reason: gate.merge_blocked_reason,
    }
}

fn review_out(review: &Value) -> PullRequestReview {
    PullRequestReview {
        author_login: review["user"]["login"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        state: review["state"].as_str().unwrap_or_default().to_string(),
        body: non_empty(&review["body"]),
        submitted_at: parse_time(&review["submitted_at"]),
    }
}

fn comment_out(row: CommentRow, login: Option<&str>) -> PullRequestComment {
    PullRequestComment {
        id: ID(row.id.to_string()),
        path: row.path,
        line: row.line,
        side: DiffSide::parse(&row.side),
        body: row.body,
        author_login: login.unwrap_or("you").to_string(),
        created_at: row.created_at,
        pending: true,
        only_in_hulahoop: login.is_none(),
    }
}

#[cfg(test)]
mod tests {
    //! A fake GitHub on localhost stands in for the real one: each test gets its
    //! own, and its own engine pointed at it, so tests never share a base URL.

    use super::*;
    use crate::db::Db;
    use axum::extract::{Path as UrlPath, Query, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::{get, put};
    use axum::Json;
    use futures::StreamExt;
    use serde_json::json;
    use std::sync::Arc;

    const GOOD: &str = "github_pat_goodtoken1234";
    const ME: &str = "rez";

    #[derive(Default)]
    struct Fake {
        pulls: Vec<Value>,
        reviews: HashMap<i64, Vec<Value>>,
        checks: HashMap<String, Vec<Value>>,
        statuses: HashMap<String, Value>,
        posted_reviews: Vec<Value>,
        merges: Vec<Value>,
        refuse_reviews: bool,
        refuse_merges: bool,
        /// Pull requests GitHub answers with a server error for.
        broken: Vec<i64>,
    }

    type Shared = Arc<Mutex<Fake>>;
    type Reply = (StatusCode, Json<Value>);

    fn authorized(headers: &HeaderMap) -> bool {
        headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v == format!("Bearer {GOOD}"))
    }

    fn refused(status: StatusCode, message: &str) -> Reply {
        (status, Json(json!({ "message": message })))
    }

    fn known(owner: &str, repo: &str) -> bool {
        owner == "acme" && repo == "widgets"
    }

    async fn user(headers: HeaderMap) -> Reply {
        if !authorized(&headers) {
            return refused(StatusCode::UNAUTHORIZED, "Bad credentials");
        }
        (
            StatusCode::OK,
            Json(json!({ "login": ME, "name": "Rez", "avatar_url": "https://avatars/rez" })),
        )
    }

    async fn repo(headers: HeaderMap, UrlPath((owner, name)): UrlPath<(String, String)>) -> Reply {
        if !authorized(&headers) || !known(&owner, &name) {
            return refused(StatusCode::NOT_FOUND, "Not Found");
        }
        (
            StatusCode::OK,
            Json(json!({
                "full_name": format!("{owner}/{name}"),
                "permissions": { "pull": true, "push": true },
                "allow_merge_commit": true,
                "allow_squash_merge": true,
                "allow_rebase_merge": false,
            })),
        )
    }

    async fn pulls(
        State(fake): State<Shared>,
        headers: HeaderMap,
        UrlPath((owner, name)): UrlPath<(String, String)>,
        Query(query): Query<HashMap<String, String>>,
    ) -> Reply {
        if !authorized(&headers) || !known(&owner, &name) {
            return refused(StatusCode::NOT_FOUND, "Not Found");
        }
        let fake = fake.lock().unwrap();
        let state = query.get("state").map(String::as_str).unwrap_or("open");
        let page: usize = query.get("page").and_then(|p| p.parse().ok()).unwrap_or(1);
        if page > 1 {
            return (StatusCode::OK, Json(json!([])));
        }
        let listed: Vec<Value> = fake
            .pulls
            .iter()
            .filter(|p| state == "all" || p["state"] == state)
            .map(|p| {
                // The listing leaves the line counts out, as GitHub's does.
                let mut p = p.clone();
                if let Some(object) = p.as_object_mut() {
                    object.remove("additions");
                    object.remove("deletions");
                    object.remove("changed_files");
                }
                p
            })
            .collect();
        (StatusCode::OK, Json(Value::Array(listed)))
    }

    async fn pull(
        State(fake): State<Shared>,
        headers: HeaderMap,
        UrlPath((_, _, number)): UrlPath<(String, String, i64)>,
    ) -> Reply {
        if !authorized(&headers) {
            return refused(StatusCode::NOT_FOUND, "Not Found");
        }
        let fake = fake.lock().unwrap();
        if fake.broken.contains(&number) {
            return refused(StatusCode::INTERNAL_SERVER_ERROR, "Server Error");
        }
        match fake.pulls.iter().find(|p| p["number"] == number) {
            Some(p) => (StatusCode::OK, Json(p.clone())),
            None => refused(StatusCode::NOT_FOUND, "Not Found"),
        }
    }

    async fn files(UrlPath((_, _, _)): UrlPath<(String, String, i64)>) -> Reply {
        (
            StatusCode::OK,
            Json(json!([{
                "filename": "src/lib.rs", "status": "modified", "additions": 1, "deletions": 0,
                "patch": "@@ -1,1 +1,2 @@\n fn a() {}\n+fn b() {}"
            }])),
        )
    }

    async fn reviews(
        State(fake): State<Shared>,
        UrlPath((_, _, number)): UrlPath<(String, String, i64)>,
    ) -> Reply {
        let fake = fake.lock().unwrap();
        let list = fake.reviews.get(&number).cloned().unwrap_or_default();
        (StatusCode::OK, Json(Value::Array(list)))
    }

    async fn post_review(
        State(fake): State<Shared>,
        headers: HeaderMap,
        UrlPath((_, _, number)): UrlPath<(String, String, i64)>,
        Json(body): Json<Value>,
    ) -> Reply {
        if !authorized(&headers) {
            return refused(StatusCode::UNAUTHORIZED, "Bad credentials");
        }
        let mut fake = fake.lock().unwrap();
        if fake.refuse_reviews {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(
                    json!({ "message": "Unprocessable Entity", "errors": ["Review cannot be submitted"] }),
                ),
            );
        }
        fake.posted_reviews
            .push(json!({ "number": number, "body": body }));
        let state = match body["event"].as_str() {
            Some("APPROVE") => "APPROVED",
            _ => "CHANGES_REQUESTED",
        };
        fake.reviews.entry(number).or_default().push(json!({
            "user": { "login": ME }, "state": state, "body": body["body"],
            "submitted_at": "2026-01-02T00:00:00Z"
        }));
        (StatusCode::OK, Json(json!({ "id": 1, "state": state })))
    }

    async fn comments(UrlPath((_, _, _)): UrlPath<(String, String, i64)>) -> Reply {
        (
            StatusCode::OK,
            Json(json!([{
                "id": 7, "path": "src/lib.rs", "line": 2, "side": "RIGHT", "body": "nice",
                "user": { "login": "octo" }, "created_at": "2026-01-01T00:00:00Z"
            }])),
        )
    }

    async fn merge(
        State(fake): State<Shared>,
        headers: HeaderMap,
        UrlPath((_, _, number)): UrlPath<(String, String, i64)>,
        Json(body): Json<Value>,
    ) -> Reply {
        if !authorized(&headers) {
            return refused(StatusCode::UNAUTHORIZED, "Bad credentials");
        }
        let mut fake = fake.lock().unwrap();
        if fake.refuse_merges {
            return refused(
                StatusCode::METHOD_NOT_ALLOWED,
                "Pull Request is not mergeable",
            );
        }
        let head = fake
            .pulls
            .iter()
            .find(|p| p["number"] == number)
            .map(|p| p["head"]["sha"].clone());
        if body
            .get("sha")
            .is_some_and(|sha| Some(sha) != head.as_ref())
        {
            return refused(
                StatusCode::CONFLICT,
                "Head branch was modified. Review and try the merge again.",
            );
        }
        fake.merges.push(json!({ "number": number, "body": body }));
        if let Some(p) = fake.pulls.iter_mut().find(|p| p["number"] == number) {
            // GitHub moves `updated_at` on a merge, past any listing read before it.
            p["updated_at"] =
                json!((chrono::Utc::now() + chrono::Duration::seconds(5)).to_rfc3339());
            p["state"] = json!("closed");
            p["merged"] = json!(true);
            p["merged_at"] = json!("2026-01-03T00:00:00Z");
            p["merge_commit_sha"] = json!("feedfacecafe0123456789");
        }
        (
            StatusCode::OK,
            Json(json!({ "merged": true, "sha": "feedfacecafe0123456789" })),
        )
    }

    async fn check_runs(
        State(fake): State<Shared>,
        UrlPath((_, _, sha)): UrlPath<(String, String, String)>,
    ) -> Reply {
        let fake = fake.lock().unwrap();
        let runs = fake.checks.get(&sha).cloned().unwrap_or_default();
        (
            StatusCode::OK,
            Json(json!({ "total_count": runs.len(), "check_runs": runs })),
        )
    }

    async fn status(
        State(fake): State<Shared>,
        UrlPath((_, _, sha)): UrlPath<(String, String, String)>,
    ) -> Reply {
        let fake = fake.lock().unwrap();
        let status = fake
            .statuses
            .get(&sha)
            .cloned()
            .unwrap_or(json!({ "state": "pending", "statuses": [] }));
        (StatusCode::OK, Json(status))
    }

    async fn fake_github(fake: Shared) -> String {
        let app = axum::Router::new()
            .route("/user", get(user))
            .route("/repos/{owner}/{repo}", get(repo))
            .route("/repos/{owner}/{repo}/pulls", get(pulls))
            .route("/repos/{owner}/{repo}/pulls/{number}", get(pull))
            .route("/repos/{owner}/{repo}/pulls/{number}/files", get(files))
            .route(
                "/repos/{owner}/{repo}/pulls/{number}/reviews",
                get(reviews).post(post_review),
            )
            .route(
                "/repos/{owner}/{repo}/pulls/{number}/comments",
                get(comments),
            )
            .route("/repos/{owner}/{repo}/pulls/{number}/merge", put(merge))
            .route(
                "/repos/{owner}/{repo}/commits/{sha}/check-runs",
                get(check_runs),
            )
            .route("/repos/{owner}/{repo}/commits/{sha}/status", get(status))
            .with_state(fake);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{address}")
    }

    fn pull_json(number: i64, author: &str, sha: &str) -> Value {
        json!({
            "number": number,
            "title": format!("Pull {number}"),
            "html_url": format!("https://github.com/acme/widgets/pull/{number}"),
            "state": "open",
            "draft": false,
            "merged": false,
            "merged_at": null,
            "closed_at": null,
            "mergeable": true,
            "body": "What it does",
            "user": { "login": author, "avatar_url": null },
            "head": { "ref": format!("hul-{number}"), "sha": sha },
            "base": { "ref": "main" },
            "additions": 3,
            "deletions": 1,
            "changed_files": 1,
            "requested_reviewers": [],
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": chrono::Utc::now().to_rfc3339(),
        })
    }

    struct World {
        engine: Arc<Engine>,
        fake: Shared,
        repo: Uuid,
        workspace: Uuid,
        project: Uuid,
        directory: std::path::PathBuf,
    }

    async fn world() -> World {
        let directory = std::env::temp_dir().join(format!("hulahoop-pulls-{}", Uuid::now_v7()));
        let db = Db::open(&directory).await.unwrap();
        let workspace = db.seed_local().await.unwrap();
        let (project, repo, locked) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let now = Utc::now();
        sqlx::query(
            "insert into projects (id, workspace_id, name, created_at) values (?,?, 'Widgets',?)",
        )
        .bind(project)
        .bind(workspace)
        .bind(now)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query("insert into repos (id, project_id, name, remote_url, contract_version, created_at) values (?,?, 'widgets','https://github.com/acme/widgets.git','1',?)")
            .bind(repo).bind(project).bind(now).execute(&db.pool).await.unwrap();
        sqlx::query("insert into repos (id, project_id, name, remote_url, contract_version, created_at) values (?,?, 'locked','git@github.com:acme/locked.git','1',?)")
            .bind(locked).bind(project).bind(now + chrono::Duration::seconds(1)).execute(&db.pool).await.unwrap();

        let fake: Shared = Arc::default();
        let base = fake_github(fake.clone()).await;
        let mut engine = Engine::new(db);
        engine.github_api = base;
        World {
            engine: Arc::new(engine),
            fake,
            repo,
            workspace,
            project,
            directory,
        }
    }

    impl Drop for World {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    impl World {
        /// An approving review from someone else, so the gate opens.
        fn approve(&self, number: i64) {
            self.fake.lock().unwrap().reviews.insert(
                number,
                vec![json!({ "user": { "login": "octo" }, "state": "APPROVED" })],
            );
        }

        /// A run of this task, `queued` or `running`, with its artifacts in the
        /// world's directory.
        async fn run(&self, issue: Uuid, status: &str) -> Uuid {
            let (agent, run) = (Uuid::now_v7(), Uuid::now_v7());
            let now = Utc::now();
            let artifacts = self.directory.join(format!("run-{run}"));
            sqlx::query("insert into agents (id, workspace_id, name, adapter, model, created_at, updated_at) values (?,?,?,'pi','model-x',?,?)")
                .bind(agent).bind(self.workspace).bind(format!("builder-{agent}")).bind(now).bind(now)
                .execute(&self.engine.db.pool).await.unwrap();
            sqlx::query("insert into runs (id, issue_id, repo_id, agent_id, attempt, base_bookmark, prompt_revision, artifact_dir, status, model, created_at) values (?,?,?,?,1,'main','prompt',?,?,'model-x',?)")
                .bind(run).bind(issue).bind(self.repo).bind(agent)
                .bind(artifacts.to_string_lossy().to_string()).bind(status).bind(now)
                .execute(&self.engine.db.pool).await.unwrap();
            run
        }

        async fn task(&self, status: &str, pr_number: i64) -> Uuid {
            let id = Uuid::now_v7();
            let now = Utc::now();
            sqlx::query("insert into issues (id, workspace_id, project_id, repo_id, issue_key, number, title, status, pr_number, created_at, updated_at) values (?,?,?,?, 'WID-1',1,'Task',?,?,?,?)")
                .bind(id).bind(self.workspace).bind(self.project).bind(self.repo)
                .bind(status).bind(pr_number).bind(now).bind(now)
                .execute(&self.engine.db.pool).await.unwrap();
            id
        }

        async fn status_of(&self, issue: Uuid) -> String {
            self.engine.issue(issue).await.unwrap().status
        }
    }

    #[tokio::test]
    async fn a_token_is_checked_and_what_it_cannot_do_is_listed() {
        let world = world().await;
        let engine = &world.engine;

        let refused = engine.set_github_token("ghp_wrongwrongwrong").await;
        let message = refused.unwrap_err().to_string();
        assert!(message.contains("Bad credentials"), "{message}");
        assert!(
            !message.contains("ghp_wrong"),
            "the token leaked: {message}"
        );
        assert!(engine.github_account().await.unwrap().is_none());

        let account = engine.set_github_token(GOOD).await.unwrap();
        assert_eq!(account.login, ME);
        assert_eq!(account.name.as_deref(), Some("Rez"));
        assert_eq!(account.token_hint, "github_pat_…1234");
        assert_eq!(account.problems, vec!["cannot see acme/locked".to_string()]);
        assert_eq!(engine.github_account().await.unwrap(), Some(account));

        // The token is sealed, never stored as written.
        let stored: String = sqlx::query_scalar("select secret_ciphertext from personal_tokens")
            .fetch_one(&engine.db.pool)
            .await
            .unwrap();
        assert!(!stored.contains(GOOD));

        assert!(engine.clear_github_token().await.unwrap());
        assert!(engine.github_account().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_sync_fills_the_cache_with_review_and_ci_state_and_links_tasks() {
        let world = world().await;
        {
            let mut fake = world.fake.lock().unwrap();
            fake.pulls = vec![
                pull_json(1, "hulahoop-dev[bot]", "sha1"),
                pull_json(2, "octo", "sha2"),
                pull_json(3, ME, "sha3"),
            ];
            fake.pulls[1]["requested_reviewers"] = json!([{ "login": ME }]);
            fake.reviews.insert(
                1,
                vec![
                    json!({ "user": { "login": "octo" }, "state": "CHANGES_REQUESTED" }),
                    json!({ "user": { "login": "octo" }, "state": "COMMENTED" }),
                    json!({ "user": { "login": "octo" }, "state": "APPROVED" }),
                ],
            );
            fake.checks.insert(
                "sha1".into(),
                vec![json!({ "name": "ci", "status": "completed", "conclusion": "success" })],
            );
            fake.checks.insert(
                "sha2".into(),
                vec![json!({ "name": "ci", "status": "completed", "conclusion": "failure" })],
            );
            fake.checks.insert(
                "sha3".into(),
                vec![json!({ "name": "ci", "status": "in_progress", "conclusion": null })],
            );
        }
        let issue = world.task("in_review", 1).await;
        world.engine.set_github_token(GOOD).await.unwrap();
        world.engine.sync_pull_requests(None).await.unwrap();

        let all = world
            .engine
            .pull_requests(PullRequestFilter::Open)
            .await
            .unwrap();
        assert_eq!(all.len(), 3);
        let first = all.iter().find(|p| p.number == 1).unwrap();
        assert_eq!(first.review_state, ReviewState::Approved);
        assert_eq!(first.ci_state, CiState::Passing);
        assert!(first.by_agent);
        assert_eq!(first.issue_id, Some(issue));
        assert_eq!(
            first.additions, 3,
            "line counts come from the pull request itself"
        );
        assert_eq!(first.repo_name, "acme/widgets");
        assert_eq!(first.id.as_str(), format!("{}#1", world.repo));
        let second = all.iter().find(|p| p.number == 2).unwrap();
        assert_eq!(second.ci_state, CiState::Failing);
        assert!(second.review_requested_from_me);
        assert!(!second.by_agent);
        let third = all.iter().find(|p| p.number == 3).unwrap();
        assert_eq!(third.ci_state, CiState::Running);

        let numbers = |list: Vec<PullRequest>| list.iter().map(|p| p.number).collect::<Vec<_>>();
        let mut review = numbers(
            world
                .engine
                .pull_requests(PullRequestFilter::NeedsMyReview)
                .await
                .unwrap(),
        );
        review.sort();
        assert_eq!(review, vec![2]);
        assert_eq!(
            numbers(
                world
                    .engine
                    .pull_requests(PullRequestFilter::Mine)
                    .await
                    .unwrap()
            ),
            vec![3]
        );
        assert_eq!(
            numbers(
                world
                    .engine
                    .pull_requests(PullRequestFilter::ByAgents)
                    .await
                    .unwrap()
            ),
            vec![1]
        );
        assert_eq!(
            numbers(
                world
                    .engine
                    .pull_requests(PullRequestFilter::FailingCi)
                    .await
                    .unwrap()
            ),
            vec![2]
        );
        assert_eq!(world.engine.open_pull_request_count().await.unwrap(), 3);
        assert!(world
            .engine
            .pull_requests_synced_at()
            .await
            .unwrap()
            .is_some());
        // The repository the token cannot reach is a problem, not a failure.
        assert_eq!(
            world
                .engine
                .pull_request_sync_problems()
                .await
                .unwrap()
                .len(),
            1,
        );
        assert!(world.engine.pull_request_sync_problems().await.unwrap()[0].contains("acme/locked"));

        // Through GraphQL, the task link resolves.
        let schema = crate::gql::build_schema(world.engine.clone());
        let response = schema
            .execute("{ pullRequests(filter: BY_AGENTS) { number byAgent reviewState ciState issue { key status } liveRun { id } } openPullRequestCount }")
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = response.data.into_json().unwrap();
        assert_eq!(data["pullRequests"][0]["issue"]["status"], "in_review");
        assert_eq!(data["pullRequests"][0]["reviewState"], "APPROVED");
        assert_eq!(data["openPullRequestCount"], 3);
    }

    #[tokio::test]
    async fn merge_and_review_buttons_say_why_they_are_off() {
        let world = world().await;
        {
            let mut fake = world.fake.lock().unwrap();
            fake.pulls = vec![
                pull_json(1, "hulahoop-dev[bot]", "sha1"),
                pull_json(2, ME, "sha2"),
            ];
            fake.reviews.insert(
                1,
                vec![json!({ "user": { "login": "octo" }, "state": "APPROVED" })],
            );
            fake.checks.insert(
                "sha1".into(),
                vec![json!({ "name": "ci", "status": "completed", "conclusion": "failure" })],
            );
        }

        // No token: everything is off for the same reason, and reading uses
        // nothing, so the detail cannot be fetched either.
        assert!(world
            .engine
            .pull_request_detail(world.repo, 1)
            .await
            .is_err());

        world.engine.set_github_token(GOOD).await.unwrap();
        let detail = world
            .engine
            .pull_request_detail(world.repo, 1)
            .await
            .unwrap()
            .unwrap();
        assert!(detail.can_review);
        assert!(!detail.can_merge);
        assert_eq!(
            detail.merge_blocked_reason.as_deref(),
            Some("CI is failing")
        );
        assert_eq!(
            detail.merge_methods,
            vec![MergeMethod::Merge, MergeMethod::Squash]
        );
        assert_eq!(detail.files.len(), 1);
        assert_eq!(detail.comments.len(), 1);
        assert_eq!(detail.checks.len(), 1);

        let own = world
            .engine
            .pull_request_detail(world.repo, 2)
            .await
            .unwrap()
            .unwrap();
        assert!(!own.can_review);
        assert_eq!(own.review_blocked_reason.as_deref(), Some(OWN_PULL_REASON));
        assert!(
            own.can_merge,
            "your own pull request needs no approval: {:?}",
            own.merge_blocked_reason
        );

        // The pure gate, rule by rule.
        let mut pull = detail.pull_request.clone();
        pull.ci_state = CiState::None;
        let methods = [MergeMethod::Squash];
        let open = gate(Some(ME), &pull, Some(true), &methods, None);
        assert!(open.can_merge, "no checks at all counts as passing");
        let none = gate(None, &pull, Some(true), &methods, None);
        assert_eq!(none.review_blocked_reason.as_deref(), Some(NO_TOKEN_REASON));
        assert_eq!(none.merge_blocked_reason.as_deref(), Some(NO_TOKEN_REASON));
        pull.draft = true;
        assert_eq!(
            gate(Some(ME), &pull, Some(true), &methods, None)
                .merge_blocked_reason
                .as_deref(),
            Some("This pull request is still a draft")
        );
        pull.draft = false;
        pull.review_state = ReviewState::ChangesRequested;
        assert!(!gate(Some(ME), &pull, Some(true), &methods, None).can_merge);
        pull.review_state = ReviewState::Approved;
        pull.ci_state = CiState::Running;
        assert_eq!(
            gate(Some(ME), &pull, Some(true), &methods, None)
                .merge_blocked_reason
                .as_deref(),
            Some("CI is still running")
        );
        pull.ci_state = CiState::Passing;
        assert!(!gate(Some(ME), &pull, Some(false), &methods, None).can_merge);
        assert!(!gate(Some(ME), &pull, Some(true), &[], None).can_merge);
        pull.merged = true;
        assert_eq!(
            gate(Some(ME), &pull, Some(true), &methods, None)
                .review_blocked_reason
                .as_deref(),
            Some("This pull request is already merged")
        );
    }

    #[tokio::test]
    async fn comments_without_a_token_stay_in_hulahoop() {
        let world = world().await;
        world.fake.lock().unwrap().pulls = vec![pull_json(1, "hulahoop-dev[bot]", "sha1")];
        let comment = world
            .engine
            .add_pull_request_comment(world.repo, 1, "src/lib.rs", 2, DiffSide::Right, "why?")
            .await
            .unwrap();
        assert!(comment.pending && comment.only_in_hulahoop);
        assert_eq!(comment.author_login, "you");
        let refused = world
            .engine
            .review_pull_request(world.repo, 1, ReviewEvent::Approve, Some("ok"))
            .await
            .unwrap_err();
        assert_eq!(refused.to_string(), NO_TOKEN_REASON);
        assert!(world
            .engine
            .delete_pull_request_comment(&comment.id)
            .await
            .unwrap());
        assert!(world
            .engine
            .delete_pull_request_comment("github-7")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn requesting_changes_sends_the_comments_and_reopens_the_task() {
        let world = world().await;
        world.fake.lock().unwrap().pulls = vec![pull_json(1, "hulahoop-dev[bot]", "sha1")];
        let issue = world.task("in_review", 1).await;
        world.engine.set_github_token(GOOD).await.unwrap();
        world
            .engine
            .add_pull_request_comment(
                world.repo,
                1,
                "src/lib.rs",
                2,
                DiffSide::Right,
                "rename this",
            )
            .await
            .unwrap();

        // A comment on another pull request is not part of this review.
        world
            .engine
            .add_pull_request_comment(world.repo, 9, "a", 1, DiffSide::Left, "x")
            .await
            .unwrap();

        // GitHub refuses: the task stays in review and the comment stays pending.
        world.fake.lock().unwrap().refuse_reviews = true;
        let refused = world
            .engine
            .review_pull_request(
                world.repo,
                1,
                ReviewEvent::RequestChanges,
                Some("Please fix"),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(refused.contains("Review cannot be submitted"), "{refused}");
        assert_eq!(world.status_of(issue).await, "in_review");
        let pending: i64 =
            sqlx::query_scalar("select count(*) from pull_request_comments where number = 1")
                .fetch_one(&world.engine.db.pool)
                .await
                .unwrap();
        assert_eq!(pending, 1);

        world.fake.lock().unwrap().refuse_reviews = false;
        let detail = world
            .engine
            .review_pull_request(
                world.repo,
                1,
                ReviewEvent::RequestChanges,
                Some("Please fix"),
            )
            .await
            .unwrap();
        let posted = world.fake.lock().unwrap().posted_reviews.clone();
        assert_eq!(posted.len(), 1);
        assert_eq!(posted[0]["body"]["event"], "REQUEST_CHANGES");
        assert_eq!(posted[0]["body"]["body"], "Please fix");
        assert_eq!(
            posted[0]["body"]["comments"],
            json!([{ "path": "src/lib.rs", "line": 2, "side": "RIGHT", "body": "rename this" }])
        );
        assert!(detail.comments.iter().all(|c| !c.pending));
        assert_eq!(
            detail.pull_request.review_state,
            ReviewState::ChangesRequested
        );
        assert_eq!(world.status_of(issue).await, "in_progress");
        let note: String = sqlx::query_scalar(
            "select body from comments where issue_id = ? and author_type = 'human' order by created_at desc limit 1",
        )
        .bind(issue)
        .fetch_one(&world.engine.db.pool)
        .await
        .unwrap();
        assert!(
            note.contains("Please fix") && note.contains("rename this"),
            "{note}"
        );

        // An empty review is refused.
        let empty = world
            .engine
            .review_pull_request(world.repo, 1, ReviewEvent::Approve, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            empty.contains("a note or at least one line comment"),
            "{empty}"
        );
    }

    #[tokio::test]
    async fn merging_a_linked_pull_request_closes_its_task() {
        let world = world().await;
        world.fake.lock().unwrap().pulls = vec![pull_json(1, "hulahoop-dev[bot]", "sha1")];
        world.approve(1);
        let issue = world.task("in_review", 1).await;
        world.engine.set_github_token(GOOD).await.unwrap();

        let merged = world
            .engine
            .merge_pull_request(world.repo, 1, MergeMethod::Squash, Some("sha1"))
            .await
            .unwrap();
        assert!(merged.merged);
        assert_eq!(merged.review_state, ReviewState::Merged);
        let sent = world.fake.lock().unwrap().merges[0]["body"].clone();
        assert_eq!(sent["merge_method"], "squash");
        // The merge is pinned to the head the person saw.
        assert_eq!(sent["sha"], "sha1");

        let task = world.engine.issue(issue).await.unwrap();
        assert_eq!(task.status, "done");
        assert_eq!(
            task.merge_revision.as_deref(),
            Some("feedfacecafe0123456789")
        );
        let base: Option<String> =
            sqlx::query_scalar("select base_revision from repos where id = ?")
                .bind(world.repo)
                .fetch_one(&world.engine.db.pool)
                .await
                .unwrap();
        assert_eq!(base.as_deref(), Some("feedfacecafe0123456789"));
        let last: Option<String> = sqlx::query_scalar(
            "select last_merge_method from pull_request_repos where repo_id = ?",
        )
        .bind(world.repo)
        .fetch_one(&world.engine.db.pool)
        .await
        .unwrap();
        assert_eq!(last.as_deref(), Some("squash"));
        assert_eq!(
            world
                .engine
                .pull_requests(PullRequestFilter::Merged)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn run_diff_streams_the_current_diff_then_changes_then_the_final_one() {
        let world = world().await;
        let issue = world.task("in_progress", 1).await;
        let run = world.run(issue, "running").await;

        world.engine.record_live_diff(run, "first\n").await.unwrap();

        let schema = crate::gql::build_schema(world.engine.clone());
        let mut stream = schema.execute_stream(format!(
            "subscription {{ runDiff(runId: \"{run}\") {{ runId patch sha live updatedAt }} }}"
        ));
        let next = |response: async_graphql::Response| {
            assert!(response.errors.is_empty(), "{:?}", response.errors);
            response.data.into_json().unwrap()["runDiff"].clone()
        };
        let current = next(stream.next().await.unwrap());
        assert_eq!(current["patch"], "first\n");
        assert_eq!(current["live"], true);

        // A repeat is not news; a change is.
        world.engine.record_live_diff(run, "first\n").await.unwrap();
        world
            .engine
            .record_live_diff(run, "second\n")
            .await
            .unwrap();
        let update = next(stream.next().await.unwrap());
        assert_eq!(update["patch"], "second\n");
        assert_eq!(update["sha"], crate::db::sha256_hex(b"second\n"));

        let completion: crate::engine::RunCompletion =
            serde_json::from_value(json!({ "status": "failed", "diff": "final\n" })).unwrap();
        world.engine.complete_run(run, completion).await.unwrap();
        let last = next(stream.next().await.unwrap());
        assert_eq!(last["patch"], "final\n");
        assert_eq!(last["live"], false);
        assert!(
            stream.next().await.is_none(),
            "the stream ends after the final diff"
        );

        // A watcher arriving late gets the final diff and nothing more, and a
        // runner that reports after the end is ignored.
        world.engine.record_live_diff(run, "late\n").await.unwrap();
        let mut late = schema.execute_stream(format!(
            "subscription {{ runDiff(runId: \"{run}\") {{ patch live }} }}"
        ));
        let only = next(late.next().await.unwrap());
        assert_eq!(only["patch"], "final\n");
        assert_eq!(only["live"], false);
        assert!(late.next().await.is_none());
    }

    async fn merge_refusal(world: &World, method: MergeMethod, sha: Option<&str>) -> String {
        world
            .engine
            .merge_pull_request(world.repo, 1, method, sha)
            .await
            .unwrap_err()
            .to_string()
    }

    async fn set_status(world: &World, issue: Uuid, status: &str) {
        sqlx::query("update issues set status = ? where id = ?")
            .bind(status)
            .bind(issue)
            .execute(&world.engine.db.pool)
            .await
            .unwrap();
    }

    /// The desktop greys the button out, but the server decides: a merge the
    /// gate refuses never reaches GitHub, whatever the caller sends.
    #[tokio::test]
    async fn the_server_refuses_a_merge_the_gate_does_not_allow() {
        let world = world().await;
        world.fake.lock().unwrap().pulls = vec![pull_json(1, "hulahoop-dev[bot]", "sha1")];
        let issue = world.task("in_review", 1).await;
        world.engine.set_github_token(GOOD).await.unwrap();

        let refused = merge_refusal(&world, MergeMethod::Squash, None).await;
        assert_eq!(refused, "Waiting for an approving review");

        world.approve(1);
        let run = world.run(issue, "running").await;
        let refused = merge_refusal(&world, MergeMethod::Squash, None).await;
        assert_eq!(refused, "An agent is still working on WID-1");
        sqlx::query("update runs set status = 'succeeded' where id = ?")
            .bind(run)
            .execute(&world.engine.db.pool)
            .await
            .unwrap();

        set_status(&world, issue, "in_progress").await;
        let refused = merge_refusal(&world, MergeMethod::Squash, None).await;
        assert_eq!(refused, "Move WID-1 to In review before merging");
        set_status(&world, issue, "in_review").await;

        // The repository does not allow rebase merges.
        let refused = merge_refusal(&world, MergeMethod::Rebase, None).await;
        assert!(
            refused.contains("does not allow rebase merges"),
            "{refused}"
        );

        // New commits since the person looked: refused before GitHub is asked.
        let refused = merge_refusal(&world, MergeMethod::Squash, Some("sha0")).await;
        assert!(
            refused.contains("New commits were pushed to #1"),
            "{refused}"
        );

        assert!(world.fake.lock().unwrap().merges.is_empty());
        assert_eq!(world.status_of(issue).await, "in_review");

        // A task that is already done does not stand in the way.
        set_status(&world, issue, "done").await;
        let merged = world
            .engine
            .merge_pull_request(world.repo, 1, MergeMethod::Merge, None)
            .await
            .unwrap();
        assert!(merged.merged);
        assert_eq!(world.fake.lock().unwrap().merges[0]["body"]["sha"], "sha1");
    }

    #[tokio::test]
    async fn a_merge_github_refuses_leaves_the_task_alone() {
        let world = world().await;
        world.fake.lock().unwrap().pulls = vec![pull_json(1, "hulahoop-dev[bot]", "sha1")];
        world.approve(1);
        let issue = world.task("in_review", 1).await;
        world.engine.set_github_token(GOOD).await.unwrap();
        world.fake.lock().unwrap().refuse_merges = true;

        let refused = merge_refusal(&world, MergeMethod::Squash, Some("sha1")).await;
        assert!(refused.contains("not mergeable"), "{refused}");
        let task = world.engine.issue(issue).await.unwrap();
        assert_eq!(task.status, "in_review");
        assert_eq!(task.merge_revision, None);
        let cached = world
            .engine
            .load_pull(world.repo, 1)
            .await
            .unwrap()
            .unwrap();
        assert!(!cached.merged);
        let last: Option<String> = sqlx::query_scalar(
            "select last_merge_method from pull_request_repos where repo_id = ?",
        )
        .bind(world.repo)
        .fetch_optional(&world.engine.db.pool)
        .await
        .unwrap()
        .flatten();
        assert_eq!(last, None);
    }

    /// A sync that read GitHub before a merge finishes after it: what it read
    /// is older than the merge and must not write the pull request back open.
    #[tokio::test]
    async fn a_sync_read_before_a_merge_does_not_reopen_it() {
        let world = world().await;
        world.fake.lock().unwrap().pulls = vec![pull_json(1, "hulahoop-dev[bot]", "sha1")];
        world.approve(1);
        world.engine.set_github_token(GOOD).await.unwrap();
        world.engine.sync_pull_requests(None).await.unwrap();
        let read_before = world.fake.lock().unwrap().pulls[0].clone();

        world
            .engine
            .merge_pull_request(world.repo, 1, MergeMethod::Squash, None)
            .await
            .unwrap();
        let repo = world.engine.linked_repo(world.repo).await.unwrap().unwrap();
        world
            .engine
            .store_pull(&repo, &read_before, ReviewState::Approved, None, None)
            .await
            .unwrap();
        let cached = world
            .engine
            .load_pull(world.repo, 1)
            .await
            .unwrap()
            .unwrap();
        assert!(
            cached.merged,
            "the stale read reopened the merged pull request"
        );
        assert_eq!(cached.review_state, ReviewState::Merged);

        // Even a read stamped the same second as the merge does not undo it.
        let mut same_second = read_before.clone();
        same_second["updated_at"] = json!(cached.updated_at.to_rfc3339());
        world
            .engine
            .store_pull(&repo, &same_second, ReviewState::Approved, None, None)
            .await
            .unwrap();
        assert!(
            world
                .engine
                .load_pull(world.repo, 1)
                .await
                .unwrap()
                .unwrap()
                .merged
        );
    }

    /// A pull request missing from the open listing is only dropped when GitHub
    /// says it is gone, never because GitHub had a bad moment.
    #[tokio::test]
    async fn a_cached_pull_request_is_only_dropped_when_github_says_it_is_gone() {
        let world = world().await;
        world.fake.lock().unwrap().pulls = vec![
            pull_json(1, "hulahoop-dev[bot]", "sha1"),
            pull_json(2, "octo", "sha2"),
        ];
        world.engine.set_github_token(GOOD).await.unwrap();
        world.engine.sync_pull_requests(None).await.unwrap();
        let open = |world: &World| {
            let engine = world.engine.clone();
            async move {
                engine
                    .pull_requests(PullRequestFilter::Open)
                    .await
                    .unwrap()
                    .len()
            }
        };
        assert_eq!(open(&world).await, 2);

        {
            let mut fake = world.fake.lock().unwrap();
            fake.pulls.retain(|p| p["number"] == 1);
            fake.broken = vec![2];
        }
        world.engine.sync_pull_requests(None).await.unwrap();
        assert_eq!(
            open(&world).await,
            2,
            "a server error dropped a cached pull request"
        );

        world.fake.lock().unwrap().broken.clear();
        world.engine.sync_pull_requests(None).await.unwrap();
        assert_eq!(open(&world).await, 1);
    }

    #[tokio::test]
    async fn a_review_the_gate_does_not_allow_is_refused_before_github() {
        let world = world().await;
        world.fake.lock().unwrap().pulls = vec![pull_json(1, ME, "sha1")];
        world.engine.set_github_token(GOOD).await.unwrap();
        let refused = world
            .engine
            .review_pull_request(world.repo, 1, ReviewEvent::Approve, Some("ship it"))
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(refused, OWN_PULL_REASON);
        assert!(world.fake.lock().unwrap().posted_reviews.is_empty());
    }

    /// Whichever way a run ends, a watcher gets the final diff and the stream
    /// ends: no watcher is left waiting on a run that is over.
    #[tokio::test]
    async fn run_diff_ends_when_a_run_is_cancelled_or_reaped() {
        let world = world().await;
        let issue = world.task("in_progress", 1).await;
        let schema = crate::gql::build_schema(world.engine.clone());
        let watch = |run: Uuid| {
            schema.execute_stream(format!(
                "subscription {{ runDiff(runId: \"{run}\") {{ patch live }} }}"
            ))
        };
        let next = |response: async_graphql::Response| {
            assert!(response.errors.is_empty(), "{:?}", response.errors);
            response.data.into_json().unwrap()["runDiff"].clone()
        };
        async fn within<F: std::future::Future>(what: F) -> F::Output {
            tokio::time::timeout(std::time::Duration::from_secs(5), what)
                .await
                .expect("the watcher was left waiting")
        }

        let cancelled = world.run(issue, "running").await;
        world
            .engine
            .record_live_diff(cancelled, "working\n")
            .await
            .unwrap();
        let mut stream = watch(cancelled);
        assert_eq!(next(stream.next().await.unwrap())["live"], true);
        world.engine.cancel_run(cancelled).await.unwrap();
        let last = next(within(stream.next()).await.unwrap());
        assert_eq!(last["live"], false);
        assert_eq!(last["patch"], "working\n");
        assert!(within(stream.next()).await.is_none());

        let reaped = world.run(issue, "running").await;
        world
            .engine
            .record_live_diff(reaped, "dying\n")
            .await
            .unwrap();
        let mut stream = watch(reaped);
        assert_eq!(next(stream.next().await.unwrap())["live"], true);
        let dead = world.engine.reap_dead_runs().await.unwrap();
        assert!(dead.iter().any(|run| run.id == reaped));
        let last = next(within(stream.next()).await.unwrap());
        assert_eq!(last["live"], false);
        assert!(within(stream.next()).await.is_none());
    }

    /// A watcher that falls behind the other runs' signals still sees its own
    /// run's change: falling behind means "look again", never "skip".
    #[tokio::test]
    async fn a_watcher_that_falls_behind_reads_the_diff_again() {
        let world = world().await;
        let issue = world.task("in_progress", 1).await;
        let run = world.run(issue, "running").await;
        world.engine.record_live_diff(run, "first\n").await.unwrap();
        let schema = crate::gql::build_schema(world.engine.clone());
        let mut stream = schema.execute_stream(format!(
            "subscription {{ runDiff(runId: \"{run}\") {{ patch live }} }}"
        ));
        let first = stream.next().await.unwrap().data.into_json().unwrap();
        assert_eq!(first["runDiff"]["patch"], "first\n");

        // This run's signal is pushed out of the channel by everyone else's.
        world
            .engine
            .record_live_diff(run, "second\n")
            .await
            .unwrap();
        for _ in 0..1000 {
            world.engine.publish_diff(Uuid::now_v7());
        }
        let update = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .expect("the change was lost")
            .unwrap()
            .data
            .into_json()
            .unwrap();
        assert_eq!(update["runDiff"]["patch"], "second\n");
        assert_eq!(update["runDiff"]["live"], true);
    }

    #[test]
    fn token_hints_show_the_kind_and_the_last_four() {
        assert_eq!(
            token_hint("github_pat_11ABCDEFG0123a1B2"),
            "github_pat_…a1B2"
        );
        assert_eq!(token_hint("ghp_abcdefghijklmnop"), "ghp_…mnop");
        assert_eq!(token_hint("short"), "…");
    }
}
