//! Everything that touches the repo: workspaces, bookmarks, diffs, rebases.
//! jj is the supported path; git is a fallback for repos without `.jj`.

use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

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

    pub fn add_workspace(&self, checkout: &Path, revision: &str) -> Result<()> {
        if let Some(parent) = checkout.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if checkout.exists() {
            let _ = std::fs::remove_dir_all(checkout);
        }
        let name = workspace_name(checkout);
        let _ = self.run(&self.root, "jj", &["workspace", "forget", &name]);
        let revision = self.resolve(revision)?;
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

    pub fn remove_workspace(&self, checkout: &Path) -> Result<()> {
        let name = workspace_name(checkout);
        let _ = self.run(&self.root, "jj", &["workspace", "forget", &name]);
        if checkout.exists() {
            let _ = std::fs::remove_dir_all(checkout);
        }
        Ok(())
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

    /// The paths one revision changed relative to another. Two revisions rather
    /// than a checkout, because a base replay asks about the candidate suite, not
    /// about whatever the user happens to have open.
    pub fn changed_paths_between(&self, from: &str, to: &str) -> Result<Vec<String>> {
        let from = self.resolve(from)?;
        let to = self.resolve(to)?;
        let output = self.run(
            &self.root,
            "jj",
            &["diff", "--from", &from, "--to", &to, "--name-only"],
        )?;
        Ok(output
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// Base replay: the authored tests, restored onto the base revision. The
    /// tests must fail there; if they pass, they were not proving anything.
    pub fn restore_from(&self, checkout: &Path, from: &str, paths: &[String]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let resolved = self.resolve(from)?;
        let mut args: Vec<String> = vec!["restore".into(), "--from".into(), resolved];
        if !paths.is_empty() {
            args.push("--".into());
            args.extend(paths.iter().cloned());
        }
        let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
        self.run(checkout, "jj", &args_ref)?;
        Ok(())
    }

    /// Landing: move the approved head onto the refreshed base. jj moves the
    /// bookmark with the commit it rewrites, so the same name now points at the
    /// rebased commit.
    pub fn rebase_bookmark(&self, checkout: &Path, bookmark: &str, dest: &str) -> Result<String> {
        let dest = self.resolve(dest)?;
        self.run(checkout, "jj", &["rebase", "-s", bookmark, "-d", &dest])?;
        let moved = self.run(
            checkout,
            "jj",
            &["log", "-r", bookmark, "--no-graph", "-T", "commit_id"],
        )?;
        Ok(moved.trim().to_string())
    }

    /// True when the approved head is contained in the merged revision.
    /// `checkout` is kept in the signature because every jj call runs in one; the
    /// repo is resolved from it.
    pub fn contains(&self, merge: &str, head: &str) -> Result<bool> {
        let merge = self.resolve(merge)?;
        let head = self.resolve(head)?;
        let revset = format!("{head} & ::{merge}");
        let output = self.run(
            &self.root,
            "jj",
            &["log", "-r", &revset, "--no-graph", "-T", "commit_id"],
        )?;
        Ok(!output.trim().is_empty())
    }

    pub fn fetch(&self) -> Result<()> {
        match self.kind() {
            Kind::Jj => {
                let _ = self.run(&self.root, "jj", &["git", "fetch"]);
                Ok(())
            }
            Kind::Git => {
                self.run(&self.root, "git", &["fetch", "--all", "--prune"])?;
                Ok(())
            }
        }
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

fn workspace_name(checkout: &Path) -> String {
    checkout
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string())
}
