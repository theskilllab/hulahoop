//! Hulahoop runner: claims work from the server, runs agents in their own
//! process groups, and reports back. It never touches the database.

mod adapter;
mod client;
mod exec;
mod vcs;

use anyhow::Context;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,hulahoop_runner=debug")
            }),
        )
        .init();

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
    // a different HOME (the scratch home). Pointing the allow-list at one place
    // owned by the runner makes `direnv allow` and `direnv exec` agree.
    let direnv_data = data_dir.join("direnv");
    std::fs::create_dir_all(&direnv_data)?;
    std::env::set_var("XDG_DATA_HOME", &direnv_data);

    let connection = client::Connection::load(&data_dir)?;
    let data_dir = connection.data_dir.clone();
    let client = client::Client::new(&connection)?;
    let executor = exec::Executor::new(client, data_dir);
    let runner_id = std::env::var("HULAHOOP_RUNNER_ID")
        .unwrap_or_else(|_| format!("runner-{}", std::process::id()));

    tracing::info!(
        runner_id,
        port = connection.port,
        "runner watching for work"
    );
    let mut idle = Duration::from_millis(250);
    loop {
        match executor.tick(&runner_id).await {
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
