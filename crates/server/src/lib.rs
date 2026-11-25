//! Hulahoop server: GraphQL for the desktop, an internal HTTP API for the
//! runner, and the single writer for the SQLite database.

pub mod api;
pub mod connectors;
pub mod cost;
pub mod db;
pub mod engine;
pub mod github;
pub mod gql;
pub mod lumi;
pub mod mentions;
pub mod model;
pub mod models;
pub mod pulls;
pub mod signin;
pub mod skills;
pub mod tasks;
pub mod tenancy;
pub mod transcript;
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
    // A server stopped mid sign-in can leave a login on disk; none is live now.
    signin::sweep(&data_dir);

    // The token lives with the data, so restarting the server — a rebuild, a crash, a
    // laptop that woke up — does not sign every client out of its own workspace.
    let engine = Arc::new(engine::Engine::new(database));
    // Every workspace reads the same contract, and an agent created before it
    // existed still has to have it. Startup is the one place that sees them all.
    let workspaces: Vec<(uuid::Uuid,)> = sqlx::query_as("select id from workspaces")
        .fetch_all(&engine.db.pool)
        .await?;
    for (id,) in workspaces {
        engine.seed_builtin_skills(id).await?;
    }
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

    let app = router(api_state);

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
            if let Err(error) = reaper.reap_lumi_jobs().await {
                tracing::warn!(%error, "reaping Lumi jobs failed");
            }
        }
    });

    // Pull requests are read from a cache this keeps in step with GitHub.
    tokio::spawn(engine.clone().pull_sync_loop());

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

/// Every route the server answers. `serve` binds it; tests drive it directly.
pub fn router(api_state: Arc<api::ApiState>) -> axum::Router {
    let schema = gql::build_schema(api_state.engine.clone());
    let ws_schema = schema.clone();
    let ws_token: Arc<str> = Arc::from(api_state.token.as_str());
    let http_token = ws_token.clone();
    axum::Router::new()
        .route(
            "/graphql",
            axum::routing::any_service(async_graphql_axum::GraphQL::new(schema)).layer(
                axum::middleware::from_fn(move |request, next| {
                    graphql_auth(http_token.clone(), request, next)
                }),
            ),
        )
        .route(
            "/graphql/ws",
            axum::routing::get(
                move |headers: axum::http::HeaderMap, upgrade: axum::extract::WebSocketUpgrade| {
                    gql::serve_websocket(ws_schema.clone(), ws_token.clone(), headers, upgrade)
                },
            ),
        )
        .route("/health", axum::routing::get(|| async { "ok" }))
        .merge(api::router(api_state))
        // The desktop runs from the Tauri origin (or Vite's fixed dev port).
        // Multi-user deployment also requires identity-aware GraphQL authorization.
        // An arbitrary website must not read workspace reports or mutate budgets
        // through the local GraphQL endpoint.
        .layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin([
                    axum::http::HeaderValue::from_static("tauri://localhost"),
                    axum::http::HeaderValue::from_static("http://tauri.localhost"),
                    axum::http::HeaderValue::from_static("https://tauri.localhost"),
                    axum::http::HeaderValue::from_static("http://localhost:5183"),
                    axum::http::HeaderValue::from_static("http://127.0.0.1:5183"),
                ])
                .allow_methods([axum::http::Method::GET, axum::http::Method::POST])
                .allow_headers([
                    axum::http::header::CONTENT_TYPE,
                    axum::http::header::AUTHORIZATION,
                ]),
        )
        .layer(tower_http::trace::TraceLayer::new_for_http())
}

/// GraphQL over HTTP carries the same bearer token as the internal API and the
/// WebSocket's `connection_init`. Without it, any page in the browser could post a
/// `text/plain` "simple request" here and act on the workspace — CORS only stops
/// it reading the answer.
async fn graphql_auth(
    token: Arc<str>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let provided = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    if api::same_token(&token, provided) {
        next.run(request).await
    } else {
        axum::response::IntoResponse::into_response(axum::http::StatusCode::UNAUTHORIZED)
    }
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

#[cfg(test)]
mod auth_tests {
    //! Both ways into GraphQL — HTTP and the WebSocket — want the server token.

    use super::*;
    use async_graphql::http::{WebSocketProtocols, WsMessage};
    use futures::StreamExt;
    use serde_json::json;
    use std::time::Duration;

    const TOKEN: &str = "the-server-token";

    async fn engine() -> (Arc<engine::Engine>, std::path::PathBuf) {
        let directory =
            std::env::temp_dir().join(format!("hulahoop-auth-{}", uuid::Uuid::now_v7()));
        let database = db::Db::open(&directory).await.unwrap();
        database.seed_local().await.unwrap();
        (Arc::new(engine::Engine::new(database)), directory)
    }

    #[tokio::test]
    async fn graphql_over_http_needs_the_token() {
        let (engine, directory) = engine().await;
        let app = router(Arc::new(api::ApiState {
            engine,
            token: TOKEN.to_string(),
            runner_id_default: "runner-test".to_string(),
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let http = reqwest::Client::new();
        let url = format!("http://{address}/graphql");
        let query = json!({ "query": "{ workspaces { id } }" });

        let none = http.post(&url).json(&query).send().await.unwrap();
        assert_eq!(none.status(), reqwest::StatusCode::UNAUTHORIZED);
        let wrong = http
            .post(&url)
            .bearer_auth("not-the-token")
            .json(&query)
            .send()
            .await
            .unwrap();
        assert_eq!(wrong.status(), reqwest::StatusCode::UNAUTHORIZED);
        // A cross-site "simple request" gets nowhere either.
        let simple = http
            .post(&url)
            .header("content-type", "text/plain")
            .body(query.to_string())
            .send()
            .await
            .unwrap();
        assert_eq!(simple.status(), reqwest::StatusCode::UNAUTHORIZED);

        let right = http
            .post(&url)
            .bearer_auth(TOKEN)
            .json(&query)
            .send()
            .await
            .unwrap();
        assert_eq!(right.status(), reqwest::StatusCode::OK);
        let body: serde_json::Value = right.json().await.unwrap();
        assert!(body.get("errors").is_none(), "{body}");
        assert!(body["data"]["workspaces"].is_array(), "{body}");

        // Health stays open, so a launcher can wait for the server.
        let health = http
            .get(format!("http://{address}/health"))
            .send()
            .await
            .unwrap();
        assert_eq!(health.status(), reqwest::StatusCode::OK);
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// The frames a socket answers to `input`, until it closes or goes quiet.
    async fn answers(input: Vec<serde_json::Value>, init_timeout: Duration) -> Vec<WsMessage> {
        let (engine, directory) = engine().await;
        let schema = gql::build_schema(engine);
        let frames: Vec<String> = input.iter().map(|frame| frame.to_string()).collect();
        // The client keeps the socket open after its frames, as a real one would.
        let input = futures::stream::iter(frames)
            .chain(futures::stream::pending())
            .boxed();
        let output = gql::socket_messages(
            schema,
            Arc::from(TOKEN),
            input,
            WebSocketProtocols::GraphQLWS,
            init_timeout,
        );
        futures::pin_mut!(output);
        let mut seen = Vec::new();
        while let Ok(Some(message)) =
            tokio::time::timeout(Duration::from_secs(1), output.next()).await
        {
            seen.push(message);
        }
        let _ = std::fs::remove_dir_all(&directory);
        seen
    }

    fn closed_with(messages: &[WsMessage]) -> Option<(u16, String)> {
        messages.iter().find_map(|message| match message {
            WsMessage::Close(code, reason) => Some((*code, reason.clone())),
            WsMessage::Text(_) => None,
        })
    }

    #[tokio::test]
    async fn a_socket_with_the_wrong_token_is_closed_as_forbidden() {
        let seen = answers(
            vec![json!({ "type": "connection_init", "payload": { "token": "nope" } })],
            Duration::from_secs(10),
        )
        .await;
        assert_eq!(
            closed_with(&seen),
            Some((gql::CLOSE_FORBIDDEN, gql::BAD_TOKEN.to_string()))
        );
    }

    #[tokio::test]
    async fn a_subscription_before_connection_init_closes_the_socket() {
        let seen = answers(
            vec![json!({
                "type": "subscribe", "id": "1",
                "payload": { "query": "subscription { runDiff(runId: \"00000000-0000-0000-0000-000000000000\") { patch } }" }
            })],
            Duration::from_secs(10),
        )
        .await;
        let (code, _) = closed_with(&seen).expect("the socket is closed");
        assert_eq!(code, 1011);
        assert!(
            seen.iter()
                .all(|m| !matches!(m, WsMessage::Text(text) if text.contains("\"next\""))),
            "{seen:?}"
        );
    }

    #[tokio::test]
    async fn a_socket_that_never_initialises_is_closed() {
        let seen = answers(Vec::new(), Duration::from_millis(100)).await;
        assert_eq!(
            closed_with(&seen).map(|(code, _)| code),
            Some(gql::CLOSE_INIT_TIMEOUT)
        );
    }

    #[tokio::test]
    async fn the_right_token_is_acknowledged_and_the_timeout_stops() {
        let seen = answers(
            vec![json!({ "type": "connection_init", "payload": { "token": TOKEN } })],
            Duration::from_millis(200),
        )
        .await;
        assert!(
            matches!(&seen[0], WsMessage::Text(text) if text.contains("connection_ack")),
            "{seen:?}"
        );
        assert_eq!(closed_with(&seen), None, "an initialised socket stays open");
    }
}
