//! One binary, several jobs: `hulahoop up` runs the server and the runner,
//! `serve` and `runner` run one of them, and `seed`/`doctor` set a repo up and
//! check the machine. The server and runner are still separate *processes* —
//! `up` re-execs this same executable — because a crashed agent must not take the
//! server with it, and each run is killed by process group.

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
        Some("serve") => hulahoop_server::serve(args[1..].to_vec()).await,
        Some("runner") => hulahoop_runner::run(args[1..].to_vec()).await,
        Some("up") => up(&args[1..]).await,
        Some("doctor") => doctor(),
        Some("seed") => seed(&args[1..]).await,
        Some("help") | Some("--help") | Some("-h") | None => {
            println!(
                "hulahoop — the acceptance loop that keeps coding agents honest\n\
                 \n\
                 usage:\n  \
                 hulahoop up [--listen ADDR] [--port N] [--data-dir DIR]   server and runner\n  \
                 hulahoop serve [--data-dir DIR] [--port N]                just the server\n  \
                 hulahoop runner [--server URL --token T]                  just the runner\n  \
                 hulahoop seed --repo PATH [--name NAME]                   register a repo\n  \
                 hulahoop doctor                                           check this machine"
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
async fn up(args: &[String]) -> Result<()> {
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

    let connection_path = data_dir.join("connection.json");

    // One data directory holds exactly one connection file, so it can hold exactly one
    // live server: a second one would overwrite the file the first one's clients are
    // using, and the token with it.
    if let Some((url, pid, running_port)) = already_running(&connection_path) {
        let wanted = port
            .as_deref()
            .and_then(|text| text.parse::<i64>().ok())
            .unwrap_or(running_port);
        if wanted == running_port {
            tracing::info!(
                "a server is already up on this data directory; nothing to do url={url} pid={pid} \
                 (stop it with `kill {pid}`)"
            );
            return Ok(());
        }
        bail!(
            "a server is already up on this data directory at {url} (pid {pid}, port {running_port}), \
             and the data directory holds one connection file. Stop it with `kill {pid}` first, \
             use that server, or pick another --data-dir."
        );
    }

    // Anything left here is stale, and a stale file reads like a live server.
    let _ = std::fs::remove_file(&connection_path);

    let mut server = spawn(&["serve"], &server_args)?;

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

    let mut runner = spawn(&["runner"], &runner_args)?;
    tracing::info!(
        server = %url,
        data_dir = %data_dir.display(),
        token_file = %connection_path.display(),
        "server and runner are up; the desktop needs the token in that file (jq -r .token …)"
    );

    // Ctrl-C in `just dev` reaches this process only, so the children have to be
    // stopped here. Otherwise the server keeps the port and the next `just dev`
    // fails with "Address already in use".
    loop {
        if let Some(status) = runner.try_wait()? {
            let _ = server.kill();
            let _ = server.wait();
            let _ = std::fs::remove_file(&connection_path);
            if !status.success() {
                bail!("the runner exited with {status}");
            }
            return Ok(());
        }
        if let Some(status) = server.try_wait()? {
            let _ = runner.kill();
            let _ = runner.wait();
            let _ = std::fs::remove_file(&connection_path);
            bail!("the server exited with {status}");
        }
        if interrupted() {
            let _ = runner.kill();
            let _ = server.kill();
            let _ = runner.wait();
            let _ = server.wait();
            // The server removes this itself on its own Ctrl-C; a SIGKILL leaves it
            // behind, and a stale file tells the next reader the wrong token.
            let _ = std::fs::remove_file(&connection_path);
            tracing::info!("stopped");
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

/// Ctrl-C or SIGTERM, watched once in the background: `timeout`, `kill`, and a
/// closed terminal all send the second one.
fn interrupted() -> bool {
    use std::sync::atomic::{AtomicBool, Ordering};
    static FLAG: AtomicBool = AtomicBool::new(false);
    static ONCE: std::sync::Once = std::sync::Once::new();

    ONCE.call_once(|| {
        tokio::spawn(async {
            #[cfg(unix)]
            {
                let mut term =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = async {
                        match term.as_mut() {
                            Some(stream) => {
                                stream.recv().await;
                            }
                            None => std::future::pending::<()>().await,
                        }
                    } => {}
                }
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
            FLAG.store(true, Ordering::SeqCst);
        });
    });

    FLAG.load(Ordering::SeqCst)
}

/// Starts another instance of this executable, so `up` needs no second binary.
fn spawn(command: &[&str], args: &[String]) -> Result<std::process::Child> {
    let exe = std::env::current_exe().context("locating this executable")?;
    Command::new(&exe)
        .args(command)
        .args(args)
        .spawn()
        .with_context(|| format!("starting {}", command.join(" ")))
}

/// The url and pid of a server that is still alive on this data directory, if the
/// connection file names one and the requested port matches.
fn already_running(path: &std::path::Path) -> Option<(String, i64, i64)> {
    let bytes = std::fs::read(path).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let pid = value.get("pid")?.as_i64()?;
    let port = value.get("port")?.as_i64()?;
    let alive = std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !alive {
        return None;
    }
    let url = value.get("url")?.as_str()?.to_string();
    Some((url, pid, port))
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
