//! Turning one adapter into one process. The runner owns process groups and
//! timeouts; the adapter layer owns nothing but argv.

use anyhow::{Context, Result};
use hulahoop_core::Adapter;
use std::path::Path;
use std::process::{Command, Stdio};

pub struct Invocation {
    pub program: String,
    pub args: Vec<String>,
    /// Variables the process starts with, on top of the runner's own.
    pub env: Vec<(String, String)>,
}

/// Build the argv for one role run.
///
/// `pi` and `claude` both take the prompt as the trailing message and the role
/// instructions as a system prompt, so the same two files serve both: the
/// prompt text is the role, the message is the task.
///
/// The instructions go in by file, never as an argument. Anything in argv is in
/// every process listing, and an agent cleaning up with `pkill -f "just test"`
/// matched its own instructions (which name `just test`) and killed itself.
pub fn invocation(
    adapter: Adapter,
    model: &str,
    system_prompt_file: &Path,
    message: &str,
) -> Result<Invocation> {
    let system_prompt_file = system_prompt_file.to_string_lossy().to_string();
    let (program, args) = match adapter {
        Adapter::Pi => {
            // An empty model means "whatever the adapter is configured for", so the
            // flag is left out rather than passed empty.
            let mut args: Vec<String> = vec![
                "-p".to_string(),
                "--mode".to_string(),
                "json".to_string(),
                "--no-session".to_string(),
                // A run sees what Hulahoop hands it and nothing of the user's
                // own setup: no extensions or packages, no prompt templates or
                // themes, and no context files Pi would collect from the
                // directories above the checkout. The checkout's own
                // instructions come in through `repo_instructions`.
                "--no-extensions".to_string(),
                "--no-context-files".to_string(),
                "--no-prompt-templates".to_string(),
                "--no-themes".to_string(),
            ];
            if !model.trim().is_empty() {
                args.push("--model".to_string());
                args.push(model.to_string());
            }
            args.push("--approve".to_string());
            // `--system-prompt` reads an existing file's contents.
            args.push("--system-prompt".to_string());
            args.push(system_prompt_file);
            args.push(message.to_string());
            ("pi".to_string(), args)
        }
        Adapter::ClaudeCode => {
            let mut args: Vec<String> = vec![
                "-p".to_string(),
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--verbose".to_string(),
            ];
            if !model.trim().is_empty() {
                args.push("--model".to_string());
                args.push(model.to_string());
            }
            // Only the checkout's settings apply, and no MCP server is
            // started: the user's settings, hooks and servers stay on the host.
            args.push("--setting-sources".to_string());
            args.push("project,local".to_string());
            args.push("--strict-mcp-config".to_string());
            args.push("--append-system-prompt-file".to_string());
            args.push(system_prompt_file);
            // The environment is the boundary in v0: the agent cannot see the user's
            // shell or credentials, so its permission prompts would deadlock a
            // non-interactive run.
            args.push("--permission-mode".to_string());
            args.push("bypassPermissions".to_string());
            args.push("--dangerously-skip-permissions".to_string());
            args.push(message.to_string());
            ("claude".to_string(), args)
        }
        // The fake adapter is a script: the fixture and the end-to-end test own
        // the behavior, so the product code stays free of test scenarios.
        Adapter::Fake => {
            let script = std::env::var("HULAHOOP_FAKE_SCRIPT")
                .unwrap_or_else(|_| "fake-agent.sh".to_string());
            (
                script,
                vec![
                    "--model".to_string(),
                    model.to_string(),
                    "--message".to_string(),
                    message.to_string(),
                ],
            )
        }
    };
    Ok(Invocation {
        program,
        args,
        env: Vec::new(),
    })
}

/// The message the agent receives. Everything it must know is in `input.json`
/// and in the instructions it was started with, so the message only says where to
/// look and how to end the turn.
/// The opening message for a run on a task with no repository: the directory is
/// scratch space, and the result is the closing message.
pub fn scratch_message_for(agent_name: &str, input_path: &Path, scratch: &Path) -> String {
    let who = if agent_name.trim().is_empty() {
        "one task"
    } else {
        agent_name.trim()
    };
    format!(
        "You are {who} for one task.\n\nRead {} first; it carries the task, its files, the latest comments, the parent turn, and the limits.\nThis task has no repository. Work in the empty scratch directory at {}; it is deleted when the turn ends, so put your result in your final message.\n\nWhen you are done, end your final message with the line your instructions ask for.",
        input_path.display(),
        scratch.display(),
    )
}

pub fn message_for(agent_name: &str, input_path: &Path, checkout: &Path) -> String {
    let who = if agent_name.trim().is_empty() {
        "one task"
    } else {
        agent_name.trim()
    };
    format!(
        "You are {who} for one task.\n\nRead {} first; it carries the task, its files, the latest comments, the parent turn, and the limits.\nWork in the checkout at {}.\n\nWhen you are done, end your final message with the line your instructions ask for.",
        input_path.display(),
        checkout.display(),
    )
}

/// The checkout's own agent instructions, for a harness that no longer finds
/// them itself. Pi runs with `--no-context-files`, which keeps out the
/// `AGENTS.md` files of every directory above the checkout, so the checkout's
/// `AGENTS.md` (or `CLAUDE.md` when it has none) is added to its instructions.
/// Claude Code still reads the checkout's `CLAUDE.md` itself.
pub fn repo_instructions(adapter: Adapter, checkout: &Path) -> String {
    if adapter != Adapter::Pi {
        return String::new();
    }
    for name in ["AGENTS.md", "CLAUDE.md"] {
        let path = checkout.join(name);
        // A link could point outside the checkout; only a plain file counts.
        let plain = std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_file());
        if !plain {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            if !text.trim().is_empty() {
                return format!(
                    "\n\n# The repository's instructions ({name})\n\n{}\n",
                    text.trim()
                );
            }
        }
    }
    String::new()
}

/// Write the instructions where the agent's command line can point at them:
/// the run's artifact directory, outside the checkout, so nothing commits it.
pub fn write_system_prompt(dir: &Path, text: &str) -> Result<std::path::PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join("system-prompt.md");
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(std::fs::canonicalize(&path).unwrap_or(path))
}

/// Spawn the agent in its own process group so a cancel or a timeout can kill
/// the whole tree, including a build the agent started.
pub fn spawn(
    invocation: &Invocation,
    cwd: &Path,
    env: &[(String, String)],
) -> Result<std::process::Child> {
    let mut command = Command::new(&invocation.program);
    command
        .args(&invocation.args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear();
    for (key, value) in env {
        command.env(key, value);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
        .spawn()
        .with_context(|| format!("starting {}", invocation.program))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The instructions reach the agent as a file: no argument carries their
    /// text, so a `pkill -f` pattern taken from them cannot match the agent.
    #[test]
    fn instructions_stay_out_of_argv() {
        let dir = std::env::temp_dir().join(format!("hulahoop-argv-{}", uuid::Uuid::now_v7()));
        let instructions = "Run `just test` before you hand over.";
        let file = write_system_prompt(&dir, instructions).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), instructions);
        for (adapter, flag) in [
            (Adapter::Pi, "--system-prompt"),
            (Adapter::ClaudeCode, "--append-system-prompt-file"),
        ] {
            let invocation = invocation(adapter, "model-x", &file, "Read input.json").unwrap();
            assert!(invocation.args.iter().all(|arg| !arg.contains("just test")));
            let at = invocation.args.iter().position(|arg| arg == flag).unwrap();
            assert_eq!(invocation.args[at + 1], file.to_string_lossy());
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A harness loads nothing of the user's own setup: Pi starts without
    /// extensions, context files, prompt templates or themes, and Claude Code
    /// without the user's settings or MCP servers.
    #[test]
    fn a_harness_starts_without_the_users_setup() {
        let file = Path::new("/tmp/system-prompt.md");
        let pi = invocation(Adapter::Pi, "anthropic/x", file, "go").unwrap();
        for flag in [
            "--no-extensions",
            "--no-context-files",
            "--no-prompt-templates",
            "--no-themes",
        ] {
            assert!(pi.args.iter().any(|arg| arg == flag), "pi lacks {flag}");
        }
        let claude = invocation(Adapter::ClaudeCode, "sonnet", file, "go").unwrap();
        let at = claude
            .args
            .iter()
            .position(|arg| arg == "--setting-sources")
            .unwrap();
        assert_eq!(claude.args[at + 1], "project,local");
        assert!(claude.args.iter().any(|arg| arg == "--strict-mcp-config"));
    }

    /// Pi no longer collects context files itself, so the checkout's own
    /// `AGENTS.md` is handed to it; a linked one is not followed.
    #[test]
    fn the_checkouts_instructions_reach_pi() {
        let dir = std::env::temp_dir().join(format!("hulahoop-agents-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(repo_instructions(Adapter::Pi, &dir), "");
        std::fs::write(dir.join("CLAUDE.md"), "Claude notes").unwrap();
        assert!(repo_instructions(Adapter::Pi, &dir).contains("Claude notes"));
        std::fs::write(dir.join("AGENTS.md"), "Use just test.").unwrap();
        let added = repo_instructions(Adapter::Pi, &dir);
        assert!(added.contains("(AGENTS.md)") && added.contains("Use just test."));
        assert_eq!(repo_instructions(Adapter::ClaudeCode, &dir), "");
        #[cfg(unix)]
        {
            let outside = dir.join("outside.md");
            std::fs::write(&outside, "secret").unwrap();
            std::fs::remove_file(dir.join("AGENTS.md")).unwrap();
            std::fs::remove_file(dir.join("CLAUDE.md")).unwrap();
            std::os::unix::fs::symlink(&outside, dir.join("AGENTS.md")).unwrap();
            assert_eq!(repo_instructions(Adapter::Pi, &dir), "");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A Claude Code turn ends when its work ends: no background task or
    /// monitor can wake the agent after it has handed over.
    #[test]
    fn claude_cannot_start_background_tasks() {
        let file = Path::new("/tmp/system-prompt.md");
        let claude = invocation(Adapter::ClaudeCode, "sonnet", file, "go").unwrap();
        let at = claude
            .args
            .iter()
            .position(|arg| arg == "--disallowedTools")
            .expect("--disallowedTools is set");
        assert!(claude.args[at + 1].contains("Monitor"));
        assert!(claude
            .env
            .iter()
            .any(|(k, v)| k == "CLAUDE_CODE_DISABLE_BACKGROUND_TASKS" && v == "1"));
        let pi = invocation(Adapter::Pi, "m", file, "go").unwrap();
        assert!(pi.env.is_empty());
    }
}
