//! Turning one adapter into one process. The runner owns process groups and
//! timeouts; the adapter layer owns nothing but argv.

use anyhow::{Context, Result};
use hulahoop_core::Adapter;
use std::path::Path;
use std::process::{Command, Stdio};

pub struct Invocation {
    pub program: String,
    pub args: Vec<String>,
}

/// Build the argv for one role run.
///
/// `pi` and `claude` both take the prompt as the trailing message and the role
/// instructions as a system prompt, so the same two files serve both: the
/// prompt text is the role, the message is the task.
pub fn invocation(
    adapter: Adapter,
    model: &str,
    system_prompt: &str,
    message: &str,
) -> Result<Invocation> {
    let (program, args) = match adapter {
        Adapter::Pi => {
            // An empty model means "whatever the adapter is configured for", so the
            // flag is left out rather than passed empty.
            let mut args: Vec<String> = vec![
                "-p".to_string(),
                "--mode".to_string(),
                "json".to_string(),
                "--no-session".to_string(),
            ];
            if !model.trim().is_empty() {
                args.push("--model".to_string());
                args.push(model.to_string());
            }
            args.push("--approve".to_string());
            args.push("--system-prompt".to_string());
            args.push(system_prompt.to_string());
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
            args.push("--append-system-prompt".to_string());
            args.push(system_prompt.to_string());
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
    Ok(Invocation { program, args })
}

/// The message the agent receives. Everything it must know is in `input.json`
/// and in the instructions it was started with, so the message only says where to
/// look and how to end the turn.
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
