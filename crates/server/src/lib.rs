//! Hulahoop server: GraphQL for the desktop, an internal HTTP API for the
//! runner, and the single writer for the SQLite database.

pub mod api;
pub mod db;
pub mod engine;
pub mod gql;
pub mod model;

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

    let database = db::Db::open(&data_dir).await?;
    let workspace_id = database.seed_local().await?;
    tracing::info!(%workspace_id, data_dir = %data_dir.display(), "server starting");

    let engine = Arc::new(engine::Engine::new(database));
    let token = random_token();

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

    let listener = tokio::net::TcpListener::bind(SocketAddr::from((listen, port)))
        .await
        .context("binding the server port")?;
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
