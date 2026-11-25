//! One tick of the loop: claim work, set up a checkout, run the agent, report.

use crate::adapter;
use crate::client::Client;
use crate::redact::Redactor;
use crate::vcs::{Kind, Vcs};
use anyhow::{anyhow, bail, Context, Result};
use hulahoop_core::credential::{Refresh, CLAUDE_CREDENTIALS_FILE, PI_AUTH_FILE};
use hulahoop_core::{contract, github_api, slugify, Adapter};
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
        if let Some(job) = self.client.claim_job(false).await? {
            self.run_job(job).await?;
            return Ok(true);
        }
        if let Some(run) = self.client.claim_run(runner_id).await? {
            self.run_run(run).await?;
            return Ok(true);
        }
        Ok(false)
    }

    /// A second runner lane answers questions while a long agent run is active.
    /// Lumi never edits a checkout, so it can share the data directory safely.
    pub async fn tick_lumi(&self) -> Result<bool> {
        if let Some(job) = self.client.claim_job(true).await? {
            self.run_job(job).await?;
            return Ok(true);
        }
        Ok(false)
    }

    async fn run_run(&self, envelope: Value) -> Result<()> {
        let run = &envelope["run"];
        let spec = &envelope["spec"];
        let run_id = run["id"].as_str().context("run has no id")?.to_string();
        let result = self.execute_run(&run_id, spec).await;
        // A repo-less run's scratch directory is private to the run and goes the
        // moment it is over, whatever the outcome; its artifacts stay under
        // runs/<id>/ like any other run's.
        if is_repo_less(spec) {
            remove_scratch(&self.data_dir, &run_id);
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

    /// A run on a task with no repository: an empty scratch directory, the
    /// agent, and its closing message. No repository, lock, clone, workspace,
    /// setup, checks, diff, commit or push — there is nothing to change but the
    /// answer.
    async fn execute_scratch_run(&self, run_id: &str, spec: &Value) -> Result<()> {
        let adapter = Adapter::parse(spec["adapter"].as_str().unwrap_or_default())
            .ok_or_else(|| anyhow!("unknown adapter in spec"))?;
        let scratch = prepare_scratch(&self.data_dir, run_id)?;
        let artifact_dir = PathBuf::from(spec["artifact_dir"].as_str().unwrap_or_default());
        std::fs::create_dir_all(&artifact_dir)?;
        let model = spec["model"].as_str().unwrap_or_default().to_string();
        let system_prompt = spec["prompt"].as_str().unwrap_or_default().to_string();
        let input_path = PathBuf::from(spec["input_path"].as_str().unwrap_or_default());
        let wall_clock = spec["timeout_minutes"]
            .as_i64()
            .unwrap_or(hulahoop_core::AGENT_WALL_CLOCK_MINUTES);

        self.client
            .event(
                run_id,
                "checkout",
                json!({ "path": scratch, "scratch": true }),
            )
            .await
            .ok();

        // The run's login files go however this function ends, after the
        // hand-back has read them: the next run of this shared home must not
        // start on this run's connector.
        let home = agent_home(spec, &self.data_dir)?;
        let login = LoginFiles::new(spec, &home);
        let env = agent_env(spec, &scratch, &self.data_dir, &home)?;
        let redactor = Redactor::new(spec, &env);
        self.write_skills(spec, &scratch)?;
        let agent_name = spec["agent_name"].as_str().unwrap_or_default();
        let message = adapter::scratch_message_for(agent_name, &input_path, &scratch);
        let prompt_file = adapter::write_system_prompt(&artifact_dir, &system_prompt)?;
        let invocation = adapter::invocation(adapter, &model, &prompt_file, &message)?;
        let outcome = self
            .spawn_and_stream(
                run_id,
                &scratch,
                &artifact_dir,
                &invocation,
                &system_prompt,
                &message,
                &env,
                wall_clock,
                None,
                &redactor,
            )
            .await;
        self.hand_back(&format!("runs/{run_id}"), spec, &home).await;
        drop(login);
        let outcome = outcome?;

        let artifact_limit_hit = dir_size(&artifact_dir) > hulahoop_core::ARTIFACT_LIMIT_BYTES;
        self.client
            .complete_run(
                run_id,
                json!({
                    "status": run_status(&outcome),
                    "exit_code": outcome.exit_code,
                    "error": outcome.error,
                    "diff": "",
                    "changed_paths": Vec::<String>::new(),
                    "commit": Value::Null,
                    "question": outcome.question,
                    "verdict": outcome.verdict,
                    "body": outcome.final_text,
                    "artifact_limit_hit": artifact_limit_hit,
                    "transcript": transcript_tail(&outcome),
                }),
            )
            .await?;
        Ok(())
    }

    async fn execute_run(&self, run_id: &str, spec: &Value) -> Result<()> {
        if is_repo_less(spec) {
            return self.execute_scratch_run(run_id, spec).await;
        }
        let adapter = Adapter::parse(spec["adapter"].as_str().unwrap_or_default())
            .ok_or_else(|| anyhow!("unknown adapter in spec"))?;
        let repo_path = host_repository(spec).await?;
        let checkout = owned_checkout(
            &self.data_dir,
            &spec["checkout_project_id"],
            &spec["issue_id"],
        )?;
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

        // The task's checkout is a workspace of the mirror, reused across its
        // turns; the server allows one live run per task, so no other turn is in
        // it. Everything after this point asks the checkout.
        let mirror = Vcs::detect(&repo_path)?;
        mirror.ensure_vcs()?;
        // A task that was in flight when its repository moved to a fresh mirror
        // can name a revision the mirror never had; its bookmark on the remote,
        // or the repository's base there, is where it picks up instead.
        let repo_base = spec["repo_base_bookmark"].as_str().unwrap_or_default();
        let start = mirror.resolve_first(&[
            base.clone(),
            base_bookmark.clone(),
            on_origin(&base_bookmark),
            on_origin(repo_base),
            repo_base.to_string(),
        ])?;
        if mirror.resolve(&base).ok().as_deref() != Some(start.as_str()) {
            tracing::warn!(run_id, %base, %start, "the start revision is not in the mirror; starting from its bookmark instead");
        }
        base = start;
        mirror.add_workspace(&checkout, &base)?;
        let vcs = Vcs::detect(&checkout)?;

        // A rework starts on the previous head; the task lands on the repository's
        // base. Moving the work onto the newest base before the turn keeps the pull
        // request mergeable and leaves conflicts to the agent that understands them.
        let carried = match spec["repo_base_bookmark"].as_str().filter(|b| !b.is_empty()) {
            Some(landing) => match vcs.newest_base(landing) {
                Some(newest) => vcs.carry_onto(&checkout, &base, &newest).unwrap_or_else(|error| {
                    tracing::warn!(%error, run_id, "could not move the work onto the newest base");
                    None
                }),
                None => None,
            },
            None => None,
        };
        // The scaffolding merge goes however this function ends: a setup failure,
        // an error from the agent, or a normal finish.
        let _scaffold = Scaffold {
            repo: repo_path.clone(),
            merge: carried.as_ref().and_then(|c| c.merge.clone()),
        };
        if let Some(carried) = &carried {
            base = carried.diff_base.clone();
            self.client
                .event(
                    run_id,
                    "rebase",
                    json!({ "onto": spec["repo_base_bookmark"], "conflicts": carried.conflicts }),
                )
                .await
                .ok();
        }
        // After the carry: the working copy it builds starts from the base's tree,
        // which would drop a generated `.envrc` written before it.
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

        // The run's login files go however this function ends, after the
        // hand-back has read them: the next run of this shared home must not
        // start on this run's connector.
        let home = agent_home(spec, &self.data_dir)?;
        let login = LoginFiles::new(spec, &home);
        let env = agent_env(spec, &checkout, &self.data_dir, &home)?;
        let redactor = Redactor::new(spec, &env);
        self.write_skills(spec, &checkout)?;
        let agent_name = spec["agent_name"].as_str().unwrap_or_default();
        let mut message = adapter::message_for(agent_name, &input_path, &checkout);
        if let Some(conflicts) = carried
            .as_ref()
            .map(|c| &c.conflicts)
            .filter(|c| !c.is_empty())
        {
            message.push_str(&conflict_note(
                spec["repo_base_bookmark"].as_str().unwrap_or("main"),
                conflicts,
            ));
        }

        // Every adapter goes through the same spawn path, the fake one included:
        // a test that does not exercise the real plumbing proves nothing about it.
        let system_prompt = format!(
            "{system_prompt}{}",
            adapter::repo_instructions(adapter, &checkout)
        );
        let prompt_file = adapter::write_system_prompt(&artifact_dir, &system_prompt)?;
        let invocation = adapter::invocation(adapter, &model, &prompt_file, &message)?;
        // The base is resolved once, before the agent starts: from here on the
        // live diff only reads the checkout with git, never with jj.
        let live_diff = match vcs.resolve(&base) {
            Ok(commit) => crate::live::LiveDiffSource::new(&checkout, &commit).await,
            Err(error) => {
                tracing::debug!(run_id, %error, "could not resolve the live diff's base");
                None
            }
        };
        if live_diff.is_none() {
            tracing::info!(run_id, checkout = %checkout.display(), "git cannot read this checkout; no live diff for this run");
        }
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
                live_diff,
                &redactor,
            )
            .await;
        self.hand_back(&format!("runs/{run_id}"), spec, &home).await;
        drop(login);
        let mut outcome = outcome?;

        // The team is named in the workflow, so the commit carries the agent's
        // own name: `Test Author` becomes `hulahoop-dev-test-author`.
        let agent_name = spec["commit_agent_name"].as_str();
        let author_name = hulahoop_core::commit_name(spec["commit_app_slug"].as_str(), agent_name);
        let author_email = hulahoop_core::commit_email(
            spec["commit_app_id"].as_str(),
            spec["commit_app_slug"].as_str(),
            agent_name,
        );
        vcs.set_working_copy_author(&checkout, &author_name, &author_email)
            .with_context(|| {
                format!(
                    "the loop could not take ownership of the commit as {}; refusing to produce a commit under somebody else's name",
                    author_name
                )
            })?;
        // Did this turn change anything? A turn that read, answered and left the
        // checkout alone produced no commit, so it publishes no head: the next turn
        // in line still starts from the last head that carried a change.
        let touched = !vcs
            .changed_paths(&checkout, &base)
            .unwrap_or_default()
            .is_empty();
        // A head with conflicts cannot be pushed or merged, so a turn that leaves
        // any behind fails instead of publishing one.
        let unresolved = if touched {
            vcs.conflicted_paths(&checkout).unwrap_or_default()
        } else {
            Vec::new()
        };
        if !unresolved.is_empty() {
            outcome.error = Some(format!(
                "the turn left conflicts in {}, so its work was not published",
                unresolved.join(", ")
            ));
        }
        let changed = touched && unresolved.is_empty();
        // A commit with no message cannot be pushed and reads as noise in `jj log`.
        // The author says who, the issue says what.
        if changed {
            let title = spec["issue"]["title"].as_str().unwrap_or("the issue");
            vcs.describe_as(
                &checkout,
                &format!("implement: {title}"),
                &author_name,
                &author_email,
            )
            .with_context(|| "describing the commit")?;
        }
        // A head that fails `fmt-check` in CI is a head nobody can merge, so the
        // loop formats the tree before it records the diff.
        self.format_tree(&checkout, &artifact_dir);
        let (commit_name, commit_email) = vcs.working_copy_author(&checkout).unwrap_or_default();

        let diff = vcs.diff_from(&checkout, &base).unwrap_or_default();
        let changed_paths = vcs.changed_paths(&checkout, &base).unwrap_or_default();
        let commit = vcs.working_copy_revision(&checkout).unwrap_or_default();

        // The bookmark the server will resolve later. It is created here because
        // only the checkout knows the commit id.
        let published: Vec<String> = if changed {
            let name = format!("issue/{issue_id}/head/{attempt}");
            let _ = vcs.set_bookmark_here(&checkout, &name);
            vec![name]
        } else {
            Vec::new()
        };
        // A workspace shares the mirror's repository, so the bookmark set above is
        // already in the mirror; a pull request job pushes it from there.
        let _ = &published;

        let status = run_status(&outcome);

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
                    "commit": if changed { Some(commit.clone()) } else { None },
                    "question": outcome.question,
                    "verdict": outcome.verdict,
                    "body": outcome.final_text,
                    "artifact_limit_hit": artifact_limit_hit,
                    "commit_name": commit_name,
                    "commit_email": commit_email,
                    "transcript": transcript_tail(&outcome),
                }),
            )
            .await?;

        // The checkout stays: the task's next turn reuses it, build outputs and
        // all, and it goes when the task is merged, closed or cancelled.
        // Scaffolding and empty commits are litter in everybody's `jj log`: the
        // merge only measured the diff, and a turn that changed nothing published
        // nothing, so its working copy has no reason to outlive it. The next turn
        // brings the stale checkout back with `jj workspace update-stale`. A turn
        // that left conflicts published nothing either, but its work stays in its
        // commit for a person to look at.
        if !touched && !commit.is_empty() {
            let _ = mirror.abandon(&commit);
        }
        Ok(())
    }

    async fn run_job(&self, envelope: Value) -> Result<()> {
        let job = &envelope["job"];
        let spec = &envelope["spec"];
        let job_id = job["id"].as_str().context("job has no id")?.to_string();
        let kind = spec["kind"].as_str().unwrap_or_default().to_string();

        // A cleanup and a forgotten mirror only delete; cloning or fetching the
        // mirror first would be work for nothing. Every other job works from an
        // up-to-date mirror, and one that cannot have it fails with the reason.
        let result = match kind.as_str() {
            "cleanup" => job_cleanup(&self.data_dir, spec),
            "forget_mirror" => job_forget_mirror(&self.data_dir, spec).await,
            _ => match host_repository(spec).await {
                Ok(repo_path) => match kind.as_str() {
                    "fetch" => self.job_fetch(&repo_path, spec).await,
                    "land" => {
                        // Landing moves a bookmark in the mirror, which a fetch
                        // in the other lane must not interleave with.
                        let lock = mirror_lock(&repo_path);
                        let _turn = lock.lock().await;
                        job_land(&repo_path, spec)
                    }
                    "preflight" => self.job_preflight(&repo_path, spec).await,
                    "bookmark" => self.job_bookmark(&repo_path, spec).await,
                    "open_pr" => self.job_open_pr(&repo_path, spec).await,
                    "review_pr" => job_review_pr(spec).await,
                    "watch_checks" => job_watch_checks(spec).await,
                    "lumi" => self.job_lumi(&repo_path, spec, &job_id).await,
                    other => Err(anyhow!("unknown job kind {other}")),
                },
                Err(error) => Err(error),
            },
        };

        if kind == "lumi" {
            if let Ok(id) = uuid::Uuid::parse_str(&job_id) {
                let _ =
                    std::fs::remove_dir_all(self.data_dir.join("home/lumi").join(id.to_string()));
            }
        }
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
        // The recorded base can be ahead of what the remote says: a landing was
        // recorded after the remote answered an earlier fetch, or the remote
        // lags. Reporting that, and which base it was compared with, lets the
        // server keep the newer base instead of moving it back.
        let recorded = span["base_revision"].as_str().unwrap_or_default().trim();
        let behind = !recorded.is_empty()
            && recorded != revision
            && vcs.is_strict_ancestor(&revision, recorded).unwrap_or(false);
        Ok(json!({ "base_revision": revision, "behind": behind, "compared_with": recorded }))
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
        let direnv_home = self.data_dir.join("direnv");
        if has_envrc {
            // `direnv allow` has to write into the same XDG home the probe will
            // read, so both get the same environment.
            std::fs::create_dir_all(&direnv_home).ok();
            let _ = Command::new("direnv")
                .arg("allow")
                .arg(repo_path)
                .env("XDG_DATA_HOME", &direnv_home)
                .env("NO_COLOR", "1")
                .output();
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
        let path = if adapter == Adapter::Fake {
            std::env::var("PATH").unwrap_or_default()
        } else {
            dev_shell_path(repo_path).unwrap_or_else(|| std::env::var("PATH").unwrap_or_default())
        };
        let output = cmd
            .current_dir(repo_path)
            .env_clear()
            .env("PATH", path)
            .env("HOME", &home)
            .env("XDG_DATA_HOME", &direnv_home)
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

    /// One Lumi answer: the workspace's harness, run once with the question and
    /// the snapshot the server packed. The process works from an empty scratch
    /// directory — Lumi reads a snapshot and writes a reply, never the repo —
    /// while the repo's dev shell supplies the PATH the CLI lives on.
    async fn job_lumi(&self, repo_path: &Path, spec: &Value, job_id: &str) -> Result<Value> {
        let payload = &spec["payload"];
        let adapter = Adapter::parse(payload["adapter"].as_str().unwrap_or_default())
            .ok_or_else(|| anyhow!("unknown adapter"))?;
        let model = payload["model"].as_str().unwrap_or_default();
        let system_prompt = payload["system_prompt"].as_str().unwrap_or_default();
        let mut message = payload["message"].as_str().unwrap_or_default().to_string();
        if let Some(history) = payload["history"].as_array().filter(|h| !h.is_empty()) {
            let lines: Vec<String> = history
                .iter()
                .map(|turn| {
                    format!(
                        "{}: {}",
                        turn["role"].as_str().unwrap_or("user"),
                        turn["text"].as_str().unwrap_or_default()
                    )
                })
                .collect();
            message = format!(
                "Earlier in this conversation:\n\n{}\n\n---\n\n{message}",
                lines.join("\n\n")
            );
        }

        let workdir = PathBuf::from(spec["artifact_dir"].as_str().unwrap_or_default());
        std::fs::create_dir_all(&workdir)?;

        let has_envrc = repo_path.join(".envrc").exists();
        let direnv_home = self.data_dir.join("direnv");
        if has_envrc {
            std::fs::create_dir_all(&direnv_home).ok();
            let _ = Command::new("direnv")
                .arg("allow")
                .arg(repo_path)
                .env("XDG_DATA_HOME", &direnv_home)
                .env("NO_COLOR", "1")
                .output();
        }
        // Each question has a private home: concurrent runners and credential
        // changes cannot replace another question's live login files.
        let id = uuid::Uuid::parse_str(job_id).context("invalid Lumi job id")?;
        let home = self.data_dir.join("home/lumi").join(id.to_string());
        std::fs::create_dir_all(&home)?;
        seed_home(&home);
        let home = std::fs::canonicalize(&home).unwrap_or(home);
        let path = if adapter == Adapter::Fake {
            std::env::var("PATH").unwrap_or_default()
        } else {
            dev_shell_path(repo_path).unwrap_or_else(|| std::env::var("PATH").unwrap_or_default())
        };
        let mut env: Vec<(String, String)> = vec![
            ("PATH".into(), path),
            ("HOME".into(), home.to_string_lossy().to_string()),
            (
                "TMPDIR".into(),
                std::env::temp_dir().to_string_lossy().to_string(),
            ),
            ("NO_COLOR".into(), "1".into()),
            (
                "XDG_DATA_HOME".into(),
                direnv_home.to_string_lossy().to_string(),
            ),
        ];

        let login = LoginFiles::new(spec, &home);
        apply_credential(&mut env, spec, &home)?;
        let redactor = Redactor::new(spec, &env);
        let prompt_file = adapter::write_system_prompt(&home, system_prompt)?;
        let invocation = adapter::invocation(adapter, model, &prompt_file, &message)?;
        let mut child = adapter::spawn(&invocation, &workdir, &env)?;
        let pid = child.id() as i64;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("the harness has no stdout"))?;
        // The harness writes auth and model errors to stderr. Drain it while
        // stdout streams to prevent a full pipe from blocking the process.
        // Keep only a bounded in-memory sample for a safe category on failure.
        let stderr = child.stderr.take().context("the harness has no stderr")?;
        let stderr_task = std::thread::spawn(move || {
            let mut sample = String::new();
            let mut reader = BufReader::new(stderr);
            let mut chunk = [0u8; 4096];
            loop {
                use std::io::Read;
                let Ok(size) = reader.read(&mut chunk) else {
                    break;
                };
                if size == 0 {
                    break;
                }
                if sample.len() < 16_384 {
                    let remaining = 16_384 - sample.len();
                    sample.push_str(&String::from_utf8_lossy(&chunk[..size.min(remaining)]));
                }
            }
            sample
        });
        let (line_tx, mut line_rx) = mpsc::unbounded_channel::<String>();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let _ = line_tx.send(line);
            }
        });

        let mut transcript = Transcript {
            forbidden: vec![system_prompt.to_string(), message.clone()],
            ..Default::default()
        };
        let wall_clock = Duration::from_secs((hulahoop_core::LUMI_WALL_CLOCK_MINUTES as u64) * 60);
        let started = Instant::now();
        let mut timed_out = false;
        loop {
            let left = wall_clock.saturating_sub(started.elapsed());
            if left.is_zero() {
                timed_out = true;
                kill_group(pid);
                break;
            }
            tokio::select! {
                line = line_rx.recv() => {
                    match line {
                        Some(line) => transcript.feed(&redactor.redact(&line)),
                        None => break,
                    }
                }
                _ = tokio::time::sleep(left) => {
                    timed_out = true;
                    kill_group(pid);
                    break;
                }
            }
        }
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) => tokio::time::sleep(Duration::from_millis(100)).await,
                Err(error) => break Err(error),
            }
        };
        self.hand_back(&format!("jobs/{job_id}"), spec, &home).await;
        drop(login);
        let status = status?;
        if timed_out {
            bail!(
                "the harness did not answer within {} minutes",
                hulahoop_core::LUMI_WALL_CLOCK_MINUTES
            );
        }
        let stderr = stderr_task.join().unwrap_or_default();
        if !status.success() {
            let category = lumi_failure_category(&stderr, &transcript.haystack());
            bail!("the harness exited with {:?}: {category}", status.code());
        }

        let text = transcript.final_text();
        if text.trim().is_empty() {
            bail!("the harness answered with nothing");
        }
        Ok(parse_lumi_reply(&text))
    }

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

        // The description says what was asked; the run's own summary says what
        // was done, which is what a reader of the pull request wants first.
        let summary = pull["summary"].as_str().unwrap_or("").trim();
        let summary = if summary.is_empty() {
            "The agent left no summary for this change."
        } else {
            summary
        };
        let review = pull["review"].as_str().unwrap_or("").trim();
        let mut body = format!(
            "{}\n\n---\n\n**What changed**\n\n{summary}\n",
            pull["body"].as_str().unwrap_or("")
        );
        if !review.is_empty() {
            body.push_str(&format!("\n**Reviewer verdict**\n\n{review}\n"));
        }
        body.push_str("\n_Opened by Hulahoop._\n");
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

    /// Write the agent's skills into the checkout, where the harness can open the
    /// one its description calls for.
    ///
    /// They live under `.hulahoop/`, which every repository the loop works in
    /// ignores: a file the agent was given to read is not part of the change it is
    /// making, so it must not show up in the diff, in the commit, or in a review.
    fn write_skills(&self, spec: &Value, checkout: &Path) -> Result<()> {
        write_skills(spec, checkout)
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

    fn format_tree(&self, checkout: &Path, artifact_dir: &Path) {
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
    }

    /// Hand back a login the harness refreshed, before the next run of this
    /// home overwrites it. Best effort: a failed hand-back costs a sign-in later,
    /// never this run. The login itself is never logged.
    async fn hand_back(&self, owner: &str, spec: &Value, home: &Path) {
        let Some((refresh, entry)) = refreshed_login(spec, home) else {
            return;
        };
        let body = json!({ "connector": refresh.connector, "row": refresh.row, "entry": entry });
        match self.client.hand_back_credential(owner, body).await {
            Ok(()) => {
                tracing::info!(owner, connector = %refresh.connector, "handed back a refreshed login")
            }
            Err(error) => tracing::warn!(owner, %error, "could not hand back a refreshed login"),
        }
    }

    /// The recipes a repository declares beyond the suite, run at the same revision
    /// the suite ran at. The pull request's checks run `just ci`, so a head that
    /// passes the suite and fails `fmt-check` is a head that was never finished.
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
        live_diff: Option<crate::live::LiveDiffSource>,
        redactor: &Redactor,
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

        // The working diff is taken every few seconds while the agent works, so
        // a person can watch the change grow. It runs beside this loop, so the
        // heartbeat and the output never wait on it; dropping it stops it.
        let _live_diff = live_diff.map(|source| {
            crate::live::spawn(
                self.client.clone(),
                run_id.to_string(),
                source,
                Duration::from_secs(hulahoop_core::LIVE_DIFF_INTERVAL_SECS),
                Duration::from_secs(hulahoop_core::LIVE_DIFF_TIMEOUT_SECS),
            )
        });

        loop {
            let wait = next_heartbeat.saturating_duration_since(Instant::now());
            tokio::select! {
                message = line_rx.recv() => {
                    match message {
                        Some((stream, line)) => {
                            log_bytes += line.len();
                            // Nothing the agent printed leaves the runner with
                            // the run's own keys or tokens in it.
                            let line = redactor.redact(&line);
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
fn agent_env(
    spec: &Value,
    checkout: &Path,
    data_dir: &Path,
    home: &Path,
) -> Result<Vec<(String, String)>> {
    seed_home(home);
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
        (
            "HULAHOOP_AGENT".into(),
            spec["agent_name"].as_str().unwrap_or_default().into(),
        ),
        (
            "HULAHOOP_BASE_BOOKMARK".into(),
            spec["base_bookmark"].as_str().unwrap_or("main").into(),
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
    apply_credential(&mut env, spec, home)?;
    // The commit says who wrote it. jj reads its identity from the environment,
    // so the agent commits under its own name even where the runner's identity is
    // the machine's. The team is named in the workflow, so the commit carries the
    // agent's name: `Test Author` becomes `hulahoop-dev-test-author`.
    let agent_name = spec["commit_agent_name"].as_str();
    env.push((
        "JJ_USER".into(),
        hulahoop_core::commit_name(spec["commit_app_slug"].as_str(), agent_name),
    ));
    env.push((
        "JJ_EMAIL".into(),
        hulahoop_core::commit_email(
            spec["commit_app_id"].as_str(),
            spec["commit_app_slug"].as_str(),
            agent_name,
        ),
    ));
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

/// The scratch home an agent run of this spec's adapter uses, created and then
/// made absolute, so every run of it (and the hand-back after each) names the
/// same directory even when the data directory sits behind a symlink.
/// Everything the agent is handed has to be absolute: its working directory is
/// the checkout, so a relative home resolves to nothing and pi reports a missing
/// API key instead of a missing directory.
fn agent_home(spec: &Value, data_dir: &Path) -> Result<PathBuf> {
    let adapter =
        Adapter::parse(spec["adapter"].as_str().unwrap_or_default()).unwrap_or(Adapter::Pi);
    let home = contract::scratch_home(data_dir, adapter);
    std::fs::create_dir_all(&home)?;
    Ok(std::fs::canonicalize(&home)?)
}

/// The login files a run's credential writes into its home, removed when this
/// goes out of scope — after the hand-back, and on every error path too. Agent
/// homes are shared per adapter, so a file left behind would sign the next run
/// in as this run's connector.
struct LoginFiles {
    home: PathBuf,
    files: Vec<String>,
}

impl LoginFiles {
    fn new(spec: &Value, home: &Path) -> Self {
        let files = spec["credential"]["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|file| file.get(0).and_then(Value::as_str))
            .filter(|relative| LOGIN_FILES.contains(relative))
            .map(str::to_string)
            .collect();
        LoginFiles {
            home: home.to_path_buf(),
            files,
        }
    }
}

impl Drop for LoginFiles {
    fn drop(&mut self) {
        let mut removed = true;
        for relative in &self.files {
            if let Err(error) = remove_home_file(&self.home, relative) {
                removed = false;
                tracing::warn!(file = %relative, %error, "could not remove a run's login file");
            }
        }
        // The marker outlives a failed removal, so the next host-login run
        // still knows to replace what is left.
        if removed && !self.files.is_empty() {
            let _ = std::fs::remove_file(self.home.join(CONNECTOR_MARKER));
        }
    }
}

/// Left in a home while it holds a connector's login. A host-login run that
/// finds it replaces the login files with the host's; without it the home's
/// files came from the host, and a newer one is a refresh worth keeping.
const CONNECTOR_MARKER: &str = ".hulahoop-connector-login";

/// The only files a credential may write, relative to `HOME`.
const LOGIN_FILES: [&str; 2] = [PI_AUTH_FILE, CLAUDE_CREDENTIALS_FILE];
/// A login file is a few hundred bytes; anything this big is not one.
const LOGIN_FILE_CAP: u64 = 64 * 1024;

/// The OAuth login a harness rewrote during the run, when it differs from the
/// one the run was given. The file is read only when it is a plain file inside
/// the home: a symlink could point at a login that is not this connector's.
fn refreshed_login(spec: &Value, home: &Path) -> Option<(Refresh, Value)> {
    let credential = spec.get("credential")?;
    let refresh: Refresh = serde_json::from_value(credential.get("refresh")?.clone()).ok()?;
    if !LOGIN_FILES.contains(&refresh.file.as_str()) {
        return None;
    }
    let written = credential["files"]
        .as_array()?
        .iter()
        .find(|file| file.get(0).and_then(Value::as_str) == Some(refresh.file.as_str()))
        .and_then(|file| file.get(1)?.as_str())
        .and_then(|contents| refresh.entry_in(contents));
    let current = refresh.entry_in(&read_home_file(home, &refresh.file).ok()?)?;
    (written.as_ref() != Some(&current)).then_some((refresh, current))
}

/// Agent runs and Lumi use the same credential boundary. A stored connector
/// wins over inherited host keys, particularly when it represents a subscription.
fn apply_credential(env: &mut Vec<(String, String)>, spec: &Value, home: &Path) -> Result<()> {
    let host_home = std::env::var_os("HOME").map(PathBuf::from);
    apply_credential_from(env, spec, home, host_home.as_deref())
}

/// [`apply_credential`] with the host's home named, so a test can stand one in.
fn apply_credential_from(
    env: &mut Vec<(String, String)>,
    spec: &Value,
    home: &Path,
    host_home: Option<&Path>,
) -> Result<()> {
    let supplied = spec.get("credential").is_some_and(|credential| {
        credential["env"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
            || credential["files"]
                .as_array()
                .is_some_and(|items| !items.is_empty())
    });
    if supplied {
        // A chosen connector must not silently fall back to a different host
        // subscription when the CLI sees an old login in this reused home.
        for relative in LOGIN_FILES {
            remove_home_file(home, relative)?;
        }
    } else {
        // A connector's login a killed runner never removed: start from the
        // host's own files, however new the leftovers look. Without the
        // marker `seed_home` already kept the newer of host and home, which
        // keeps a refresh an earlier host-login run made.
        let leftover = home.join(CONNECTOR_MARKER).exists();
        for relative in LOGIN_FILES.iter().filter(|_| leftover) {
            remove_home_file(home, relative)?;
            let Some(source) = host_home.map(|host| host.join(relative)) else {
                continue;
            };
            if !source.is_file() {
                continue;
            }
            let contents = std::fs::read_to_string(&source)
                .with_context(|| format!("reading the host's {relative}"))?;
            write_private_file(&safe_home_path(home, relative)?, &contents)
                .with_context(|| format!("copying the host's {relative} into the run's home"))?;
        }
        if leftover {
            let _ = std::fs::remove_file(home.join(CONNECTOR_MARKER));
        }
        for key in contract::CREDENTIAL_ENV {
            if let Ok(value) = std::env::var(key) {
                env.push((key.into(), value));
            }
        }
    }
    if let Some(credential) = spec.get("credential") {
        for entry in credential["env"].as_array().into_iter().flatten() {
            if let (Some(name), Some(value)) = (
                entry.get(0).and_then(Value::as_str),
                entry.get(1).and_then(Value::as_str),
            ) {
                env.push((name.to_string(), value.to_string()));
            }
        }
        for entry in credential["files"].as_array().into_iter().flatten() {
            if let (Some(relative), Some(contents)) = (
                entry.get(0).and_then(Value::as_str),
                entry.get(1).and_then(Value::as_str),
            ) {
                if !LOGIN_FILES.contains(&relative) {
                    bail!("invalid credential destination");
                }
                let path = safe_home_path(home, relative)?;
                write_private_file(&path, contents)
                    .with_context(|| format!("writing {relative} into the run's home"))?;
                let marker = home.join(CONNECTOR_MARKER);
                if !marker.exists() {
                    write_private_file(&marker, "")
                        .context("marking the run's home as holding a connector's login")?;
                }
            }
        }
    }
    Ok(())
}

/// A CLI-writable home can contain symlinks left by an earlier run. Keep host
/// credentials under the canonical scratch root and reject symlinked parents.
fn safe_home_path(home: &Path, relative: &str) -> Result<PathBuf> {
    let path = checked_home_path(home, relative, true)?.context("credential path has no parent")?;
    if std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        bail!("symlink in credential path");
    }
    Ok(path)
}

/// `relative` under `home` when every directory on the way is a real directory
/// (no symlink) inside the canonical home. A missing directory is created when
/// `create` is set, and otherwise means there is nothing there: `None`. The
/// last component is not checked; each caller decides what a symlink there
/// means.
fn checked_home_path(home: &Path, relative: &str, create: bool) -> Result<Option<PathBuf>> {
    let root = home.canonicalize()?;
    let parts: Vec<_> = Path::new(relative).components().collect();
    if parts.is_empty()
        || parts
            .iter()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        bail!("invalid home path");
    }
    let mut directory = home.to_path_buf();
    for part in &parts[..parts.len() - 1] {
        directory.push(part);
        match std::fs::symlink_metadata(&directory) {
            Ok(meta) if meta.file_type().is_symlink() => bail!("symlink in credential path"),
            Ok(meta) if !meta.is_dir() => bail!("credential path is not a directory"),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !create {
                    return Ok(None);
                }
                std::fs::create_dir(&directory)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    if !directory.canonicalize()?.starts_with(&root) {
        bail!("credential path left scratch home");
    }
    Ok(Some(home.join(relative)))
}

/// Remove a login file from a home without following a symlink on the way:
/// a symlinked `.pi` must not lead the removal to the host's own login. A
/// symlink in the file's own place is removed as the link it is.
fn remove_home_file(home: &Path, relative: &str) -> Result<()> {
    let Some(path) = checked_home_path(home, relative, false)? else {
        return Ok(());
    };
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// A login file the harness may have rewritten, read only when it is a plain
/// file inside the home reached through no symlink, and at most
/// [`LOGIN_FILE_CAP`] bytes: the agent controls this directory.
fn read_home_file(home: &Path, relative: &str) -> Result<String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let path = checked_home_path(home, relative, false)?.context("no such login file")?;
    let file = std::fs::OpenOptions::new()
        // Non-blocking too, so a FIFO planted in the file's place cannot
        // stall the runner; a regular file reads the same either way.
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
        .open(&path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > LOGIN_FILE_CAP {
        bail!("not a login file");
    }
    let mut contents = String::new();
    file.take(LOGIN_FILE_CAP + 1)
        .read_to_string(&mut contents)?;
    if contents.len() as u64 > LOGIN_FILE_CAP {
        bail!("login file too large");
    }
    Ok(contents)
}

/// A credentials file the CLI will read. Owner-only and created exclusively,
/// so a stale hardlink or symlink cannot redirect a host login into the checkout.
fn write_private_file(path: &Path, contents: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
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

/// The mirror a run or job works from: Hulahoop's own clone of the
/// repository's url, under the data directory. The first use clones into it,
/// later ones fetch. Cloning here keeps the loop out of any working copy the
/// user is using, and lets a private repository work without the token ever
/// being written to disk.
///
/// The job lane and the Lumi lane can both want the same mirror at once, so
/// clones and fetches of one mirror take turns. A clone lands in a temporary
/// directory first and is renamed into place, so a half-done clone is never
/// mistaken for a mirror.
async fn host_repository(spec: &Value) -> Result<PathBuf> {
    let raw = spec["repo_path"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_string();
    if raw.is_empty() {
        bail!("the server named no mirror for this repository, so there is nothing to work from");
    }
    let path = PathBuf::from(&raw);
    let url = spec["clone_url"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_string();
    if url.is_empty() {
        bail!("this repository has no url to clone from. Set its url in the project settings.");
    }
    let token = spec["clone_token"].as_str().map(str::to_string);
    let lock = mirror_lock(&path);
    let _turn = lock.lock().await;
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
    let parent = path
        .parent()
        .context("the mirror path has no parent directory")?;
    std::fs::create_dir_all(parent)?;
    let staging = parent.join(format!(".tmp-{}", uuid::Uuid::now_v7()));
    let mut command = Command::new("jj");
    command
        .args(["git", "clone", "--colocate", "--", &url])
        .arg(&staging);
    if let Some(token) = &token {
        command.envs(git_auth(token));
    }
    let output = command
        .output()
        .context("running jj git clone for the mirror");
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
    };
    if !output.status.success() {
        let _ = std::fs::remove_dir_all(&staging);
        bail!(
            "cloning {url} into the mirror at {} failed: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    // Another runner on this data directory may have finished the same clone
    // first; theirs is as good as this one.
    if std::fs::rename(&staging, &path).is_err() && path.join(".jj").exists() {
        let _ = std::fs::remove_dir_all(&staging);
    } else if staging.exists() {
        let _ = std::fs::remove_dir_all(&staging);
        bail!(
            "could not move the new clone into the mirror at {}",
            path.display()
        );
    }
    Ok(path)
}

/// The lock that makes clones and fetches of one mirror take turns inside this
/// runner. jj records every fetch as an operation, and two at once in one
/// repository leave divergent operations behind.
fn mirror_lock(path: &Path) -> Arc<tokio::sync::Mutex<()>> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    let mut locks = LOCKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    locks.entry(path.to_path_buf()).or_default().clone()
}

/// A bookmark's name on the remote the mirror was cloned from; empty for no
/// bookmark.
fn on_origin(bookmark: &str) -> String {
    let bookmark = bookmark.trim();
    if bookmark.is_empty() {
        String::new()
    } else {
        format!("{bookmark}@origin")
    }
}

/// The checkout of the task `issue` in project `project`, derived here instead
/// of taken from the server: the runner rebuilds and deletes this directory, so
/// it is only ever one under the data directory's `checkouts/` that it owns.
fn owned_checkout(data_dir: &Path, project: &Value, issue: &Value) -> Result<PathBuf> {
    let id = |value: &Value, what: &str| {
        value
            .as_str()
            .and_then(|raw| uuid::Uuid::parse_str(raw.trim()).ok())
            .with_context(|| format!("the server named no {what} for the task's checkout"))
    };
    let project = id(project, "project")?;
    let issue = id(issue, "task")?;
    let checkout = contract::checkout_dir(data_dir, &project.to_string(), &issue);
    if !contract::is_owned_checkout(data_dir, &checkout) {
        bail!(
            "refusing to use {}: a task checkout must be a real directory under {}",
            checkout.display(),
            data_dir.join("checkouts").display()
        );
    }
    Ok(checkout)
}

/// Abandons a carried turn's scaffolding merge when the run is over, on every
/// path out of it.
struct Scaffold {
    repo: PathBuf,
    merge: Option<String>,
}

impl Drop for Scaffold {
    fn drop(&mut self) {
        if let (Some(merge), Ok(repo)) = (self.merge.as_deref(), Vcs::detect(&self.repo)) {
            let _ = repo.abandon(merge);
        }
    }
}

/// What the agent is told when moving the task onto the newest base conflicted.
/// Resolving them comes first: a head with conflicts cannot be pushed or merged.
fn conflict_note(landing: &str, conflicts: &[String]) -> String {
    let files = conflicts
        .iter()
        .map(|path| format!("- `{path}`"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "\n\nBefore this turn the loop moved the task's work onto the newest `{landing}`, and these files conflict:\n{files}\n\nIf your turn changes the work, resolve them first: edit each file so it keeps both `{landing}`'s changes and the task's intent, and remove every conflict marker; `jj resolve --list` must print nothing before you finish, or the turn's work is not published. A review judges the work as it will land, conflicts included."
    )
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

/// What a harness may keep in its home under `.pi/agent`: the login and Pi's
/// model catalogue. Settings, packages, extensions, skills, prompts and MCP
/// configuration are the user's own setup and stay on the host.
const PI_HOME_KEEPS: [&str; 2] = ["auth.json", "models-store.json"];
/// What a harness may keep under `.claude`: the login. Claude Code recreates
/// its runtime state; settings, hooks, plugins, skills, agents, commands and
/// `CLAUDE.md` stay on the host.
const CLAUDE_HOME_KEEPS: [&str; 1] = [".credentials.json"];

/// Refresh the host login in a scratch home before each invocation. Both CLIs
/// store credentials under `$HOME`, and subscriptions can rotate their tokens.
/// Everything else a harness would read from its home is cleared, so a run
/// sees only what Hulahoop hands it, never the user's extensions or skills.
fn seed_home(home: &Path) {
    let real_home = std::env::var_os("HOME").map(PathBuf::from);
    seed_home_from(home, real_home.as_deref());
}

/// [`seed_home`] with the host's home named, so a test can stand one in.
fn seed_home_from(home: &Path, real_home: Option<&Path>) {
    let pi_dst = home.join(".pi/agent");
    let claude_dst = home.join(".claude");
    keep_only(home, &pi_dst, &PI_HOME_KEEPS);
    keep_only(home, &claude_dst, &CLAUDE_HOME_KEEPS);
    // Claude Code keeps MCP servers and project trust in `~/.claude.json`.
    for stray in [".claude.json", ".claude.json.backup"] {
        if let Ok(path) = safe_home_path(home, stray) {
            let _ = std::fs::remove_file(path);
        }
    }
    let Some(real_home) = real_home else {
        return;
    };
    let pi_src = real_home.join(".pi/agent");
    if pi_src.exists() {
        let _ = std::fs::create_dir_all(&pi_dst);
        for name in PI_HOME_KEEPS {
            refresh_login(&pi_src.join(name), &pi_dst.join(name), home);
        }
    } else {
        let _ = std::fs::remove_file(pi_dst.join("auth.json"));
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
    if claude_src.exists() {
        let _ = std::fs::create_dir_all(&claude_dst);
        for name in CLAUDE_HOME_KEEPS {
            refresh_login(&claude_src.join(name), &claude_dst.join(name), home);
        }
    } else {
        let _ = std::fs::remove_file(claude_dst.join(".credentials.json"));
    }
}

/// Remove everything in `dir` but the named entries. A symlinked `dir` (an
/// agent can plant one in its own home) is removed as a link, never followed.
fn keep_only(home: &Path, dir: &Path, keep: &[&str]) {
    let Ok(relative) = dir.strip_prefix(home) else {
        return;
    };
    let Some(relative) = relative.to_str() else {
        return;
    };
    // Walk down from the home: the first link found is removed as a link (its
    // parents are real directories by then), and nothing below it is touched.
    let mut walked = home.to_path_buf();
    for part in Path::new(relative).components() {
        walked.push(part);
        match std::fs::symlink_metadata(&walked) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let _ = std::fs::remove_file(&walked);
                return;
            }
            Ok(meta) if meta.is_dir() => {}
            _ => return,
        }
    }
    let Ok(entries) = std::fs::read_dir(&walked) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if keep.iter().any(|kept| name == std::ffi::OsStr::new(kept)) {
            continue;
        }
        let path = entry.path();
        let removed = match entry.file_type() {
            Ok(kind) if kind.is_dir() => std::fs::remove_dir_all(&path),
            _ => std::fs::remove_file(&path),
        };
        if let Err(error) = removed {
            tracing::warn!(path = %path.display(), %error, "could not clear a harness home entry");
        }
    }
}

fn refresh_login(source: &Path, destination: &Path, home: &Path) {
    let Ok(relative) = destination.strip_prefix(home) else {
        return;
    };
    let Some(relative) = relative.to_str() else {
        return;
    };
    let Ok(path) = safe_home_path(home, relative) else {
        return;
    };
    if !source.exists() {
        let _ = std::fs::remove_file(path);
        return;
    }
    let host_newer = match (std::fs::metadata(source), std::fs::metadata(&path)) {
        (Ok(host), Ok(scratch)) => host.modified().ok() >= scratch.modified().ok(),
        _ => true,
    };
    if !host_newer {
        return;
    }
    if let Ok(contents) = std::fs::read_to_string(source) {
        if path.exists() && std::fs::remove_file(&path).is_err() {
            return;
        }
        let _ = write_private_file(&path, &contents);
    }
}

/// Lumi's output contract is one JSON object `{reply, proposal?}`. A model
/// that wrapped it in a fence, or answered in prose, still gets through: the
/// words become the reply and there is simply no proposal.
/// Never send raw CLI diagnostics to chat: they may contain keys or prompt text.
fn lumi_failure_category(stderr: &str, stdout: &str) -> &'static str {
    let text = format!("{stderr}\n{stdout}").to_lowercase();
    if [
        "unauthorized",
        "authentication",
        "login",
        "invalid api key",
        "api key",
        "oauth",
        "credentials",
    ]
    .iter()
    .any(|word| text.contains(word))
    {
        "authentication failed; check the model connector or renew its login"
    } else if ["rate limit", "too many requests", "429"]
        .iter()
        .any(|word| text.contains(word))
    {
        "provider rate limit; try again later"
    } else if ["insufficient quota", "billing", "credit balance"]
        .iter()
        .any(|word| text.contains(word))
    {
        "provider quota or billing issue"
    } else if [
        "model not found",
        "unknown model",
        "invalid model",
        "model is not available",
    ]
    .iter()
    .any(|word| text.contains(word))
    {
        "model unavailable; check the selected model"
    } else {
        "the harness reported an error; check its model and connector setup"
    }
}

fn parse_lumi_reply(text: &str) -> Value {
    let trimmed = text.trim();
    let candidate = if trimmed.starts_with("```") {
        trimmed
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim()
    } else {
        trimmed
    };
    // The object may sit inside prose; take the outermost braces.
    let sliced = match (candidate.find('{'), candidate.rfind('}')) {
        (Some(start), Some(end)) if start < end => &candidate[start..=end],
        _ => candidate,
    };
    if let Ok(value) = serde_json::from_str::<Value>(sliced) {
        if value["reply"].is_string() {
            return json!({
                "reply": value["reply"],
                "proposal": value.get("proposal").cloned().unwrap_or(Value::Null),
            });
        }
    }
    json!({ "reply": trimmed, "proposal": Value::Null })
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
            "{}/repos/{owner}/{repo}/installation",
            github_api()
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
            "{}/app/installations/{installation}/access_tokens",
            github_api()
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
            "{}/repos/{owner}/{repo}/pulls/{number}",
            github_api()
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
                "{}/repos/{owner}/{name}/commits/{sha}/check-runs?per_page=100",
                github_api()
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
            "{}/repos/{owner}/{repo}/pulls/{number}/reviews?per_page=100",
            github_api()
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
            "{}/repos/{owner}/{repo}/issues/{number}/comments",
            github_api()
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
            "{}/repos/{owner}/{name}/pulls/{number}/reviews",
            github_api()
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
        .post(format!("{}/repos/{owner}/{repo}/pulls", github_api()))
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

/// Write the agent's skills into the checkout, where the harness can open the one
/// its description calls for.
///
/// They live under `.hulahoop/`, which every repository the loop works in ignores:
/// a file the agent was given to read is not part of the change it is making, so it
/// must not show up in the diff, in the commit, or in a review.
/// What the server reports for a run's end, from how the agent process ended.
fn run_status(outcome: &Outcome) -> &'static str {
    if outcome.timed_out {
        "timed_out"
    } else if outcome.exit_code == Some(0) && outcome.error.is_none() {
        "succeeded"
    } else {
        "failed"
    }
}

/// The last lines of the agent's stream, for the server to diagnose a turn
/// that said nothing.
fn transcript_tail(outcome: &Outcome) -> String {
    outcome
        .lines
        .iter()
        .rev()
        .take(40)
        .rev()
        .cloned()
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether the server sent a run for a task with no repository. Such a run has
/// no checkout to make: the server says so with `checkout: "none"`.
fn is_repo_less(spec: &Value) -> bool {
    spec["checkout"].as_str() == Some("none")
}

/// A fresh, empty working directory for a repo-less run, private to it:
/// `data_dir/scratch/<run_id>`. Anything left from an earlier attempt with the
/// same id is cleared first, so the agent always starts from nothing. The run id
/// must be a uuid, which also keeps it from naming a path outside `scratch/`.
fn prepare_scratch(data_dir: &Path, run_id: &str) -> Result<PathBuf> {
    let id =
        uuid::Uuid::parse_str(run_id).with_context(|| format!("run id {run_id} is not a uuid"))?;
    let dir = contract::scratch_dir(data_dir, &id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .with_context(|| format!("clearing a stale scratch directory at {}", dir.display()))?;
    }
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating the scratch directory {}", dir.display()))?;
    Ok(dir)
}

/// Deletes a task's checkout once no turn will use it: the workspace is
/// forgotten in the mirror and its directory removed. The path is derived from
/// the project and task ids in the payload, and only a real directory below the
/// data directory's `checkouts/` may go this way, so a wrong payload cannot
/// delete anything else. A checkout that is already gone is not an error:
/// there is nothing left to do.
fn job_cleanup(data_dir: &Path, spec: &Value) -> Result<Value> {
    let payload = &spec["payload"];
    let checkout = owned_checkout(data_dir, &payload["project_id"], &payload["issue_id"])?;
    let mirror = PathBuf::from(spec["repo_path"].as_str().unwrap_or_default().trim());
    match Vcs::detect(&mirror) {
        Ok(mirror) => mirror.remove_workspace(&checkout)?,
        Err(_) => {
            if checkout.exists() {
                std::fs::remove_dir_all(&checkout)
                    .with_context(|| format!("removing {}", checkout.display()))?;
            }
        }
    }
    Ok(json!({ "removed": checkout }))
}

/// Deletes a repository's mirror after its url changed, so the next job or
/// run clones the new url instead of fetching the old one. Only a
/// `mirrors/<repository id>` directory may go this way: the mirror the server
/// names has to be exactly that, an absolute path with no `..` in it, and
/// without a name the one under this runner's data directory is used. A mirror
/// that is already gone is not an error.
async fn job_forget_mirror(data_dir: &Path, spec: &Value) -> Result<Value> {
    use std::path::Component;
    let id = spec["repo_id"]
        .as_str()
        .and_then(|raw| uuid::Uuid::parse_str(raw.trim()).ok())
        .context("the server named no repository whose mirror to forget")?;
    let named = spec["repo_path"].as_str().unwrap_or_default().trim();
    let mirror = if named.is_empty() {
        contract::mirror_dir(data_dir, &id)
    } else {
        PathBuf::from(named)
    };
    let plain = mirror
        .components()
        .all(|part| matches!(part, Component::RootDir | Component::Normal(_)));
    if !mirror.is_absolute()
        || !plain
        || !mirror.ends_with(Path::new("mirrors").join(id.to_string()))
    {
        bail!(
            "refusing to delete {}: a mirror is mirrors/<repository id> under the data directory",
            mirror.display()
        );
    }
    let lock = mirror_lock(&mirror);
    let _turn = lock.lock().await;
    match std::fs::symlink_metadata(&mirror) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(&mirror)
            .with_context(|| format!("removing {}", mirror.display()))?,
        Ok(_) => std::fs::remove_file(&mirror)
            .with_context(|| format!("removing {}", mirror.display()))?,
        Err(_) => {}
    }
    Ok(json!({ "removed": mirror }))
}

/// Lands a merge made without a pull request: the repository's base bookmark
/// moves forward to the merged head in the mirror and is pushed to the remote,
/// so the next fetch finds the merge there instead of undoing it. A base that
/// moved on since the head was made cannot be moved to it without dropping
/// work, so that is refused and reported.
fn job_land(repo_path: &Path, spec: &Value) -> Result<Value> {
    let base = spec["base_bookmark"].as_str().unwrap_or("main").trim();
    let revision = spec["payload"]["revision"]
        .as_str()
        .unwrap_or_default()
        .trim();
    if base.is_empty() || revision.is_empty() {
        bail!("a landing needs the repository's base bookmark and the merged revision");
    }
    let vcs = Vcs::detect(repo_path)?;
    let landed = vcs.advance_bookmark(base, revision).with_context(|| {
        format!("{base} could not move forward to {revision}; it has work the merged head does not")
    })?;
    let mut command = Command::new("jj");
    command
        .args(["git", "push", "--bookmark", base])
        .current_dir(repo_path);
    if let Some(token) = spec["clone_token"].as_str() {
        command.envs(git_auth(token));
    }
    let output = command.output().context("running jj git push")?;
    if !output.status.success() {
        bail!(
            "pushing {base} to the repository's remote failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(json!({ "base_revision": landed }))
}

/// Deletes a repo-less run's scratch directory. Best effort: a leftover is
/// cleared by `prepare_scratch` the next time the id is used.
fn remove_scratch(data_dir: &Path, run_id: &str) {
    if let Ok(id) = uuid::Uuid::parse_str(run_id) {
        let _ = std::fs::remove_dir_all(contract::scratch_dir(data_dir, &id));
    }
}

fn write_skills(spec: &Value, checkout: &Path) -> Result<()> {
    // The checkout is reused across turns, and the agent of an earlier turn may
    // have had other skills: only this turn's agent's skills are left in it.
    let root = checkout.join(".hulahoop").join("skills");
    match std::fs::symlink_metadata(&root) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(&root)?,
        Ok(_) => std::fs::remove_file(&root)?,
        Err(_) => {}
    }
    let Some(skills) = spec["skills"].as_array() else {
        return Ok(());
    };
    for skill in skills {
        let name = skill["name"].as_str().unwrap_or_default().trim();
        let body = skill["body"].as_str().unwrap_or_default();
        if name.is_empty() || body.trim().is_empty() {
            continue;
        }
        let dir = checkout
            .join(".hulahoop")
            .join("skills")
            .join(slugify(name));
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("SKILL.md"), body)?;
    }
    Ok(())
}

#[cfg(test)]
mod lumi_auth_tests {
    use super::{
        apply_credential, apply_credential_from, lumi_failure_category, read_home_file,
        refresh_login, refreshed_login, remove_home_file, seed_home_from, LoginFiles,
    };
    use serde_json::json;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("hulahoop-{name}-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    /// A connector's login is gone once its run is over, and a host-login run
    /// after it starts on the host's own file — even when a killed runner left
    /// the connector's file behind, newer than the host's.
    #[test]
    fn a_connector_login_never_outlives_its_run() {
        let root = temp("leftover");
        let host = root.join("host");
        let home = root.join("home");
        std::fs::create_dir_all(host.join(".pi/agent")).unwrap();
        std::fs::create_dir_all(host.join(".claude")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(host.join(".pi/agent/auth.json"), "host-pi-login").unwrap();
        std::fs::write(host.join(".claude/.credentials.json"), "host-claude-login").unwrap();

        let connector = json!({ "credential": { "env": [], "files": [
            [".pi/agent/auth.json", "{\"opencode-go\":{\"type\":\"api_key\",\"key\":\"k\"}}"],
            [".claude/.credentials.json", "{\"claudeAiOauth\":{}}"],
        ]}});
        let login = LoginFiles::new(&connector, &home);
        apply_credential_from(&mut Vec::new(), &connector, &home, Some(&host)).unwrap();
        assert!(std::fs::read_to_string(home.join(".pi/agent/auth.json"))
            .unwrap()
            .contains("opencode-go"));
        drop(login);
        assert!(!home.join(".pi/agent/auth.json").exists());
        assert!(!home.join(".claude/.credentials.json").exists());

        // A killed runner never dropped its guard: the connector's files are
        // still here and newer than the host's.
        apply_credential_from(&mut Vec::new(), &connector, &home, Some(&host)).unwrap();
        let empty = json!({ "credential": { "env": [], "files": [] }});
        apply_credential_from(&mut Vec::new(), &empty, &home, Some(&host)).unwrap();
        assert_eq!(
            std::fs::read_to_string(home.join(".pi/agent/auth.json")).unwrap(),
            "host-pi-login"
        );
        assert_eq!(
            std::fs::read_to_string(home.join(".claude/.credentials.json")).unwrap(),
            "host-claude-login"
        );

        // With no host login at all, the run has none either.
        std::fs::remove_file(host.join(".pi/agent/auth.json")).unwrap();
        apply_credential_from(&mut Vec::new(), &connector, &home, Some(&host)).unwrap();
        apply_credential_from(&mut Vec::new(), &empty, &home, Some(&host)).unwrap();
        assert!(!home.join(".pi/agent/auth.json").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    /// A host-login run whose CLI refreshed the token leaves the newer login in
    /// the home. The next host-login run keeps it: the host's copy holds a
    /// refresh token the provider may already have rotated away.
    #[test]
    fn a_host_login_refresh_survives_the_next_host_login_run() {
        let root = temp("host-refresh");
        let host = root.join("host");
        let home = root.join("home");
        std::fs::create_dir_all(host.join(".pi/agent")).unwrap();
        std::fs::create_dir_all(home.join(".pi/agent")).unwrap();
        std::fs::write(host.join(".pi/agent/auth.json"), "host-pi-login").unwrap();
        std::fs::write(home.join(".pi/agent/auth.json"), "refreshed-in-a-run").unwrap();

        let empty = json!({ "credential": { "env": [], "files": [] }});
        apply_credential_from(&mut Vec::new(), &empty, &home, Some(&host)).unwrap();
        assert_eq!(
            std::fs::read_to_string(home.join(".pi/agent/auth.json")).unwrap(),
            "refreshed-in-a-run"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// A harness home holds the login and Pi's model catalogue, nothing of the
    /// user's own setup — also when an earlier version copied it in, or an
    /// agent planted a link to the host's directory.
    #[test]
    fn a_harness_home_keeps_only_the_login() {
        let root = temp("clean-home");
        let host = root.join("host");
        let home = root.join("home");
        for dir in [".pi/agent/npm", ".pi/agent/skills/x", ".claude/plugins"] {
            std::fs::create_dir_all(host.join(dir)).unwrap();
        }
        for (file, text) in [
            (".pi/agent/auth.json", "pi-login"),
            (".pi/agent/models-store.json", "catalogue"),
            (".pi/agent/settings.json", "{\"packages\":[\"npm:x\"]}"),
            (".pi/agent/AGENTS.md", "personal"),
            (".claude/.credentials.json", "claude-login"),
            (".claude/settings.json", "{\"hooks\":{}}"),
            (".claude/CLAUDE.md", "personal"),
        ] {
            std::fs::write(host.join(file), text).unwrap();
        }
        // What an earlier version (or the harness itself) left in the home.
        std::fs::create_dir_all(home.join(".pi/agent/npm/node_modules")).unwrap();
        std::fs::write(home.join(".pi/agent/settings.json"), "old").unwrap();
        std::fs::create_dir_all(home.join(".claude/skills/y")).unwrap();
        std::fs::write(home.join(".claude.json"), "{\"mcpServers\":{}}").unwrap();

        seed_home_from(&home, Some(&host));
        let names = |dir: &str| {
            let mut names: Vec<String> = std::fs::read_dir(home.join(dir))
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
                .collect();
            names.sort();
            names
        };
        assert_eq!(names(".pi/agent"), ["auth.json", "models-store.json"]);
        assert_eq!(names(".claude"), [".credentials.json"]);
        assert!(!home.join(".claude.json").exists());
        assert_eq!(
            std::fs::read_to_string(home.join(".pi/agent/auth.json")).unwrap(),
            "pi-login"
        );

        // A planted link to the host's `.pi` goes as a link; the host keeps
        // everything it had.
        #[cfg(unix)]
        {
            std::fs::remove_dir_all(home.join(".pi")).unwrap();
            std::os::unix::fs::symlink(host.join(".pi"), home.join(".pi")).unwrap();
            seed_home_from(&home, Some(&host));
            assert!(host.join(".pi/agent/settings.json").exists());
            assert!(host.join(".pi/agent/npm").exists());
            assert!(!std::fs::symlink_metadata(home.join(".pi"))
                .unwrap()
                .file_type()
                .is_symlink());
            assert_eq!(names(".pi/agent"), ["auth.json", "models-store.json"]);
        }
        let _ = std::fs::remove_dir_all(root);
    }

    /// The agent owns its home. A symlinked `.pi` pointing at the host's login
    /// is neither handed back as the connector's login nor deleted through.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_login_directory_is_neither_read_nor_removed_through() {
        let root = temp("symlinked");
        let host = root.join("host");
        let home = root.join("home");
        std::fs::create_dir_all(host.join(".pi/agent")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let host_login = json!({"openai-codex": {"type": "oauth", "access": "host", "refresh": "host", "expires": 9}});
        std::fs::write(host.join(".pi/agent/auth.json"), host_login.to_string()).unwrap();
        std::os::unix::fs::symlink(host.join(".pi"), home.join(".pi")).unwrap();

        let spec = json!({ "credential": {
            "env": [],
            "files": [[".pi/agent/auth.json", "{}"]],
            "refresh": {"connector": "codex-sub", "row": uuid::Uuid::now_v7(), "file": ".pi/agent/auth.json",
                        "key": "openai-codex", "format": "pi"},
        }});
        assert!(refreshed_login(&spec, &home).is_none());
        assert!(remove_home_file(&home, ".pi/agent/auth.json").is_err());
        assert!(apply_credential_from(&mut Vec::new(), &spec, &home, None).is_err());
        drop(LoginFiles::new(&spec, &home));
        assert!(
            host.join(".pi/agent/auth.json").exists(),
            "the host login stays"
        );

        // A symlink in the file's own place is removed as a link.
        std::fs::remove_file(home.join(".pi")).unwrap();
        std::fs::create_dir_all(home.join(".pi/agent")).unwrap();
        std::os::unix::fs::symlink(
            host.join(".pi/agent/auth.json"),
            home.join(".pi/agent/auth.json"),
        )
        .unwrap();
        assert!(read_home_file(&home, ".pi/agent/auth.json").is_err());
        remove_home_file(&home, ".pi/agent/auth.json").unwrap();
        assert!(host.join(".pi/agent/auth.json").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_oversized_login_file_is_not_read() {
        let home = temp("oversized");
        std::fs::create_dir_all(home.join(".pi/agent")).unwrap();
        std::fs::write(home.join(".pi/agent/auth.json"), vec![b' '; 65 * 1024]).unwrap();
        assert!(read_home_file(&home, ".pi/agent/auth.json").is_err());
        std::fs::write(home.join(".pi/agent/auth.json"), "{}").unwrap();
        assert_eq!(read_home_file(&home, ".pi/agent/auth.json").unwrap(), "{}");
        let _ = std::fs::remove_dir_all(home);
    }

    /// A Claude setup token reaches Pi in the environment with no file, and the
    /// host's stale `auth.json` must still go, or Pi would sign in as the host.
    #[test]
    fn a_token_only_credential_still_removes_the_host_login() {
        let home = std::env::temp_dir().join(format!("hulahoop-token-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(home.join(".pi/agent")).unwrap();
        std::fs::write(home.join(".pi/agent/auth.json"), "{\"anthropic\":{}}").unwrap();
        let mut env = Vec::new();
        apply_credential(
            &mut env,
            &json!({ "credential": { "env": [["ANTHROPIC_OAUTH_TOKEN", "sk-ant-oat"]], "files": [] }}),
            &home,
        )
        .unwrap();
        assert!(!home.join(".pi/agent/auth.json").exists());
        assert!(env.contains(&("ANTHROPIC_OAUTH_TOKEN".into(), "sk-ant-oat".into())));
        let _ = std::fs::remove_dir_all(home);
    }

    /// Only a login the harness changed is handed back, and a Claude Code file
    /// is handed back in Pi's entry format.
    #[test]
    fn a_refreshed_login_is_found_only_when_it_changed() {
        let home = std::env::temp_dir().join(format!("hulahoop-refresh-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&home).unwrap();
        let row = uuid::Uuid::now_v7();
        let written = json!({"openai-codex": {"type": "oauth", "access": "a1", "refresh": "r1", "expires": 1}});
        let spec = json!({ "credential": {
            "env": [],
            "files": [[".pi/agent/auth.json", written.to_string()]],
            "refresh": {"connector": "codex-sub", "row": row, "file": ".pi/agent/auth.json",
                        "key": "openai-codex", "format": "pi"},
        }});
        apply_credential(&mut Vec::new(), &spec, &home).unwrap();
        assert!(
            refreshed_login(&spec, &home).is_none(),
            "nothing changed yet"
        );

        let refreshed = json!({"openai-codex": {"type": "oauth", "access": "a2", "refresh": "r2", "expires": 2, "accountId": "acc"}});
        std::fs::write(home.join(".pi/agent/auth.json"), refreshed.to_string()).unwrap();
        let (refresh, entry) = refreshed_login(&spec, &home).expect("a changed login");
        assert_eq!(refresh.connector, "codex-sub");
        assert_eq!(refresh.row, row);
        assert_eq!(entry, refreshed["openai-codex"]);

        let claude = json!({ "credential": {
            "env": [],
            "files": [[".claude/.credentials.json", "{\"claudeAiOauth\":{\"accessToken\":\"a\",\"refreshToken\":\"r\",\"expiresAt\":1}}"]],
            "refresh": {"connector": "claude-sub", "row": row, "file": ".claude/.credentials.json",
                        "key": null, "format": "claude"},
        }});
        apply_credential(&mut Vec::new(), &claude, &home).unwrap();
        std::fs::write(
            home.join(".claude/.credentials.json"),
            "{\"claudeAiOauth\":{\"accessToken\":\"b\",\"refreshToken\":\"s\",\"expiresAt\":9}}",
        )
        .unwrap();
        let (_, entry) = refreshed_login(&claude, &home).expect("a changed Claude login");
        assert_eq!(
            entry,
            json!({"type": "oauth", "access": "b", "refresh": "s", "expires": 9})
        );
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn stored_connector_replaces_host_login_and_passes_its_secret() {
        let home = std::env::temp_dir().join(format!("hulahoop-lumi-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(home.join(".pi/agent")).unwrap();
        std::fs::write(home.join(".pi/agent/auth.json"), "stale-host-login").unwrap();
        let mut env = Vec::new();
        apply_credential(
            &mut env,
            &json!({ "credential": {
                "env": [["ANTHROPIC_API_KEY", "connector-secret"]],
                "files": []
            }}),
            &home,
        )
        .unwrap();
        assert!(!home.join(".pi/agent/auth.json").exists());
        assert!(env.contains(&("ANTHROPIC_API_KEY".into(), "connector-secret".into())));
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn next_question_uses_rotated_login() {
        let dir = std::env::temp_dir().join(format!("hulahoop-login-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source");
        let destination = dir.join("destination");
        std::fs::write(&source, "first").unwrap();
        refresh_login(&source, &destination, &dir);
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&source, "rotated").unwrap();
        refresh_login(&source, &destination, &dir);
        assert_eq!(std::fs::read_to_string(destination).unwrap(), "rotated");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn cli_refreshed_login_survives_older_host_file() {
        let dir = std::env::temp_dir().join(format!("hulahoop-login-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source");
        let destination = dir.join("destination");
        std::fs::write(&source, "expired").unwrap();
        refresh_login(&source, &destination, &dir);
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&destination, "cli-rotated").unwrap();
        refresh_login(&source, &destination, &dir);
        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            "cli-rotated"
        );
        std::fs::remove_file(&source).unwrap();
        refresh_login(&source, &destination, &dir);
        assert!(!destination.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn login_refresh_rejects_symlinked_destination() {
        let dir = std::env::temp_dir().join(format!("hulahoop-login-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source");
        let target = dir.join("checkout-file");
        let destination = dir.join("destination");
        std::fs::write(&source, "host-secret").unwrap();
        std::fs::write(&target, "untouched").unwrap();
        std::os::unix::fs::symlink(&target, &destination).unwrap();
        refresh_login(&source, &destination, &dir);
        assert_eq!(std::fs::read_to_string(target).unwrap(), "untouched");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn diagnostics_expose_a_category_without_the_private_error() {
        let error = lumi_failure_category("Invalid API key: secret-value", "");
        assert!(error.contains("authentication"));
        assert!(!error.contains("secret-value"));
        assert!(lumi_failure_category("rate limit 429", "").contains("rate limit"));
    }
}

#[cfg(test)]
mod hand_back_tests {
    use super::Executor;
    use crate::client::{Client, Connection};
    use serde_json::{json, Value};
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A server that answers every POST with `{}` and keeps what it was sent.
    async fn fake_server() -> (String, Arc<Mutex<Vec<(String, Value)>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kept = seen.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let kept = kept.clone();
                tokio::spawn(async move {
                    let mut buffer = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let (head, body) = loop {
                        let size = socket.read(&mut chunk).await.unwrap_or(0);
                        if size == 0 {
                            return;
                        }
                        buffer.extend_from_slice(&chunk[..size]);
                        let text = String::from_utf8_lossy(&buffer).to_string();
                        let Some(end) = text.find("\r\n\r\n") else {
                            continue;
                        };
                        let length = text[..end]
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|value| value.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        if buffer.len() >= end + 4 + length {
                            break (
                                text[..end].to_string(),
                                String::from_utf8_lossy(&buffer[end + 4..end + 4 + length])
                                    .to_string(),
                            );
                        }
                    };
                    let path = head
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or_default()
                        .to_string();
                    kept.lock()
                        .unwrap()
                        .push((path, serde_json::from_str(&body).unwrap_or(Value::Null)));
                    let _ = socket
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                        .await;
                });
            }
        });
        (base, seen)
    }

    /// A Lumi answer whose harness refreshed its login hands the new entry
    /// back before the login file goes — on success and on failure alike — and
    /// its reply never carries the token it was given.
    #[tokio::test]
    async fn a_refreshed_lumi_login_is_handed_back_before_it_is_removed() {
        let root = std::env::temp_dir().join(format!("hulahoop-handback-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let script = root.join("fake-harness.sh");
        std::fs::write(
            &script,
            r#"#!/bin/sh
given=$(cat "$HOME/.pi/agent/auth.json")
printf '%s' '{"openai-codex":{"type":"oauth","access":"fresh-access","refresh":"fresh-refresh","expires":2}}' > "$HOME/.pi/agent/auth.json"
echo "reply: $given"
[ "$2" = fail ] && exit 3
exit 0
"#,
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::env::set_var("HULAHOOP_FAKE_SCRIPT", &script);
        let (base, seen) = fake_server().await;
        let client = Client::new(&Connection::remote(&base, "token", root.clone())).unwrap();
        let executor = Executor::new(client, root.clone());

        let row = uuid::Uuid::now_v7();
        let given = json!({"openai-codex": {"type": "oauth", "access": "given-access-token-0123456789",
                                             "refresh": "given-refresh-token-0123456789", "expires": 1}});
        for model in ["ok", "fail"] {
            let job = uuid::Uuid::now_v7().to_string();
            let spec = json!({
                "artifact_dir": root.join(format!("work-{model}")),
                "payload": {"adapter": "fake", "model": model, "message": "hi", "system_prompt": ""},
                "credential": {
                    "env": [],
                    "files": [[".pi/agent/auth.json", given.to_string()]],
                    "refresh": {"connector": "codex-sub", "row": row, "file": ".pi/agent/auth.json",
                                "key": "openai-codex", "format": "pi"},
                },
            });
            let result = executor.job_lumi(&root, &spec, &job).await;
            let home = root.join("home/lumi").join(&job);
            assert!(
                !home.join(".pi/agent/auth.json").exists(),
                "the login file is gone after the answer"
            );
            let posted = seen.lock().unwrap().clone();
            let (path, body) = posted.last().expect("a hand-back").clone();
            assert_eq!(path, format!("/internal/jobs/{job}/credential"));
            assert_eq!(body["connector"], "codex-sub");
            assert_eq!(body["row"], json!(row));
            assert_eq!(body["entry"]["access"], "fresh-access");
            match model {
                "ok" => {
                    let reply = result.expect("an answer");
                    let text = reply.to_string();
                    assert!(!text.contains("given-access-token"), "{text}");
                }
                _ => assert!(result.is_err()),
            }
        }
        let _ = std::fs::remove_dir_all(root);
    }
}

#[cfg(test)]
mod write_skills_tests {
    use super::write_skills;

    /// The prompt names a skill by its path, so the file has to land exactly
    /// there: a slug rule that differs between server and runner sends the agent
    /// to a file that does not exist.
    #[test]
    fn a_skill_lands_where_the_prompt_says_it_does() {
        let dir = std::env::temp_dir().join(format!("hulahoop-skills-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("checkout");
        let spec = serde_json::json!({ "skills": [
            { "name": "House Style", "description": "how we write", "body": "# House Style\nUse sentences." },
            { "name": "", "description": "no name", "body": "ignored" },
        ]});
        write_skills(&spec, &dir).expect("write skills");
        let path = dir.join(".hulahoop/skills/house-style/SKILL.md");
        assert_eq!(
            std::fs::read_to_string(&path).expect("skill file"),
            "# House Style\nUse sentences."
        );
        assert!(dir.join(".hulahoop/skills").read_dir().unwrap().count() == 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_agent_with_no_skills_writes_nothing() {
        let dir = std::env::temp_dir().join(format!("hulahoop-no-skills-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("checkout");
        write_skills(&serde_json::json!({}), &dir).expect("no skills");
        assert!(!dir.join(".hulahoop").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
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

#[cfg(test)]
mod scratch_tests {
    use super::{is_repo_less, prepare_scratch, remove_scratch, write_skills};

    /// A repo-less run works in its own empty directory under the data dir,
    /// gets its skills written there like a checkout would, starts clean on a
    /// retry, and leaves nothing behind once it is removed.
    #[test]
    fn a_repo_less_run_gets_a_private_scratch_directory() {
        let data = std::env::temp_dir().join(format!("hulahoop-scratch-{}", uuid::Uuid::now_v7()));
        let run_id = uuid::Uuid::now_v7().to_string();
        let spec = serde_json::json!({
            "checkout": "none",
            "repo_path": null,
            "clone_url": null,
            "skills": [{ "name": "Report", "description": "how", "body": "# Report" }],
        });
        assert!(is_repo_less(&spec));
        assert!(!is_repo_less(&serde_json::json!({ "checkout": "repo" })));
        assert!(!is_repo_less(&serde_json::json!({})));

        let dir = prepare_scratch(&data, &run_id).expect("scratch");
        assert_eq!(dir, data.join("scratch").join(&run_id));
        assert_eq!(dir.read_dir().unwrap().count(), 0, "starts empty");
        write_skills(&spec, &dir).expect("skills");
        assert!(dir.join(".hulahoop/skills/report/SKILL.md").exists());
        std::fs::write(dir.join("left-over.txt"), "x").unwrap();

        let again = prepare_scratch(&data, &run_id).expect("scratch again");
        assert_eq!(again.read_dir().unwrap().count(), 0, "a retry starts clean");

        remove_scratch(&data, &run_id);
        assert!(!again.exists());
        assert!(prepare_scratch(&data, "../escape").is_err());
        let _ = std::fs::remove_dir_all(&data);
    }
}

#[cfg(test)]
mod checkout_tests {
    use super::{
        contract, host_repository, job_cleanup, job_forget_mirror, job_land, owned_checkout,
    };
    use std::path::Path;
    use std::process::Command;

    fn sh(cwd: &Path, program: &str, args: &[&str]) -> String {
        // CI machines have no jj identity, and jj will not push a commit without one.
        let output = Command::new(program)
            .args(args)
            .current_dir(cwd)
            .env("JJ_USER", "t")
            .env("JJ_EMAIL", "t@example.test")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{program} {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// A repository with no url has nothing to clone, and the run says so
    /// instead of working in some other directory.
    #[tokio::test]
    async fn a_repository_without_a_url_fails_clearly() {
        let mirror = std::env::temp_dir().join(format!("hulahoop-mirror-{}", uuid::Uuid::now_v7()));
        let spec = serde_json::json!({
            "repo_path": mirror,
            "clone_url": null,
        });
        let error = host_repository(&spec).await.unwrap_err().to_string();
        assert!(error.contains("no url to clone from"), "{error}");
        assert!(
            !mirror.exists(),
            "nothing is created for a repository without a url"
        );
    }

    /// The first use clones the url into the mirror through a temporary
    /// directory that does not outlive it.
    #[tokio::test]
    async fn a_mirror_is_cloned_into_place() {
        let root = std::env::temp_dir().join(format!("hulahoop-clone-{}", uuid::Uuid::now_v7()));
        let origin = root.join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        sh(&origin, "git", &["init", "-q", "-b", "main"]);
        std::fs::write(origin.join("a.txt"), "one\n").unwrap();
        sh(&origin, "git", &["add", "-A"]);
        sh(
            &origin,
            "git",
            &[
                "-c",
                "user.email=t@example.test",
                "-c",
                "user.name=t",
                "commit",
                "-qm",
                "one",
            ],
        );
        let mirror = root.join("data/mirrors/repo");
        let spec = serde_json::json!({
            "repo_path": mirror,
            "clone_url": origin,
        });
        assert_eq!(host_repository(&spec).await.unwrap(), mirror);
        assert!(mirror.join(".jj").exists() && mirror.join("a.txt").exists());
        // The second use fetches the mirror it has.
        assert_eq!(host_repository(&spec).await.unwrap(), mirror);
        let left: Vec<_> = std::fs::read_dir(root.join("data/mirrors"))
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(left, vec![std::ffi::OsString::from("repo")]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A checkout is always `checkouts/<project id>/<task id>` under the data
    /// directory: ids that are not ids, and a symlink that leads out of it, are
    /// refused.
    #[test]
    fn a_checkout_is_derived_from_ids_and_stays_inside() {
        let data = std::env::temp_dir().join(format!("hulahoop-owned-{}", uuid::Uuid::now_v7()));
        let (project, issue) = (uuid::Uuid::now_v7(), uuid::Uuid::now_v7());
        let ids = |p: &str, i: &str| (serde_json::json!(p), serde_json::json!(i));

        let (p, i) = ids(&project.to_string(), &issue.to_string());
        let checkout = owned_checkout(&data, &p, &i).unwrap();
        assert_eq!(
            checkout,
            data.join("checkouts")
                .join(project.to_string())
                .join(issue.to_string())
        );
        for (p, i) in [
            ids("project", &issue.to_string()),
            ids("../../mirrors", &issue.to_string()),
            ids(&project.to_string(), ""),
        ] {
            assert!(owned_checkout(&data, &p, &i).is_err(), "{p} {i}");
        }

        // A project directory that is a symlink to the mirrors is not a checkout.
        let mirrors = data.join("mirrors");
        std::fs::create_dir_all(&mirrors).unwrap();
        std::fs::create_dir_all(data.join("checkouts")).unwrap();
        std::os::unix::fs::symlink(&mirrors, data.join("checkouts").join(project.to_string()))
            .unwrap();
        let (p, i) = ids(&project.to_string(), &issue.to_string());
        assert!(owned_checkout(&data, &p, &i).is_err());
        let _ = std::fs::remove_dir_all(&data);
    }

    /// A cleanup deletes the task's checkout under the data directory and
    /// nothing else; a payload that names no task is refused.
    #[test]
    fn a_cleanup_only_deletes_task_checkouts() {
        let data = std::env::temp_dir().join(format!("hulahoop-cleanup-{}", uuid::Uuid::now_v7()));
        let (project, issue) = (uuid::Uuid::now_v7(), uuid::Uuid::now_v7());
        let checkout = data
            .join("checkouts")
            .join(project.to_string())
            .join(issue.to_string());
        let mirror = data.join("mirrors").join("repo");
        std::fs::create_dir_all(checkout.join("target")).unwrap();
        std::fs::create_dir_all(&mirror).unwrap();

        for payload in [
            serde_json::json!({ "checkout": mirror }),
            serde_json::json!({ "project_id": "..", "issue_id": issue }),
            serde_json::json!({ "project_id": project }),
        ] {
            let spec = serde_json::json!({
                "repo_path": data.join("mirrors/none"),
                "payload": payload,
            });
            assert!(job_cleanup(&data, &spec).is_err(), "{payload}");
        }
        assert!(mirror.exists() && checkout.exists());

        // The mirror is not a repository here, so only the directory goes.
        let spec = serde_json::json!({
            "repo_path": data.join("mirrors/none"),
            "payload": { "project_id": project, "issue_id": issue },
        });
        job_cleanup(&data, &spec).expect("cleanup");
        assert!(!checkout.exists());
        // Already gone is done, not an error.
        job_cleanup(&data, &spec).expect("cleanup again");
        let _ = std::fs::remove_dir_all(&data);
    }

    /// A repository whose url changed loses its mirror and nothing else: the
    /// path has to be `mirrors/<repository id>`, whatever the server sent.
    #[tokio::test]
    async fn forgetting_a_mirror_only_deletes_that_mirror() {
        let data = std::env::temp_dir().join(format!("hulahoop-forget-{}", uuid::Uuid::now_v7()));
        let repo = uuid::Uuid::now_v7();
        let mirror = contract::mirror_dir(&data, &repo);
        let other = contract::mirror_dir(&data, &uuid::Uuid::now_v7());
        std::fs::create_dir_all(mirror.join(".jj")).unwrap();
        std::fs::create_dir_all(&other).unwrap();

        for spec in [
            serde_json::json!({ "repo_path": mirror }),
            serde_json::json!({ "repo_id": repo, "repo_path": other }),
            serde_json::json!({ "repo_id": repo, "repo_path": data }),
            serde_json::json!({ "repo_id": repo, "repo_path": other.join("..").join(repo.to_string()) }),
            serde_json::json!({ "repo_id": repo, "repo_path": format!("mirrors/{repo}") }),
        ] {
            assert!(job_forget_mirror(&data, &spec).await.is_err(), "{spec}");
        }
        assert!(mirror.exists() && other.exists());

        let spec = serde_json::json!({ "repo_id": repo, "repo_path": mirror });
        job_forget_mirror(&data, &spec).await.expect("forget");
        assert!(!mirror.exists() && other.exists());
        // Already gone is done, not an error.
        job_forget_mirror(&data, &spec).await.expect("forget again");
        let _ = std::fs::remove_dir_all(&data);
    }

    /// A merge without a pull request moves the base forward to the merged
    /// head and pushes it, so the remote has the merge; a base that moved on
    /// sideways is refused instead of losing its work.
    #[test]
    fn a_landing_pushes_the_base_forward_only() {
        let root = std::env::temp_dir().join(format!("hulahoop-land-{}", uuid::Uuid::now_v7()));
        let origin = root.join("origin.git");
        std::fs::create_dir_all(&origin).unwrap();
        sh(&origin, "git", &["init", "-q", "--bare", "-b", "main"]);
        let seed = root.join("seed");
        std::fs::create_dir_all(&seed).unwrap();
        sh(&seed, "git", &["init", "-q", "-b", "main"]);
        std::fs::write(seed.join("a.txt"), "one\n").unwrap();
        sh(&seed, "git", &["add", "-A"]);
        let identity = ["-c", "user.email=t@example.test", "-c", "user.name=t"];
        sh(
            &seed,
            "git",
            &[&identity[..], &["commit", "-qm", "one"]].concat(),
        );
        sh(
            &seed,
            "git",
            &["push", "-q", &origin.to_string_lossy(), "main"],
        );

        let mirror = root.join("mirror");
        sh(
            &root,
            "jj",
            &[
                "git",
                "clone",
                "--colocate",
                "--",
                &origin.to_string_lossy(),
                "mirror",
            ],
        );
        // The task's head, on top of main.
        sh(&mirror, "jj", &["new", "main", "-m", "task"]);
        std::fs::write(mirror.join("b.txt"), "b\n").unwrap();
        let head = sh(
            &mirror,
            "jj",
            &["log", "-r", "@", "--no-graph", "-T", "commit_id"],
        );
        sh(&mirror, "jj", &["new"]);

        let spec = serde_json::json!({
            "base_bookmark": "main",
            "payload": { "revision": head },
        });
        let landed = job_land(&mirror, &spec).unwrap();
        assert_eq!(landed["base_revision"].as_str(), Some(head.as_str()));
        assert_eq!(sh(&origin, "git", &["rev-parse", "main"]), head);

        // main moves on elsewhere; an older sibling head cannot land on it.
        sh(&mirror, "jj", &["new", "main-", "-m", "other"]);
        std::fs::write(mirror.join("c.txt"), "c\n").unwrap();
        let sideways = sh(
            &mirror,
            "jj",
            &["log", "-r", "@", "--no-graph", "-T", "commit_id"],
        );
        let spec = serde_json::json!({
            "base_bookmark": "main",
            "payload": { "revision": sideways },
        });
        let error = job_land(&mirror, &spec).unwrap_err();
        assert!(
            format!("{error:#}").contains("could not move forward"),
            "{error:#}"
        );
        assert_eq!(sh(&origin, "git", &["rev-parse", "main"]), head);
        let _ = std::fs::remove_dir_all(&root);
    }
}
