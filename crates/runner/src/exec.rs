//! One tick of the loop: claim work, set up a checkout, run the agent, report.

use crate::adapter;
use crate::client::Client;
use crate::vcs::{Kind, Vcs};
use anyhow::{anyhow, bail, Context, Result};
use hulahoop_core::{contract, Adapter, AgentRole, ReplayKind};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

pub struct Executor {
    client: Client,
    data_dir: PathBuf,
}

#[derive(Default)]
struct Outcome {
    exit_code: Option<i64>,
    timed_out: bool,
    verdict: Option<String>,
    question: Option<String>,
    final_text: String,
    lines: Vec<String>,
    error: Option<String>,
}

impl Executor {
    pub fn new(client: Client, data_dir: PathBuf) -> Self {
        Executor { client, data_dir }
    }

    /// Claim at most one piece of work per tick, so a slow run cannot starve the
    /// heartbeats of another.
    pub async fn tick(&self, runner_id: &str) -> Result<bool> {
        if let Some(job) = self.client.claim_job().await? {
            self.run_job(job).await?;
            return Ok(true);
        }
        if let Some(run) = self.client.claim_run(runner_id).await? {
            self.run_run(run).await?;
            return Ok(true);
        }
        if let Some(replay) = self.client.claim_replay().await? {
            self.run_replay(replay).await?;
            return Ok(true);
        }
        Ok(false)
    }

    async fn run_run(&self, envelope: Value) -> Result<()> {
        let run = &envelope["run"];
        let spec = &envelope["spec"];
        let run_id = run["id"].as_str().context("run has no id")?.to_string();
        let result = self.execute_run(&run_id, spec).await;
        // A local checkout is a copy of the repository, and a copy per run
        // compounds: one project, ten issues, four attempts, three roles. It is
        // deleted the moment the run is over; the artifacts it produced live
        // under runs/<id>/ and are what anyone reads afterwards.
        if spec["runner_mode"].as_str() == Some("local") {
            let checkout = PathBuf::from(spec["checkout_path"].as_str().unwrap_or_default());
            let mirror = PathBuf::from(spec["repo_path"].as_str().unwrap_or_default());
            // The directory goes, the registration goes with it: the mirror is long
            // lived, and a workspace name for a path that no longer exists is litter.
            forget_workspace(&mirror, &format!("run-{run_id}"));
            if let Some(run_dir) = checkout.parent() {
                let _ = std::fs::remove_dir_all(run_dir);
            }
        }
        if let Err(error) = &result {
            tracing::error!(%error, run_id, "run failed to execute");
            let _ = self
                .client
                .complete_run(
                    &run_id,
                    json!({
                        "status": "failed",
                        "error": format!("the runner could not execute the run: {error}"),
                    }),
                )
                .await;
        }
        result.map(|_| ())
    }

    async fn execute_run(&self, run_id: &str, spec: &Value) -> Result<()> {
        let role = AgentRole::parse(spec["role"].as_str().unwrap_or_default())
            .ok_or_else(|| anyhow!("unknown role in spec"))?;
        let adapter = Adapter::parse(spec["adapter"].as_str().unwrap_or_default())
            .ok_or_else(|| anyhow!("unknown adapter in spec"))?;
        let repo_path = host_repository(spec)?;
        let checkout = PathBuf::from(spec["checkout_path"].as_str().unwrap_or_default());
        let artifact_dir = PathBuf::from(spec["artifact_dir"].as_str().unwrap_or_default());
        std::fs::create_dir_all(&artifact_dir)?;
        let base_bookmark = spec["base_bookmark"].as_str().unwrap_or("main").to_string();
        // The server resolved the bookmark when it accepted the run. Using the name
        // again here would branch from wherever the bookmark has since moved, and the
        // diff would then blame the agent for commits it never wrote.
        let base_revision = spec["base_revision"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let mut base = if base_revision.is_empty() {
            base_bookmark.clone()
        } else {
            base_revision
        };
        let issue_id = spec["issue_id"].as_str().unwrap_or_default().to_string();
        let attempt = spec["attempt"].as_i64().unwrap_or(0);
        let model = spec["model"].as_str().unwrap_or_default().to_string();
        let system_prompt = spec["prompt"].as_str().unwrap_or_default().to_string();
        let input_path = PathBuf::from(spec["input_path"].as_str().unwrap_or_default());
        let wall_clock = spec["timeout_minutes"]
            .as_i64()
            .unwrap_or(hulahoop_core::AGENT_WALL_CLOCK_MINUTES);

        let local = spec["runner_mode"].as_str() == Some("local");
        if local {
            // Local mode gives the run its own directory, and shares the mirror's
            // repository: one object store, one op log, no clone, no fetch, and no
            // bookmarks to push back afterwards.
            base = prepare_workspace(&repo_path, &checkout, &base, &format!("run-{run_id}"))?;
        } else {
            let vcs = Vcs::detect(&repo_path)?;
            vcs.ensure_vcs()?;
            vcs.add_workspace(&checkout, &base)?;
        }
        // A clone is its own repository; a workspace shares the one at repo_path.
        // Everything after this point asks the checkout, which is right either way.
        let vcs = Vcs::detect(&checkout)?;
        ensure_envrc(&checkout)?;

        self.client
            .event(
                run_id,
                "checkout",
                json!({ "path": checkout, "base": base, "bookmark": base_bookmark }),
            )
            .await
            .ok();

        if let Some(failure) = self.run_setup(&checkout, &artifact_dir).await? {
            self.client
                .complete_run(run_id, json!({ "status": "failed", "error": failure }))
                .await?;
            return Ok(());
        }

        let env = agent_env(spec, &checkout, &self.data_dir)?;
        let message = adapter::message_for(role, &input_path, &checkout);

        // Every adapter goes through the same spawn path, the fake one included:
        // a test that does not exercise the real plumbing proves nothing about it.
        let invocation = adapter::invocation(adapter, &model, &system_prompt, &message)?;
        let outcome = self
            .spawn_and_stream(
                run_id,
                &checkout,
                &artifact_dir,
                &invocation,
                &system_prompt,
                &message,
                &env,
                wall_clock,
            )
            .await?;

        // Frozen means frozen: a formatter run over the whole workspace rewrites the
        // suite the implementer is not allowed to touch, and that should not throw
        // away an hour of real work. Restoring first also means a deliberate edit to
        // a test has no effect, which is the rule the gate is protecting.
        let frozen: Vec<String> = spec["frozen_paths"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        // A Test Author is the one role that writes tests, so it is exempt.
        let mut restored = Vec::new();
        let before = if role == AgentRole::TestAuthor {
            Vec::new()
        } else {
            vcs.changed_paths(&checkout, &base).unwrap_or_default()
        };
        for path in &frozen {
            if before.contains(path) {
                vcs.restore_from(&checkout, &base, std::slice::from_ref(path))?;
                restored.push(path.clone());
            }
        }
        if !restored.is_empty() {
            self.client
                .event(run_id, "frozen_restored", json!({ "paths": restored }))
                .await
                .ok();
        }

        // The role owns the commit, whichever agent produced it: the working
        // copy is rewritten to the role's identity before the bookmark names it,
        // so `jj log` answers "who wrote this" without the runner's config. A
        // failure here is fatal on purpose: a machine-authored commit that carries
        // the human's name is worse than a failed run, because nothing shows it.
        // The address decides whether GitHub links the commit to the app's bot
        // user, so it is built from the app id and slug when both are known.
        let author_name = role.app_commit_name(spec["commit_app_slug"].as_str());
        let author_email = role.app_commit_email(
            spec["commit_app_id"].as_str(),
            spec["commit_app_slug"].as_str(),
        );
        vcs.set_working_copy_author(&checkout, &author_name, &author_email)
            .with_context(|| {
                format!(
                    "the loop could not take ownership of the commit as {}; refusing to produce a commit under somebody else's name",
                    author_name
                )
            })?;
        // A commit with no message cannot be pushed and reads as noise in `jj log`.
        // The role says who, the issue says what.
        if role != AgentRole::Reviewer {
            let prefix = match role {
                AgentRole::TestAuthor => "tests",
                _ => "implement",
            };
            let title = spec["issue"]["title"].as_str().unwrap_or("the issue");
            vcs.describe_as(
                &checkout,
                &format!("{prefix}: {title}"),
                &author_name,
                &author_email,
            )
            .with_context(|| format!("describing the {prefix} commit"))?;
        }
        // The suite the gate freezes has to be formatted, or every head built on it
        // fails `fmt-check` in CI and no agent may touch the frozen file to fix it.
        // The loop formats what it is about to freeze.
        if role != AgentRole::Reviewer {
            let frozen: Vec<String> = spec["frozen_paths"]
                .as_array()
                .map(|paths| {
                    paths
                        .iter()
                        .filter_map(|path| path.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let tests_bookmark = spec["tests_bookmark"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            self.format_tree(&checkout, &artifact_dir, &frozen, &tests_bookmark);
        }
        let (commit_name, commit_email) = vcs.working_copy_author(&checkout).unwrap_or_default();

        let diff = vcs.diff_from(&checkout, &base).unwrap_or_default();
        let changed_paths = vcs.changed_paths(&checkout, &base).unwrap_or_default();
        let commit = vcs.working_copy_revision(&checkout).unwrap_or_default();

        // The bookmark the server will resolve later. It is created here because
        // only the checkout knows the commit id.
        let published: Vec<String> = match role {
            AgentRole::TestAuthor => {
                let name = format!("tests/{issue_id}/cand/{run_id}");
                let _ = vcs.set_bookmark_here(&checkout, &name);
                vec![name]
            }
            AgentRole::Implementer => {
                let name = format!("issue/{issue_id}/head/{attempt}");
                let _ = vcs.set_bookmark_here(&checkout, &name);
                vec![name]
            }
            AgentRole::Reviewer => Vec::new(),
        };
        // A workspace shares the loop's repository, so the bookmark set above is
        // already visible to the replay that has to prove the work; nothing to push.
        let _ = &published;

        let status = if outcome.timed_out {
            "timed_out"
        } else if outcome.exit_code == Some(0) && outcome.error.is_none() {
            "succeeded"
        } else {
            "failed"
        };

        let artifact_limit_hit = dir_size(&artifact_dir) > hulahoop_core::ARTIFACT_LIMIT_BYTES;

        self.client
            .complete_run(
                run_id,
                json!({
                    "status": status,
                    "exit_code": outcome.exit_code,
                    "error": outcome.error,
                    "diff": diff,
                    "changed_paths": changed_paths,
                    "commit": commit,
                    "question": outcome.question,
                    "verdict": outcome.verdict,
                    "body": outcome.final_text,
                    "artifact_limit_hit": artifact_limit_hit,
                    "commit_name": commit_name,
                    "commit_email": commit_email,
                    "transcript": outcome
                        .lines
                        .iter()
                        .rev()
                        .take(40)
                        .rev()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("\n"),
                }),
            )
            .await?;

        // The checkout is removed on success; a failure keeps it so the human can
        // look at the mess a cancel or a crash left behind... for one attempt.
        // A workspace has to be forgotten or the repository accumulates them; a clone
        // is a directory, and `run_run` deletes the whole run directory.
        if status == "succeeded" && !local {
            let _ = vcs.remove_workspace(&checkout);
        }
        Ok(())
    }

    async fn run_replay(&self, envelope: Value) -> Result<()> {
        let replay = &envelope["replay"];
        let spec = &envelope["spec"];
        let replay_id = replay["id"]
            .as_str()
            .context("replay has no id")?
            .to_string();
        if let Err(error) = self.execute_replay(&replay_id, spec).await {
            tracing::error!(%error, replay_id, "replay failed to execute");
            let _ = self
                .client
                .complete_replay(
                    &replay_id,
                    json!({
                        "status": "timed_out",
                        "error": format!("the runner could not execute the replay: {error}"),
                    }),
                )
                .await;
        }
        Ok(())
    }

    async fn execute_replay(&self, replay_id: &str, spec: &Value) -> Result<()> {
        let kind = ReplayKind::parse(spec["kind"].as_str().unwrap_or_default())
            .ok_or_else(|| anyhow!("unknown replay kind"))?;
        let repo_path = host_repository(spec)?;
        let checkout = PathBuf::from(spec["checkout_path"].as_str().unwrap_or_default());
        let artifact_dir = PathBuf::from(spec["artifact_dir"].as_str().unwrap_or_default());
        std::fs::create_dir_all(&artifact_dir)?;

        let vcs = Vcs::detect(&repo_path)?;
        vcs.ensure_vcs()?;

        // Which revision the suite runs at, and what has to be laid on top of it.
        let (revision, restore_from, restore_paths) = match kind {
            ReplayKind::Base => {
                let revision = spec["base_bookmark"].as_str().unwrap_or("main").to_string();
                let candidate = spec["candidate_bookmark"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                if candidate.is_empty() {
                    (revision, None, Vec::new())
                } else {
                    // What the Test Author wrote, relative to the base it branched
                    // from. These files are laid on top of the untouched base, so a
                    // green suite there would mean the tests prove nothing.
                    // Where the candidate was authored, not where the repo stands now:
                    // a candidate branched before later commits landed must still lay
                    // down only its own files.
                    let base = spec["candidate_base_revision"]
                        .as_str()
                        .or_else(|| spec["base_revision"].as_str())
                        .filter(|value| !value.is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| revision.clone());
                    let paths = vcs
                        .changed_paths_between(&base, &candidate)
                        .unwrap_or_default();
                    (revision, Some(candidate), paths)
                }
            }
            ReplayKind::Head => {
                let head = spec["head_bookmark"]
                    .as_str()
                    .context("head replay without a head bookmark")?
                    .to_string();
                (head, None, Vec::new())
            }
            ReplayKind::Landing => {
                let approved_head = spec["approval_head_revision"]
                    .as_str()
                    .context("landing replay without the approved head")?
                    .to_string();
                let landing_bookmark = spec["landing_bookmark"]
                    .as_str()
                    .context("landing replay without a landing bookmark")?
                    .to_string();
                let head_bookmark = spec["head_bookmark"]
                    .as_str()
                    .context("landing replay without a head bookmark")?
                    .to_string();
                // The suite commit is where the chain starts, and it is the same
                // bookmark the freeze recorded: the landing replay has no candidate
                // run of its own.
                let candidate_bookmark = spec["candidate_bookmark"]
                    .as_str()
                    .or_else(|| spec["tests_bookmark"].as_str())
                    .context("landing replay without the suite it was approved from")?
                    .to_string();
                let base = spec["base_bookmark"].as_str().unwrap_or("main").to_string();
                // The suite is a commit of its own beneath the implementation, so
                // rebasing the head alone lands the implementation **without** the
                // tests: the preview then runs the base suite and proves nothing.
                // Move the whole chain, then put the run's bookmarks back so the
                // approved revision stays the one the human approved.
                let tests_before = vcs.resolve(&candidate_bookmark)?;
                let head_before = vcs.resolve(&head_bookmark)?;
                vcs.add_workspace(&checkout, &base)?;
                vcs.rebase_chain(&checkout, &tests_before, &base)?;
                // jj resolves an abandoned commit id to the commit that replaced it,
                // so the landed revision is the rewritten approved head.
                let landed = vcs.resolve(&approved_head)?;
                vcs.set_bookmark(&landing_bookmark, &landed)?;
                vcs.set_bookmark(&head_bookmark, &head_before)?;
                vcs.set_bookmark(&candidate_bookmark, &tests_before)?;
                vcs.edit(&checkout, &landed)?;
                (landing_bookmark.clone(), None, Vec::new())
            }
        };
        if kind != ReplayKind::Landing {
            vcs.add_workspace(&checkout, &revision)?;
        }
        ensure_envrc(&checkout)?;
        if let Some(from) = restore_from {
            vcs.restore_from(&checkout, &from, &restore_paths)?;
        }

        if let Some(failure) = self.run_setup(&checkout, &artifact_dir).await? {
            self.client
                .complete_replay(
                    replay_id,
                    json!({ "status": "timed_out", "error": failure }),
                )
                .await?;
            return Ok(());
        }

        let outcome = self.run_suite(&checkout, &artifact_dir, kind).await?;
        let junit_path = checkout.join(".hulahoop").join("junit.xml");
        let junit_xml = std::fs::read_to_string(&junit_path).ok();
        // The house rules run after the suite, at the same revision.
        let house_rules = if outcome.timed_out {
            Vec::new()
        } else {
            self.run_house_rules(&checkout, &artifact_dir, kind)?
        };

        // The runner reports whether the suite *ran*, not what its colour means: a
        // red suite is the wanted outcome for a base replay and the wrong one for a
        // head replay. The server owns that reading.
        let status = if outcome.timed_out {
            "timed_out"
        } else if junit_xml.is_some() {
            "succeeded"
        } else {
            "failed"
        };

        self.client
            .complete_replay(
                replay_id,
                json!({
                    "status": status,
                    "exit_code": outcome.exit_code,
                    "error": outcome.error,
                    "junit_xml": junit_xml,
                    "house_rules": house_rules,
                }),
            )
            .await?;

        let _ = vcs.remove_workspace(&checkout);
        Ok(())
    }

    async fn run_job(&self, envelope: Value) -> Result<()> {
        let job = &envelope["job"];
        let spec = &envelope["spec"];
        let job_id = job["id"].as_str().context("job has no id")?.to_string();
        let kind = spec["kind"].as_str().unwrap_or_default().to_string();
        let repo_path = host_repository(spec)?;

        let result = match kind.as_str() {
            "fetch" => self.job_fetch(&repo_path, spec).await,
            "preflight" => self.job_preflight(&repo_path, spec).await,
            "merge_verify" => self.job_merge_verify(&repo_path, spec).await,
            "bookmark" => self.job_bookmark(&repo_path, spec).await,
            "open_pr" => self.job_open_pr(&repo_path, spec).await,
            "review_pr" => job_review_pr(spec).await,
            "watch_checks" => job_watch_checks(spec).await,
            other => Err(anyhow!("unknown job kind {other}")),
        };

        match result {
            Ok(value) => self.client.complete_job(&job_id, value, None).await?,
            Err(error) => {
                self.client
                    .complete_job(&job_id, Value::Null, Some(error.to_string()))
                    .await?
            }
        }
        Ok(())
    }

    async fn job_fetch(&self, repo_path: &Path, span: &Value) -> Result<Value> {
        let vcs = Vcs::detect(repo_path)?;
        vcs.ensure_vcs()?;
        vcs.fetch()?;
        let base_bookmark = span["base_bookmark"].as_str().unwrap_or("main");
        // After a fetch the remote bookmark is the truth; fall back to the local
        // one when the repo has no remote.
        let remote = format!("{base_bookmark}@origin");
        let revision = vcs
            .base_revision(&remote)?
            .or(vcs.base_revision(base_bookmark)?)
            .unwrap_or_default();
        if revision.is_empty() {
            return Err(anyhow!(
                "bookmark {base_bookmark} does not exist in the repo; set it in the repo, then fetch again"
            ));
        }
        Ok(json!({ "base_revision": revision }))
    }

    async fn job_preflight(&self, repo_path: &Path, span: &Value) -> Result<Value> {
        let payload = &span["payload"];
        let adapter = Adapter::parse(payload["adapter"].as_str().unwrap_or_default())
            .ok_or_else(|| anyhow!("unknown adapter"))?;
        let model = payload["model"].as_str().unwrap_or_default().to_string();

        // The probe runs where the agent will run: inside the repo's dev shell,
        // because a CLI that only exists on the host PATH proves nothing. direnv
        // refuses an unauthorised `.envrc`, and the user's own tree is the one
        // place the runner may not approve silently, so approval is per repo and
        // idempotent.
        let has_envrc = repo_path.join(".envrc").exists();
        if has_envrc {
            let _ = Command::new("direnv").arg("allow").arg(repo_path).output();
        }
        let command = contract::probe_command(adapter, &model);
        let mut cmd = Command::new(&command[0]);
        if !has_envrc {
            // Without a dev shell the probe still tells us whether the CLI exists.
            cmd = Command::new(command.last().cloned().unwrap_or_default());
            cmd.arg("--version");
        } else {
            cmd.args(&command[1..]);
        }
        // The probe needs a home and a PATH the dev shell agrees with, and
        // nothing else from the runner.
        let home = contract::scratch_home(&self.data_dir, adapter);
        std::fs::create_dir_all(&home)?;
        seed_home(&home);
        let path =
            dev_shell_path(repo_path).unwrap_or_else(|| std::env::var("PATH").unwrap_or_default());
        let output = cmd
            .current_dir(repo_path)
            .env_clear()
            .env("PATH", path)
            .env("HOME", &home)
            .env("XDG_DATA_HOME", self.data_dir.join("direnv"))
            .env("NO_COLOR", "1")
            .output();

        let (ok, detail) = match output {
            Ok(output) => {
                let text = if output.status.success() {
                    String::from_utf8_lossy(&output.stdout).trim().to_string()
                } else {
                    String::from_utf8_lossy(&output.stderr).trim().to_string()
                };
                let detail = if text.is_empty() {
                    format!("exit {}", output.status)
                } else {
                    text.lines().last().unwrap_or_default().to_string()
                };
                (output.status.success(), detail)
            }
            Err(error) => (false, error.to_string()),
        };

        Ok(json!({
            "agent_id": payload["agent_id"].as_str().unwrap_or_default(),
            "ok": ok,
            "detail": detail,
        }))
    }

    async fn job_merge_verify(&self, repo_path: &Path, span: &Value) -> Result<Value> {
        let payload = &span["payload"];
        let merge_revision = payload["merge_revision"]
            .as_str()
            .context("merge_verify needs a merge revision")?;
        // The landing bookmark is the rebased commit the human merges; the
        // approval's own head is history by then, rewritten by the rebase.
        let landing = payload["landing_bookmark"]
            .as_str()
            .or_else(|| payload["head_bookmark"].as_str())
            .context("merge_verify needs the landing bookmark")?;
        let vcs = Vcs::detect(repo_path)?;
        let contained = vcs.contains(merge_revision, landing)?;
        Ok(json!({ "merge_revision": merge_revision, "ancestry_ok": contained }))
    }

    /// Push the approved commit and open the pull request a human reviews.
    ///
    /// The push is the runner's job, not the agent's: an agent runs with the
    /// network blocked, and a commit that was approved is the only thing that
    /// should ever reach the remote.
    async fn job_open_pr(&self, repo_path: &Path, spec: &Value) -> Result<Value> {
        let pull = &spec["pull_request"];
        let run_id = pull["run_id"]
            .as_str()
            .context("a pull request job needs the run it is for")?
            .to_string();
        let remote = spec["remote_url"]
            .as_str()
            .context("this repository has no origin remote, so there is nothing to open a pull request against")?;
        let (owner, name) = github_remote(remote)?;
        let jwt = pull["app_token"].as_str().context(
            "no GitHub app is stored, or its key cannot sign, so the loop cannot push as itself",
        )?;
        let head = pull["head_revision"]
            .as_str()
            .context("the run has no head revision to push")?;
        let branch = pull["branch"]
            .as_str()
            .context("the pull request job has no branch name")?
            .to_string();
        let base = spec["base_bookmark"].as_str().unwrap_or("main");

        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        let installation = installation_token(&http, jwt, &owner, &name).await?;
        // A rework rewrites the head, so the branch is force-updated rather than
        // rejected; the pull request itself is the same one.
        let existing = pull["number"].as_i64();
        push_commit(
            repo_path,
            &installation,
            &owner,
            &name,
            head,
            &branch,
            existing.is_some(),
        )?;

        let criteria = pull["criteria"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str())
                    .enumerate()
                    .map(|(index, text)| format!("{}. {text}", index + 1))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        let review = pull["review"].as_str().unwrap_or("");
        let body = format!(
            "{}\n\n---\n\n**Acceptance criteria**\n\n{}\n\n**The suite was red before the change and green after it.**\n\n**Reviewer verdict**\n\n{}\n\n_Opened by Hulahoop. The loop wrote the tests first, froze them, then implemented against them._\n",
            pull["body"].as_str().unwrap_or(""),
            criteria,
            review
        );
        let title = pull["title"]
            .as_str()
            .map(|title| format!("{title} (hulahoop)"))
            .unwrap_or_else(|| format!("hulahoop: {branch}"));
        if let Some(number) = existing {
            // The same pull request, with the new head and the story so far.
            update_pull_request(&http, &installation, &owner, &name, number, &title, &body).await?;
            return Ok(json!({
                "run_id": run_id,
                "pr_url": format!("https://github.com/{owner}/{name}/pull/{number}"),
                "pr_number": number,
                "pr_branch": branch,
            }));
        }
        let (pr_url, pr_number) = open_pull_request(
            &http,
            PullRequest {
                token: &installation,
                owner: &owner,
                repo: &name,
                branch: &branch,
                base,
                title: &title,
                body: &body,
            },
        )
        .await?;

        Ok(json!({
            "run_id": run_id,
            "pr_url": pr_url,
            "pr_number": pr_number,
            "pr_branch": branch,
        }))
    }

    async fn job_bookmark(&self, repo_path: &Path, span: &Value) -> Result<Value> {
        let payload = &span["payload"];
        let name = payload["name"]
            .as_str()
            .context("bookmark job needs a name")?;
        let revision = payload["from"].as_str().unwrap_or("main");
        let vcs = Vcs::detect(repo_path)?;
        let resolved = vcs.set_bookmark(name, revision)?;
        Ok(json!({ "revision": resolved }))
    }

    /// `just setup` is the only recipe allowed to need the network, and it is
    /// idempotent, so a failure here is a repo problem, not a flake.
    async fn run_setup(&self, checkout: &Path, artifact_dir: &Path) -> Result<Option<String>> {
        let log = artifact_dir.join("setup.log");
        let outcome = run_command(
            checkout,
            &["direnv", "exec", ".", "just", "setup"],
            hulahoop_core::SETUP_WALL_CLOCK_MINUTES,
            Some(&log),
        )?;
        if outcome.timed_out {
            return Ok(Some(format!(
                "`just setup` did not finish within {} minutes",
                hulahoop_core::SETUP_WALL_CLOCK_MINUTES
            )));
        }
        if outcome.exit_code != Some(0) {
            let tail = std::fs::read_to_string(&log).unwrap_or_default();
            let tail: String = tail.lines().rev().take(20).collect::<Vec<_>>().join("\n");
            return Ok(Some(format!(
                "`just setup` failed (exit {:?}). Hulahoop runs it in a clean shell, so this is usually a missing toolchain or a certificate problem.\n{}",
                outcome.exit_code, tail
            )));
        }
        Ok(None)
    }

    async fn run_suite(
        &self,
        checkout: &Path,
        artifact_dir: &Path,
        kind: ReplayKind,
    ) -> Result<Outcome> {
        let log = artifact_dir.join("just-test.log");
        let outcome = run_command(
            checkout,
            &["direnv", "exec", ".", "just", "test"],
            hulahoop_core::REPLAY_WALL_CLOCK_MINUTES,
            Some(&log),
        )?;
        let junit_path = checkout.join(".hulahoop").join("junit.xml");
        if junit_path.exists() {
            let _ = std::fs::copy(&junit_path, artifact_dir.join("junit.xml"));
        }
        let _ = kind;
        Ok(outcome)
    }

    /// `just fmt` when the repository declares it, so the tree the loop records is
    /// the tree the house rules would accept.
    fn format_tree(
        &self,
        checkout: &Path,
        artifact_dir: &Path,
        frozen: &[String],
        tests_bookmark: &str,
    ) {
        if std::fs::create_dir_all(artifact_dir).is_err() {
            return;
        }
        let summary = match run_command(
            checkout,
            &["direnv", "exec", ".", "just", "--summary"],
            hulahoop_core::SETUP_WALL_CLOCK_MINUTES,
            None,
        ) {
            Ok(outcome) => outcome.final_text,
            Err(_) => return,
        };
        if !summary.split_whitespace().any(|recipe| recipe == "fmt") {
            return;
        }
        let log = artifact_dir.join("just-fmt.log");
        let _ = run_command(
            checkout,
            &["direnv", "exec", ".", "just", "fmt"],
            hulahoop_core::SETUP_WALL_CLOCK_MINUTES,
            Some(&log),
        );
        // A formatter will happily rewrite a frozen test, and the tree has to stay
        // byte-identical to the suite the gate recorded, so those paths come back
        // from the suite afterwards.
        if frozen.is_empty() || tests_bookmark.trim().is_empty() {
            return;
        }
        let mut argv: Vec<&str> = vec!["restore", "--from", tests_bookmark, "--"];
        argv.extend(frozen.iter().map(String::as_str));
        let _ = Command::new("jj")
            .args(&argv)
            .current_dir(checkout)
            .output();
    }

    /// The recipes a repository declares beyond the suite, run at the same revision
    /// the suite ran at. The pull request's checks run `just ci`, so a head that
    /// passes the suite and fails `fmt-check` is a head that was never finished.
    fn run_house_rules(
        &self,
        checkout: &Path,
        artifact_dir: &Path,
        kind: ReplayKind,
    ) -> Result<Vec<Value>> {
        if kind == ReplayKind::Base {
            return Ok(Vec::new());
        }
        let summary = match run_command(
            checkout,
            &["direnv", "exec", ".", "just", "--summary"],
            hulahoop_core::SETUP_WALL_CLOCK_MINUTES,
            None,
        ) {
            Ok(outcome) => outcome.final_text,
            Err(_) => return Ok(Vec::new()),
        };
        let declared: Vec<&str> = summary.split_whitespace().collect();
        let mut results = Vec::new();
        for recipe in ["fmt-check", "lint"] {
            if !declared.contains(&recipe) {
                continue;
            }
            let log = artifact_dir.join(format!("just-{recipe}.log"));
            let outcome = run_command(
                checkout,
                &["direnv", "exec", ".", "just", recipe],
                hulahoop_core::SETUP_WALL_CLOCK_MINUTES,
                Some(&log),
            )?;
            results.push(json!({
                "recipe": recipe,
                "ok": !outcome.timed_out && outcome.exit_code == Some(0),
                "timed_out": outcome.timed_out,
                "exit_code": outcome.exit_code,
            }));
        }
        Ok(results)
    }

    /// Spawn the agent, stream its stdout, and heartbeat while it works.
    #[allow(clippy::too_many_arguments)]
    async fn spawn_and_stream(
        &self,
        run_id: &str,
        checkout: &Path,
        artifact_dir: &Path,
        invocation: &adapter::Invocation,
        system_prompt: &str,
        message: &str,
        env: &[(String, String)],
        wall_clock_minutes: i64,
    ) -> Result<Outcome> {
        let pid_slot = Arc::new(AtomicI64::new(0));
        let pgid_slot = Arc::new(AtomicI64::new(0));
        let (line_tx, mut line_rx) = mpsc::unbounded_channel::<(String, String)>();

        let mut child = adapter::spawn(invocation, checkout, env)?;
        let pid = child.id() as i64;
        pid_slot.store(pid, Ordering::SeqCst);
        pgid_slot.store(pid, Ordering::SeqCst);
        tracing::info!(run_id, pid, program = %invocation.program, "agent started");

        let _ = self.client.heartbeat(run_id, Some(pid), Some(pid)).await;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("agent has no stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("agent has no stderr"))?;

        // stdout carries JSONL events; stderr carries the agent's own noise.
        let tx_stdout = line_tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let _ = tx_stdout.send(("stdout".into(), line));
            }
        });
        let tx_stderr = line_tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let _ = tx_stderr.send(("stderr".into(), line));
            }
        });
        drop(line_tx);

        let wall_clock = Duration::from_secs((wall_clock_minutes as u64).saturating_mul(60));
        let started = Instant::now();
        let mut transcript = Transcript {
            forbidden: vec![system_prompt.to_string(), message.to_string()],
            ..Default::default()
        };
        let mut outcome = Outcome::default();

        let heartbeat_every = Duration::from_secs(hulahoop_core::HEARTBEAT_INTERVAL_SECS as u64);
        let mut next_heartbeat = Instant::now() + heartbeat_every;
        // The transcript is an artifact the review screen reads, so it is written
        // where the server expects it while the run is still going.
        let agent_log = std::fs::File::create(artifact_dir.join("agent.jsonl"))
            .ok()
            .map(|file| std::sync::Arc::new(std::sync::Mutex::new(std::io::BufWriter::new(file))));
        let mut log_bytes = 0usize;
        let mut artifact_limit_hit = false;

        loop {
            let wait = next_heartbeat.saturating_duration_since(Instant::now());
            tokio::select! {
                message = line_rx.recv() => {
                    match message {
                        Some((stream, line)) => {
                            log_bytes += line.len();
                            if stream == "stdout" {
                                if let Some(log) = &agent_log {
                                    use std::io::Write;
                                    if let Ok(mut handle) = log.lock() {
                                        let _ = writeln!(handle, "{line}");
                                        let _ = handle.flush();
                                    }
                                }
                            }
                            if stream == "stdout" {
                                transcript.feed(&line);
                            }
                            outcome.lines.push(line.clone());
                            let _ = self
                                .client
                                .event(run_id, if stream == "stdout" { "stdout" } else { "stderr" }, json!({ "line": line }))
                                .await;
                        }
                        None => break,
                    }
                }
                _ = tokio::time::sleep(wait) => {
                    let _ = self.client.heartbeat(run_id, Some(pid), Some(pid)).await;
                    next_heartbeat = Instant::now() + heartbeat_every;
                }
            }
            if started.elapsed() > wall_clock {
                outcome.timed_out = true;
                kill_group(pid);
                break;
            }
            if log_bytes as u64 > hulahoop_core::ARTIFACT_LIMIT_BYTES {
                artifact_limit_hit = true;
                kill_group(pid);
                outcome.error = Some(format!(
                    "the run wrote more than {} MB of output and was stopped",
                    hulahoop_core::ARTIFACT_LIMIT_BYTES / (1024 * 1024)
                ));
                break;
            }
        }

        if outcome.timed_out {
            outcome.error = Some(format!(
                "the agent did not finish within {wall_clock_minutes} minutes and was stopped"
            ));
        }

        // Drain whatever the process wrote between the last read and its exit.
        let status = loop {
            match child.try_wait()? {
                Some(status) => break Some(status),
                None => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        };
        outcome.exit_code = status.and_then(|s| s.code()).map(i64::from);
        if outcome.exit_code != Some(0) && outcome.error.is_none() && !outcome.timed_out {
            let tail: Vec<&str> = outcome
                .lines
                .iter()
                .rev()
                .take(15)
                .map(String::as_str)
                .collect();
            outcome.error = Some(format!(
                "the agent exited with {:?}.\n{}",
                outcome.exit_code,
                tail.into_iter().rev().collect::<Vec<_>>().join("\n")
            ));
        }
        // Whichever line the agent wrote last is the answer: an agent that reports
        // and then asks is asking, and the reverse also holds.
        let closing = transcript.final_text();
        let lower = closing.to_lowercase();
        let asked = lower.rfind("hulahoop_question:");
        let told = lower.rfind("hulahoop_verdict:");
        if asked.is_some() && asked > told {
            outcome.question = transcript.question();
        } else {
            outcome.verdict = transcript.verdict();
        }
        outcome.final_text = closing;
        if artifact_limit_hit {
            // Complete_run rejects an over-limit artifact dir on its own, but the
            // runner is the one that saw the size grow.
            outcome.error = outcome.error.or(Some("artifact limit reached".into()));
        }
        Ok(outcome)
    }
}

fn ensure_envrc(checkout: &Path) -> Result<()> {
    let envrc = checkout.join(".envrc");
    if !envrc.exists() {
        std::fs::write(&envrc, "use flake\n")?;
    }
    // An unallowed `.envrc` makes `direnv exec` refuse to run, and the checkout is
    // fresh for every run, so the allow has to happen here.
    let _ = std::process::Command::new("direnv")
        .arg("allow")
        .arg(checkout)
        .output();
    Ok(())
}

/// The agent's environment is the v0 boundary: an allowlist, a scratch home, and
/// a checkout. Nothing from the user's shell comes through.
fn agent_env(spec: &Value, checkout: &Path, data_dir: &Path) -> Result<Vec<(String, String)>> {
    let role = spec["role"].as_str().unwrap_or("run");
    let adapter =
        Adapter::parse(spec["adapter"].as_str().unwrap_or_default()).unwrap_or(Adapter::Pi);
    let home = contract::scratch_home(data_dir, adapter);
    std::fs::create_dir_all(&home)?;
    seed_home(&home);
    // Everything the agent is handed has to be absolute: its working directory is
    // the checkout, so a relative home resolves to nothing and pi reports a missing
    // API key instead of a missing directory.
    let home = std::fs::canonicalize(&home).unwrap_or(home);
    let checkout = std::fs::canonicalize(checkout).unwrap_or_else(|_| checkout.to_path_buf());
    let checkout = checkout.as_path();

    let path =
        dev_shell_path(checkout).unwrap_or_else(|| std::env::var("PATH").unwrap_or_default());
    let mut env: Vec<(String, String)> = vec![
        ("PATH".into(), path),
        ("HOME".into(), home.to_string_lossy().to_string()),
        (
            "TMPDIR".into(),
            std::env::temp_dir().to_string_lossy().to_string(),
        ),
        ("NO_COLOR".into(), "1".into()),
        (
            "HULAHOOP_INPUT".into(),
            spec["input_path"].as_str().unwrap_or_default().into(),
        ),
        (
            "HULAHOOP_CHECKOUT".into(),
            checkout.to_string_lossy().to_string(),
        ),
        (
            "HULAHOOP_RUN_ID".into(),
            spec["run_id"].as_str().unwrap_or_default().into(),
        ),
        ("HULAHOOP_ROLE".into(), role.into()),
        (
            "HULAHOOP_BASE_BOOKMARK".into(),
            spec["base_bookmark"].as_str().unwrap_or("main").into(),
        ),
        (
            "HULAHOOP_TESTS_BOOKMARK".into(),
            spec["tests_bookmark"].as_str().unwrap_or_default().into(),
        ),
        (
            "HULAHOOP_ARTIFACT_DIR".into(),
            spec["artifact_dir"].as_str().unwrap_or_default().into(),
        ),
        // Same allow-list as the runner, so `direnv exec` inside the checkout is
        // authorised even though HOME points at the scratch home.
        (
            "XDG_DATA_HOME".into(),
            data_dir.join("direnv").to_string_lossy().to_string(),
        ),
    ];
    // The fake adapter is a script the developer points at; passing its path
    // through the allowlist keeps `env_clear` honest for real adapters.
    if let Ok(script) = std::env::var("HULAHOOP_FAKE_SCRIPT") {
        env.push(("HULAHOOP_FAKE_SCRIPT".into(), script));
    }
    // Model credentials travel in the environment, never in a file in the repo.
    for key in contract::CREDENTIAL_ENV {
        if let Ok(value) = std::env::var(key) {
            env.push((key.into(), value));
        }
    }
    // A connector the user pasted wins over the machine's own login: the run acts
    // as the credential it was given, not as whoever started the runner. Files land
    // in the scratch home, so a subscription is used by a run and never by the host.
    if let Some(credential) = spec.get("credential") {
        for entry in credential["env"].as_array().into_iter().flatten() {
            let (Some(name), Some(value)) = (
                entry.get(0).and_then(|v| v.as_str()),
                entry.get(1).and_then(|v| v.as_str()),
            ) else {
                continue;
            };
            env.push((name.to_string(), value.to_string()));
        }
        for entry in credential["files"].as_array().into_iter().flatten() {
            let (Some(relative), Some(contents)) = (
                entry.get(0).and_then(|v| v.as_str()),
                entry.get(1).and_then(|v| v.as_str()),
            ) else {
                continue;
            };
            let path = home.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            write_private_file(&path, contents)
                .with_context(|| format!("writing {relative} into the run's home"))?;
        }
    }
    // The commit this role produces has to say who wrote it. jj reads its
    // identity from the environment, so the agent commits under the role's name
    // even where the runner's own identity is the machine's.
    if let Some(role) = AgentRole::parse(role) {
        env.push((
            "JJ_USER".into(),
            role.app_commit_name(spec["commit_app_slug"].as_str()),
        ));
        env.push((
            "JJ_EMAIL".into(),
            role.app_commit_email(
                spec["commit_app_id"].as_str(),
                spec["commit_app_slug"].as_str(),
            ),
        ));
    }
    // A token minted for the stored app; absent when there is no app, because a
    // variable holding an empty credential is worse than one that is not set.
    if let Some(token) = spec["github_token"]
        .as_str()
        .filter(|token| !token.is_empty())
    {
        env.push(("GITHUB_TOKEN".into(), token.to_string()));
    }
    Ok(env)
}

/// A credentials file the CLI will read. Owner-only, because the directory is
/// shared with the next run and a mode that drifts is a leak nobody notices.
fn write_private_file(path: &Path, contents: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents.as_bytes())?;
    Ok(())
}

/// Remove workspace directories that no run can still be using.
///
/// A runner that is killed mid-run leaves its clone behind, and nothing else knows
/// the directory is garbage. Six hours is longer than any wall clock the loop
/// allows, so a directory older than that belongs to no live run.
pub fn prune_workspace(data_dir: &Path) -> usize {
    const STALE_AFTER_SECS: u64 = 6 * 60 * 60;
    let root = data_dir.join("workspace");
    let mut pruned = 0;
    let Ok(projects) = std::fs::read_dir(&root) else {
        return 0;
    };
    for project in projects.flatten() {
        let Ok(issues) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for issue in issues.flatten() {
            let Ok(runs) = std::fs::read_dir(issue.path()) else {
                continue;
            };
            for run in runs.flatten() {
                let stale = run
                    .metadata()
                    .and_then(|meta| meta.modified())
                    .ok()
                    .and_then(|modified| modified.elapsed().ok())
                    .is_some_and(|age| age.as_secs() > STALE_AFTER_SECS);
                if stale && std::fs::remove_dir_all(run.path()).is_ok() {
                    pruned += 1;
                }
            }
        }
    }
    pruned
}

/// Produce the checkout a local runner works in: a fresh clone of the repository,
/// parked on the revision the server accepted the run against.
///
/// The base revision has to exist in the clone. It does not when the base was never
/// pushed, and the honest answer then is to say so instead of quietly branching
/// from the branch head and blaming the agent for the difference.
/// Environment that makes git and jj send the token, without it ever appearing in
/// `ps` or in a stored remote URL.
fn git_auth(token: &str) -> Vec<(&'static str, String)> {
    use base64::Engine as _;
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}"));
    vec![
        ("GIT_TERMINAL_PROMPT", "0".to_string()),
        ("GIT_CONFIG_COUNT", "1".to_string()),
        (
            "GIT_CONFIG_KEY_0",
            "http.https://github.com/.extraheader".to_string(),
        ),
        ("GIT_CONFIG_VALUE_0", format!("Authorization: Basic {auth}")),
    ]
}

/// The host repository a run borrows from. A repository attached by its remote has
/// no checkout on this machine, so the server gives it a mirror path it owns; the
/// first run clones into it, later runs fetch. Cloning here keeps the loop out of
/// any working copy the user is using, and lets a private repository work without
/// the token ever being written to disk.
fn host_repository(spec: &Value) -> Result<PathBuf> {
    let raw = spec["repo_path"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_string();
    if raw.is_empty() {
        bail!(
            "this repository has neither a checkout nor a remote, so a run has nothing to work from. Attach it by path or by its GitHub remote in Settings."
        );
    }
    let path = PathBuf::from(&raw);
    if !spec["repo_mirror"].as_bool().unwrap_or(false) {
        return Ok(path);
    }
    let url = spec["clone_url"].as_str().unwrap_or_default().to_string();
    if url.is_empty() {
        bail!("the mirror has no clone url, so the run cannot fetch the repository");
    }
    let token = spec["clone_token"].as_str().map(str::to_string);
    if path.join(".git").exists() || path.join(".jj").exists() {
        let mut command = Command::new("jj");
        command.args(["git", "fetch"]).current_dir(&path);
        if let Some(token) = &token {
            command.envs(git_auth(token));
        }
        let output = command
            .output()
            .context("running jj git fetch in the mirror")?;
        if !output.status.success() {
            bail!(
                "fetching the mirror at {} failed: {}",
                path.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        return Ok(path);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut command = Command::new("jj");
    command
        .args(["git", "clone", "--colocate", &url])
        .arg(&path);
    if let Some(token) = &token {
        command.envs(git_auth(token));
    }
    let output = command
        .output()
        .context("running jj git clone for the mirror")?;
    if !output.status.success() {
        bail!(
            "cloning {url} into the mirror at {} failed: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(path)
}

/// Local mode: the run gets its own directory but shares the mirror's repository.
///
/// A workspace costs the working copy and nothing else — no second object store,
/// no fetch, and no bookmarks to push back — which is what keeps a run per attempt
/// affordable. The price is one shared op log, so the repository pins history:
/// `immutable_heads` covers everything reachable from a bookmark or a tag, and jj
/// refuses to rewrite the revision a run started from.
fn prepare_workspace(root: &Path, checkout: &Path, base: &str, name: &str) -> Result<String> {
    let vcs = Vcs::detect(root)?;
    vcs.ensure_vcs()?;
    ensure_immutability(root)?;
    forget_workspace(root, name);
    if checkout.exists() {
        std::fs::remove_dir_all(checkout)
            .with_context(|| format!("clearing a stale workspace at {}", checkout.display()))?;
    }
    if let Some(parent) = checkout.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut refused = Vec::new();
    for candidate in [
        base.to_string(),
        format!("{base}@origin"),
        "main@origin".to_string(),
        "main".to_string(),
    ] {
        let added = std::process::Command::new("jj")
            .args(["workspace", "add"])
            .arg(checkout)
            .args(["--name", name, "-r", &candidate])
            .current_dir(root)
            .output()
            .context("running jj workspace add")?;
        if added.status.success() {
            return Ok(candidate);
        }
        refused.push(format!(
            "{candidate}: {}",
            String::from_utf8_lossy(&added.stderr).trim()
        ));
    }
    bail!(
        "the mirror holds no revision to start from ({}). A run cannot start from a revision that does not exist, and starting from the wrong one would blame the agent for changes it never made.",
        refused.join("; ")
    )
}

/// Drops a workspace registration before its directory disappears, so the mirror
/// does not accumulate names for paths that are gone.
fn forget_workspace(root: &Path, name: &str) {
    let _ = std::process::Command::new("jj")
        .args(["workspace", "forget", name])
        .current_dir(root)
        .output();
}

/// A run may extend history, never rewrite it. Bookmarks carry every revision the
/// loop has accepted, so the revset below makes them immutable for every workspace
/// that shares the repository — enforced by jj, with no wrapper in the agent's path.
fn ensure_immutability(root: &Path) -> Result<()> {
    // jj 0.47 keeps repo-level config outside the repository, and prints the path
    // it would read, so ask instead of guessing at `.jj/repo/config.toml`.
    let printed = std::process::Command::new("jj")
        .args(["config", "path", "--repo"])
        .current_dir(root)
        .output()
        .context("asking jj where the repository config lives")?;
    let path = String::from_utf8_lossy(&printed.stdout).trim().to_string();
    let config = if path.is_empty() {
        root.join(".jj").join("repo").join("config.toml")
    } else {
        PathBuf::from(path)
    };
    if let Some(parent) = config.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let existing = std::fs::read_to_string(&config).unwrap_or_default();
    if existing.contains("immutable_heads()") {
        return Ok(());
    }
    // The alias form is the one jj reads: `revsets.immutable_heads` is accepted as
    // a plain value and silently ignored, which is worse than no guard at all.
    let body = format!(
        "{existing}[revset-aliases]\n# Hulahoop pins history: a run may extend it, never rewrite it.\n\"immutable_heads()\" = \"builtin_immutable_heads() | bookmarks() | tags()\"\n"
    );
    std::fs::write(&config, body).with_context(|| format!("writing {}", config.display()))?;
    Ok(())
}

/// The dev shell decides what `just`, the compiler, and the test runner are; the
/// agent must see the same ones the suite will.
fn dev_shell_path(checkout: &Path) -> Option<String> {
    let output = std::process::Command::new("direnv")
        .args(["exec", ".", "sh", "-c", "printf %s \"$PATH\""])
        .current_dir(checkout)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Copy the agent's own login into the scratch home once. Both CLIs store their
/// credentials under `$HOME`, and a scratch home would otherwise log out.
fn seed_home(home: &Path) {
    let real_home = match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home),
        None => return,
    };
    let pi_src = real_home.join(".pi/agent");
    let pi_dst = home.join(".pi/agent");
    if pi_src.exists() && !pi_dst.exists() {
        let _ = std::fs::create_dir_all(&pi_dst);
        for name in ["auth.json", "models-store.json", "settings.json"] {
            let from = pi_src.join(name);
            if from.exists() {
                let _ = std::fs::copy(&from, pi_dst.join(name));
            }
        }
    }
    // `nix-direnv` lives in the user's direnv config; without it the agent's fresh
    // home re-evaluates every flake and hammers the GitHub API instead of reusing the
    // cached dev shell.
    let direnv_src = real_home.join(".config/direnv");
    let direnv_dst = home.join(".config/direnv");
    if direnv_src.exists() && !direnv_dst.exists() {
        let _ = std::process::Command::new("cp")
            .arg("-r")
            .arg(&direnv_src)
            .arg(&direnv_dst)
            .output();
    }
    let claude_src = real_home.join(".claude");
    let claude_dst = home.join(".claude");
    if claude_src.exists() && !claude_dst.exists() {
        let _ = std::fs::create_dir_all(&claude_dst);
        for name in [".credentials.json", "settings.json", ".claude.json"] {
            let from = claude_src.join(name);
            if from.exists() {
                let _ = std::fs::copy(&from, claude_dst.join(name));
            }
        }
    }
}

#[derive(Default)]
struct Transcript {
    /// Our own words, so an echo of the prompt is never read as an answer.
    forbidden: Vec<String>,
    texts: Vec<String>,
    /// Lines the adapter printed as plain text. A verdict sentence is often the
    /// last thing an agent says, and pi's JSON modes are not the only shape.
    plain: Vec<String>,
    json_lines: usize,
}

impl Transcript {
    /// Store a message unless it is an echo of our own prompt.
    fn keep(&mut self, text: String) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        if self
            .forbidden
            .iter()
            .any(|needle| !needle.is_empty() && trimmed.contains(needle.trim()))
        {
            return;
        }
        self.texts.push(text);
    }

    fn feed(&mut self, line: &str) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return;
        }
        if !trimmed.starts_with('{') {
            self.plain.push(trimmed.to_string());
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            return;
        };
        self.json_lines += 1;
        let kind = value["type"].as_str().unwrap_or_default();

        // pi: `message_end` carries the assistant message; `agent_end` carries the
        // whole conversation. claude: `assistant`/`result`.
        match kind {
            "message_end" | "assistant" | "result" => {
                if let Some(text) = collect_text(&value["message"]) {
                    self.keep(text);
                }
                if let Some(text) = value["result"].as_str() {
                    self.keep(text.to_string());
                }
            }
            "agent_end" => {
                if let Some(messages) = value["messages"].as_array() {
                    for message in messages {
                        if message["role"].as_str() == Some("assistant") {
                            if let Some(text) = collect_text(message) {
                                self.keep(text);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn final_text(&self) -> String {
        self.texts
            .last()
            .cloned()
            .or_else(|| self.plain.last().cloned())
            .unwrap_or_default()
    }

    fn verdict(&self) -> Option<String> {
        let haystack = self.haystack().to_lowercase();
        let index = haystack.rfind("hulahoop_verdict:")?;
        let rest = &haystack[index + "hulahoop_verdict:".len()..];
        let token: String = rest
            .chars()
            .skip_while(|c| c.is_whitespace())
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        match token.as_str() {
            "approved" => Some("approved".into()),
            "changes_requested" | "changes-requested" | "changesrequested" => {
                Some("changes_requested".into())
            }
            _ => Some(token).filter(|value| !value.is_empty()),
        }
    }

    fn haystack(&self) -> String {
        let mut all = self.texts.clone();
        all.extend(self.plain.iter().cloned());
        all.join("\n")
    }

    fn question(&self) -> Option<String> {
        let haystack = self.haystack();
        let index = haystack.to_lowercase().rfind("hulahoop_question:")?;
        let rest = &haystack[index + "hulahoop_question:".len()..];
        let question = rest.lines().next().unwrap_or_default().trim();
        Some(question.to_string())
    }
}

fn collect_text(message: &Value) -> Option<String> {
    match &message["content"] {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let joined = items
                .iter()
                .filter_map(|item| {
                    if item["type"].as_str() == Some("text") {
                        item["text"].as_str().map(str::to_string)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            (!joined.is_empty()).then_some(joined)
        }
        Value::Null => message["text"].as_str().map(str::to_string),
        _ => None,
    }
}

fn run_command(
    cwd: &Path,
    argv: &[&str],
    timeout_minutes: i64,
    log: Option<&Path>,
) -> Result<Outcome> {
    let mut command = std::process::Command::new(argv[0]);
    command
        .args(&argv[1..])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("running {argv:?}"))?;
    let pid = child.id() as i64;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let log_path = log.map(|path| path.to_path_buf());
    let writer = std::thread::spawn(move || {
        let mut collected = String::new();
        if let Some(path) = &log_path {
            let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
        }
        let mut streams: Vec<(Option<Box<dyn std::io::Read + Send>>, &str)> = Vec::new();
        if let Some(stdout) = stdout {
            streams.push((Some(Box::new(stdout)), "stdout"));
        }
        if let Some(stderr) = stderr {
            streams.push((Some(Box::new(stderr)), "stderr"));
        }
        for (stream, label) in streams {
            if let Some(stream) = stream {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    collected.push_str(&line);
                    collected.push('\n');
                    if let Some(path) = &log_path {
                        use std::io::Write;
                        if let Ok(mut file) = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(path)
                        {
                            let _ = writeln!(file, "[{label}] {line}");
                        }
                    }
                }
            }
        }
        collected
    });

    let deadline = Instant::now() + Duration::from_secs((timeout_minutes as u64) * 60);
    let mut timed_out = false;
    let exit = loop {
        match child.try_wait()? {
            Some(status) => break Some(status),
            None => {
                if Instant::now() > deadline {
                    timed_out = true;
                    kill_group(pid);
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    };
    let output = writer.join().unwrap_or_default();
    Ok(Outcome {
        exit_code: exit.and_then(|status| status.code()).map(i64::from),
        timed_out,
        error: if timed_out {
            Some(format!(
                "`{}` did not finish within {timeout_minutes} minutes",
                argv.join(" ")
            ))
        } else {
            None
        },
        final_text: output,
        ..Default::default()
    })
}

fn kill_group(pgid: i64) {
    #[cfg(unix)]
    {
        use nix::sys::signal::{killpg, Signal};
        use nix::unistd::Pid;
        let _ = killpg(Pid::from_raw(pgid as i32), Signal::SIGTERM);
        std::thread::sleep(Duration::from_secs(2));
        let _ = killpg(Pid::from_raw(pgid as i32), Signal::SIGKILL);
    }
    #[cfg(not(unix))]
    {
        let _ = pgid;
    }
}

fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            total += dir_size(&entry.path());
        } else {
            total += metadata.len();
        }
    }
    total
}

/// Unused import guard: `Kind` is matched in `vcs`, but the runner also needs to
/// know which template it is looking at when it explains a failure.
#[allow(dead_code)]
fn is_git(kind: &Kind) -> bool {
    matches!(kind, Kind::Git)
}

/// `git@github.com:owner/repo.git` and `https://github.com/owner/repo` name the
/// same repository; the API needs the two halves and refuses anything else.
fn github_remote(remote: &str) -> Result<(String, String)> {
    let trimmed = remote.trim().trim_end_matches(".git").trim_end_matches('/');
    let path = trimmed
        .strip_prefix("git@github.com:")
        .or_else(|| trimmed.strip_prefix("https://github.com/"))
        .or_else(|| trimmed.strip_prefix("ssh://git@github.com/"))
        .context("pull requests are opened on github.com, and this remote is somewhere else")?;
    let mut parts = path.split('/');
    let owner = parts
        .next()
        .filter(|owner| !owner.is_empty())
        .context("the remote names no owner")?;
    let name = parts
        .next()
        .filter(|name| !name.is_empty())
        .context("the remote names no repository")?;
    Ok((owner.to_string(), name.to_string()))
}

/// The app JWT names the app; the installation token is what can push and open a
/// pull request for one particular repository.
async fn installation_token(
    http: &reqwest::Client,
    jwt: &str,
    owner: &str,
    repo: &str,
) -> Result<String> {
    let response = http
        .get(format!(
            "https://api.github.com/repos/{owner}/{repo}/installation"
        ))
        .header("accept", "application/vnd.github+json")
        .header("user-agent", "hulahoop")
        .bearer_auth(jwt)
        .send()
        .await
        .context("asking github which installation covers this repository")?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        bail!(
            "github would not name the installation for {owner}/{repo} ({status}): {}",
            body["message"].as_str().unwrap_or("no reason given")
        );
    }
    let installation = body["id"]
        .as_i64()
        .context("the installation response had no id")?;
    let response = http
        .post(format!(
            "https://api.github.com/app/installations/{installation}/access_tokens"
        ))
        .header("accept", "application/vnd.github+json")
        .header("user-agent", "hulahoop")
        .bearer_auth(jwt)
        .json(&json!({}))
        .send()
        .await
        .context("minting an installation token")?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        bail!(
            "github refused an installation token ({status}): {}",
            body["message"].as_str().unwrap_or("no reason given")
        );
    }
    body["token"]
        .as_str()
        .map(str::to_string)
        .context("github minted no installation token")
}

/// Push one commit to a new branch. The token travels in the environment rather
/// than the command line, so it is not in `ps` while the push runs.
/// How long the runner waits for GitHub's checks before it reports a timeout, and
/// how often it asks.
const CHECK_WATCH_SECS: u64 = 45 * 60;
const CHECK_POLL_SECS: u64 = 20;

fn push_commit(
    repo_path: &Path,
    token: &str,
    owner: &str,
    repo: &str,
    head: &str,
    branch: &str,
    force: bool,
) -> Result<()> {
    let refspec = if force {
        format!("+{head}:refs/heads/{branch}")
    } else {
        format!("{head}:refs/heads/{branch}")
    };
    let output = Command::new("git")
        .args([
            "push",
            &format!("https://github.com/{owner}/{repo}.git"),
            &refspec,
        ])
        .current_dir(repo_path)
        .envs(git_auth(token))
        .output()
        .context("running git push")?;
    if !output.status.success() {
        bail!(
            "git could not push {branch}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// The pull request already exists, so the change is a body and title refresh.
#[allow(clippy::too_many_arguments)]
async fn update_pull_request(
    http: &reqwest::Client,
    token: &str,
    owner: &str,
    repo: &str,
    number: i64,
    title: &str,
    body: &str,
) -> Result<()> {
    let response = http
        .patch(format!(
            "https://api.github.com/repos/{owner}/{repo}/pulls/{number}"
        ))
        .header("accept", "application/vnd.github+json")
        .header("user-agent", "hulahoop")
        .bearer_auth(token)
        .json(&json!({ "title": title, "body": body }))
        .send()
        .await
        .context("updating the pull request")?;
    let status = response.status();
    if !status.is_success() {
        let answer: Value = response.json().await.unwrap_or(Value::Null);
        bail!(
            "github refused the pull request update ({status}): {}",
            answer["message"].as_str().unwrap_or("no reason given")
        );
    }
    Ok(())
}

/// Polls the checks GitHub runs on the head, and the reviews the team has left.
/// Ready means: every check concluded without a failure, and nobody is still
/// asking for changes.
async fn job_watch_checks(spec: &Value) -> Result<Value> {
    let pull = &spec["pull_request"];
    let number = pull["number"]
        .as_i64()
        .context("watching checks needs a pull request number")?;
    let sha = pull["sha"]
        .as_str()
        .filter(|sha| !sha.is_empty())
        .context("watching checks needs the head revision")?
        .to_string();
    let token = pull["app_token"].as_str().context(
        "no GitHub app is stored, or its key cannot sign, so the loop cannot read the checks",
    )?;
    let remote = spec["remote_url"]
        .as_str()
        .context("this repository has no origin remote, so there are no checks to read")?;
    let (owner, name) = github_remote(remote)?;
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let installation = installation_token(&http, token, &owner, &name).await?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(CHECK_WATCH_SECS);
    loop {
        let checks = http
            .get(format!(
                "https://api.github.com/repos/{owner}/{name}/commits/{sha}/check-runs?per_page=100"
            ))
            .header("accept", "application/vnd.github+json")
            .header("user-agent", "hulahoop")
            .bearer_auth(&installation)
            .send()
            .await
            .context("reading the check runs")?;
        let status = checks.status();
        let answer: Value = checks.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            bail!(
                "github refused the check runs ({status}): {}",
                answer["message"].as_str().unwrap_or("no reason given")
            );
        }
        let runs = answer["check_runs"].as_array().cloned().unwrap_or_default();
        let pending = runs
            .iter()
            .filter(|check| check["status"].as_str() != Some("completed"))
            .count();
        let failing: Vec<String> = runs
            .iter()
            .filter(|check| {
                matches!(
                    check["conclusion"].as_str(),
                    Some("failure")
                        | Some("timed_out")
                        | Some("cancelled")
                        | Some("action_required")
                )
            })
            .filter_map(|check| check["name"].as_str().map(str::to_string))
            .collect();
        if !failing.is_empty() {
            return Ok(json!({
                "sha": sha,
                "number": number,
                "conclusion": "failure",
                "failing": failing,
                "change_requests": open_change_requests(&http, &installation, &owner, &name, number).await?,
            }));
        }
        if pending == 0 && !runs.is_empty() {
            let requests =
                open_change_requests(&http, &installation, &owner, &name, number).await?;
            if requests.is_empty() {
                return Ok(json!({
                    "sha": sha,
                    "number": number,
                    "conclusion": "success",
                    "failing": Vec::<String>::new(),
                    "change_requests": Vec::<String>::new(),
                }));
            }
            return Ok(json!({
                "sha": sha,
                "number": number,
                "conclusion": "changes requested",
                "failing": Vec::<String>::new(),
                "change_requests": requests,
            }));
        }
        if std::time::Instant::now() > deadline {
            return Ok(json!({
                "sha": sha,
                "number": number,
                "conclusion": "timeout",
                "failing": Vec::<String>::new(),
                "change_requests": Vec::<String>::new(),
                "pending": pending,
            }));
        }
        tokio::time::sleep(std::time::Duration::from_secs(CHECK_POLL_SECS)).await;
    }
}

/// The people still asking for changes on the pull request. A resolved thread does
/// not clear the `CHANGES_REQUESTED` state, so this reads the latest review per
/// author and counts only those that are still the last word.
async fn open_change_requests(
    http: &reqwest::Client,
    token: &str,
    owner: &str,
    repo: &str,
    number: i64,
) -> Result<Vec<String>> {
    let response = http
        .get(format!(
            "https://api.github.com/repos/{owner}/{repo}/pulls/{number}/reviews?per_page=100"
        ))
        .header("accept", "application/vnd.github+json")
        .header("user-agent", "hulahoop")
        .bearer_auth(token)
        .send()
        .await
        .context("reading the reviews")?;
    let answer: Value = response.json().await.unwrap_or(Value::Null);
    let reviews = answer.as_array().cloned().unwrap_or_default();
    let mut latest: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for review in &reviews {
        let author = review["user"]["login"].as_str().unwrap_or("someone");
        let state = review["state"].as_str().unwrap_or("");
        if state == "COMMENTED" {
            continue;
        }
        latest.insert(author.to_string(), state.to_string());
    }
    Ok(latest
        .into_iter()
        .filter(|(_, state)| state == "CHANGES_REQUESTED")
        .map(|(author, _)| author)
        .collect())
}

/// A review the app cannot file on its own pull request still has to be visible,
/// so it is posted as a comment carrying the same verdict.
async fn comment_on_pull_request(
    http: &reqwest::Client,
    token: &str,
    owner: &str,
    repo: &str,
    number: i64,
    event: &str,
    body: &str,
) -> Result<Value> {
    let headline = if event == "APPROVE" {
        "**Reviewer verdict: approved**"
    } else {
        "**Reviewer verdict: changes requested**"
    };
    let response = http
        .post(format!(
            "https://api.github.com/repos/{owner}/{repo}/issues/{number}/comments"
        ))
        .header("accept", "application/vnd.github+json")
        .header("user-agent", "hulahoop")
        .bearer_auth(token)
        .json(&json!({ "body": format!("{headline}\n\n{body}") }))
        .send()
        .await
        .context("posting the verdict as a comment")?;
    let status = response.status();
    let answer: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        bail!(
            "github refused the verdict comment ({status}): {}",
            answer["message"].as_str().unwrap_or("no reason given")
        );
    }
    Ok(json!({
        "number": number,
        "comment_id": answer["id"].as_i64(),
        "via": "comment",
    }))
}

/// The reviewer's verdict as a GitHub review, so the pull request carries who
/// approved it instead of appearing approved by nobody.
async fn job_review_pr(spec: &Value) -> Result<Value> {
    let pull = &spec["pull_request"];
    let number = pull["number"]
        .as_i64()
        .context("the review needs a pull request number")?;
    let token = pull["app_token"].as_str().context(
        "no GitHub app is stored, or its key cannot sign, so the loop cannot review as itself",
    )?;
    let remote = spec["remote_url"]
        .as_str()
        .context("this repository has no origin remote, so there is no pull request to review")?;
    let (owner, name) = github_remote(remote)?;
    let verdict = pull["verdict"].as_str().unwrap_or("changes_requested");
    let event = if verdict == "approved" {
        "APPROVE"
    } else {
        "REQUEST_CHANGES"
    };
    let body = pull["body"].as_str().unwrap_or("");
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let installation = installation_token(&http, token, &owner, &name).await?;
    let response = http
        .post(format!(
            "https://api.github.com/repos/{owner}/{name}/pulls/{number}/reviews"
        ))
        .header("accept", "application/vnd.github+json")
        .header("user-agent", "hulahoop")
        .bearer_auth(&installation)
        .json(&json!({ "event": event, "body": body }))
        .send()
        .await
        .context("posting the review")?;
    let status = response.status();
    let answer: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let detail = answer["errors"]
            .as_array()
            .map(|errors| {
                errors
                    .iter()
                    .filter_map(|error| {
                        // `errors` is an array of strings for a review refusal and an
                        // array of objects elsewhere, so accept both shapes.
                        error
                            .as_str()
                            .or_else(|| error["message"].as_str())
                            .map(str::to_string)
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .filter(|text| !text.is_empty())
            .or_else(|| answer["message"].as_str().map(str::to_string))
            .unwrap_or_else(|| "no reason given".to_string());
        // GitHub refuses a review on a pull request the same identity opened, and
        // every branch here is pushed by the one app. The verdict still belongs on
        // the pull request, so it goes as a comment instead of vanishing.
        if detail.contains("your own pull request") {
            return comment_on_pull_request(
                &http,
                &installation,
                &owner,
                &name,
                number,
                event,
                body,
            )
            .await;
        }
        bail!("github refused the review ({status}): {detail}");
    }
    Ok(json!({
        "number": number,
        "review_id": answer["id"].as_i64(),
        "state": answer["state"].as_str(),
    }))
}

struct PullRequest<'a> {
    token: &'a str,
    owner: &'a str,
    repo: &'a str,
    branch: &'a str,
    base: &'a str,
    title: &'a str,
    body: &'a str,
}

async fn open_pull_request(http: &reqwest::Client, pull: PullRequest<'_>) -> Result<(String, i64)> {
    let PullRequest {
        token,
        owner,
        repo,
        branch,
        base,
        title,
        body,
    } = pull;
    let response = http
        .post(format!("https://api.github.com/repos/{owner}/{repo}/pulls"))
        .header("accept", "application/vnd.github+json")
        .header("user-agent", "hulahoop")
        .bearer_auth(token)
        .json(&json!({
            "title": title,
            "head": branch,
            "base": base,
            "body": body,
        }))
        .send()
        .await
        .context("opening the pull request")?;
    let status = response.status();
    let answer: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        bail!(
            "github refused the pull request ({status}): {}",
            answer["message"].as_str().unwrap_or("no reason given")
        );
    }
    let url = answer["html_url"]
        .as_str()
        .context("the pull request response had no url")?
        .to_string();
    let number = answer["number"].as_i64().unwrap_or_default();
    Ok((url, number))
}

#[cfg(test)]
mod remote_tests {
    use super::github_remote;

    #[test]
    fn a_github_remote_names_its_owner_and_repository() {
        for remote in [
            "git@github.com:theskilllab/hulahoop.git",
            "https://github.com/theskilllab/hulahoop",
            "https://github.com/theskilllab/hulahoop.git",
            "ssh://git@github.com/theskilllab/hulahoop.git",
        ] {
            let (owner, name) = github_remote(remote).expect(remote);
            assert_eq!(owner, "theskilllab", "{remote}");
            assert_eq!(name, "hulahoop", "{remote}");
        }
    }

    #[test]
    fn a_remote_somewhere_else_is_refused_by_name() {
        let error = github_remote("https://gitlab.com/theskilllab/hulahoop.git")
            .expect_err("gitlab is not github");
        assert!(
            error.to_string().contains("somewhere else"),
            "unhelpful: {error}"
        );
    }
}
