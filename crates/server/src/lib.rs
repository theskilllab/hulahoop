//! Hulahoop server: GraphQL for the desktop, an internal HTTP API for the
//! runner, and the single writer for the SQLite database.

pub mod api;
pub mod connectors;
pub mod db;
pub mod engine;
pub mod github;
pub mod gql;
pub mod model;
pub mod skills;
pub mod tenancy;
pub mod vault;
pub mod workflows;

use anyhow::Context;
use std::net::SocketAddr;
use std::sync::Arc;

/// Runs the server until Ctrl-C. The caller owns the runtime and log filter,
/// so `hulahoop serve` and any test can start this without a second main.
pub async fn serve(args: Vec<String>) -> anyhow::Result<()> {
    let mut data_dir = std::env::var_os("HULAHOOP_DATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(default_data_dir);
    let mut port: u16 = 0;
    let mut listen: std::net::IpAddr = std::net::IpAddr::from([127, 0, 0, 1]);
    let mut print_connection = false;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--data-dir" => {
                index += 1;
                data_dir =
                    std::path::PathBuf::from(args.get(index).context("--data-dir needs a path")?);
            }
            "--port" => {
                index += 1;
                port = args
                    .get(index)
                    .context("--port needs a number")?
                    .parse()
                    .context("--port must be a number")?;
            }
            "--listen" => {
                index += 1;
                listen = args
                    .get(index)
                    .context("--listen needs an address")?
                    .parse()
                    .context("--listen takes an IP address; 127.0.0.1 keeps it loopback-only")?;
            }
            "--print-connection" => print_connection = true,
            other => anyhow::bail!("unknown argument {other}"),
        }
        index += 1;
    }

    // A relative `--data-dir` is what a person types, and it is resolved against this
    // process's working directory. Every path handed to a runner is resolved against
    // *that* process's working directory — a checkout, elsewhere entirely — where the
    // same relative string points at nothing. Six hours went into a run that could not
    // find its own credentials for exactly this reason, so the data dir is absolute
    // from here on.
    std::fs::create_dir_all(&data_dir).context("creating the data directory")?;
    data_dir = std::fs::canonicalize(&data_dir)
        .with_context(|| format!("resolving {}", data_dir.display()))?;

    let database = db::Db::open(&data_dir).await?;
    let workspace_id = database.seed_local().await?;
    tracing::info!(%workspace_id, data_dir = %data_dir.display(), "server starting");

    // The token lives with the data, so restarting the server — a rebuild, a crash, a
    // laptop that woke up — does not sign every client out of its own workspace.
    let engine = Arc::new(engine::Engine::new(database));
    let token = match stored_token(&engine).await? {
        Some(token) => token,
        None => {
            let token = random_token();
            remember_token(&engine, &token).await?;
            token
        }
    };

    let api_state = Arc::new(api::ApiState {
        engine: engine.clone(),
        token: token.clone(),
        runner_id_default: format!("runner-{}", std::process::id()),
    });

    let schema = gql::build_schema(engine.clone());
    let app = axum::Router::new()
        .route(
            "/graphql",
            axum::routing::any_service(async_graphql_axum::GraphQL::new(schema)),
        )
        .route("/health", axum::routing::get(|| async { "ok" }))
        .merge(api::router(api_state))
        .layer(tower_http::cors::CorsLayer::very_permissive())
        .layer(tower_http::trace::TraceLayer::new_for_http());

    let socket = SocketAddr::from((listen, port));
    let listener = tokio::net::TcpListener::bind(socket)
        .await
        .map_err(|cause| {
            if cause.kind() == std::io::ErrorKind::AddrInUse {
                anyhow::anyhow!(
                    "{socket} is already taken, so this server cannot start. \
                 Something else is listening there — a Hulahoop server from an earlier run \
                 answers `pgrep -ax hulahoop` and can be stopped with `kill <pid>`. \
                 Pass a different --port, or use the server that is already up: its token \
                 is in {}/connection.json.",
                    data_dir.display()
                )
            } else {
                anyhow::Error::new(cause).context(format!("binding {socket}"))
            }
        })?;
    let address = listener.local_addr()?;

    let connection_path = hulahoop_core::contract::connection_file(&data_dir);
    let connection = serde_json::json!({
        "listen": listen.to_string(),
        "port": address.port(),
        "url": format!("http://{}:{}", advertise_host(listen), address.port()),
        "token": token,
        "pid": std::process::id(),
        "data_dir": data_dir.display().to_string(),
    });
    std::fs::write(&connection_path, serde_json::to_vec_pretty(&connection)?)?;
    restrict_permissions(&connection_path)?;
    if print_connection {
        println!("{}", serde_json::to_string(&connection)?);
    }
    tracing::info!(
        listen = %listen,
        port = address.port(),
        url = %format!("http://{}:{}", advertise_host(listen), address.port()),
        "listening"
    );

    // A runner that dies leaves runs `running`; the reaper is the only place
    // that can tell a dead run from a slow one.
    let reaper = engine.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            ticker.tick().await;
            if let Err(error) = reaper.reap_dead_runs().await {
                tracing::warn!(%error, "reaping runs failed");
            }
        }
    });

    let connection_path_clone = connection_path.clone();
    let shutdown = async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = tokio::fs::remove_file(&connection_path_clone).await;
        tracing::info!("server stopped");
    };

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .context("serving HTTP")?;
    Ok(())
}

/// The host a client should dial: loopback when the server listens everywhere,
/// otherwise the address it was told to bind. A runner on the same machine reads
/// this file, and the desktop only uses the same value for its local mode.
/// The address to put in `connection.json`, and so the one a remote desktop pastes
/// into its connect screen. Binding `0.0.0.0` accepts every interface; advertising
/// `0.0.0.0` or `127.0.0.1` would send a Mac on the LAN to itself.
fn advertise_host(listen: std::net::IpAddr) -> std::net::IpAddr {
    if listen.is_unspecified() {
        primary_ip().unwrap_or_else(|| std::net::IpAddr::from([127, 0, 0, 1]))
    } else {
        listen
    }
}

/// The address this machine reaches the default route with. A UDP socket sends no
/// packets here; `connect` only resolves which local address the kernel would use.
fn primary_ip() -> Option<std::net::IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    socket.local_addr().ok().map(|addr| addr.ip())
}

fn default_data_dir() -> std::path::PathBuf {
    if let Some(dir) = dirs_data_dir() {
        return dir.join("hulahoop");
    }
    std::path::PathBuf::from(".hulahoop-data")
}

fn dirs_data_dir() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(|home| std::path::PathBuf::from(home).join("Library/Application Support"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|home| std::path::PathBuf::from(home).join(".local/share"))
            })
    }
}

async fn stored_token(engine: &engine::Engine) -> anyhow::Result<Option<String>> {
    let stored: Option<(String,)> = sqlx::query_as("select value from meta where key = 'token'")
        .fetch_optional(&engine.db.pool)
        .await?;
    Ok(stored
        .map(|(value,)| value)
        .filter(|value| !value.is_empty()))
}

async fn remember_token(engine: &engine::Engine, token: &str) -> anyhow::Result<()> {
    sqlx::query("insert into meta (key, value) values ('token', ?) on conflict(key) do update set value = excluded.value")
        .bind(token)
        .execute(&engine.db.pool)
        .await?;
    Ok(())
}

fn random_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn restrict_permissions(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path)?.permissions();
        permissions.set_mode(0o600);
        std::fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

/// The `origin` remote of a repository, whichever VCS answers first.
///
/// git is asked directly because a colocated checkout has one; jj is the fallback
/// for a repo that only jj knows about. A repo without a remote returns `None`,
/// which a pull request job reports rather than guessing a URL.
pub fn vcs_remote(root: &std::path::Path) -> Option<String> {
    let run = |program: &str, args: &[&str]| -> Option<String> {
        let output = std::process::Command::new(program)
            .args(args)
            .current_dir(root)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    };
    if let Some(url) = run("git", &["remote", "get-url", "origin"]) {
        return Some(url);
    }
    // `jj git remote list` prints one line per remote, "origin <url>".
    let listed = run("jj", &["git", "remote", "list"])?;
    listed.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        match (parts.next(), parts.next()) {
            (Some("origin"), Some(url)) => Some(url.to_string()),
            _ => None,
        }
    })
}
