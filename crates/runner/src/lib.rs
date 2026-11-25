//! Hulahoop runner: claims work from the server, runs agents in their own
//! process groups, and reports back. It never touches the database.

mod adapter;
mod client;
mod exec;
mod live;
mod redact;
mod resources;
mod vcs;

use anyhow::Context;
use std::time::Duration;

/// How many runs share the host at most, unless `HULAHOOP_MAX_RUNS` says.
const DEFAULT_MAX_RUNS: usize = 4;

/// Where the work comes from: `--server`/`--token` (or `HULAHOOP_SERVER`,
/// `HULAHOOP_TOKEN`) point the runner at a host that already runs the server;
/// without them it reads the `connection.json` a local server wrote.
fn resolve_connection(
    args: &[String],
    mut data_dir: std::path::PathBuf,
) -> anyhow::Result<(client::Connection, std::path::PathBuf)> {
    let mut server = std::env::var("HULAHOOP_SERVER").ok();
    let mut token = std::env::var("HULAHOOP_TOKEN").ok();

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--server" => {
                index += 1;
                server = Some(
                    args.get(index)
                        .context("--server needs a URL, like http://host:51234")?
                        .clone(),
                );
            }
            "--token" => {
                index += 1;
                token = Some(
                    args.get(index)
                        .context("--token needs the server's token")?
                        .clone(),
                );
            }
            "--data-dir" => {
                index += 1;
                data_dir =
                    std::path::PathBuf::from(args.get(index).context("--data-dir needs a path")?);
            }
            other => anyhow::bail!("unknown argument {other}"),
        }
        index += 1;
    }

    let connection = match (server, token) {
        (Some(server), Some(token)) => {
            client::Connection::remote(&server, &token, data_dir.clone())
        }
        (Some(_), None) => anyhow::bail!("--server needs --token (or HULAHOOP_TOKEN)"),
        _ => client::Connection::load(&data_dir)?,
    };
    let data_dir = connection.data_dir.clone();
    Ok((connection, data_dir))
}

/// Runs the runner loop until it is killed. The caller owns the runtime and the
/// log filter.
/// A path is only useful to a child process when it does not depend on where that
/// child stands: the runner is started from a checkout, and the agent works in
/// another one.
fn absolute(path: std::path::PathBuf) -> std::path::PathBuf {
    if path.is_absolute() {
        return path;
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(&path))
        .unwrap_or(path)
}

pub async fn run(args: Vec<String>) -> anyhow::Result<()> {
    let data_dir = std::env::var_os("HULAHOOP_DATA")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_DATA_HOME")
                .map(std::path::PathBuf::from)
                .map(|dir| dir.join("hulahoop"))
        })
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|home| std::path::PathBuf::from(home).join(".local/share/hulahoop"))
        })
        .context("cannot locate the data directory; set HULAHOOP_DATA")?;

    // direnv keeps its allow-list under XDG_DATA_HOME, and every child runs with
    // a different HOME (the scratch home) and a different working directory (the
    // checkout). An absolute path here is what makes `direnv allow` and the agent's
    // `direnv exec` talk about the same allow-list; a relative one resolves inside
    // the checkout and every run reports `.envrc is blocked`.
    let data_dir = absolute(data_dir);
    let direnv_data = data_dir.join("direnv");
    std::fs::create_dir_all(&direnv_data)?;
    std::env::set_var("XDG_DATA_HOME", &direnv_data);

    let (connection, data_dir) = resolve_connection(&args, data_dir)?;
    let data_dir = absolute(data_dir);
    let client = client::Client::new(&connection)?;
    let executor = exec::Executor::new(client, data_dir.clone());
    let lumi_executor = exec::Executor::new(client::Client::new(&connection)?, data_dir.clone());
    tokio::spawn(async move {
        let mut idle = Duration::from_millis(250);
        loop {
            match lumi_executor.tick_lumi().await {
                Ok(did_work) => {
                    idle = if did_work {
                        Duration::from_millis(50)
                    } else {
                        (idle * 2).min(Duration::from_secs(2))
                    };
                }
                Err(error) => {
                    tracing::warn!(%error, "Lumi lane failed");
                    idle = Duration::from_secs(2);
                }
            }
            tokio::time::sleep(idle).await;
        }
    });
    let runner_id = std::env::var("HULAHOOP_RUNNER_ID")
        .unwrap_or_else(|_| format!("runner-{}", std::process::id()));

    let pruned = exec::prune_workspace(&data_dir);
    if pruned > 0 {
        tracing::info!(pruned, "removed checkouts a killed runner left behind");
    }
    let pruned = exec::prune_run_homes(&data_dir);
    if pruned > 0 {
        tracing::info!(pruned, "removed run homes a killed runner left behind");
    }

    // Runs share the host while it has room for them, and never more than
    // this many at once whatever the room.
    let max_runs = std::env::var("HULAHOOP_MAX_RUNS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|&runs| runs > 0)
        .unwrap_or(DEFAULT_MAX_RUNS);

    tracing::info!(
        runner_id,
        server = %connection.base,
        data_dir = %data_dir.display(),
        max_runs,
        "runner watching for work"
    );
    let mut live = Vec::new();
    let mut idle = Duration::from_millis(250);
    loop {
        match executor.tick(&runner_id, &mut live, max_runs).await {
            Ok(did_work) => {
                idle = if did_work {
                    Duration::from_millis(50)
                } else {
                    (idle * 2).min(Duration::from_secs(2))
                };
            }
            Err(error) => {
                tracing::warn!(%error, "tick failed");
                idle = Duration::from_secs(2);
            }
        }
        tokio::time::sleep(idle).await;
    }
}
