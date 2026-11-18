//! `hulahoop` runs the three pieces a developer needs: the server, the runner,
//! and a seed that turns a repo into a project with agents.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Command;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("serve") => passthrough("hulahoop-server", &args[1..]),
        Some("runner") => passthrough("hulahoop-runner", &args[1..]),
        Some("up") => up(&args[1..]),
        Some("doctor") => doctor(),
        Some("seed") => seed(&args[1..]).await,
        Some("help") | Some("--help") | Some("-h") | None => {
            println!(
                "hulahoop — the acceptance loop that keeps coding agents honest\n\n\
                 usage:\n  \
                 hulahoop up [--listen ADDR] [--port N] [--data-dir DIR]\n                                               server and runner, one host\n  \
                 hulahoop serve [--data-dir DIR] [--port N]   just the server\n  \
                 hulahoop runner                              just the runner\n  \
                 hulahoop seed --repo PATH [--name NAME]      register a repo as a project\n  \
                 hulahoop doctor                              check this machine"
            );
            Ok(())
        }
        Some(other) => bail!("unknown command {other}; try `hulahoop help`"),
    }
}

/// The host that does the work runs two processes: the server owns the state, and
/// the runner owns the checkouts, the agents, and the replays. `up` starts both,
/// waits for the server's connection file, points the runner at it, and shuts the
/// other one down when either exits.
fn up(args: &[String]) -> Result<()> {
    let mut data_dir: Option<PathBuf> = None;
    let mut listen: Option<String> = None;
    let mut port: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--data-dir" => {
                index += 1;
                data_dir = Some(PathBuf::from(
                    args.get(index).context("--data-dir needs a path")?,
                ));
            }
            "--listen" => {
                index += 1;
                listen = Some(
                    args.get(index)
                        .context("--listen needs an address")?
                        .clone(),
                );
            }
            "--port" => {
                index += 1;
                port = Some(args.get(index).context("--port needs a number")?.clone());
            }
            other => bail!("unknown argument {other}"),
        }
        index += 1;
    }

    let data_dir = match data_dir {
        Some(dir) => dir,
        None => platform_data_dir()?,
    };
    std::fs::create_dir_all(&data_dir)?;

    let mut server_args: Vec<String> = vec!["--data-dir".into(), data_dir.display().to_string()];
    if let Some(listen) = &listen {
        server_args.push("--listen".into());
        server_args.push(listen.clone());
    }
    if let Some(port) = &port {
        server_args.push("--port".into());
        server_args.push(port.clone());
    }

    let mut server = spawn("hulahoop-server", &server_args)?;
    let connection_path = data_dir.join("connection.json");
    let _ = std::fs::remove_file(&connection_path);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let connection = loop {
        if let Ok(bytes) = std::fs::read(&connection_path) {
            if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
                break value;
            }
        }
        if let Some(status) = server.try_wait()? {
            bail!("the server exited with {status} before it was ready");
        }
        if std::time::Instant::now() > deadline {
            let _ = server.kill();
            bail!(
                "the server did not write {} within 30s",
                connection_path.display()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };

    let url = connection["url"]
        .as_str()
        .map(|url| url.to_string())
        .unwrap_or_else(|| format!("http://127.0.0.1:{}", connection["port"]));
    let token = connection["token"].as_str().unwrap_or_default().to_string();
    let runner_args = vec![
        "--data-dir".to_string(),
        data_dir.display().to_string(),
        "--server".to_string(),
        url.clone(),
        "--token".to_string(),
        token,
    ];

    let mut runner = spawn("hulahoop-runner", &runner_args)?;
    tracing::info!(server = %url, data_dir = %data_dir.display(), "server and runner are up");

    loop {
        if let Some(status) = runner.try_wait()? {
            let _ = server.kill();
            let _ = server.wait();
            if !status.success() {
                bail!("the runner exited with {status}");
            }
            return Ok(());
        }
        if let Some(status) = server.try_wait()? {
            let _ = runner.kill();
            let _ = runner.wait();
            bail!("the server exited with {status}");
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

fn spawn(program: &str, args: &[String]) -> Result<std::process::Child> {
    Command::new(program_path(program))
        .args(args)
        .spawn()
        .with_context(|| format!("starting {program}; is it installed next to this binary?"))
}

fn platform_data_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("HULAHOOP_DATA") {
        return Ok(PathBuf::from(dir));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        if cfg!(target_os = "macos") {
            return Ok(home.join("Library/Application Support/hulahoop"));
        }
        if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
            return Ok(PathBuf::from(xdg).join("hulahoop"));
        }
        return Ok(home.join(".local/share/hulahoop"));
    }
    bail!("cannot locate a data directory; pass --data-dir or set HULAHOOP_DATA")
}

fn program_path(program: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.join(program)))
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from(program))
}

fn passthrough(program: &str, args: &[String]) -> Result<()> {
    // `cargo run` puts the binary next to the CLI, and an installed app ships all
    // three side by side, so the sibling binary wins over PATH.
    let program_path = program_path(program);
    let status = Command::new(&program_path)
        .args(args)
        .status()
        .with_context(|| {
            format!(
                "starting {}; is it installed next to this binary?",
                program_path.display()
            )
        })?;
    if !status.success() {
        bail!("{program} exited with {status}");
    }
    Ok(())
}

fn doctor() -> Result<()> {
    let mut ok = true;
    let probes: [(&str, &[&str]); 6] = [
        ("nix", &["--version"]),
        ("direnv", &["version"]),
        ("just", &["--version"]),
        ("jj", &["--version"]),
        ("pi", &["--version"]),
        ("claude", &["--version"]),
    ];
    for (name, args) in probes {
        match Command::new(name).args(args).output() {
            Ok(output) if output.status.success() => {
                let text = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_string();
                println!("ok    {name:<8} {text}");
            }
            Ok(output) => {
                ok = false;
                println!("fail  {name:<8} exited with {}", output.status);
            }
            Err(_) => {
                ok = false;
                println!(
                    "fail  {name:<8} missing — {}",
                    match name {
                        "nix" | "direnv" | "just" =>
                            "every repo contract needs nix, direnv and just",
                        "jj" => "Hulahoop runs work in jj workspaces",
                        _ => "an agent adapter needs its CLI",
                    }
                );
            }
        }
    }
    if !ok {
        bail!("some prerequisites are missing");
    }
    println!("\nall prerequisites present");
    Ok(())
}

async fn seed(args: &[String]) -> Result<()> {
    let mut repo = None;
    let mut name = None;
    let mut data_dir = std::env::var_os("HULAHOOP_DATA")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share/hulahoop"))
        })
        .context("cannot locate the data directory; set HULAHOOP_DATA")?;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--repo" => {
                index += 1;
                repo = args.get(index).map(PathBuf::from);
            }
            "--name" => {
                index += 1;
                name = args.get(index).cloned();
            }
            "--data-dir" => {
                index += 1;
                data_dir = args
                    .get(index)
                    .map(PathBuf::from)
                    .context("--data-dir needs a path")?;
            }
            // A bare path is the repo, so `hulahoop seed ~/src/thing` works.
            other if !other.starts_with('-') && repo.is_none() => {
                repo = Some(PathBuf::from(other));
            }
            other => bail!("unknown argument {other}"),
        }
        index += 1;
    }

    let repo = repo.context("--repo is required")?;
    let repo = repo.canonicalize().context("repo path does not exist")?;
    let name = name.unwrap_or_else(|| {
        repo.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "project".into())
    });

    let connection_path = hulahoop_core::contract::connection_file(&data_dir);
    let connection: Value =
        serde_json::from_slice(&std::fs::read(&connection_path).with_context(|| {
            format!(
                "read {} — is the server running?",
                connection_path.display()
            )
        })?)?;
    let port = connection["port"]
        .as_u64()
        .context("connection.json has no port")?;
    let token = connection["token"]
        .as_str()
        .context("connection.json has no token")?
        .to_string();
    let http = reqwest::Client::new();

    let gql = |query: String| {
        let http = http.clone();
        let token = token.clone();
        async move {
            let response = http
                .post(format!("http://127.0.0.1:{port}/graphql"))
                .bearer_auth(token)
                .json(&json!({ "query": query }))
                .send()
                .await?;
            let value: Value = response.json().await?;
            if let Some(errors) = value.get("errors") {
                anyhow::bail!("{errors}");
            }
            Ok::<Value, anyhow::Error>(value)
        }
    };

    // Local mode has one workspace; the seed reuses it.
    let workspaces = gql("{ workspaces { id name } }".into()).await?;
    let workspace_id = workspaces["data"]["workspaces"][0]["id"]
        .as_str()
        .context("no workspace yet — start the app once so it seeds local mode")?
        .to_string();

    let project = gql(format!(
        "mutation {{ createProject(workspaceId: \"{workspace_id}\", name: \"{name}\") {{ id }} }}"
    ))
    .await?;
    let project_id = project["data"]["createProject"]["id"]
        .as_str()
        .context("createProject returned no id")?
        .to_string();

    let repo_json = gql(format!(
        "mutation {{ addRepo(projectId: \"{project_id}\", name: \"{name}\", path: \"{}\") {{ id vcs contractVersion }} }}",
        repo.display()
    ))
    .await?;
    let repo_id = repo_json["data"]["addRepo"]["id"]
        .as_str()
        .context("addRepo returned no id")?
        .to_string();
    println!(
        "project {project_id} with repo {repo_id} ({})",
        repo_json["data"]["addRepo"]["vcs"].as_str().unwrap_or("?")
    );

    // Two pi agents and one claude agent: the roles differ by configuration, so
    // the test author and the implementer can use different models.
    let mut agent_ids = Vec::new();
    for (agent_name, adapter, model) in [
        ("Test Author", "pi", "anthropic/claude-sonnet-4-5"),
        ("Implementer", "pi", "anthropic/claude-opus-4-5"),
        ("Reviewer", "claude_code", "sonnet"),
    ] {
        let created = gql(format!(
            "mutation {{ createAgent(workspaceId: \"{workspace_id}\", name: \"{agent_name}\", adapter: \"{adapter}\", model: \"{model}\") {{ id }} }}"
        ))
        .await?;
        let agent_id = created["data"]["createAgent"]["id"]
            .as_str()
            .context("createAgent returned no id")?
            .to_string();
        agent_ids.push(agent_id);
    }

    let bindings = format!(
        "[{{role: \"test_author\", agentId: \"{}\"}}, {{role: \"implementer\", agentId: \"{}\"}}, {{role: \"reviewer\", agentId: \"{}\"}}]",
        agent_ids[0], agent_ids[1], agent_ids[2]
    );
    gql(format!(
        "mutation {{ setProjectAgents(projectId: \"{project_id}\", bindings: {bindings}) {{ role }} }}"
    ))
    .await?;

    println!("agents bound: test_author, implementer, reviewer");
    println!(
        "repo root: {}\nnext: create an issue in the app, or open it and press Start",
        repo.display()
    );
    Ok(())
}
