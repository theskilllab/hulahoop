//! The working diff of a run in progress, taken while the agent works so a
//! person can watch the change grow.
//!
//! It runs as its own task with its own interval: a slow diff or a slow server
//! never holds up the heartbeat, the output stream, or the run's limits. It
//! reads the checkout with git only, and never with jj: jj snapshots the working
//! copy on every command, which would race the agent's own jj commands. Every
//! checkout is a colocated jj workspace (a git worktree), so git sees the same
//! files; `GIT_OPTIONAL_LOCKS=0` keeps git off the index lock the agent may need.

use crate::client::{Client, LiveDiffSent};
use anyhow::{bail, Context, Result};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

/// A checkout git can read, and the commit its diff is taken from.
#[derive(Debug, Clone)]
pub(crate) struct LiveDiffSource {
    checkout: PathBuf,
    base: String,
}

impl LiveDiffSource {
    /// The source for a checkout, or `None` when git cannot read it (a checkout
    /// that is not colocated) or does not know the base commit. Called once,
    /// before the agent starts; `base` is a commit id.
    pub(crate) async fn new(checkout: &Path, base: &str) -> Option<Self> {
        let source = LiveDiffSource {
            checkout: checkout.to_path_buf(),
            base: base.trim().to_string(),
        };
        if source.base.is_empty() {
            return None;
        }
        let commit = format!("{}^{{commit}}", source.base);
        let known = source
            .git(&["rev-parse", "--verify", "--quiet", &commit])
            .await
            .ok()?;
        Some(LiveDiffSource {
            base: known.trim().to_string(),
            ..source
        })
    }

    async fn git(&self, args: &[&str]) -> Result<String> {
        let (status, stdout, stderr) = self.git_raw(args).await?;
        if !status.success() {
            bail!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&stdout).into_owned())
    }

    async fn git_raw(&self, args: &[&str]) -> Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>)> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.checkout)
            .args(args)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            // A timed-out tick drops this future; the git process goes with it.
            .kill_on_drop(true)
            .output()
            .await
            .with_context(|| format!("running git {}", args.join(" ")))?;
        Ok((output.status, output.stdout, output.stderr))
    }

    /// The checkout against the base: tracked changes, then every untracked,
    /// unignored file as a new file. Stops adding new files once the patch is
    /// past the byte limit, since the cap would drop them anyway.
    pub(crate) async fn take(&self) -> Result<String> {
        let mut patch = self
            .git(&[
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--no-renames",
                &self.base,
            ])
            .await?;
        let untracked = self
            .git(&["ls-files", "--others", "--exclude-standard", "-z"])
            .await?;
        for path in untracked.split('\0').filter(|path| !path.is_empty()) {
            if patch.len() > hulahoop_core::LIVE_DIFF_BYTE_LIMIT {
                break;
            }
            // `--no-index` exits 1 when the files differ, which is always here.
            let (status, stdout, stderr) = self
                .git_raw(&[
                    "diff",
                    "--no-color",
                    "--no-ext-diff",
                    "--no-index",
                    "--",
                    "/dev/null",
                    path,
                ])
                .await?;
            if status.code().is_some_and(|code| code > 1) {
                tracing::debug!(path, stderr = %String::from_utf8_lossy(&stderr).trim(), "could not diff an untracked file");
                continue;
            }
            patch.push_str(&String::from_utf8_lossy(&stdout));
        }
        Ok(patch)
    }
}

/// Remembers the working diff sent last, so the same diff is not sent on every
/// tick of a run where nothing changed.
#[derive(Debug, Default)]
pub(crate) struct LiveDiffTracker {
    last: Option<u64>,
}

impl LiveDiffTracker {
    /// The diff to send, capped at a file boundary under the live byte limit,
    /// or `None` when it is the one sent last.
    pub(crate) fn changed(&mut self, patch: &str) -> Option<String> {
        let capped = hulahoop_core::cap_patch(patch, hulahoop_core::LIVE_DIFF_BYTE_LIMIT);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        capped.hash(&mut hasher);
        let hash = hasher.finish();
        if self.last == Some(hash) {
            return None;
        }
        self.last = Some(hash);
        Some(capped)
    }

    /// Forgets the last diff, after a send that failed.
    pub(crate) fn forget(&mut self) {
        self.last = None;
    }
}

/// The live diff task of one run. Dropping it stops the task, however the run
/// ends.
pub(crate) struct LiveDiffTask(tokio::task::JoinHandle<()>);

impl Drop for LiveDiffTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Starts taking and sending the live diff of `run_id` every `every`.
pub(crate) fn spawn(
    client: Client,
    run_id: String,
    source: LiveDiffSource,
    every: Duration,
    limit: Duration,
) -> LiveDiffTask {
    LiveDiffTask(tokio::spawn(async move {
        let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
        // A tick that ran long is followed by a full interval, never a burst.
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut tracker = LiveDiffTracker::default();
        loop {
            ticker.tick().await;
            tick(&client, &run_id, &source, &mut tracker, limit).await;
        }
    }))
}

/// One live diff: taken, and sent when it changed. Any failure is only logged:
/// the live diff is a view, and the run goes on.
async fn tick(
    client: &Client,
    run_id: &str,
    source: &LiveDiffSource,
    tracker: &mut LiveDiffTracker,
    limit: Duration,
) {
    let patch = match tokio::time::timeout(limit, source.take()).await {
        Ok(Ok(patch)) => patch,
        Ok(Err(error)) => {
            tracing::debug!(run_id, %error, "could not take the live diff");
            return;
        }
        Err(_) => {
            tracing::debug!(run_id, "the live diff took too long; skipping this tick");
            return;
        }
    };
    let Some(patch) = tracker.changed(&patch) else {
        return;
    };
    match tokio::time::timeout(limit, client.live_diff(run_id, &patch)).await {
        Ok(Ok(LiveDiffSent::Sent)) => {}
        // The same diff would be refused again; the next change is tried.
        Ok(Ok(LiveDiffSent::TooLarge)) => {
            tracing::debug!(
                run_id,
                bytes = patch.len(),
                "the server refused the live diff as too large"
            );
        }
        Ok(Err(error)) => {
            tracing::debug!(run_id, %error, "could not send the live diff");
            // Nothing arrived, so the next tick sends the same diff again.
            tracker.forget();
        }
        Err(_) => {
            tracing::debug!(run_id, "sending the live diff took too long");
            tracker.forget();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;

    #[test]
    fn an_unchanged_diff_is_sent_once() {
        let mut tracker = LiveDiffTracker::default();
        assert_eq!(tracker.changed("a\n").as_deref(), Some("a\n"));
        assert_eq!(tracker.changed("a\n"), None);
        assert_eq!(tracker.changed("a\nb\n").as_deref(), Some("a\nb\n"));
        assert_eq!(tracker.changed("a\nb\n"), None);
    }

    #[test]
    fn a_failed_send_is_tried_again() {
        let mut tracker = LiveDiffTracker::default();
        assert!(tracker.changed("a\n").is_some());
        tracker.forget();
        assert!(tracker.changed("a\n").is_some());
    }

    #[test]
    fn a_diff_is_capped_at_a_file_boundary() {
        let mut tracker = LiveDiffTracker::default();
        let limit = hulahoop_core::LIVE_DIFF_BYTE_LIMIT;
        let small = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -0,0 +1 @@\n+a\n";
        let big: String =
            std::iter::once("diff --git a/b b/b\n--- a/b\n+++ b/b\n@@ -0,0 +1 @@\n".to_string())
                .chain((0..limit / 8).map(|i| format!("+{i:07}\n")))
                .collect();
        let sent = tracker.changed(&format!("{small}{big}")).unwrap();
        assert_eq!(sent, small);
        // Growth inside the file past the cap changes nothing a watcher would see.
        assert_eq!(tracker.changed(&format!("{small}{big}+more\n")), None);
    }

    fn run(dir: &Path, program: &str, args: &[&str]) -> String {
        let out = StdCommand::new(program)
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("JJ_USER", "t")
            .env("JJ_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{program} {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    /// A checkout made the way the runner makes one — a colocated clone, then
    /// `jj workspace add` — is readable by git, and its live diff shows edits,
    /// deletions, jj commits made since the base, and files nobody added.
    #[tokio::test]
    async fn a_workspace_checkout_diffs_with_git_including_untracked_files() {
        let root = std::env::temp_dir().join(format!("hh-live-{}", uuid::Uuid::now_v7()));
        let origin = root.join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        run(&origin, "git", &["init", "-q", "-b", "main"]);
        std::fs::write(origin.join("a.txt"), "a\n").unwrap();
        std::fs::write(origin.join("b.txt"), "b\n").unwrap();
        std::fs::write(origin.join(".gitignore"), "ignored.txt\n").unwrap();
        run(&origin, "git", &["add", "-A"]);
        run(&origin, "git", &["commit", "-qm", "init"]);
        run(
            &root,
            "jj",
            &["git", "clone", "--colocate", "origin", "mirror"],
        );
        let mirror = root.join("mirror");
        let checkout = root.join("checkout");
        run(
            &mirror,
            "jj",
            &[
                "workspace",
                "add",
                &checkout.to_string_lossy(),
                "--name",
                "live",
                "-r",
                "main",
            ],
        );
        let base = run(
            &mirror,
            "jj",
            &["log", "-r", "main", "--no-graph", "-T", "commit_id"],
        );

        // The runner's checkout is colocated: git can read it.
        run(&checkout, "git", &["rev-parse", "--show-toplevel"]);
        let source = LiveDiffSource::new(&checkout, &base)
            .await
            .expect("git reads the checkout");
        assert_eq!(source.take().await.unwrap(), "");

        std::fs::write(checkout.join("a.txt"), "changed\n").unwrap();
        std::fs::remove_file(checkout.join("b.txt")).unwrap();
        std::fs::create_dir_all(checkout.join("d")).unwrap();
        std::fs::write(checkout.join("d/new.txt"), "new\n").unwrap();
        std::fs::write(checkout.join("ignored.txt"), "secret\n").unwrap();
        let patch = source.take().await.unwrap();
        assert!(patch.contains("diff --git a/a.txt b/a.txt"), "{patch}");
        assert!(patch.contains("+changed"), "{patch}");
        assert!(patch.contains("deleted file mode"), "{patch}");
        assert!(
            patch.contains("diff --git a/d/new.txt b/d/new.txt"),
            "{patch}"
        );
        assert!(patch.contains("new file mode"), "{patch}");
        assert!(patch.contains("+new"), "{patch}");
        assert!(!patch.contains("ignored.txt"), "{patch}");

        // The agent commits with jj, then keeps editing: both still show.
        run(&checkout, "jj", &["commit", "-m", "wip"]);
        std::fs::write(checkout.join("d/new.txt"), "new\nmore\n").unwrap();
        std::fs::write(checkout.join("later.txt"), "later\n").unwrap();
        let patch = source.take().await.unwrap();
        assert!(patch.contains("+changed"), "{patch}");
        assert!(patch.contains("+more"), "{patch}");
        assert!(
            patch.contains("diff --git a/later.txt b/later.txt"),
            "{patch}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_checkout_git_cannot_read_has_no_live_diff() {
        let root = std::env::temp_dir().join(format!("hh-live-none-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        assert!(LiveDiffSource::new(&root, "0123456789abcdef")
            .await
            .is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The live diff runs beside the run loop: a server that never answers
    /// holds up neither the caller nor the next heartbeat, and dropping the
    /// task stops it.
    #[tokio::test]
    async fn a_stuck_live_diff_does_not_hold_up_the_run() {
        // A server that accepts the connection and never answers.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let accepted = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket);
            }
        });
        let root = std::env::temp_dir().join(format!("hh-live-stuck-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        run(&root, "git", &["init", "-q", "-b", "main"]);
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        run(&root, "git", &["add", "-A"]);
        run(&root, "git", &["commit", "-qm", "init"]);
        let base = run(&root, "git", &["rev-parse", "HEAD"]);
        std::fs::write(root.join("a.txt"), "b\n").unwrap();
        let source = LiveDiffSource::new(&root, &base).await.unwrap();
        let connection =
            crate::client::Connection::remote(&format!("http://{address}"), "token", root.clone());
        let client = Client::new(&connection).unwrap();

        let task = spawn(
            client,
            "run".into(),
            source,
            Duration::from_millis(10),
            Duration::from_secs(60),
        );
        // The "heartbeat" keeps its schedule while the send hangs.
        let started = std::time::Instant::now();
        let mut beats = 0;
        let mut ticker = tokio::time::interval(Duration::from_millis(20));
        while beats < 10 {
            ticker.tick().await;
            beats += 1;
        }
        assert!(started.elapsed() < Duration::from_secs(2));
        let stopper = task.0.abort_handle();
        assert!(!stopper.is_finished());
        drop(task);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(stopper.is_finished(), "dropping the task stops it");
        accepted.abort();
        let _ = std::fs::remove_dir_all(&root);
    }
}
