//! The desktop shell: it owns the server and the runner, and hands the webview a
//! port and a token. Nothing else crosses the boundary.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{Manager, State};

struct Shell {
    server: Mutex<Option<Child>>,
    runner: Mutex<Option<Child>>,
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Connection {
    url: String,
    token: String,
    data_dir: String,
    label: String,
}

#[tauri::command]
fn connection() -> Result<Connection, String> {
    let dir = data_dir();
    let path = dir.join("connection.json");
    let raw = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&raw).map_err(|error| format!("cannot parse connection.json: {error}"))?;
    let port = value["port"].as_u64().unwrap_or_default();
    Ok(Connection {
        url: value["url"]
            .as_str()
            .map(|url| url.to_string())
            .unwrap_or_else(|| format!("http://127.0.0.1:{port}")),
        token: value["token"].as_str().unwrap_or_default().to_string(),
        data_dir: value["data_dir"].as_str().unwrap_or_default().to_string(),
        label: "this machine".to_string(),
    })
}

/// macOS keeps application state in Application Support; the server makes the
/// same choice, but the shell passes it explicitly so both agree.
fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("HULAHOOP_DATA") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/hulahoop")
    } else if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        PathBuf::from(xdg).join("hulahoop")
    } else {
        home.join(".local/share/hulahoop")
    }
}

/// Bundled builds ship the binaries next to the app executable; a development
/// build can point at the cargo target directory instead.
/// One sidecar serves both roles: it is the same `hulahoop` executable that runs
/// `serve` or `runner` as a subcommand.
fn find_binary() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("HULAHOOP_BIN") {
        return Some(PathBuf::from(explicit));
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let sibling = dir.join("hulahoop");
    if sibling.exists() {
        return Some(sibling);
    }
    let resource = dir.join("../Resources").join("hulahoop");
    if resource.exists() {
        return Some(resource);
    }
    Some(PathBuf::from("hulahoop"))
}

fn spawn(args: &[&str], data_dir: &Path) -> Result<Child, String> {
    let binary = find_binary().ok_or_else(|| "cannot locate hulahoop".to_string())?;
    Command::new(&binary)
        .args(args)
        .env("HULAHOOP_DATA", data_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("starting {}: {error}", binary.display()))
}

fn wait_for_connection(data_dir: &Path) -> Result<(), String> {
    let path = data_dir.join("connection.json");
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if path.exists() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(120));
    }
    Err(format!("the server did not write {} in 30s", path.display()))
}

fn main() {
    tauri::Builder::default()
        .manage(Shell {
            server: Mutex::new(None),
            runner: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![connection])
        .setup(|app| {
            let dir = data_dir();
            std::fs::create_dir_all(&dir)?;
            let shell: State<Shell> = app.state();

            // A server that is already running (for example from `just dev`) owns
            // the port and the token; starting a second one would fight for it.
            let already_running = dir.join("connection.json").exists();
            if !already_running {
                let server = spawn(
                    &["serve", "--data-dir", &dir.to_string_lossy()],
                    &dir,
                )?;
                *shell.server.lock().unwrap() = Some(server);
                wait_for_connection(&dir)?;
            }
            let runner = spawn(&["runner"], &dir)?;
            *shell.runner.lock().unwrap() = Some(runner);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building the Hulahoop window")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                let shell: State<Shell> = app.state();
                for slot in [&shell.runner, &shell.server] {
                    if let Some(mut child) = slot.lock().unwrap().take() {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                }
            }
        });
}
