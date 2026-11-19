//! Business rules: every mutation and every runner callback lands here, so the
//! state machine and the gate live in one place.

use crate::connectors::Credential;
use crate::db::{json_list, kill_process_group, sha256_hex, Db};
use crate::github::StoredApp;
use crate::model::ConnectorRow;
use crate::model::*;
use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use hulahoop_core::contract;
use hulahoop_core::diff;
use hulahoop_core::junit;
use hulahoop_core::{AgentRole, IssueEvent, IssueStatus, ReplayKind, RunFlag, RunStatus};
use uuid::Uuid;

/// Words that fill a sentence without naming anything. A criterion built only from these
/// says "it works", which is true of every program and assertable by no test.
const FILLER: &[&str] = &[
    "it",
    "is",
    "are",
    "the",
    "a",
    "an",
    "works",
    "work",
    "fine",
    "good",
    "correct",
    "ok",
    "okay",
    "should",
    "will",
    "be",
    "able",
    "to",
    "properly",
    "correctly",
    "well",
    "and",
    "this",
    "that",
    "must",
    "do",
    "does",
    "not",
    "crash",
    "fast",
    "handles",
    "errors",
];

/// Future-tense intent: true of a product goal, and true of nothing a test can read.
const GOAL_PHRASES: &[&str] = &[
    "be able to",
    "as a user",
    "as an user",
    "i want",
    "should work",
    "works well",
];

/// The provider's own refusal, when the adapter recorded one. pi reports a rejected
/// model as a transcript line and an exit code of 0, which is why this looks at the
/// text instead of the status.
fn adapter_failure(payload: &RunCompletion) -> Option<String> {
    let transcript = payload.transcript.as_deref().unwrap_or_default();
    let marker = "\"finalError\":\"";
    let start = transcript.rfind(marker)? + marker.len();
    let rest = &transcript[start..];
    let end = rest.find("\"}").or_else(|| rest.find('"'))?;
    let reason = rest[..end].trim();
    if reason.is_empty() {
        return None;
    }
    Some(reason.to_string())
}

pub struct Engine {
    pub db: Db,
}

impl Engine {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    // ---------------------------------------------------------------- issues

    pub async fn issue(&self, id: Uuid) -> Result<Issue> {
        sqlx::query_as::<_, Issue>("select * from issues where id = ?")
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await?
            .ok_or_else(|| anyhow!("issue {id} not found"))
    }

    pub async fn criteria(&self, issue_id: Uuid) -> Result<Vec<Criterion>> {
        Ok(sqlx::query_as::<_, Criterion>(
            "select * from criteria where issue_id = ? order by position",
        )
        .bind(issue_id)
        .fetch_all(&self.db.pool)
        .await?)
    }

    pub async fn transition(&self, issue_id: Uuid, event: IssueEvent) -> Result<Issue> {
        let issue = self.issue(issue_id).await?;
        let from = issue.status_enum();
        let to = hulahoop_core::next(from, &event).map_err(|error| anyhow!(error))?;
        let now = Utc::now().to_rfc3339();

        let status_before = if to == IssueStatus::Blocked {
            Some(from.as_str().to_string())
        } else if from == IssueStatus::Blocked && to != IssueStatus::Blocked {
            None
        } else {
            issue.status_before_blocked.clone()
        };

        sqlx::query(
            "update issues set status = ?, status_before_blocked = ?, updated_at = ? where id = ?",
        )
        .bind(to.as_str())
        .bind(status_before)
        .bind(&now)
        .bind(issue_id)
        .execute(&self.db.pool)
        .await?;

        self.issue(issue_id).await
    }

    pub async fn comment(
        &self,
        issue_id: Uuid,
        author: hulahoop_core::AuthorType,
        author_id: Option<&str>,
        body: &str,
        pinned: bool,
    ) -> Result<Uuid> {
        let id = Uuid::now_v7();
        sqlx::query("insert into comments (id, issue_id, author_type, author_id, body, pinned, created_at) values (?, ?, ?, ?, ?, ?, ?)")
            .bind(id)
            .bind(issue_id)
            .bind(author.as_str())
            .bind(author_id)
            .bind(body)
            .bind(pinned)
            .bind(crate::db::now_rfc3339())
            .execute(&self.db.pool)
            .await?;
        if pinned {
            sqlx::query("update issues set pinned_comment_id = ? where id = ?")
                .bind(id)
                .bind(issue_id)
                .execute(&self.db.pool)
                .await?;
        }
        Ok(id)
    }

    /// Criteria are the contract with the Test Author, so they are validated
    /// before an issue may leave `backlog`. A criterion like "works" produces a
    /// test that asserts nothing, and the loop then proves nothing.
    pub fn validate_criteria(texts: &[String]) -> Result<()> {
        if texts.is_empty() {
            bail!("an issue needs at least one acceptance criterion");
        }
        for (index, text) in texts.iter().enumerate() {
            let trimmed = text.trim();
            if trimmed.len() < 8 {
                bail!(
                    "criterion {} is too short to test: {trimmed:?}. Write the observable behavior and the condition, for example `add(2,3) returns 5`.",
                    index + 1
                );
            }
            let lowered = trimmed.to_lowercase();
            let generic = [
                "works",
                "is fast",
                "handles errors",
                "does not crash",
                "is correct",
            ];
            let words: Vec<String> = trimmed
                .split_whitespace()
                .map(|word| {
                    word.trim_matches(|c: char| !c.is_alphanumeric())
                        .to_lowercase()
                })
                .filter(|word| !word.is_empty())
                .collect();
            let filler_only =
                !words.is_empty() && words.iter().all(|word| FILLER.iter().any(|f| f == word));
            if filler_only || generic.iter().any(|g| lowered == *g) {
                bail!(
                    "criterion {} is generic: {trimmed:?}. Name the observable behavior, for example `add(2,3) returns 5`.",
                    index + 1
                );
            }
            if GOAL_PHRASES.iter().any(|goal| lowered.contains(goal)) {
                bail!(
                    "criterion {} states a goal rather than an observable result: {trimmed:?}. Name what a test can read back, for example `the connections query lists github after the app is created`.",
                    index + 1
                );
            }
        }
        Ok(())
    }

    // ----------------------------------------------------------- connectors

    /// Every provider the screen offers, stored or not. "Available" and
    /// "connected" are two different answers, and only a complete list shows both.
    pub const PROVIDERS: [&'static str; 4] = crate::connectors::ALL;

    fn vault(&self) -> Result<crate::vault::Vault> {
        crate::vault::Vault::open(&self.db.data_dir)
    }

    async fn connector_row(&self, provider: &str) -> Result<Option<ConnectorRow>> {
        Ok(
            sqlx::query_as::<_, ConnectorRow>("select * from connectors where provider = ?")
                .bind(provider)
                .fetch_optional(&self.db.pool)
                .await?,
        )
    }

    /// One field out of the stored payload. Each field is sealed on its own, so
    /// reading the app id never requires opening the private key.
    fn open_field(vault: &crate::vault::Vault, payload: &str, field: &str) -> Option<String> {
        let map: serde_json::Value = serde_json::from_str(payload).ok()?;
        vault.open_string(map.get(field)?.as_str()?).ok()
    }

    fn sealed_payload(fields: &[(&str, String)]) -> String {
        let mut map = serde_json::Map::new();
        for (name, value) in fields {
            map.insert(
                (*name).to_string(),
                serde_json::Value::String(value.clone()),
            );
        }
        serde_json::Value::Object(map).to_string()
    }

    /// The stored app's credentials, or `None` when nothing is stored or the
    /// vault cannot open what is stored. An unreadable credential is a reason to
    /// refuse work, not a reason to guess.
    async fn stored_app(&self, provider: &str) -> Result<Option<StoredApp>> {
        let Some(row) = self.connector_row(provider).await? else {
            return Ok(None);
        };
        let vault = self.vault()?;
        let (Some(app_id), Some(private_key)) = (
            Self::open_field(&vault, &row.secret_ciphertext, "app_id"),
            Self::open_field(&vault, &row.secret_ciphertext, "private_key"),
        ) else {
            return Ok(None);
        };
        Ok(Some(StoredApp {
            app_id,
            private_key,
            slug: row.label,
            updated_at: row.updated_at,
        }))
    }

    /// What a run of this adapter authenticates with: a key in the environment, or
    /// the credentials file the CLI wrote, copied into the run's scratch home.
    pub async fn agent_credential(&self, adapter: hulahoop_core::Adapter) -> Result<Credential> {
        let Some(provider) = crate::connectors::for_adapter(adapter) else {
            return Ok(Credential::default());
        };
        let Some(row) = self.connector_row(provider).await? else {
            return Ok(Credential::default());
        };
        let vault = self.vault()?;
        let mut credential = Credential::default();
        if let Some(secret) = Self::open_field(&vault, &row.secret_ciphertext, "secret") {
            if let Some(name) = crate::connectors::api_key_env(provider) {
                credential.env.push((name.to_string(), secret));
            }
        }
        if let Some(contents) = Self::open_field(&vault, &row.secret_ciphertext, "contents") {
            if let Some(path) = crate::connectors::credentials_file(provider) {
                credential.files.push((path.to_string(), contents));
            }
        }
        Ok(credential)
    }

    /// Whether a stored secret can still act is the only honest definition of
    /// "connected": an app whose key cannot sign is not the app it claims to be,
    /// and a key the CLI would reject is not a login.
    pub async fn connections(&self) -> Result<Vec<Connection>> {
        let mut connections = Vec::with_capacity(Self::PROVIDERS.len());
        for provider in Self::PROVIDERS {
            let row = self.connector_row(provider).await?;
            let vault = self.vault()?;
            let (connected, app_id, slug, label) = match &row {
                None => (false, None, None, None),
                Some(row) => {
                    let app_id = Self::open_field(&vault, &row.secret_ciphertext, "app_id");
                    let private_key =
                        Self::open_field(&vault, &row.secret_ciphertext, "private_key");
                    // Sealed and openable is what every provider can prove locally;
                    // a github app has one more answer, whether the key parses.
                    let openable = app_id.is_some()
                        || private_key.is_some()
                        || Self::open_field(&vault, &row.secret_ciphertext, "secret").is_some()
                        || Self::open_field(&vault, &row.secret_ciphertext, "contents").is_some();
                    let signs = private_key.as_deref().is_none_or(crate::github::can_sign);
                    (
                        openable && signs,
                        if openable { app_id } else { None },
                        if openable { row.label.clone() } else { None },
                        row.label.clone(),
                    )
                }
            };
            connections.push(Connection {
                provider: provider.to_string(),
                kind: row.as_ref().map(|row| row.kind.clone()).unwrap_or_default(),
                connected,
                app_id,
                slug,
                label,
                connected_at: row.map(|row| row.updated_at),
            });
        }
        Ok(connections)
    }

    /// Forget a stored credential. The loop stops acting as that provider, so the
    /// next attempt goes back to what the machine has until one is stored again.
    pub async fn clear_connection(&self, provider: &str) -> Result<Connection> {
        if !Self::PROVIDERS.contains(&provider) {
            bail!("unknown provider `{provider}`");
        }
        sqlx::query("delete from connectors where provider = ?")
            .bind(provider)
            .execute(&self.db.pool)
            .await?;
        Ok(Connection {
            provider: provider.to_string(),
            kind: String::new(),
            connected: false,
            app_id: None,
            slug: None,
            label: None,
            connected_at: None,
        })
    }

    async fn write_connector(
        &self,
        provider: &str,
        kind: &str,
        payload: &str,
        label: Option<&str>,
    ) -> Result<()> {
        let now = crate::db::now_rfc3339();
        sqlx::query(
            "insert into connectors (id, provider, kind, secret_ciphertext, label, created_at, updated_at)
             values (?, ?, ?, ?, ?, ?, ?)
             on conflict(provider) do update set
                 kind = excluded.kind,
                 secret_ciphertext = excluded.secret_ciphertext,
                 label = excluded.label,
                 updated_at = excluded.updated_at",
        )
        .bind(Uuid::now_v7())
        .bind(provider)
        .bind(kind)
        .bind(payload)
        .bind(label)
        .bind(&now)
        .bind(&now)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Store (or replace) a GitHub app. The key is sealed before it reaches the
    /// database, and the app id with it: both identify the account the loop can
    /// act as, so neither belongs in a page a backup would copy in the clear.
    pub async fn set_github_app(&self, app_id: &str, private_key: &str) -> Result<Connection> {
        let app_id = app_id.trim();
        let private_key = private_key.trim();
        if app_id.is_empty() {
            bail!("a github app needs an app id; GitHub shows it next to the app name");
        }
        if private_key.is_empty() {
            bail!("a github app needs its private key; GitHub generates the .pem once");
        }
        let vault = self.vault()?;
        let payload = Self::sealed_payload(&[
            ("app_id", vault.seal_string(app_id)?),
            ("private_key", vault.seal_string(private_key)?),
        ]);
        self.write_connector(
            crate::github::PROVIDER,
            crate::connectors::KIND_GITHUB_APP,
            &payload,
            None,
        )
        .await?;
        self.refresh_slug(app_id, private_key).await;
        self.connection(crate::github::PROVIDER).await
    }

    /// Store an API key for an agent CLI. The key is sealed like every other
    /// secret, and the adapter it belongs to decides where it goes.
    pub async fn set_agent_key(&self, provider: &str, key: &str) -> Result<Connection> {
        let key = key.trim();
        if key.is_empty() {
            bail!("a key is required; the CLI uses it as {provider} credentials");
        }
        if crate::connectors::api_key_env(provider).is_none() {
            bail!("`{provider}` takes a subscription file, not an API key");
        }
        let vault = self.vault()?;
        let payload = Self::sealed_payload(&[("secret", vault.seal_string(key)?)]);
        self.write_connector(
            provider,
            crate::connectors::KIND_API_KEY,
            &payload,
            Some("api key"),
        )
        .await?;
        self.connection(provider).await
    }

    /// Store the credentials file a CLI wrote when it logged in. The contents are
    /// copied into every run's scratch home, so a subscription is used by a run and
    /// never by the host.
    pub async fn set_agent_subscription(
        &self,
        provider: &str,
        contents: &str,
    ) -> Result<Connection> {
        let contents = contents.trim();
        if !contents.starts_with('{') {
            bail!("paste the contents of the credentials file, which is JSON starting with `{{`");
        }
        let Some(path) = crate::connectors::credentials_file(provider) else {
            bail!("`{provider}` does not take a subscription file");
        };
        let vault = self.vault()?;
        let payload = Self::sealed_payload(&[("contents", vault.seal_string(contents)?)]);
        self.write_connector(
            provider,
            crate::connectors::KIND_SUBSCRIPTION,
            &payload,
            Some(path),
        )
        .await?;
        self.connection(provider).await
    }

    /// One provider as the screen sees it.
    pub async fn connection(&self, provider: &str) -> Result<Connection> {
        let all = self.connections().await?;
        all.into_iter()
            .find(|connection| connection.provider == provider)
            .ok_or_else(|| anyhow!("unknown provider `{provider}`"))
    }

    /// Ask GitHub which app this key belongs to and remember the slug. Best
    /// effort: no network keeps the app, and the slug is learned on a later call.
    async fn refresh_slug(&self, app_id: &str, private_key: &str) {
        if !crate::github::can_sign(private_key) {
            return;
        }
        let Ok(jwt) = crate::github::mint_token(app_id, private_key) else {
            return;
        };
        let Ok(http) = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
        else {
            return;
        };
        match crate::github::fetch_slug(&http, &jwt).await {
            Ok(slug) => {
                if let Err(error) =
                    sqlx::query("update connectors set label = ? where provider = ?")
                        .bind(&slug)
                        .bind(crate::github::PROVIDER)
                        .execute(&self.db.pool)
                        .await
                {
                    tracing::warn!(%error, "could not remember the app slug");
                }
            }
            Err(error) => {
                tracing::warn!(%error, "github did not name the app; the slug stays unknown");
            }
        }
    }

    pub async fn commit_identity(&self) -> Result<(Option<String>, Option<String>)> {
        let Some(app) = self.stored_app(crate::github::PROVIDER).await? else {
            return Ok((None, None));
        };
        if !crate::github::can_sign(&app.private_key) {
            return Ok((None, None));
        }
        Ok((Some(app.app_id.clone()), app.slug.clone()))
    }

    /// The token a run acts as the app with. `None` when no app is stored or the
    /// stored key cannot sign, because handing out a token nobody can verify is
    /// worse than handing out nothing.
    pub async fn github_token(&self) -> Result<Option<String>> {
        let Some(app) = self.stored_app(crate::github::PROVIDER).await? else {
            return Ok(None);
        };
        if !crate::github::can_sign(&app.private_key) {
            return Ok(None);
        }
        crate::github::mint_token(&app.app_id, &app.private_key).map(Some)
    }

    // ------------------------------------------------------------------ runs

    async fn live_run_on_issue(&self, issue_id: Uuid) -> Result<Option<Run>> {
        Ok(sqlx::query_as::<_, Run>(
            "select * from runs where issue_id = ? and status in ('queued','running') limit 1",
        )
        .bind(issue_id)
        .fetch_optional(&self.db.pool)
        .await?)
    }

    async fn live_run_on_repo(&self, repo_id: Uuid) -> Result<Option<Run>> {
        Ok(sqlx::query_as::<_, Run>(
            "select * from runs where repo_id = ? and status in ('queued','running') limit 1",
        )
        .bind(repo_id)
        .fetch_optional(&self.db.pool)
        .await?)
    }

    pub async fn agent_role_binding(
        &self,
        project_id: Uuid,
        role: AgentRole,
    ) -> Result<Option<Uuid>> {
        let row: Option<(Uuid,)> =
            sqlx::query_as("select agent_id from project_agents where project_id = ? and role = ?")
                .bind(project_id)
                .bind(role.as_str())
                .fetch_optional(&self.db.pool)
                .await?;
        Ok(row.map(|(agent_id,)| agent_id))
    }

    pub async fn start_run(
        &self,
        issue_id: Uuid,
        agent_id: Option<Uuid>,
        role: AgentRole,
        parent_run_id: Option<Uuid>,
    ) -> Result<Run> {
        let issue = self.issue(issue_id).await?;
        let repo: Repo = sqlx::query_as("select * from repos where id = ?")
            .bind(issue.repo_id)
            .fetch_one(&self.db.pool)
            .await?;

        let agent_id = match agent_id {
            Some(id) => {
                let bound = self.agent_role_binding(issue.project_id, role).await?;
                if bound != Some(id) {
                    let bound_name = match bound {
                        Some(other) => other.to_string(),
                        None => "none".to_string(),
                    };
                    bail!(
                        "agent {id} is not bound to the {} role in this project (bound: {bound_name}). Assign it on the project screen first.",
                        role.as_str()
                    );
                }
                id
            }
            None => self
                .agent_role_binding(issue.project_id, role)
                .await?
                .ok_or_else(|| {
                    anyhow!(
                        "no agent is bound to the {} role in this project. Assign one on the project screen before starting a run.",
                        role.as_str()
                    )
                })?,
        };

        // A probe is the only evidence about the CLI, so a run without one would
        // be a bet on an unknown binary.
        let preflight: Option<(i64, Option<String>)> = sqlx::query_as(
            "select ok, detail from agent_preflight where agent_id = ? and repo_id = ?",
        )
        .bind(agent_id)
        .bind(issue.repo_id)
        .fetch_optional(&self.db.pool)
        .await?;
        match preflight {
            None => {
                let agent = self.agent(agent_id).await?;
                self.queue_job(
                    issue.repo_id,
                    Some(issue_id),
                    "preflight",
                    serde_json::json!({
                        "agent_id": agent_id,
                        "adapter": agent.adapter,
                        "model": agent.model,
                    }),
                )
                .await?;
                bail!(
                    "agent {agent_id} has not passed preflight on this repo yet. A probe was queued; retry when Settings shows it green."
                );
            }
            Some((0, detail)) => bail!(
                "agent {agent_id} failed its preflight: {}. Fix the adapter setup in Settings, then re-run the probe.",
                detail.unwrap_or_default()
            ),
            Some(_) => {}
        }

        if let Some(live) = self.live_run_on_issue(issue_id).await? {
            bail!("a run is already active on this issue ({})", live.id);
        }
        if let Some(live) = self.live_run_on_repo(issue.repo_id).await? {
            bail!(
                "another run is active on this repo ({}). One agent per repo keeps the jj op log and the build cache uncontended; start this run when that one finishes.",
                live.id
            );
        }

        // The cap counts runs since the last human override, so clearing it is a
        // deliberate act rather than a side effect of a new attempt.
        let since = issue
            .run_cap_cleared_at
            .map(|t| t.to_rfc3339())
            .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string());
        let (count,): (i64,) =
            sqlx::query_as("select count(*) from runs where issue_id = ? and created_at >= ?")
                .bind(issue_id)
                .bind(&since)
                .fetch_one(&self.db.pool)
                .await?;
        if count >= hulahoop_core::RUNS_PER_ISSUE {
            bail!(
                "this issue has used {count} of {} runs. Human override required: call clearRunCap to continue.",
                hulahoop_core::RUNS_PER_ISSUE
            );
        }

        let (base_bookmark, attempt) = match role {
            AgentRole::TestAuthor => (repo.base_bookmark.clone(), issue.attempt),
            AgentRole::Implementer => {
                let base = match parent_run_id {
                    Some(parent_id) => {
                        let parent: Run = sqlx::query_as("select * from runs where id = ?")
                            .bind(parent_id)
                            .fetch_optional(&self.db.pool)
                            .await?
                            .ok_or_else(|| anyhow!("parent run {parent_id} not found"))?;
                        parent
                            .head_bookmark
                            .clone()
                            .ok_or_else(|| anyhow!("parent run {parent_id} has no head bookmark"))?
                    }
                    None => issue.tests_bookmark.clone().ok_or_else(|| {
                        anyhow!(
                            "the suite is not frozen: freeze it before starting an implementer run"
                        )
                    })?,
                };
                (base, issue.attempt + 1)
            }
            AgentRole::Reviewer => {
                let current = issue.current_run_id.ok_or_else(|| {
                    anyhow!("there is no implementer run to review yet on this issue")
                })?;
                let run: Run = sqlx::query_as("select * from runs where id = ?")
                    .bind(current)
                    .fetch_one(&self.db.pool)
                    .await?;
                (
                    run.head_bookmark
                        .clone()
                        .ok_or_else(|| anyhow!("the run under review has no head bookmark"))?,
                    issue.attempt,
                )
            }
        };

        let run_id = Uuid::now_v7();
        let artifact_dir = contract::run_artifact_dir(&self.db.data_dir, &run_id);
        std::fs::create_dir_all(&artifact_dir)?;
        let prompt_revision = format!("{}+{}", env!("CARGO_PKG_VERSION"), prompt_hash());

        sqlx::query(
            "insert into runs (id, issue_id, repo_id, agent_id, role, attempt, base_bookmark, base_revision,
                prompt_revision, artifact_dir, status, created_at)
             values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'queued', ?)",
        )
        .bind(run_id)
        .bind(issue_id)
        .bind(issue.repo_id)
        .bind(agent_id)
        .bind(role.as_str())
        .bind(attempt)
        .bind(&base_bookmark)
        // The base is a revision expression, not a frozen hash: an implementer run
        // has to start on the frozen suite's bookmark, and resolving it here would
        // mean a second process to ask jj. Storing the bookmark keeps the run on
        // the suite the gate proved red, even after another attempt moves it.
        .bind(&base_bookmark)
        .bind(&prompt_revision)
        .bind(artifact_dir.display().to_string())
        .bind(crate::db::now_rfc3339())
        .execute(&self.db.pool)
        .await?;

        sqlx::query(
            "update issues set run_count = run_count + 1, attempt = ?, updated_at = ? where id = ?",
        )
        .bind(if role == AgentRole::Implementer {
            issue.attempt + 1
        } else {
            issue.attempt
        })
        .bind(crate::db::now_rfc3339())
        .bind(issue_id)
        .execute(&self.db.pool)
        .await?;

        let event = match role {
            AgentRole::TestAuthor => IssueEvent::StartTestAuthor,
            AgentRole::Implementer => IssueEvent::StartImplementer { parent_run_id },
            AgentRole::Reviewer => IssueEvent::StartReviewer,
        };
        if let Err(error) = self.transition(issue_id, event.clone()).await {
            // The run row exists; drop it so a refused transition cannot leave a
            // queued run the runner would pick up.
            sqlx::query("delete from runs where id = ?")
                .bind(run_id)
                .execute(&self.db.pool)
                .await?;
            bail!("{error}");
        }

        self.run(run_id).await
    }

    pub async fn agent(&self, id: Uuid) -> Result<crate::model::Agent> {
        sqlx::query_as::<_, crate::model::Agent>("select * from agents where id = ?")
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await?
            .ok_or_else(|| anyhow!("agent {id} not found"))
    }

    pub async fn run(&self, id: Uuid) -> Result<Run> {
        sqlx::query_as::<_, Run>("select * from runs where id = ?")
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await?
            .ok_or_else(|| anyhow!("run {id} not found"))
    }

    pub async fn replay(&self, id: Uuid) -> Result<Replay> {
        sqlx::query_as::<_, Replay>("select * from replays where id = ?")
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await?
            .ok_or_else(|| anyhow!("replay {id} not found"))
    }

    pub async fn cancel_run(&self, run_id: Uuid) -> Result<Run> {
        let run = self.run(run_id).await?;
        if !run.status_enum().is_live() {
            bail!("run {run_id} is already {}", run.status_enum());
        }
        if let Some(pgid) = run.pgid {
            kill_process_group(pgid as i32);
        }
        sqlx::query("update runs set status = 'cancelled', finished_at = ?, error = 'cancelled by a human' where id = ?")
            .bind(crate::db::now_rfc3339())
            .bind(run_id)
            .execute(&self.db.pool)
            .await?;
        // Cancelling a run leaves the issue where it was: a misclick costs one
        // run, never the issue.
        self.transition(run.issue_id, IssueEvent::CancelRun)
            .await
            .ok();
        self.run(run_id).await
    }

    // ------------------------------------------------------------------ gate

    pub async fn freeze_suite(&self, issue_id: Uuid) -> Result<Issue> {
        if let Some(live) = self.live_run_on_issue(issue_id).await? {
            bail!("a run is active on this issue ({})", live.id);
        }
        let authoring: Option<Run> = sqlx::query_as(
            "select * from runs where issue_id = ? and role = 'test_author' and status = 'succeeded'
             order by created_at desc limit 1",
        )
        .bind(issue_id)
        .fetch_optional(&self.db.pool)
        .await?;
        let authoring = authoring.ok_or_else(|| {
            anyhow!("no finished Test Author run on this issue yet: start one, then freeze")
        })?;

        let replay: Option<Replay> = sqlx::query_as(
            "select * from replays where candidate_run_id = ? and kind = 'base'
             order by created_at desc limit 1",
        )
        .bind(authoring.id)
        .fetch_optional(&self.db.pool)
        .await?;
        let replay = replay.ok_or_else(|| {
            anyhow!("the base replay for this candidate suite has not finished yet")
        })?;
        if !matches!(replay.status.as_str(), "failed" | "succeeded") {
            bail!(
                "the base replay is {}, wait for it to finish",
                replay.status
            );
        }

        let report_path = std::path::Path::new(&replay.artifact_dir).join("junit.xml");
        let report = junit::parse_file(&report_path).with_context(|| {
            format!(
                "the base replay wrote no parsable JUnit report at {}",
                report_path.display()
            )
        })?;

        let criteria = self.criteria(issue_id).await?;
        let mut problems = Vec::new();
        let mut mapping = Vec::new();
        for criterion in &criteria {
            let matches = report.tests_for_criterion(criterion.position);
            match matches.len() {
                0 => problems.push(format!(
                    "criterion {}: no test name carries `ac{}`",
                    criterion.position, criterion.position
                )),
                1 => {
                    let case = matches[0];
                    if case.failed {
                        mapping.push((criterion.id, case.qualified_name()));
                    } else {
                        problems.push(format!(
                            "criterion {}: `{}` passes at the base revision, so it does not prove the work",
                            criterion.position,
                            case.qualified_name()
                        ));
                    }
                }
                _ => problems.push(format!(
                    "criterion {}: {} tests carry `ac{}` ({}); one criterion needs exactly one test",
                    criterion.position,
                    matches.len(),
                    criterion.position,
                    matches
                        .iter()
                        .map(|c| c.qualified_name())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            }
        }

        // A suite that could not even be collected at base is red for the wrong
        // reason: the failure belongs to the import, not to a criterion.
        let uncollected: Vec<String> = report
            .cases
            .iter()
            // pytest names a collection error after the module it failed to import,
            // with no class: `classname` empty and a dotted name. A real test that
            // failed carries the criterion token and must not be reported here.
            .filter(|case| {
                case.failed
                    && case.classname.is_none()
                    && case.name.contains('.')
                    && !criteria.iter().any(|criterion| {
                        junit::contains_token(&case.name, &format!("ac{}", criterion.position))
                    })
            })
            .map(|case| case.qualified_name())
            .collect();
        for name in uncollected {
            problems.push(format!(
                "`{name}` could not be collected at the base revision. Import the API inside the test function, so the failure lands on the test that names its criterion"
            ));
        }

        if !problems.is_empty() {
            let body = format!(
                "The suite cannot be frozen:\n\n{}\n\nRun the Test Author again after fixing the tests.",
                problems
                    .iter()
                    .map(|p| format!("- {p}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            self.comment(
                issue_id,
                hulahoop_core::AuthorType::Server,
                None,
                &body,
                false,
            )
            .await?;
            self.transition(
                issue_id,
                IssueEvent::FreezeRejected {
                    reason: problems.join("; "),
                },
            )
            .await?;
            bail!(body);
        }

        for (criterion_id, test_name) in mapping {
            sqlx::query("update criteria set test_name = ? where id = ?")
                .bind(&test_name)
                .bind(criterion_id)
                .execute(&self.db.pool)
                .await?;
        }

        let frozen_paths = json_list(&authoring.changed_paths);
        if frozen_paths.is_empty() {
            bail!("the Test Author changed no files, so there is nothing to freeze");
        }
        let tests_bookmark = authoring
            .candidate_bookmark
            .clone()
            .ok_or_else(|| anyhow!("the authoring run recorded no candidate bookmark"))?;

        sqlx::query(
            "update issues set tests_bookmark = ?, tests_revision = ?, frozen_paths = ?,
                tests_frozen_at = ?, suite_seq = suite_seq + 1, updated_at = ? where id = ?",
        )
        .bind(&tests_bookmark)
        .bind(&authoring.head_revision)
        .bind(serde_json::to_string(&frozen_paths)?)
        .bind(crate::db::now_rfc3339())
        .bind(crate::db::now_rfc3339())
        .bind(issue_id)
        .execute(&self.db.pool)
        .await?;

        self.transition(issue_id, IssueEvent::FreezeSuite).await
    }

    pub async fn unfreeze_suite(&self, issue_id: Uuid, reason: &str) -> Result<Issue> {
        let issue = self.issue(issue_id).await?;
        // Frozen is the condition, not the status: the human can learn the criteria
        // were wrong at any point before the merge.
        if issue.tests_bookmark.is_none()
            || matches!(
                issue.status_enum(),
                IssueStatus::Backlog
                    | IssueStatus::Ready
                    | IssueStatus::Done
                    | IssueStatus::Cancelled
            )
        {
            bail!(
                "there is no frozen suite to thaw (issue is {}, tests bookmark {})",
                issue.status_enum(),
                if issue.tests_bookmark.is_some() {
                    "set"
                } else {
                    "empty"
                }
            );
        }
        // Refuse before touching anything: the transition is the part that can
        // fail, and a refused one used to leave the issue with no suite at all.
        hulahoop_core::next(issue.status_enum(), &IssueEvent::StartTestAuthor).with_context(
            || {
                format!(
                    "an issue in `{}` cannot thaw its suite",
                    issue.status_enum()
                )
            },
        )?;
        let body = format!("Suite thawed: {reason}");
        self.comment(
            issue_id,
            hulahoop_core::AuthorType::Human,
            None,
            &body,
            false,
        )
        .await?;
        // The archive keeps the handle the old attempts branched from, numbered by
        // the suite it replaced, so several thaws in one attempt stay distinct.
        if let Some(bookmark) = &issue.tests_bookmark {
            let archive = format!("tests/{}/archive/{}", issue.id, issue.suite_seq);
            sqlx::query("update runs set status = status where id = id")
                .execute(&self.db.pool)
                .await?;
            self.queue_job(
                issue.repo_id,
                Some(issue_id),
                "bookmark",
                serde_json::json!({ "name": archive, "from": bookmark }),
            )
            .await?;
        }
        sqlx::query("update issues set current_run_id = null, tests_bookmark = null, tests_revision = null, frozen_paths = '[]', tests_frozen_at = null, updated_at = ? where id = ?")
            .bind(crate::db::now_rfc3339())
            .bind(issue_id)
            .execute(&self.db.pool)
            .await?;
        self.transition(issue_id, IssueEvent::StartTestAuthor).await
    }

    // -------------------------------------------------------------- approval

    pub async fn approve_issue(
        &self,
        issue_id: Uuid,
        run_id: Uuid,
        reviewed_head: &str,
        diff_sha: &str,
        acknowledge_flags: Vec<RunFlag>,
    ) -> Result<Issue> {
        let issue = self.issue(issue_id).await?;
        if issue.current_run_id != Some(run_id) {
            bail!(
                "run {run_id} is not the run this issue is waiting on ({}). Approve the run the review screen shows.",
                issue
                    .current_run_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "none".into())
            );
        }
        let run = self.run(run_id).await?;
        if run.head_revision.as_deref() != Some(reviewed_head) {
            bail!(
                "the head moved since you opened the review: run {run_id} is now at {:?}",
                run.head_revision
            );
        }
        if run.diff_sha.as_deref() != Some(diff_sha) {
            bail!(
                "the diff changed since you opened the review; reload the review before approving"
            );
        }

        let review: Option<Review> = sqlx::query_as(
            "select * from reviews where reviewed_run_id = ? and verdict = 'approved' order by created_at desc limit 1",
        )
        .bind(run_id)
        .fetch_optional(&self.db.pool)
        .await?;
        if review.is_none() {
            bail!("the Reviewer has not approved run {run_id} yet. An approval without a verdict skips the review gate.");
        }

        let flags = run.flags_enum();
        for flag in &flags {
            if !acknowledge_flags.contains(flag) {
                bail!(
                    "this run carries the `{}` flag; acknowledge it to approve",
                    flag.as_str()
                );
            }
        }

        let approval_id = Uuid::now_v7();
        let landing_bookmark = format!("landing/{}/{}", issue_id, issue.attempt);
        let user_id = self.db.local_user_id().await?;
        sqlx::query(
            "insert into approvals (id, issue_id, run_id, head_revision, diff_sha, landing_bookmark,
                acknowledged_flags, approved_by, created_at) values (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(approval_id)
        .bind(issue_id)
        .bind(run_id)
        .bind(reviewed_head)
        .bind(diff_sha)
        .bind(&landing_bookmark)
        .bind(serde_json::to_string(&acknowledge_flags)?)
        .bind(user_id)
        .bind(crate::db::now_rfc3339())
        .execute(&self.db.pool)
        .await?;

        sqlx::query("update approvals set voided_at = ? where issue_id = ? and id != ? and voided_at is null")
            .bind(crate::db::now_rfc3339())
            .bind(issue_id)
            .bind(approval_id)
            .execute(&self.db.pool)
            .await?;

        let repo: Repo = sqlx::query_as("select * from repos where id = ?")
            .bind(issue.repo_id)
            .fetch_one(&self.db.pool)
            .await?;

        let replay_id = Uuid::now_v7();
        let artifact_dir = contract::replay_artifact_dir(&self.db.data_dir, &replay_id);
        std::fs::create_dir_all(&artifact_dir)?;
        sqlx::query(
            "insert into replays (id, issue_id, repo_id, run_id, candidate_run_id, approval_id, kind,
                attempt, revision, tests_bookmark, status, artifact_dir, created_at)
             values (?, ?, ?, ?, ?, ?, 'landing', ?, ?, ?, 'queued', ?, ?)",
        )
        .bind(replay_id)
        .bind(issue_id)
        .bind(issue.repo_id)
        .bind(run_id)
        .bind(run_id)
        .bind(approval_id)
        .bind(issue.attempt)
        .bind(reviewed_head)
        .bind(issue.tests_bookmark.clone())
        .bind(artifact_dir.display().to_string())
        .bind(crate::db::now_rfc3339())
        .execute(&self.db.pool)
        .await?;

        let _ = repo;
        self.transition(issue_id, IssueEvent::ApproveIssue).await
    }

    pub async fn record_merge(&self, issue_id: Uuid, merge_revision: &str) -> Result<Issue> {
        let issue = self.issue(issue_id).await?;
        if issue.status_enum() != IssueStatus::Merging {
            bail!("the issue is {}, not merging", issue.status_enum());
        }
        let approval: Option<Approval> = sqlx::query_as(
            "select * from approvals where issue_id = ? and voided_at is null order by created_at desc limit 1",
        )
        .bind(issue_id)
        .fetch_optional(&self.db.pool)
        .await?;
        let approval =
            approval.ok_or_else(|| anyhow!("there is no live approval on this issue"))?;

        let replay: Option<Replay> = sqlx::query_as(
            "select * from replays where approval_id = ? and kind = 'landing' order by created_at desc limit 1",
        )
        .bind(approval.id)
        .fetch_optional(&self.db.pool)
        .await?;
        match replay {
            Some(replay) if replay.status == "succeeded" => {}
            Some(replay) if replay.status == "failed" => {
                bail!("the landing replay failed, so this approval cannot be merged")
            }
            Some(replay) if replay.status == "timed_out" => {
                bail!("the landing replay timed out; re-approve to replay it")
            }
            Some(_) => bail!("the landing replay has not finished yet"),
            None => bail!("there is no landing replay for this approval"),
        }

        // Ancestry is checked in the repo, by the runner, because verifying a
        // merge commit means walking history. The issue closes when that check
        // comes back.
        let landing = approval
            .landing_bookmark
            .clone()
            .ok_or_else(|| anyhow!("the approval has no landing bookmark"))?;
        self.queue_job(
            issue.repo_id,
            Some(issue_id),
            "merge_verify",
            serde_json::json!({ "merge_revision": merge_revision, "landing_bookmark": landing }),
        )
        .await?;
        self.comment(
            issue_id,
            hulahoop_core::AuthorType::Server,
            None,
            &format!("Checking that `{merge_revision}` contains `{landing}`."),
            false,
        )
        .await?;
        self.issue(issue_id).await
    }

    pub async fn resume_issue(&self, issue_id: Uuid, answer: &str) -> Result<Issue> {
        let issue = self.issue(issue_id).await?;
        if issue.status_enum() != IssueStatus::Blocked {
            bail!("the issue is {}, not blocked", issue.status_enum());
        }
        let question = match issue.pinned_comment_id {
            Some(id) => {
                let row: Option<(String,)> =
                    sqlx::query_as("select body from comments where id = ?")
                        .bind(id)
                        .fetch_optional(&self.db.pool)
                        .await?;
                row.map(|(body,)| body).unwrap_or_default()
            }
            None => String::new(),
        };
        let body = if question.is_empty() {
            format!("Answer: {answer}")
        } else {
            format!("Answer to \"{question}\":\n\n{answer}")
        };
        self.comment(
            issue_id,
            hulahoop_core::AuthorType::Human,
            None,
            &body,
            false,
        )
        .await?;
        sqlx::query("update issues set pinned_comment_id = null where id = ?")
            .bind(issue_id)
            .execute(&self.db.pool)
            .await?;
        let restored = hulahoop_core::resume_to(
            issue
                .status_before_blocked
                .as_deref()
                .and_then(IssueStatus::parse),
        )
        .ok_or_else(|| anyhow!("this issue has no status to return to"))?;
        sqlx::query("update issues set status = ?, status_before_blocked = null, updated_at = ? where id = ?")
            .bind(restored.as_str())
            .bind(crate::db::now_rfc3339())
            .bind(issue_id)
            .execute(&self.db.pool)
            .await?;
        self.issue(issue_id).await
    }

    // ------------------------------------------------------------- runner api

    pub async fn queue_job(
        &self,
        repo_id: Uuid,
        issue_id: Option<Uuid>,
        kind: &str,
        payload: serde_json::Value,
    ) -> Result<Uuid> {
        let id = Uuid::now_v7();
        sqlx::query(
            "insert into runner_jobs (id, repo_id, issue_id, kind, payload, status, created_at)
             values (?, ?, ?, ?, ?, 'queued', ?)",
        )
        .bind(id)
        .bind(repo_id)
        .bind(issue_id)
        .bind(kind)
        .bind(payload.to_string())
        .bind(crate::db::now_rfc3339())
        .execute(&self.db.pool)
        .await?;
        Ok(id)
    }

    pub async fn claim_run(&self, runner_id: &str) -> Result<Option<Run>> {
        let candidate: Option<Run> = sqlx::query_as(
            "select * from runs where status = 'queued' order by created_at limit 1",
        )
        .fetch_optional(&self.db.pool)
        .await?;
        let Some(run) = candidate else {
            return Ok(None);
        };
        let now = Utc::now().to_rfc3339();
        let updated = sqlx::query(
            "update runs set status = 'running', runner_id = ?, claimed_at = ?, started_at = ?, last_heartbeat_at = ? where id = ? and status = 'queued'",
        )
        .bind(runner_id)
        .bind(&now)
        .bind(&now)
        .bind(&now)
        .bind(run.id)
        .execute(&self.db.pool)
        .await?;
        if updated.rows_affected() == 0 {
            return Ok(None);
        }
        self.run(run.id).await.map(Some)
    }

    pub async fn heartbeat(&self, run_id: Uuid, pid: Option<i64>, pgid: Option<i64>) -> Result<()> {
        sqlx::query("update runs set last_heartbeat_at = ?, pid = coalesce(?, pid), pgid = coalesce(?, pgid) where id = ?")
            .bind(crate::db::now_rfc3339())
            .bind(pid)
            .bind(pgid)
            .bind(run_id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    pub async fn append_event(
        &self,
        run_id: Uuid,
        kind: &str,
        payload: serde_json::Value,
    ) -> Result<()> {
        let (next,): (i64,) =
            sqlx::query_as("select coalesce(max(seq), 0) + 1 from run_events where run_id = ?")
                .bind(run_id)
                .fetch_one(&self.db.pool)
                .await?;
        sqlx::query("insert or ignore into run_events (id, run_id, seq, kind, payload, created_at) values (?, ?, ?, ?, ?, ?)")
            .bind(Uuid::now_v7())
            .bind(run_id)
            .bind(next)
            .bind(kind)
            .bind(payload.to_string())
            .bind(crate::db::now_rfc3339())
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    /// A run finishing is not the same as an issue moving: a successful
    /// implementer run waits for its head replay, and a test author run waits for
    /// the human's freeze.
    pub async fn complete_run(&self, run_id: Uuid, payload: RunCompletion) -> Result<Run> {
        let run = self.run(run_id).await?;
        let issue = self.issue(run.issue_id).await?;
        let status = RunStatus::parse(&payload.status)
            .ok_or_else(|| anyhow!("unknown run status {}", payload.status))?;

        // The runner reports the paths it saw; the diff is the fallback because a
        // runner that crashed before naming them still left a patch behind.
        let changed_paths = if payload.changed_paths.is_empty() {
            diff::summarize(&payload.diff).files
        } else {
            payload.changed_paths.clone()
        };
        let summary = diff::classify(&payload.diff, &issue.frozen_path_list());
        let diff_sha = sha256_hex(payload.diff.as_bytes());
        let artifact_dir = std::path::Path::new(&run.artifact_dir);
        std::fs::create_dir_all(artifact_dir)?;
        std::fs::write(artifact_dir.join("diff.patch"), &payload.diff)?;

        let mut flags = summary.flags.clone();
        if payload.artifact_limit_hit {
            flags.push(RunFlag::ArtifactLimit);
        }
        let flags_json =
            serde_json::to_string(&flags.iter().map(|f| f.as_str()).collect::<Vec<_>>())?;

        let mut final_status = status;
        let mut error = payload.error.clone();

        // Frozen tests are a hard reject: an implementer that edits the suite
        // replaced the proof with its own assertion.
        if run.role_enum() == AgentRole::Implementer && !summary.is_acceptable() {
            final_status = RunStatus::Failed;
            error = Some(format!(
                "the run changed frozen test files: {}. Implementation code must make the frozen suite pass.",
                summary.frozen_hits.join(", ")
            ));
        }
        // A run that changed nothing did no work. pi exits 0 even when its provider
        // rejected the model, so "the process ended" is not evidence that the role ran;
        // without this, the issue sits at tests_in_progress offering a suite to freeze
        // that was never written.
        let asked = payload
            .question
            .as_deref()
            .is_some_and(|question| !question.trim().is_empty());
        let answered = payload
            .verdict
            .as_deref()
            .is_some_and(|verdict| !verdict.trim().is_empty());
        if final_status == RunStatus::Succeeded && changed_paths.is_empty() && !asked && !answered {
            final_status = RunStatus::Failed;
            error = Some(match adapter_failure(&payload) {
                Some(reason) => format!(
                    "the agent changed no files: {reason}. Nothing was run for this role, so the issue stays where it is; fix the agent in Settings and retry."
                ),
                None => "the agent changed no files and said nothing. A role that edits nothing did no work; check its transcript in the run log, then retry.".to_string(),
            });
        }
        if final_status == RunStatus::Succeeded && payload.artifact_limit_hit {
            final_status = RunStatus::Failed;
            error = Some(format!(
                "the run wrote more than {} MB of artifacts and was stopped",
                hulahoop_core::ARTIFACT_LIMIT_BYTES / (1024 * 1024)
            ));
        }

        let finished = Utc::now().to_rfc3339();
        sqlx::query(
            "update runs set status = ?, exit_code = ?, error = ?, diff_sha = ?, flags = ?,
                changed_paths = ?, head_revision = coalesce(?, head_revision),
                commit_name = ?, commit_email = ?, finished_at = ? where id = ?",
        )
        .bind(final_status.as_str())
        .bind(payload.exit_code)
        .bind(&error)
        .bind(&diff_sha)
        .bind(&flags_json)
        .bind(serde_json::to_string(&changed_paths)?)
        .bind(&payload.commit)
        .bind(&payload.commit_name)
        .bind(&payload.commit_email)
        .bind(&finished)
        .bind(run_id)
        .execute(&self.db.pool)
        .await?;

        if let Some(question) = payload.question.as_deref().filter(|q| !q.trim().is_empty()) {
            self.comment(
                run.issue_id,
                hulahoop_core::AuthorType::Agent,
                Some(&run.agent_id.to_string()),
                question,
                true,
            )
            .await?;
            self.transition(run.issue_id, IssueEvent::Blocked).await?;
            return self.run(run_id).await;
        }

        match (run.role_enum(), final_status) {
            (AgentRole::TestAuthor, RunStatus::Succeeded) => {
                let bookmark = format!("tests/{}/cand/{}", issue.id, run.id);
                sqlx::query(
                    "update runs set candidate_bookmark = ?, head_bookmark = ? where id = ?",
                )
                .bind(&bookmark)
                .bind(&bookmark)
                .bind(run_id)
                .execute(&self.db.pool)
                .await?;
                // The base replay belongs to this candidate suite: freezing reads
                // it back by the authoring run that produced the tests.
                self.queue_replay(issue, ReplayKind::Base, Some(&run), None, Some(&run))
                    .await?;
                self.transition(run.issue_id, IssueEvent::AuthoringCompleted)
                    .await?;
            }
            (AgentRole::TestAuthor, _) => {
                self.transition(run.issue_id, IssueEvent::AuthoringFailed)
                    .await?;
            }
            (AgentRole::Implementer, RunStatus::Succeeded) => {
                let bookmark = format!("issue/{}/head/{}", issue.id, run.attempt);
                sqlx::query("update runs set head_bookmark = ? where id = ?")
                    .bind(&bookmark)
                    .bind(run_id)
                    .execute(&self.db.pool)
                    .await?;
                // The head replay decides whether the issue may move: a green
                // suite at head is what `in_review` means.
                self.queue_replay(issue, ReplayKind::Head, Some(&run), None, Some(&run))
                    .await?;
            }
            (AgentRole::Implementer, _) => {
                self.transition(run.issue_id, IssueEvent::ImplementerFailed)
                    .await?;
            }
            (AgentRole::Reviewer, _) => {
                self.record_review(&issue, &run, &payload).await?;
            }
        }

        self.run(run_id).await
    }

    async fn record_review(&self, issue: &Issue, run: &Run, payload: &RunCompletion) -> Result<()> {
        // A reviewer that says nothing reviewed nothing; treat silence as a
        // request for changes so the issue cannot advance on a missing verdict.
        let verdict = payload
            .verdict
            .clone()
            .unwrap_or_else(|| "changes_requested".to_string());
        let reviewed_run_id = issue
            .current_run_id
            .ok_or_else(|| anyhow!("a reviewer finished on an issue with no run under review"))?;

        sqlx::query(
            "insert into reviews (id, issue_id, reviewed_run_id, reviewer_run_id, verdict, body, created_at)
             values (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::now_v7())
        .bind(issue.id)
        .bind(reviewed_run_id)
        .bind(run.id)
        .bind(&verdict)
        .bind(&payload.body)
        .bind(crate::db::now_rfc3339())
        .execute(&self.db.pool)
        .await?;

        if verdict == "approved" {
            self.transition(issue.id, IssueEvent::ReviewerApproved)
                .await?;
        } else {
            self.comment(
                issue.id,
                hulahoop_core::AuthorType::Agent,
                Some(&run.agent_id.to_string()),
                &format!("Reviewer requested changes:\n\n{}", payload.body),
                false,
            )
            .await?;
            self.transition(issue.id, IssueEvent::ReviewerChangesRequested)
                .await?;
        }
        Ok(())
    }

    /// Runs the base replay again for the current candidate suite.
    ///
    /// The proof is the report, and the report can be missing for reasons the agent
    /// had nothing to do with: a `just test` that only copied its JUnit file after a
    /// green run left the base replay with nothing to read. The candidate tests still
    /// exist on their bookmark, so asking again costs one suite run and no model call.
    pub async fn retry_base_replay(&self, issue_id: Uuid) -> Result<Replay> {
        let issue = self.issue(issue_id).await?;
        let live = self.live_run_on_issue(issue_id).await?;
        if live.is_some() {
            bail!("an agent is still working on this issue; wait for it to finish");
        }
        let candidate = sqlx::query_as::<_, Run>(
            "select * from runs where issue_id = ? and candidate_bookmark is not null
             order by created_at desc limit 1",
        )
        .bind(issue_id)
        .fetch_optional(&self.db.pool)
        .await?
        .ok_or_else(|| anyhow!("no authoring run has produced a candidate suite yet: the base replay has nothing to replay"))?;
        if crate::db::json_list(&candidate.changed_paths).is_empty() {
            bail!("the authoring run changed no files, so there is no suite to replay. Write the tests first");
        }
        let id = self
            .queue_replay(issue, ReplayKind::Base, None, None, Some(&candidate))
            .await?;
        self.replay(id).await
    }

    pub async fn queue_replay(
        &self,
        issue: Issue,
        kind: ReplayKind,
        run: Option<&Run>,
        approval_id: Option<Uuid>,
        candidate: Option<&Run>,
    ) -> Result<Uuid> {
        let id = Uuid::now_v7();
        let artifact_dir = contract::replay_artifact_dir(&self.db.data_dir, &id);
        std::fs::create_dir_all(&artifact_dir)?;
        let revision = match kind {
            ReplayKind::Base => None,
            ReplayKind::Head => candidate.and_then(|r| r.head_revision.clone()),
            ReplayKind::Landing => None,
        };
        sqlx::query(
            "insert into replays (id, issue_id, repo_id, run_id, candidate_run_id, approval_id, kind,
                attempt, revision, tests_bookmark, status, artifact_dir, created_at)
             values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'queued', ?, ?)",
        )
        .bind(id)
        .bind(issue.id)
        .bind(issue.repo_id)
        .bind(run.map(|r| r.id))
        .bind(candidate.map(|r| r.id))
        .bind(approval_id)
        .bind(kind.as_str())
        .bind(issue.attempt)
        .bind(revision)
        .bind(issue.tests_bookmark.clone())
        .bind(artifact_dir.display().to_string())
        .bind(crate::db::now_rfc3339())
        .execute(&self.db.pool)
        .await?;
        Ok(id)
    }

    pub async fn complete_replay(&self, replay_id: Uuid, payload: ReplayCompletion) -> Result<()> {
        let replay: Replay = sqlx::query_as("select * from replays where id = ?")
            .bind(replay_id)
            .fetch_optional(&self.db.pool)
            .await?
            .ok_or_else(|| anyhow!("replay {replay_id} not found"))?;
        let issue = self.issue(replay.issue_id).await?;

        if let Some(xml) = &payload.junit_xml {
            std::fs::create_dir_all(&replay.artifact_dir)?;
            std::fs::write(
                std::path::Path::new(&replay.artifact_dir).join("junit.xml"),
                xml,
            )?;
        }
        let report = payload
            .junit_xml
            .as_deref()
            .map(junit::parse)
            .unwrap_or_default();

        sqlx::query(
            "update replays set status = ?, exit_code = ?, error = ?, ancestry_ok = ?,
                revision = coalesce(?, revision), finished_at = ? where id = ?",
        )
        .bind(&payload.status)
        .bind(payload.exit_code)
        .bind(&payload.error)
        .bind(payload.ancestry_ok)
        .bind(&payload.revision)
        .bind(crate::db::now_rfc3339())
        .bind(replay_id)
        .execute(&self.db.pool)
        .await?;

        // What the colour of the suite means depends on the kind of replay: red is
        // the proof for a base replay, green is the proof for a head or landing one.
        // A green suite without the frozen tests is the failure this product exists
        // to catch: it proves the build, not the criteria.
        let criteria = self.criteria(issue.id).await?;
        let tokens: Vec<String> = criteria
            .iter()
            .map(|criterion| format!("ac{}", criterion.position))
            .collect();
        let frozen_tests_ran = report.cases.iter().any(|case| {
            tokens
                .iter()
                .any(|token| hulahoop_core::junit::contains_token(&case.name, token))
        });

        let judged = match replay.kind.as_str() {
            "base" if payload.status == "succeeded" => {
                if report.is_green() {
                    (
                        "failed",
                        "the authored tests pass at the base revision, so they prove nothing",
                    )
                } else if report.is_red() {
                    ("succeeded", "")
                } else {
                    ("failed", "the base replay produced no test results at all")
                }
            }
            "head" | "landing" if payload.status == "succeeded" && !frozen_tests_ran => (
                "failed",
                "the report has no test named after a criterion, so the frozen suite never ran at this revision",
            ),
            "head" | "landing" if payload.status == "succeeded" => {
                if report.is_green() {
                    ("succeeded", "")
                } else {
                    ("failed", "the suite is red")
                }
            }
            _ => (payload.status.as_str(), ""),
        };
        if judged.0 != payload.status {
            sqlx::query("update replays set status = ? where id = ?")
                .bind(judged.0)
                .bind(replay_id)
                .execute(&self.db.pool)
                .await?;
        }
        if !judged.1.is_empty() {
            self.comment(
                issue.id,
                hulahoop_core::AuthorType::Server,
                None,
                &format!(
                    "The {} replay cannot be trusted: {}.",
                    replay.kind, judged.1
                ),
                false,
            )
            .await?;
        }

        // Everything below keys on the judged status: a suite that ran without the
        // frozen tests is a failure even though the runner reported success, and it
        // has to have the same consequences as any other red replay.
        let status = judged.0;
        match replay.kind.as_str() {
            "head" => {
                if status == "succeeded" && report.is_green() {
                    sqlx::query(
                        "update issues set current_run_id = ?, updated_at = ? where id = ?",
                    )
                    .bind(replay.run_id)
                    .bind(crate::db::now_rfc3339())
                    .bind(issue.id)
                    .execute(&self.db.pool)
                    .await?;
                    self.transition(issue.id, IssueEvent::ImplementerCompleted)
                        .await?;
                } else {
                    let failing = report.failed_names();
                    let body = if failing.is_empty() {
                        format!(
                            "The head replay finished {} and produced no green suite. {}",
                            payload.status,
                            payload.error.clone().unwrap_or_default()
                        )
                    } else {
                        format!(
                            "The head replay is red. Failing tests:\n\n{}",
                            failing
                                .iter()
                                .map(|n| format!("- {n}"))
                                .collect::<Vec<_>>()
                                .join("\n")
                        )
                    };
                    if let Some(run_id) = replay.run_id {
                        sqlx::query("update runs set status = 'failed', error = ? where id = ?")
                            .bind(&body)
                            .bind(run_id)
                            .execute(&self.db.pool)
                            .await?;
                    }
                    self.comment(
                        issue.id,
                        hulahoop_core::AuthorType::Server,
                        None,
                        &body,
                        false,
                    )
                    .await?;
                }
            }
            "landing" => {
                if status == "succeeded" && report.is_green() {
                    // Green here means the rebased tree passes. The pull request is
                    // the human's next move, so it is opened now and the merge is
                    // still theirs to make.
                    if let Some(run_id) = replay
                        .run_id
                        .or(replay.candidate_run_id)
                        .or(issue.current_run_id)
                    {
                        self.queue_job(
                            issue.repo_id,
                            Some(issue.id),
                            "open_pr",
                            serde_json::json!({ "run_id": run_id }),
                        )
                        .await?;
                    }
                } else {
                    let failing = report.failed_names();
                    let body = format!(
                        "The landing replay on the current base turned {}. {}\n\n{}\n\nThe approval was voided.",
                        payload.status,
                        payload.error.clone().unwrap_or_default(),
                        failing
                            .iter()
                            .map(|n| format!("- {n}"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    );
                    sqlx::query("update approvals set voided_at = ? where issue_id = ? and voided_at is null")
                        .bind(crate::db::now_rfc3339())
                        .bind(issue.id)
                        .execute(&self.db.pool)
                        .await?;
                    self.comment(
                        issue.id,
                        hulahoop_core::AuthorType::Server,
                        None,
                        &body,
                        false,
                    )
                    .await?;
                    self.transition(issue.id, IssueEvent::LandingRed).await?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub async fn complete_job(
        &self,
        job_id: Uuid,
        payload: serde_json::Value,
        error: Option<String>,
    ) -> Result<()> {
        let job: RunnerJob = sqlx::query_as("select * from runner_jobs where id = ?")
            .bind(job_id)
            .fetch_optional(&self.db.pool)
            .await?
            .ok_or_else(|| anyhow!("job {job_id} not found"))?;

        let status = if error.is_some() {
            "failed"
        } else {
            "succeeded"
        };
        sqlx::query("update runner_jobs set status = ?, result = ?, error = ?, finished_at = ? where id = ?")
            .bind(status)
            .bind(payload.to_string())
            .bind(&error)
            .bind(crate::db::now_rfc3339())
            .bind(job_id)
            .execute(&self.db.pool)
            .await?;

        match job.kind.as_str() {
            "merge_verify" => {
                if let Some(issue_id) = job.issue_id {
                    let ancestry = payload
                        .get("ancestry_ok")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if ancestry {
                        let merge_revision = payload
                            .get("merge_revision")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        sqlx::query("update issues set merge_revision = ?, merged_at = ?, updated_at = ? where id = ?")
                            .bind(&merge_revision)
                            .bind(crate::db::now_rfc3339())
                            .bind(crate::db::now_rfc3339())
                            .bind(issue_id)
                            .execute(&self.db.pool)
                            .await?;
                        if let Ok(issue) = self.issue(issue_id).await {
                            sqlx::query("update repos set base_revision = ? where id = ?")
                                .bind(&merge_revision)
                                .bind(issue.repo_id)
                                .execute(&self.db.pool)
                                .await?;
                        }
                        self.transition(issue_id, IssueEvent::RecordMerge).await?;
                    } else {
                        let body = format!(
                            "`{}` does not contain the landing revision `{}`, so the merge was not recorded. Merge the revision the landing replay verified.",
                            payload
                                .get("merge_revision")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default(),
                            payload
                                .get("landing_bookmark")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                        );
                        self.comment(
                            issue_id,
                            hulahoop_core::AuthorType::Server,
                            None,
                            &body,
                            false,
                        )
                        .await?;
                    }
                }
            }
            "open_pr" => {
                let run_id = payload
                    .get("run_id")
                    .and_then(|v| v.as_str())
                    .and_then(|id| Uuid::parse_str(id).ok());
                let url = payload.get("pr_url").and_then(|v| v.as_str());
                match (run_id, url) {
                    (Some(run_id), Some(url)) if !url.is_empty() => {
                        sqlx::query(
                            "update runs set pr_url = ?, pr_number = ?, pr_branch = ? where id = ?",
                        )
                        .bind(url)
                        .bind(payload.get("pr_number").and_then(|v| v.as_i64()))
                        .bind(payload.get("pr_branch").and_then(|v| v.as_str()))
                        .bind(run_id)
                        .execute(&self.db.pool)
                        .await?;
                    }
                    _ => {
                        let reason = payload
                            .get("error")
                            .and_then(|v| v.as_str())
                            .unwrap_or("it reported no pull request");
                        if let Some(issue_id) = job.issue_id {
                            self.comment(
                                issue_id,
                                hulahoop_core::AuthorType::Server,
                                None,
                                &format!(
                                    "The pull request could not be opened: {reason}. The approval still stands, so the merge is yours to make by hand."
                                ),
                                false,
                            )
                            .await?;
                        }
                    }
                }
            }
            "fetch" => {
                if let (Some(revision), Some(job_repo)) = (
                    payload.get("base_revision").and_then(|v| v.as_str()),
                    Some(job.repo_id),
                ) {
                    sqlx::query("update repos set base_revision = ?, fetched_at = ? where id = ?")
                        .bind(revision)
                        .bind(crate::db::now_rfc3339())
                        .bind(job_repo)
                        .execute(&self.db.pool)
                        .await?;
                }
            }
            "preflight" => {
                if let (Some(agent_id), Some(ok), Some(detail)) = (
                    payload
                        .get("agent_id")
                        .and_then(|v| v.as_str())
                        .and_then(|raw| Uuid::parse_str(raw).ok()),
                    payload.get("ok").and_then(|v| v.as_bool()),
                    payload.get("detail").and_then(|v| v.as_str()),
                ) {
                    sqlx::query(
                        "insert into agent_preflight (agent_id, repo_id, ok, detail, checked_at)
                         values (?, ?, ?, ?, ?)
                         on conflict(agent_id, repo_id) do update set ok = excluded.ok, detail = excluded.detail, checked_at = excluded.checked_at",
                    )
                    .bind(agent_id)
                    .bind(job.repo_id)
                    .bind(ok)
                    .bind(detail)
                    .bind(crate::db::now_rfc3339())
                    .execute(&self.db.pool)
                    .await?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub async fn claim_replay(&self) -> Result<Option<Replay>> {
        let candidate: Option<Replay> = sqlx::query_as(
            "select * from replays where status = 'queued' order by created_at limit 1",
        )
        .fetch_optional(&self.db.pool)
        .await?;
        let Some(replay) = candidate else {
            return Ok(None);
        };
        let updated =
            sqlx::query("update replays set status = 'running' where id = ? and status = 'queued'")
                .bind(replay.id)
                .execute(&self.db.pool)
                .await?;
        if updated.rows_affected() == 0 {
            return Ok(None);
        }
        sqlx::query_as::<_, Replay>("select * from replays where id = ?")
            .bind(replay.id)
            .fetch_optional(&self.db.pool)
            .await
            .map_err(Into::into)
    }

    pub async fn claim_job(&self) -> Result<Option<RunnerJob>> {
        let candidate: Option<RunnerJob> = sqlx::query_as(
            "select * from runner_jobs where status = 'queued' order by created_at limit 1",
        )
        .fetch_optional(&self.db.pool)
        .await?;
        let Some(job) = candidate else {
            return Ok(None);
        };
        let updated = sqlx::query(
            "update runner_jobs set status = 'running', claimed_at = ? where id = ? and status = 'queued'",
        )
        .bind(crate::db::now_rfc3339())
        .bind(job.id)
        .execute(&self.db.pool)
        .await?;
        if updated.rows_affected() == 0 {
            return Ok(None);
        }
        sqlx::query_as::<_, RunnerJob>("select * from runner_jobs where id = ?")
            .bind(job.id)
            .fetch_optional(&self.db.pool)
            .await
            .map_err(Into::into)
    }

    // ------------------------------------------------------------ housekeeping

    /// A runner that stops heartbeating leaves runs `running` forever. The server
    /// is the one process that sees every heartbeat, so it declares them dead.
    pub async fn reap_dead_runs(&self) -> Result<Vec<Run>> {
        let threshold = Utc::now() - chrono::Duration::seconds(hulahoop_core::HEARTBEAT_DEAD_SECS);
        let dead: Vec<Run> = sqlx::query_as(
            "select * from runs where status = 'running' and (last_heartbeat_at is null or last_heartbeat_at < ?)",
        )
        .bind(threshold.to_rfc3339())
        .fetch_all(&self.db.pool)
        .await?;
        for run in &dead {
            if let Some(pgid) = run.pgid {
                kill_process_group(pgid as i32);
            }
            sqlx::query("update runs set status = 'failed', finished_at = ?, error = 'the runner stopped heartbeating, so the run was declared dead' where id = ?")
                .bind(crate::db::now_rfc3339())
                .bind(run.id)
                .execute(&self.db.pool)
                .await?;
        }
        Ok(dead)
    }
}

/// The completion payload a runner posts. `diff` is the unified diff the runner
/// captured, because the server is the only process that writes artifacts.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RunCompletion {
    pub status: String,
    #[serde(default)]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub diff: String,
    #[serde(default)]
    pub changed_paths: Vec<String>,
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub question: Option<String>,
    #[serde(default)]
    pub verdict: Option<String>,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub artifact_limit_hit: bool,
    /// The last lines the agent wrote, so a failure can quote the adapter instead of
    /// saying "it exited".
    #[serde(default)]
    pub transcript: Option<String>,
    /// Who the runner wrote into the commit. Reported so a run that fell back to
    /// the human's identity is visible instead of inferred.
    #[serde(default)]
    pub commit_name: Option<String>,
    #[serde(default)]
    pub commit_email: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ReplayCompletion {
    pub status: String,
    #[serde(default)]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub junit_xml: Option<String>,
    #[serde(default)]
    pub ancestry_ok: Option<bool>,
    #[serde(default)]
    pub revision: Option<String>,
}

/// The prompt files are embedded so a run never depends on the checkout's
/// contents, and the revision is a hash of exactly what was sent.
const TEST_AUTHOR_PROMPT: &str = include_str!("../../../prompts/test_author.md");
const IMPLEMENTER_PROMPT: &str = include_str!("../../../prompts/implementer.md");
const REVIEWER_PROMPT: &str = include_str!("../../../prompts/reviewer.md");

pub fn prompt_for(role: AgentRole) -> &'static str {
    match role {
        AgentRole::TestAuthor => TEST_AUTHOR_PROMPT,
        AgentRole::Implementer => IMPLEMENTER_PROMPT,
        AgentRole::Reviewer => REVIEWER_PROMPT,
    }
}

pub fn prompt_hash() -> String {
    let mut all = String::new();
    for role in AgentRole::ALL {
        all.push_str(prompt_for(role));
    }
    sha256_hex(all.as_bytes())
}

#[cfg(test)]
mod adapter_failure_tests {
    use super::*;

    fn completion(transcript: &str) -> RunCompletion {
        RunCompletion {
            status: "succeeded".into(),
            exit_code: Some(0),
            error: None,
            diff: String::new(),
            changed_paths: Vec::new(),
            commit: None,
            question: None,
            verdict: None,
            body: String::new(),
            artifact_limit_hit: false,
            transcript: Some(transcript.to_string()),
            commit_name: None,
            commit_email: None,
        }
    }

    #[test]
    fn the_provider_refusal_is_quoted_instead_of_exiting_quietly() {
        let payload = completion(
            r#"{"type":"auto_retry_end","success":false,"attempt":3,"finalError":"400: {\"type\":\"server_error\",\"message\":\"Model deepseek-4.1-flash isn't included in OpenCode Go.\"}"}"#,
        );
        let said = adapter_failure(&payload).expect("a reason");
        assert!(said.contains("isn't included in OpenCode Go"), "got {said}");
    }

    #[test]
    fn a_quiet_transcript_has_no_reason_to_quote() {
        assert!(
            adapter_failure(&completion(r#"{"type":"agent_settled","aborted":false}"#)).is_none()
        );
    }
}

#[cfg(test)]
mod criterion_tests {
    use super::*;

    fn verdict(line: &str) -> String {
        Engine::validate_criteria(&[line.to_string()])
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default()
    }

    #[test]
    fn a_goal_is_refused_by_name() {
        assert!(verdict("As a user, I will be able to create a Github app")
            .contains("states a goal rather than an observable result"));
        assert!(!verdict("In the UI, I will be able to see a list of my connections").is_empty());
    }

    #[test]
    fn a_sentence_a_test_can_assert_is_accepted() {
        // A keyword whitelist refused two of these; the freeze gate is what judges them,
        // by demanding one failing test per criterion.
        for line in [
            "the commit author is hulahoop-test-author when the implementer commits",
            "the github app credentials are used for the commit when the implementer commits",
            "Each agents who commits will have their own names in the commit username",
            "The app will use that information to do the commits on.",
            "subtract(5, 3) returns 2",
        ] {
            assert!(
                verdict(line).is_empty(),
                "should be accepted: {line} -> {}",
                verdict(line)
            );
        }
    }

    #[test]
    fn the_generic_and_the_too_short_still_go() {
        assert!(verdict("works").contains("too short"));
        assert!(verdict("it is correct").contains("generic"));
        assert!(verdict("the connections screen should work").contains("states a goal"));
    }
}
