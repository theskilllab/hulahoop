//! Everything that touches the repo: workspaces, bookmarks, diffs, rebases.
//! jj is the supported path; git is a fallback for repos without `.jj`.

use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// How long a fetch, clone or push may take. One that hangs holds the
/// mirror's lock while it does, and every run and job of the repository waits
/// behind it, all of them heartbeating as if nothing were wrong.
pub const NETWORK_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// What `command` printed, once it has exited; an error once it has run for
/// `limit`, after it and everything it started (its own process group) are
/// killed.
pub fn output_within(command: &mut Command, limit: Duration) -> Result<Output> {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let drain = |stream: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut stream) = stream {
                let _ = stream.read_to_end(&mut bytes);
            }
            bytes
        })
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let deadline = Instant::now() + limit;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            use nix::sys::signal::{killpg, Signal};
            use nix::unistd::Pid;
            let _ = killpg(Pid::from_raw(child.id() as i32), Signal::SIGKILL);
            let _ = child.wait();
            bail!("gave up after {} seconds", limit.as_secs());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    Ok(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

pub struct Vcs {
    pub root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Jj,
    Git,
}

impl Vcs {
    pub fn detect(root: &Path) -> Result<Self> {
        if !root.exists() {
            bail!("repo path {} does not exist", root.display());
        }
        Ok(Vcs {
            root: root.to_path_buf(),
        })
    }

    pub fn kind(&self) -> Kind {
        if self.root.join(".jj").exists() || self.jj_ok() {
            Kind::Jj
        } else {
            Kind::Git
        }
    }

    fn jj_ok(&self) -> bool {
        self.run(&self.root, "jj", &["--version"]).is_ok()
    }

    fn run(&self, cwd: &Path, program: &str, args: &[&str]) -> Result<String> {
        let output = Command::new(program)
            .args(args)
            .current_dir(cwd)
            .output()
            .with_context(|| format!("running {program} {args:?} in {}", cwd.display()))?;
        if !output.status.success() {
            bail!(
                "{program} {args:?} failed in {}: {}",
                cwd.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// A plain git repo has no bookmarks and no working-copy commit, so the
    /// runner colocates it once. This writes into the user's repo, which is why
    /// it is logged.
    pub fn ensure_vcs(&self) -> Result<Kind> {
        if self.root.join(".jj").exists() {
            return Ok(Kind::Jj);
        }
        if self.root.join(".git").exists() {
            tracing::info!(repo = %self.root.display(), "colocating a git repo with jj");
            self.run(&self.root, "jj", &["git", "init", "--colocate"])?;
            let _ = self.run(&self.root, "jj", &["bookmark", "create", "main", "-r", "@"]);
            return Ok(Kind::Jj);
        }
        bail!(
            "{} is not a jj or git repository. Hulahoop runs work in jj workspaces, so the repo needs version control before the loop starts.",
            self.root.display()
        )
    }

    /// Puts a task's checkout on `revision`, with a fresh working-copy commit on
    /// top of it.
    ///
    /// A task keeps one checkout for all of its turns, so one that already exists
    /// and is still a workspace of this repository is reused: what the project
    /// built there (target/, node_modules, .venv) survives, and the next turn
    /// starts warm. Anything else in the way, a directory jj does not know or a
    /// workspace that cannot be brought back, is replaced by a new workspace.
    pub fn add_workspace(&self, checkout: &Path, revision: &str) -> Result<()> {
        let revision = self.resolve(revision)?;
        match self.reuse_workspace(checkout, &revision) {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(%error, checkout = %checkout.display(), "replacing a checkout that could not be reused");
            }
        }
        if let Some(parent) = checkout.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if checkout.exists() {
            let _ = std::fs::remove_dir_all(checkout);
        }
        let name = workspace_name(checkout);
        let _ = self.run(&self.root, "jj", &["workspace", "forget", &name]);
        self.run(
            &self.root,
            "jj",
            &[
                "workspace",
                "add",
                &checkout.to_string_lossy(),
                "--name",
                &name,
                "-r",
                &revision,
            ],
        )?;
        Ok(())
    }

    /// Moves an existing checkout onto `revision` for a new turn. `false` when
    /// there is no workspace to reuse.
    ///
    /// The previous turn's working copy stays where it was: a published head
    /// keeps its bookmark, and whatever a crashed turn left uncommitted was
    /// snapshotted into that commit by jj, so none of it reaches the new turn.
    /// Only an empty leftover is abandoned, through the guarded [`Vcs::abandon`].
    ///
    /// Ignored files stay on disk between turns, and one of them can sit where
    /// the new start revision tracks a file: jj then skips that update and the
    /// stale file reads as a change of the new turn. A fresh turn has changed
    /// nothing, so its working copy is put back to the start revision's tree;
    /// one that still differs afterwards is an error, and the caller replaces
    /// the checkout.
    fn reuse_workspace(&self, checkout: &Path, revision: &str) -> Result<bool> {
        let name = workspace_name(checkout);
        if !checkout.join(".jj").exists() || !self.has_workspace(&name)? {
            return Ok(false);
        }
        // A turn that changed nothing abandons its working copy from the
        // repository root, which leaves this checkout stale until it is updated.
        let _ = self.run(checkout, "jj", &["workspace", "update-stale"]);
        let previous = self.working_copy_revision(checkout)?;
        self.run(checkout, "jj", &["new", revision])?;
        if previous != revision {
            let _ = self.abandon(&format!("{previous} & empty()"));
        }
        if !self.working_copy_diff(checkout)?.is_empty() {
            self.run(checkout, "jj", &["restore", "--from", revision])?;
            let left = self.working_copy_diff(checkout)?;
            if !left.is_empty() {
                bail!(
                    "the reused checkout still differs from its start revision: {}",
                    left.trim()
                );
            }
        }
        Ok(true)
    }

    /// What the checkout's working-copy commit changes, one path per line;
    /// empty when it changes nothing.
    fn working_copy_diff(&self, checkout: &Path) -> Result<String> {
        let summary = self.run(checkout, "jj", &["diff", "-r", "@", "--summary"])?;
        Ok(summary.trim().to_string())
    }

    /// Whether the repository has a workspace by this name.
    fn has_workspace(&self, name: &str) -> Result<bool> {
        let listed = self.run(
            &self.root,
            "jj",
            &["workspace", "list", "-T", "name ++ \"\\n\""],
        )?;
        Ok(listed.lines().any(|line| line.trim() == name))
    }

    /// Forgets a checkout's workspace in this repository and deletes its
    /// directory, once the task it belonged to is over.
    pub fn remove_workspace(&self, checkout: &Path) -> Result<()> {
        let name = workspace_name(checkout);
        let _ = self.run(&self.root, "jj", &["workspace", "forget", &name]);
        if checkout.exists() {
            std::fs::remove_dir_all(checkout)
                .with_context(|| format!("removing {}", checkout.display()))?;
        }
        Ok(())
    }

    /// The first of `candidates` that names a revision in this repository,
    /// resolved to its commit id. A task that was in flight when its repository
    /// moved to a fresh mirror may name a revision the mirror never had; its
    /// bookmark on the remote, or the repository's base there, still does.
    pub fn resolve_first(&self, candidates: &[String]) -> Result<String> {
        let mut tried = Vec::new();
        for candidate in candidates.iter().map(|c| c.trim()) {
            if candidate.is_empty() || tried.contains(&candidate) {
                continue;
            }
            tried.push(candidate);
            if let Ok(commit) = self.resolve(candidate) {
                if !commit.is_empty() {
                    return Ok(commit);
                }
            }
        }
        bail!(
            "none of {} exists in the repository, so the turn has nothing to start from",
            tried.join(", ")
        )
    }

    /// Bookmarks are the only handles that survive a rebase, so every revision
    /// the server stores is resolved from one.
    pub fn resolve(&self, revision: &str) -> Result<String> {
        if revision.trim().is_empty() {
            return Ok("@".to_string());
        }
        let output = self.run(
            &self.root,
            "jj",
            &["log", "-r", revision, "--no-graph", "-T", "commit_id"],
        )?;
        Ok(output.trim().to_string())
    }

    /// The bookmark for a working copy inside a checkout. `@` means "the working
    /// copy of the directory I am run in", so this has to run there: resolving `@`
    /// in the repo root would bookmark the user's own tree.
    /// Give the working-copy commit an author. A commit inherits the identity of
    /// whoever created it, so a checkout made by the runner would otherwise
    /// credit every agent's work to the machine.
    /// The identity on the checkout's working-copy commit, as `jj` reports it.
    pub fn working_copy_author(&self, checkout: &Path) -> Result<(String, String)> {
        let output = self.run(
            checkout,
            "jj",
            &[
                "log",
                "-r",
                "@",
                "--no-graph",
                "-T",
                "author.name() ++ \"\\n\" ++ author.email()",
            ],
        )?;
        let mut lines = output.lines();
        let name = lines.next().unwrap_or_default().trim().to_string();
        let email = lines.next().unwrap_or_default().trim().to_string();
        Ok((name, email))
    }

    /// Give the working-copy commit a message. jj refuses to push a commit that
    /// has none, and `jj log` is unreadable without it, so a run names its own.
    /// The same as `describe_here`, but the rewrite is
    /// recorded under the bot that owns the run. jj stamps the committer from the
    /// configured user on every rewrite, so describing a commit without this puts
    /// the developer back on it as committer.
    pub fn describe_as(
        &self,
        checkout: &Path,
        message: &str,
        name: &str,
        email: &str,
    ) -> Result<()> {
        let output = Command::new("jj")
            .args(["describe", "-m", message])
            .current_dir(checkout)
            .env("JJ_USER", name)
            .env("JJ_EMAIL", email)
            .output()
            .context("running jj describe")?;
        if !output.status.success() {
            bail!(
                "jj describe failed in {}: {}",
                checkout.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }

    pub fn set_working_copy_author(&self, checkout: &Path, name: &str, email: &str) -> Result<()> {
        let output = Command::new("jj")
            .args(["metaedit", "--update-author"])
            .current_dir(checkout)
            .env("JJ_USER", name)
            .env("JJ_EMAIL", email)
            .output()
            .with_context(|| format!("running jj metaedit in {}", checkout.display()))?;
        if !output.status.success() {
            bail!(
                "jj metaedit --update-author failed in {}: {}",
                checkout.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }

    pub fn set_bookmark_here(&self, checkout: &Path, name: &str) -> Result<String> {
        let output = self.run(
            checkout,
            "jj",
            &["bookmark", "set", name, "-r", "@", "--allow-backwards"],
        )?;
        let _ = output;
        self.run(
            checkout,
            "jj",
            &["log", "-r", name, "--no-graph", "-T", "commit_id"],
        )
    }

    pub fn set_bookmark(&self, name: &str, revision: &str) -> Result<String> {
        let resolved = self.resolve(revision)?;
        let _ = self.run(
            &self.root,
            "jj",
            &[
                "bookmark",
                "set",
                name,
                "-r",
                &resolved,
                "--allow-backwards",
            ],
        );
        Ok(resolved)
    }

    pub fn working_copy_revision(&self, checkout: &Path) -> Result<String> {
        let output = self.run(
            checkout,
            "jj",
            &["log", "-r", "@", "--no-graph", "-T", "commit_id"],
        )?;
        Ok(output.trim().to_string())
    }

    /// The diff the human reviews, as a patch. `--from` is the base bookmark the
    /// run branched from, so a rebase elsewhere cannot change what it means.
    pub fn diff_from(&self, checkout: &Path, base: &str) -> Result<String> {
        let resolved = self.resolve(base)?;
        let output = Command::new("jj")
            .args(["diff", "--from", &resolved, "--git"])
            .current_dir(checkout)
            .output()
            .with_context(|| format!("running jj diff in {}", checkout.display()))?;
        if !output.status.success() {
            return Err(anyhow!(
                "jj diff failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    pub fn changed_paths(&self, checkout: &Path, base: &str) -> Result<Vec<String>> {
        let resolved = self.resolve(base)?;
        let output = self.run(
            checkout,
            "jj",
            &["diff", "--from", &resolved, "--name-only"],
        )?;
        Ok(output
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// The newest revision of a base bookmark. The remote's copy wins whenever
    /// there is one: a pull request is measured against it, and a local bookmark
    /// ahead of it would drag somebody's unpushed commits under the task's work.
    /// A repository without a remote has only the local bookmark.
    pub fn newest_base(&self, bookmark: &str) -> Option<String> {
        let name = jj_string(bookmark);
        self.single_commit(&format!("remote_bookmarks(exact:{name}, exact:\"origin\")"))
            .or_else(|| self.single_commit(&format!("bookmarks(exact:{name})")))
    }

    /// The one commit a revset names, or nothing when it names none or several
    /// (a conflicted bookmark points at more than one).
    fn single_commit(&self, revset: &str) -> Option<String> {
        let output = self
            .run(
                &self.root,
                "jj",
                &[
                    "log",
                    "-r",
                    &format!("present({revset})"),
                    "--no-graph",
                    "-T",
                    "commit_id ++ \"\\n\"",
                ],
            )
            .ok()?;
        let ids: Vec<&str> = output
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        match ids.as_slice() {
            [one] => Some(one.to_string()),
            _ => None,
        }
    }

    fn is_ancestor(&self, ancestor: &str, of: &str) -> bool {
        self.run(
            &self.root,
            "jj",
            &[
                "log",
                "-r",
                &format!("{ancestor} & ::{of}"),
                "--no-graph",
                "-T",
                "commit_id",
            ],
        )
        .map(|out| !out.trim().is_empty())
        .unwrap_or(false)
    }

    /// Puts a task's work on top of the newest base, as one change in the
    /// checkout's working copy.
    ///
    /// Every turn used to start from the previous head, so a task that lived for a
    /// day drifted from `main` until its pull request conflicted, and the agent had
    /// to merge by hand, leaving commits marked as conflicted that jj refuses to
    /// push. The work is carried over as a tree instead: a merge of the base and the
    /// head gives the combined tree (conflicts recorded, not fatal), the working copy
    /// takes that tree on top of the base, and the merge stays as the turn's diff
    /// base so the agent is blamed only for what it changes next. The previous
    /// heads are left exactly as they were.
    ///
    /// `None` when the head already contains the base. On an error the checkout is
    /// put back on the head, so the turn runs as it always did.
    pub fn carry_onto(&self, checkout: &Path, head: &str, base: &str) -> Result<Option<Carried>> {
        let head = self.resolve(head)?;
        let base = self.resolve(base)?;
        if head == base || self.is_ancestor(&base, &head) {
            return Ok(None);
        }
        if self.is_ancestor(&head, &base) {
            // Nothing of the task's own is on the head yet: start on the base.
            self.run(checkout, "jj", &["new", &base])?;
            return Ok(Some(Carried {
                diff_base: base,
                merge: None,
                conflicts: Vec::new(),
            }));
        }
        let mut merge = None;
        let carried = (|| -> Result<Carried> {
            self.run(
                checkout,
                "jj",
                &["new", &base, &head, "-m", "hulahoop: carry onto base"],
            )?;
            let id = self.working_copy_revision(checkout)?;
            merge = Some(id.clone());
            self.run(checkout, "jj", &["new", &base])?;
            self.run(checkout, "jj", &["restore", "--from", &id])?;
            Ok(Carried {
                diff_base: id.clone(),
                merge: Some(id),
                conflicts: self.conflicted_paths(checkout)?,
            })
        })();
        match carried {
            Ok(carried) => Ok(Some(carried)),
            Err(error) => {
                let _ = self.run(checkout, "jj", &["new", &head]);
                if let Some(merge) = merge {
                    let _ = self.abandon(&merge);
                }
                Err(error)
            }
        }
    }

    /// Files the checkout's working copy still holds in conflict.
    pub fn conflicted_paths(&self, checkout: &Path) -> Result<Vec<String>> {
        let output = Command::new("jj")
            .args(["resolve", "--list"])
            .current_dir(checkout)
            .output()
            .with_context(|| format!("running jj resolve --list in {}", checkout.display()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("No conflicts") {
                return Ok(Vec::new());
            }
            bail!("jj resolve --list failed: {}", stderr.trim());
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(conflict_path)
            .collect())
    }

    /// Abandons a commit nobody needs: the scaffolding merge of a carried turn, or
    /// the empty working copy a turn that changed nothing leaves behind. The revset
    /// refuses anything immutable or bookmarked, so a published head or `main`
    /// cannot go this way whatever the agent did to the checkout.
    pub fn abandon(&self, revision: &str) -> Result<()> {
        let guarded = format!("{revision} & mutable() & ~bookmarks() & ~remote_bookmarks()");
        self.run(
            &self.root,
            "jj",
            &["abandon", "--ignore-working-copy", "-r", &guarded],
        )?;
        Ok(())
    }

    pub fn fetch(&self) -> Result<()> {
        let fetch = |program: &str, args: &[&str]| -> Result<()> {
            let output = output_within(
                Command::new(program).args(args).current_dir(&self.root),
                NETWORK_TIMEOUT,
            )
            .with_context(|| format!("running {program} {args:?} in {}", self.root.display()))?;
            if !output.status.success() {
                bail!(
                    "{program} {args:?} failed in {}: {}",
                    self.root.display(),
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
            Ok(())
        };
        match self.kind() {
            Kind::Jj => {
                let _ = fetch("jj", &["git", "fetch"]);
                Ok(())
            }
            Kind::Git => fetch("git", &["fetch", "--all", "--prune"]),
        }
    }

    /// Moves a bookmark forward to `revision`. jj refuses to move one backwards
    /// or sideways without being told to, and that refusal is kept: landing a
    /// merge must never drop work the bookmark already carries.
    pub fn advance_bookmark(&self, name: &str, revision: &str) -> Result<String> {
        let resolved = self.resolve(revision)?;
        self.run(
            &self.root,
            "jj",
            &["bookmark", "set", name, "-r", &resolved],
        )?;
        Ok(resolved)
    }

    /// Whether `ancestor` comes strictly before `descendant` in history. A
    /// revision this repository does not have is an error.
    pub fn is_strict_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool> {
        let revset = format!("({ancestor}) & ::({descendant}) & ~({descendant})");
        let output = self.run(
            &self.root,
            "jj",
            &["log", "-r", &revset, "--no-graph", "-T", "commit_id"],
        )?;
        Ok(!output.trim().is_empty())
    }

    pub fn base_revision(&self, bookmark: &str) -> Result<Option<String>> {
        let output = self.run(
            &self.root,
            "jj",
            &["log", "-r", bookmark, "--no-graph", "-T", "commit_id"],
        );
        Ok(output
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()))
    }
}

/// A jj string literal: quoted, with backslashes and quotes escaped.
fn jj_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The path from one `jj resolve --list` line, `<path>    2-sided conflict`. A
/// path may contain spaces, so the description is cut off the end.
fn conflict_path(line: &str) -> Option<String> {
    let line = line.trim_end();
    let cut = line.find("-sided conflict").map(|at| {
        line[..at]
            .trim_end_matches(|c: char| c.is_ascii_digit())
            .trim_end()
    });
    let path = cut.unwrap_or(line).trim();
    (!path.is_empty()).then(|| path.to_string())
}

/// What [`Vcs::carry_onto`] left in the checkout.
#[derive(Debug)]
pub struct Carried {
    /// The revision the turn's diff is measured from.
    pub diff_base: String,
    /// The scaffolding merge, abandoned once the turn is recorded.
    pub merge: Option<String>,
    /// Files still in conflict after the work moved onto the new base.
    pub conflicts: Vec<String>,
}

/// The name a checkout is registered under in the mirror. A task's checkout
/// directory is named after the task, so this is `issue-<task id>`.
fn workspace_name(checkout: &Path) -> String {
    let dir = checkout
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string());
    format!("issue-{dir}")
}

#[cfg(test)]
mod carry_tests {
    use super::*;

    /// A command that outlives its limit is killed with everything it
    /// started, and says so; one that finishes in time gives its output.
    #[test]
    fn a_command_that_hangs_is_given_up_on() {
        let started = std::time::Instant::now();
        let error = super::output_within(
            Command::new("sh").args(["-c", "sleep 30 & sleep 30"]),
            std::time::Duration::from_millis(300),
        )
        .unwrap_err();
        assert!(error.to_string().contains("gave up"), "{error}");
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        let output = super::output_within(
            Command::new("sh").args(["-c", "echo out; echo err >&2"]),
            std::time::Duration::from_secs(10),
        )
        .unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), "out\n");
        assert_eq!(String::from_utf8_lossy(&output.stderr), "err\n");
    }

    fn jj(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("jj")
            .args(args)
            .args(["--config", "user.name=t", "--config", "user.email=t@t"])
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "jj {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    fn commit(dir: &Path, file: &str, text: &str, message: &str) {
        std::fs::write(dir.join(file), text).unwrap();
        jj(dir, &["commit", "-m", message]);
    }

    /// The task's work lands on the newest base as one change; the old head and
    /// its bookmark stay where they were, and a conflict is reported, not fatal.
    #[test]
    fn a_rework_starts_on_the_newest_base() {
        let root = std::env::temp_dir().join(format!("hh-carry-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        jj(&root, &["git", "init"]);
        commit(&root, "a.txt", "one\n", "base");
        jj(&root, &["bookmark", "create", "main", "-r", "@-"]);
        // The task's head: changes a.txt and adds b.txt.
        // Its bookmark is not called "head": on a case-insensitive disk (macOS)
        // git confuses `refs/heads/head` with HEAD when jj adds a worktree.
        commit(&root, "a.txt", "task\n", "task a");
        commit(&root, "b.txt", "b\n", "task b");
        jj(&root, &["bookmark", "create", "task", "-r", "@-"]);
        // main moves on and touches a.txt too.
        jj(&root, &["new", "main"]);
        commit(&root, "c.txt", "c\n", "main c");
        commit(&root, "a.txt", "main\n", "main a");
        jj(&root, &["bookmark", "set", "main", "-r", "@-"]);
        let head_before = Vcs::detect(&root).unwrap().resolve("task").unwrap();

        let checkout = root.join("ws");
        let vcs = Vcs::detect(&root).unwrap();
        vcs.add_workspace(&checkout, "task").unwrap();
        let ws = Vcs::detect(&checkout).unwrap();
        let newest = ws.newest_base("main").unwrap();
        let carried = ws
            .carry_onto(&checkout, "task", &newest)
            .unwrap()
            .expect("the head is behind main");

        assert_eq!(carried.conflicts, vec!["a.txt".to_string()]);
        assert!(checkout.join("b.txt").exists() && checkout.join("c.txt").exists());
        // Nothing the agent did yet: the turn's diff is empty.
        assert!(ws
            .changed_paths(&checkout, &carried.diff_base)
            .unwrap()
            .is_empty());
        // The working copy sits directly on the newest main.
        let parent = jj(
            &checkout,
            &["log", "-r", "@-", "--no-graph", "-T", "commit_id"],
        );
        assert_eq!(parent.trim(), newest);
        assert_eq!(vcs.resolve("task").unwrap(), head_before);

        vcs.abandon(carried.merge.as_deref().unwrap()).unwrap();
        // Already current: nothing to carry.
        assert!(ws
            .carry_onto(&checkout, &newest, &newest)
            .unwrap()
            .is_none());
        // A bookmarked commit never goes, whatever asks for it.
        let _ = vcs.abandon(&newest);
        assert_eq!(vcs.resolve("main").unwrap(), newest);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn parent_of_working_copy(checkout: &Path) -> String {
        jj(
            checkout,
            &["log", "-r", "@-", "--no-graph", "-T", "commit_id"],
        )
        .trim()
        .to_string()
    }

    /// A task's second turn reuses its checkout: what the project built there
    /// survives, the turn starts on its own start revision, and what a crashed
    /// turn left uncommitted stays in that turn's commit.
    #[test]
    fn a_second_turn_reuses_the_checkout_and_keeps_the_build() {
        let root = std::env::temp_dir().join(format!("hh-reuse-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        jj(&root, &["git", "init"]);
        commit(&root, ".gitignore", "target/\n", "ignore builds");
        commit(&root, "a.txt", "one\n", "base");
        jj(&root, &["bookmark", "create", "main", "-r", "@-"]);
        let vcs = Vcs::detect(&root).unwrap();
        let checkout = root.join("checkouts").join("task");

        vcs.add_workspace(&checkout, "main").unwrap();
        assert_eq!(
            parent_of_working_copy(&checkout),
            vcs.resolve("main").unwrap()
        );
        std::fs::create_dir_all(checkout.join("target")).unwrap();
        std::fs::write(checkout.join("target/build.out"), "warm").unwrap();
        // A turn that crashed halfway through an edit.
        std::fs::write(checkout.join("a.txt"), "half done\n").unwrap();

        // The next turn starts somewhere else: a head the task published.
        jj(&root, &["new", "main"]);
        commit(&root, "b.txt", "b\n", "task work");
        jj(&root, &["bookmark", "create", "next", "-r", "@-"]);
        let next = vcs.resolve("next").unwrap();

        vcs.add_workspace(&checkout, "next").unwrap();
        assert_eq!(
            std::fs::read_to_string(checkout.join("target/build.out")).unwrap(),
            "warm"
        );
        assert_eq!(parent_of_working_copy(&checkout), next);
        assert!(checkout.join("b.txt").exists());
        assert_eq!(
            std::fs::read_to_string(checkout.join("a.txt")).unwrap(),
            "one\n"
        );
        let listed = jj(&root, &["workspace", "list"]);
        assert!(listed.contains("issue-task"), "{listed}");

        // A turn that changed nothing abandons its empty working copy from the
        // repository root; the stale checkout comes back on the next turn.
        let empty = vcs.working_copy_revision(&checkout).unwrap();
        vcs.abandon(&empty).unwrap();
        vcs.add_workspace(&checkout, "main").unwrap();
        assert_eq!(
            parent_of_working_copy(&checkout),
            vcs.resolve("main").unwrap()
        );
        assert!(checkout.join("target/build.out").exists());
        assert!(!checkout.join("b.txt").exists());

        vcs.remove_workspace(&checkout).unwrap();
        assert!(!checkout.exists());
        let listed = jj(&root, &["workspace", "list"]);
        assert!(!listed.contains("issue-task"), "{listed}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A file the old start revision ignored, left on disk by a build, sits
    /// where the new start revision tracks one. jj skips writing it, and the
    /// stale copy must not become the new turn's work.
    #[test]
    fn a_stale_ignored_file_does_not_leak_into_the_next_turn() {
        let root = std::env::temp_dir().join(format!("hh-stale-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        jj(&root, &["git", "init"]);
        commit(&root, ".gitignore", "gen.txt\n", "ignore generated");
        jj(&root, &["bookmark", "create", "main", "-r", "@-"]);
        let vcs = Vcs::detect(&root).unwrap();
        let checkout = root.join("checkouts").join("task");

        vcs.add_workspace(&checkout, "main").unwrap();
        std::fs::write(checkout.join("gen.txt"), "stale\n").unwrap();
        // Snapshot: the ignored file stays out of the turn's commit.
        jj(&checkout, &["status"]);

        // The base moves on and starts tracking the generated file.
        jj(&root, &["new", "main"]);
        commit(&root, ".gitignore", "\n", "track generated");
        commit(&root, "gen.txt", "fresh\n", "generated");
        jj(&root, &["bookmark", "set", "main", "-r", "@-"]);
        let main = vcs.resolve("main").unwrap();

        vcs.add_workspace(&checkout, "main").unwrap();
        assert_eq!(parent_of_working_copy(&checkout), main);
        let diff = jj(&checkout, &["diff", "-r", "@", "--summary"]);
        assert!(diff.trim().is_empty(), "the new turn starts dirty: {diff}");
        assert_eq!(
            std::fs::read_to_string(checkout.join("gen.txt")).unwrap(),
            "fresh\n"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A start revision the mirror does not have falls back to the next name
    /// that it does.
    #[test]
    fn a_missing_start_falls_back_to_the_next_candidate() {
        let root = std::env::temp_dir().join(format!("hh-start-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        jj(&root, &["git", "init"]);
        commit(&root, "a.txt", "one\n", "base");
        jj(&root, &["bookmark", "create", "main", "-r", "@-"]);
        let vcs = Vcs::detect(&root).unwrap();
        let main = vcs.resolve("main").unwrap();
        let candidates = [
            "0123456789abcdef0123456789abcdef01234567".to_string(),
            "issue/x/head/1@origin".to_string(),
            String::new(),
            "main".to_string(),
        ];
        assert_eq!(vcs.resolve_first(&candidates).unwrap(), main);
        let error = vcs.resolve_first(&candidates[..2]).unwrap_err();
        assert!(
            error.to_string().contains("nothing to start from"),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_conflicted_path_keeps_its_spaces() {
        assert_eq!(
            conflict_path("a b.txt    2-sided conflict").as_deref(),
            Some("a b.txt")
        );
        assert_eq!(
            conflict_path("src/v2.rs    3-sided conflict including 1 deletion").as_deref(),
            Some("src/v2.rs")
        );
        assert_eq!(conflict_path("   "), None);
    }
}
