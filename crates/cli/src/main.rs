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
        Some("doctor") => doctor(),
        Some("seed") => seed(&args[1..]).await,
        Some("help") | Some("--help") | Some("-h") | None => {
            println!(
                "hulahoop — the acceptance loop that keeps coding agents honest\n\n\
                 usage:\n  \
                 hulahoop serve [--data-dir DIR] [--port N]   run the server\n  \
                 hulahoop runner                              run the runner\n  \
                 hulahoop seed --repo PATH [--name NAME]      register a repo as a project\n  \
                 hulahoop doctor                              check this machine"
            );
            Ok(())
        }
        Some(other) => bail!("unknown command {other}; try `hulahoop help`"),
    }
}

fn passthrough(program: &str, args: &[String]) -> Result<()> {
    // `cargo run` puts the binary next to the CLI, and an installed app ships all
    // three side by side, so the sibling binary wins over PATH.
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.join(program)))
        .filter(|path| path.exists());
    let program_path = sibling.unwrap_or_else(|| std::path::PathBuf::from(program));
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
