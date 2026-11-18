//! One tick of the loop: claim work, set up a checkout, run the agent, report.

use crate::adapter;
use crate::client::Client;
use crate::vcs::{Kind, Vcs};
use anyhow::{anyhow, Context, Result};
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
        let repo_path = PathBuf::from(spec["repo_path"].as_str().unwrap_or_default());
        let checkout = PathBuf::from(spec["checkout_path"].as_str().unwrap_or_default());
        let artifact_dir = PathBuf::from(spec["artifact_dir"].as_str().unwrap_or_default());
        std::fs::create_dir_all(&artifact_dir)?;
        let base_bookmark = spec["base_bookmark"].as_str().unwrap_or("main").to_string();
        let issue_id = spec["issue_id"].as_str().unwrap_or_default().to_string();
        let attempt = spec["attempt"].as_i64().unwrap_or(0);
        let model = spec["model"].as_str().unwrap_or_default().to_string();
        let system_prompt = spec["prompt"].as_str().unwrap_or_default().to_string();
        let input_path = PathBuf::from(spec["input_path"].as_str().unwrap_or_default());
        let wall_clock = spec["timeout_minutes"]
            .as_i64()
            .unwrap_or(hulahoop_core::AGENT_WALL_CLOCK_MINUTES);

        let vcs = Vcs::detect(&repo_path)?;
        vcs.ensure_vcs()?;
        vcs.add_workspace(&checkout, &base_bookmark)?;
        ensure_envrc(&checkout)?;

        self.client
            .event(
                run_id,
                "checkout",
                json!({ "path": checkout, "base": base_bookmark }),
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

        let diff = vcs.diff_from(&checkout, &base_bookmark).unwrap_or_default();
        let changed_paths = vcs
            .changed_paths(&checkout, &base_bookmark)
            .unwrap_or_default();
        let commit = vcs.working_copy_revision(&checkout).unwrap_or_default();

        // The bookmark the server will resolve later. It is created here because
        // only the checkout knows the commit id.
        let _ = match role {
            AgentRole::TestAuthor => {
                vcs.set_bookmark_here(&checkout, &format!("tests/{issue_id}/cand/{run_id}"))
            }
            AgentRole::Implementer => {
                vcs.set_bookmark_here(&checkout, &format!("issue/{issue_id}/head/{attempt}"))
            }
            AgentRole::Reviewer => Ok(commit.clone()),
        };

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
        if status == "succeeded" {
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
        let repo_path = PathBuf::from(spec["repo_path"].as_str().unwrap_or_default());
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
                let base = spec["base_bookmark"].as_str().unwrap_or("main").to_string();
                // Rebase the approved head onto the refreshed base. jj carries the
                // bookmark with the rewritten commit.
                vcs.set_bookmark(&landing_bookmark, &approved_head)?;
                vcs.add_workspace(&checkout, &landing_bookmark)?;
                vcs.rebase_bookmark(&checkout, &landing_bookmark, &base)?;
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
        let repo_path = PathBuf::from(spec["repo_path"].as_str().unwrap_or_default());

        let result = match kind.as_str() {
            "fetch" => self.job_fetch(&repo_path, spec).await,
            "preflight" => self.job_preflight(&repo_path, spec).await,
            "merge_verify" => self.job_merge_verify(&repo_path, spec).await,
            "bookmark" => self.job_bookmark(&repo_path, spec).await,
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
    Ok(env)
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
