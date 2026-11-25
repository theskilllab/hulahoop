//! OAuth sign-in for the subscription connectors, through Pi's own login.
//!
//! `pi auth login <provider> --no-browser` already knows each provider's OAuth
//! flow: it prints a link (and, for a device flow, a code), waits for the person,
//! and writes the login into `~/.pi/agent/auth.json`. It insists on a terminal,
//! so the server gives it a pseudo-terminal, points `HOME` at a fresh scratch
//! directory so the host's own login is never read or touched, relays the link
//! and the code to the Connectors screen, and seals the entry Pi wrote.
//!
//! The pseudo-terminal comes from `pty-process`: it spawns onto a pty with
//! tokio's process API on Linux and macOS, and adds little beyond `rustix`,
//! which the workspace already builds.

use crate::engine::Engine;
use anyhow::{anyhow, bail, Context as _, Result};
use async_graphql::{SimpleObject, ID};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, watch};
use uuid::Uuid;

/// How long a sign-in may wait for the person before it is stopped.
pub const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// How long a finished sign-in stays readable, so a screen polling it sees the end.
const KEEP_FINISHED: Duration = Duration::from_secs(10 * 60);
/// How long a GraphQL call waits for the next thing worth showing.
const ANSWER_WITHIN: Duration = Duration::from_secs(15);
/// Output kept from Pi: the most recent this many bytes, enough for a link, a
/// code and a closing error however long a spinner ran before them.
const OUTPUT_LIMIT: usize = 64 * 1024;
/// An escape sequence or character still arriving is held back at most this
/// long before it is decoded as it stands.
const PENDING_LIMIT: usize = 4096;

/// The host settings Pi needs to reach a provider from behind a proxy or a
/// private certificate authority, and to print in the host's language. None
/// of them is a secret; everything else in the server's environment stays out.
const PASS_THROUGH_ENV: [&str; 10] = [
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "NO_PROXY",
    "https_proxy",
    "http_proxy",
    "no_proxy",
    "NODE_EXTRA_CA_CERTS",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "LANG",
];

/// The [`PASS_THROUGH_ENV`] variables the server has, for a Pi it starts.
pub(crate) fn pass_through_env() -> Vec<(&'static str, std::ffi::OsString)> {
    PASS_THROUGH_ENV
        .into_iter()
        .filter_map(|name| std::env::var_os(name).map(|value| (name, value)))
        .collect()
}

/// One sign-in as the Connectors screen sees it. The tokens never appear here.
#[derive(Debug, Clone, SimpleObject)]
pub struct SignIn {
    pub id: ID,
    pub connector_id: String,
    /// `starting`, `waiting`, `done`, `failed` or `cancelled`.
    pub state: String,
    /// The page the person opens to sign in.
    pub url: Option<String>,
    /// The code a device flow asks the person to enter on that page.
    pub user_code: Option<String>,
    /// Whether the provider shows a code the person pastes back here.
    pub needs_code: bool,
    pub error: Option<String>,
}

impl SignIn {
    fn is_finished(&self) -> bool {
        matches!(self.state.as_str(), "done" | "failed" | "cancelled")
    }

    fn is_showable(&self) -> bool {
        self.is_finished() || self.url.is_some() || self.user_code.is_some()
    }
}

enum Control {
    Code(String),
    Cancel,
}

struct Session {
    connector_id: &'static str,
    /// Set the moment the sign-in is cancelled or replaced, so a login that
    /// finishes at the same instant is not stored anyway.
    cancelled: AtomicBool,
    view: watch::Sender<SignIn>,
    control: mpsc::UnboundedSender<Control>,
    /// Pi's output so far. The view changes only under this lock, so a code
    /// being passed and a prompt being read never cross.
    output: Mutex<Output>,
}

/// The sign-ins in progress and recently finished, by id. Held in memory: a
/// sign-in does not survive a server restart, and the person starts another.
pub struct Registry {
    sessions: Mutex<Sessions>,
    /// Held while a finished sign-in checks it is still the connector's
    /// current one and stores its login, so an older sign-in can never write
    /// over a newer one.
    store: tokio::sync::Mutex<()>,
    /// How long a sign-in may wait; a field so a test need not wait ten minutes.
    pub timeout: Duration,
}

#[derive(Default)]
struct Sessions {
    by_id: HashMap<Uuid, Arc<Session>>,
    /// The newest sign-in of each connector: the only one whose login is kept.
    current: HashMap<&'static str, Uuid>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            sessions: Mutex::default(),
            store: tokio::sync::Mutex::default(),
            timeout: SIGN_IN_TIMEOUT,
        }
    }
}

impl Registry {
    fn get(&self, id: Uuid) -> Option<Arc<Session>> {
        self.sessions.lock().ok()?.by_id.get(&id).cloned()
    }

    /// Make a sign-in its connector's current one, cancelling the one before
    /// it, in one step: two starts at once cannot both stay live.
    fn begin(&self, id: Uuid, session: Arc<Session>) {
        let Ok(mut sessions) = self.sessions.lock() else {
            return;
        };
        if let Some(previous) = sessions
            .current
            .insert(session.connector_id, id)
            .and_then(|previous| sessions.by_id.get(&previous).cloned())
        {
            previous.cancel();
        }
        sessions.by_id.insert(id, session);
    }

    fn is_current(&self, connector_id: &str, id: Uuid) -> bool {
        self.sessions
            .lock()
            .is_ok_and(|sessions| sessions.current.get(connector_id) == Some(&id))
    }

    fn remove(&self, id: Uuid) {
        if let Ok(mut sessions) = self.sessions.lock() {
            if let Some(session) = sessions.by_id.remove(&id) {
                if sessions.current.get(session.connector_id) == Some(&id) {
                    sessions.current.remove(session.connector_id);
                }
            }
        }
    }
}

impl Session {
    /// Stop this sign-in. `false` when it had already ended.
    fn cancel(&self) -> bool {
        self.cancelled.store(true, Ordering::SeqCst);
        !self.view.borrow().is_finished() && self.control.send(Control::Cancel).is_ok()
    }
}

/// The Pi provider a connector signs in to, for the entries that sign in.
fn provider_for(connector_id: &str) -> Result<(&'static str, &'static str)> {
    let entry = crate::connectors::entry(connector_id)
        .ok_or_else(|| anyhow!("unknown connector `{connector_id}`"))?;
    match entry.pi_provider {
        Some(provider) if entry.kind == crate::connectors::KIND_SUBSCRIPTION => {
            Ok((entry.id, provider))
        }
        _ => bail!("`{}` connects with a key, not a sign-in", entry.name),
    }
}

fn parse_id(id: &ID) -> Result<Uuid> {
    Uuid::parse_str(id.as_str()).map_err(|_| anyhow!("unknown sign-in `{}`", id.as_str()))
}

/// Start a sign-in and answer once there is a link or a code to show, or the
/// login already ended.
pub async fn start(engine: &Arc<Engine>, connector_id: &str) -> Result<SignIn> {
    let (connector_id, provider) = provider_for(connector_id)?;
    let id = Uuid::now_v7();
    let home = scratch_home(&engine.db.data_dir, id)?;
    let spawned = spawn_login(&engine.pi_bin, provider, &home);
    let (child, pty) = match spawned {
        Ok(spawned) => spawned,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&home);
            return Err(error);
        }
    };
    let (view, mut watcher) = watch::channel(SignIn {
        id: ID(id.to_string()),
        connector_id: connector_id.to_string(),
        state: "starting".into(),
        url: None,
        user_code: None,
        needs_code: false,
        error: None,
    });
    let (control, controls) = mpsc::unbounded_channel();
    let session = Arc::new(Session {
        connector_id,
        cancelled: AtomicBool::new(false),
        view,
        control,
        output: Mutex::default(),
    });
    engine.signins.begin(id, session.clone());
    tokio::spawn(drive(
        engine.clone(),
        id,
        session,
        Login {
            provider,
            home,
            child,
            pty,
        },
        controls,
    ));
    let _ = tokio::time::timeout(ANSWER_WITHIN, watcher.wait_for(SignIn::is_showable)).await;
    let view = watcher.borrow().clone();
    Ok(view)
}

/// A sign-in as it stands, for the screen's poll.
pub fn status(engine: &Engine, id: &ID) -> Result<Option<SignIn>> {
    let id = parse_id(id)?;
    Ok(engine
        .signins
        .get(id)
        .map(|session| session.view.borrow().clone()))
}

/// Pass the code the provider showed to Pi, then answer once the login ends or
/// asks again, within a bounded wait.
pub async fn submit_code(engine: &Engine, id: &ID, code: &str) -> Result<SignIn> {
    let session = engine
        .signins
        .get(parse_id(id)?)
        .ok_or_else(|| anyhow!("that sign-in has ended; start a new one"))?;
    let code = code.trim();
    if code.is_empty() || code.contains(['\r', '\n']) {
        bail!("paste the code the provider showed, on one line");
    }
    if session.view.borrow().is_finished() {
        bail!("that sign-in has ended; start a new one");
    }
    let mut watcher = session.view.subscribe();
    if let Ok(mut output) = session.output.lock() {
        output.submitted(code);
        session.view.send_modify(|view| view.needs_code = false);
    }
    session
        .control
        .send(Control::Code(code.to_string()))
        .map_err(|_| anyhow!("that sign-in has ended; start a new one"))?;
    let _ = tokio::time::timeout(
        ANSWER_WITHIN,
        watcher.wait_for(|view| view.is_finished() || view.needs_code),
    )
    .await;
    let view = watcher.borrow().clone();
    Ok(view)
}

/// Stop a sign-in. `false` when there is nothing left to stop.
pub fn cancel(engine: &Engine, id: &ID) -> Result<bool> {
    let Some(session) = engine.signins.get(parse_id(id)?) else {
        return Ok(false);
    };
    Ok(session.cancel())
}

/// A private, empty home for one login: Pi writes the login here and nowhere
/// else, and the directory goes when the sign-in ends.
fn scratch_home(data_dir: &Path, id: Uuid) -> Result<PathBuf> {
    private_dir(&signin_root(data_dir)?, &id.to_string())
}

/// A private, empty home for one use of Pi with a login in it (a central
/// refresh), beside the sign-in homes so the start-up sweep covers it too.
pub(crate) fn private_home(data_dir: &Path, purpose: &str) -> Result<PathBuf> {
    private_dir(
        &signin_root(data_dir)?,
        &format!("{purpose}-{}", Uuid::now_v7()),
    )
}

/// `DATA_DIR/signin`, owner-only from the moment it exists.
fn signin_root(data_dir: &Path) -> Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let root = data_dir.join("signin");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&root)?;
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
    Ok(root)
}

/// A new directory created owner-only, never widened and narrowed after.
fn private_dir(root: &Path, name: &str) -> Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    let home = root.join(name);
    std::fs::DirBuilder::new().mode(0o700).create(&home)?;
    Ok(home)
}

/// Remove every sign-in and refresh home a stopped server left behind: one
/// killed between Pi writing a login and the cleanup would otherwise keep live
/// tokens on disk. Run at start-up, before any sign-in can begin.
pub fn sweep(data_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(data_dir.join("signin")) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let removed = match entry.file_type() {
            Ok(kind) if kind.is_dir() => std::fs::remove_dir_all(&path),
            _ => std::fs::remove_file(&path),
        };
        if let Err(error) = removed {
            tracing::warn!(path = %path.display(), %error, "could not remove a leftover sign-in home");
        }
    }
}

struct Login {
    provider: &'static str,
    home: PathBuf,
    child: tokio::process::Child,
    pty: pty_process::Pty,
}

fn spawn_login(
    pi: &Path,
    provider: &str,
    home: &Path,
) -> Result<(tokio::process::Child, pty_process::Pty)> {
    let (pty, pts) = pty_process::open().context("could not open a terminal for the sign-in")?;
    // Wide enough that Pi never wraps the sign-in link across lines.
    pty.resize(pty_process::Size::new(50, 2000))?;
    let child = pty_process::Command::new(pi)
        .args(["auth", "login", provider, "--no-browser"])
        .env_clear()
        .env("HOME", home)
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .envs(pass_through_env())
        .current_dir(home)
        .kill_on_drop(true)
        .spawn(pts)
        .with_context(|| format!("could not start `{}`", pi.display()))?;
    Ok((child, pty))
}

/// Everything after the start: relay Pi's output, pass the code, stop on
/// cancel or timeout, and seal the login when Pi writes one.
async fn drive(
    engine: Arc<Engine>,
    id: Uuid,
    session: Arc<Session>,
    login: Login,
    mut controls: mpsc::UnboundedReceiver<Control>,
) {
    let Login {
        provider,
        home,
        mut child,
        pty,
    } = login;
    let (mut reader, mut writer) = pty.into_split();
    let mut read_task = {
        let session = session.clone();
        tokio::spawn(async move {
            let mut chunk = [0u8; 4096];
            // A pty reports EIO once the child is gone; that is its end of file.
            while let Ok(size) = reader.read(&mut chunk).await {
                if size == 0 {
                    break;
                }
                let Ok(mut output) = session.output.lock() else {
                    break;
                };
                let parsed = output.push(&chunk[..size]);
                session.view.send_modify(|view| parsed.apply(view));
            }
        })
    };

    let deadline = tokio::time::sleep(engine.signins.timeout);
    tokio::pin!(deadline);
    let ending = loop {
        tokio::select! {
            status = child.wait() => break Ending::Exited(status.ok()),
            _ = &mut deadline => break Ending::Failed("Sign-in timed out".into()),
            control = controls.recv() => match control {
                Some(Control::Code(code)) => {
                    let line = format!("{code}\r");
                    if writer.write_all(line.as_bytes()).await.is_err() || writer.flush().await.is_err() {
                        break Ending::Failed("Pi stopped listening for the code".into());
                    }
                }
                Some(Control::Cancel) | None => break Ending::Cancelled,
            },
        }
    };
    if !matches!(ending, Ending::Exited(_)) {
        let _ = child.kill().await;
    }
    // The reader ends with the child. A grandchild still holding the terminal
    // would keep it reading, so a stuck one is stopped rather than left behind.
    if tokio::time::timeout(Duration::from_secs(2), &mut read_task)
        .await
        .is_err()
    {
        read_task.abort();
    }
    let result = match ending {
        Ending::Exited(Some(status)) if status.success() => {
            finish(&engine, id, &session, provider, &home).await
        }
        Ending::Exited(_) => {
            let message = session
                .output
                .lock()
                .map(|output| output.failure())
                .unwrap_or_default();
            Err(message)
        }
        Ending::Failed(message) => Err(message),
        Ending::Cancelled => Ok("cancelled"),
    };
    let _ = std::fs::remove_dir_all(&home);
    session.view.send_modify(|view| {
        view.needs_code = false;
        match result {
            Ok(state) => view.state = state.into(),
            Err(message) => {
                view.state = "failed".into();
                view.error = Some(message);
            }
        }
    });
    tokio::spawn(async move {
        tokio::time::sleep(KEEP_FINISHED).await;
        engine.signins.remove(id);
    });
}

enum Ending {
    Exited(Option<std::process::ExitStatus>),
    Failed(String),
    Cancelled,
}

/// Seal the login Pi wrote into the connector's row — unless the sign-in was
/// cancelled or a newer one for the connector started meanwhile, whose login
/// is the one the person is waiting for.
async fn finish(
    engine: &Engine,
    id: Uuid,
    session: &Session,
    provider: &str,
    home: &Path,
) -> std::result::Result<&'static str, String> {
    let connector_id = session.connector_id;
    let _store = engine.signins.store.lock().await;
    if session.cancelled.load(Ordering::SeqCst) || !engine.signins.is_current(connector_id, id) {
        return Ok("cancelled");
    }
    let auth = std::fs::read_to_string(home.join(hulahoop_core::credential::PI_AUTH_FILE))
        .map_err(|_| "Pi finished without writing a login".to_string())?;
    let auth: serde_json::Value =
        serde_json::from_str(&auth).map_err(|_| "Pi wrote a login Hulahoop cannot read")?;
    let entry = auth
        .get(provider)
        .filter(|entry| hulahoop_core::credential::is_oauth(entry))
        .ok_or_else(|| format!("Pi finished without an OAuth login for {provider}"))?;
    let label = ["email", "accountEmail"]
        .into_iter()
        .find_map(|field| entry[field].as_str())
        .unwrap_or("signed in");
    engine
        .set_agent_oauth(connector_id, entry, label)
        .await
        .map(|_| "done")
        .map_err(|error| {
            tracing::warn!(%error, connector_id, "could not store a sign-in");
            "Hulahoop could not store the login".to_string()
        })
}

/// What Pi printed, with escape sequences removed, and what it has said so far.
#[derive(Default)]
struct Output {
    /// Bytes read but not decoded yet: a character or an escape sequence the
    /// last read cut in half, completed by the next one.
    pending: Vec<u8>,
    /// The most recent [`OUTPUT_LIMIT`] bytes of decoded text.
    text: String,
    /// Where the text stood when a code was last passed: a new prompt after it
    /// means the code was not accepted.
    code_mark: usize,
    codes: Vec<String>,
}

/// What one read taught us about the sign-in.
struct Parsed {
    url: Option<String>,
    user_code: Option<String>,
    needs_code: bool,
}

impl Parsed {
    fn apply(&self, view: &mut SignIn) {
        if view.is_finished() {
            return;
        }
        if view.url.is_none() {
            view.url = self.url.clone();
        }
        if view.user_code.is_none() {
            view.user_code = self.user_code.clone();
        }
        view.needs_code = self.needs_code;
        if view.url.is_some() || view.user_code.is_some() {
            view.state = "waiting".into();
        }
    }
}

impl Output {
    fn push(&mut self, bytes: &[u8]) -> Parsed {
        self.pending.extend_from_slice(bytes);
        let complete = decode_complete(&mut self.pending);
        self.text.push_str(&strip_ansi(&complete));
        self.keep_tail();
        Parsed {
            url: first_url(&self.text),
            user_code: user_code(&self.text),
            needs_code: self.text[self.code_mark..].contains("Paste the code"),
        }
    }

    /// Drop the oldest text past [`OUTPUT_LIMIT`]: a spinner that ran for ten
    /// minutes must not crowd out the prompt or the error that follow it.
    fn keep_tail(&mut self) {
        if self.text.len() <= OUTPUT_LIMIT {
            return;
        }
        let mut cut = self.text.len() - OUTPUT_LIMIT;
        while !self.text.is_char_boundary(cut) {
            cut += 1;
        }
        self.text.drain(..cut);
        self.code_mark = self.code_mark.saturating_sub(cut);
    }

    fn submitted(&mut self, code: &str) {
        self.code_mark = self.text.len();
        self.codes.push(code.to_string());
    }

    /// Pi's last words, cleaned of anything that could be a secret: lines that
    /// name tokens and the codes the person pasted, which the terminal echoed.
    fn failure(&self) -> String {
        let line = self
            .text
            .lines()
            .rev()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .find(|line| {
                let lower = line.to_lowercase();
                !lower.contains("access")
                    && !lower.contains("refresh")
                    && !lower.contains("token")
                    && !self.codes.iter().any(|code| line.contains(code.as_str()))
            });
        match line {
            Some(line) => line.chars().take(200).collect(),
            None => "Sign-in failed".into(),
        }
    }
}

/// The text of every complete character and escape sequence in `pending`,
/// leaving there what a later read will complete. Invalid bytes decode as
/// U+FFFD; anything held back longer than [`PENDING_LIMIT`] is decoded as is.
fn decode_complete(pending: &mut Vec<u8>) -> String {
    let mut text = String::new();
    let mut rest: &[u8] = pending;
    loop {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                text.push_str(valid);
                rest = &[];
                break;
            }
            Err(error) => {
                let (valid, after) = rest.split_at(error.valid_up_to());
                text.push_str(std::str::from_utf8(valid).unwrap_or_default());
                match error.error_len() {
                    Some(length) => {
                        text.push('\u{fffd}');
                        rest = &after[length..];
                    }
                    // A character cut off at the end of the read.
                    None => {
                        rest = after;
                        break;
                    }
                }
            }
        }
    }
    let mut held = text.split_off(incomplete_escape(&text)).into_bytes();
    held.extend_from_slice(rest);
    if held.len() > PENDING_LIMIT {
        text.push_str(&String::from_utf8_lossy(&held));
        held.clear();
    }
    *pending = held;
    text
}

/// Where an escape sequence the text ends inside of begins, or the text's
/// length when it ends on a complete one.
fn incomplete_escape(text: &str) -> usize {
    let Some(start) = text.rfind('\u{1b}') else {
        return text.len();
    };
    let mut sequence = text[start + 1..].chars();
    let complete = match sequence.next() {
        None => false,
        Some('[') => sequence.any(|c| ('@'..='~').contains(&c)),
        Some(']') => sequence.any(|c| c == '\u{7}'),
        Some(_) => true,
    };
    if complete {
        text.len()
    } else {
        start
    }
}

/// Terminal output without its escape sequences and carriage returns.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                // CSI: parameters, then one final byte in @..~.
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                // OSC: up to BEL or ESC \.
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// The first `https://` link in the text, once whitespace after it shows it
/// is whole: a link a read cut short is not a link yet.
fn first_url(text: &str) -> Option<String> {
    let start = text.find("https://")?;
    let rest = &text[start..];
    let end = rest.find(char::is_whitespace)?;
    Some(rest[..end].trim_end_matches(['.', ',', ')']).to_string())
}

/// The code after `enter code:`, as a device flow prints it, once whitespace
/// after it shows it is whole.
fn user_code(text: &str) -> Option<String> {
    const MARK: &str = "enter code:";
    let start = text.to_ascii_lowercase().find(MARK)? + MARK.len();
    let rest = text[start..].trim_start();
    let end = rest.find(char::is_whitespace)?;
    Some(rest[..end].trim_end_matches(['.', ',']).to_string()).filter(|code| !code.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pi_output_yields_the_link_the_code_and_the_prompt() {
        let mut output = Output::default();
        let parsed = output.push(
            b"\x1b[1mSelect Anthropic login method:\x1b[0m Copy code login (headless)\r\nOpen this URL to sign in:\r\n  https://claude.ai/oauth/authorize?code=true&state=abc\r\nPaste the code Anthropic shows after you sign in: (code#state): ",
        );
        assert_eq!(
            parsed.url.as_deref(),
            Some("https://claude.ai/oauth/authorize?code=true&state=abc")
        );
        assert!(parsed.needs_code);
        assert!(parsed.user_code.is_none());

        let mut output = Output::default();
        let parsed = output.push(
            b"Select OpenAI Codex login method: Device code login (headless)\nOpen https://auth.openai.com/codex/device and enter code: 1MRK-O9PDA\nWaiting for authentication...\n",
        );
        assert_eq!(
            parsed.url.as_deref(),
            Some("https://auth.openai.com/codex/device")
        );
        assert_eq!(parsed.user_code.as_deref(), Some("1MRK-O9PDA"));
        assert!(!parsed.needs_code);
    }

    /// A read can end anywhere: inside the link, the device code, a character
    /// or an escape sequence. Nothing is taken until it is whole.
    #[test]
    fn a_link_or_code_cut_across_reads_is_taken_whole() {
        let mut output = Output::default();
        let parsed = output.push(b"Open this URL to sign in:\r\n  https://claude.ai/oauth/auth");
        assert!(parsed.url.is_none(), "half a link is not a link");
        let parsed = output.push(b"orize?code=true&state=abc\r\nPaste the code");
        assert_eq!(
            parsed.url.as_deref(),
            Some("https://claude.ai/oauth/authorize?code=true&state=abc")
        );
        assert!(parsed.needs_code);

        let mut output = Output::default();
        let parsed = output.push(b"Open https://auth.openai.com/codex/device and enter code: 1MRK");
        assert!(parsed.user_code.is_none());
        let parsed = output.push(b"-O9PDA\nWaiting");
        assert_eq!(parsed.user_code.as_deref(), Some("1MRK-O9PDA"));

        // A character and a colour code split between reads.
        let mut output = Output::default();
        let text = "S\u{e9}lect \u{1b}[1mlogin\u{1b}[0m done\n".as_bytes();
        let split = text.iter().position(|byte| *byte == 0xc3).unwrap() + 1;
        output.push(&text[..split]);
        let escape = text.iter().position(|byte| *byte == 0x1b).unwrap() + 2;
        output.push(&text[split..escape]);
        output.push(&text[escape..]);
        assert_eq!(output.text, "S\u{e9}lect login done\n");
    }

    /// Only the newest output is kept, so a prompt after a long spinner is
    /// still seen, and the code mark moves with the text.
    #[test]
    fn the_output_keeps_its_tail() {
        let mut output = Output::default();
        output.push(b"Paste the code: ");
        output.submitted("first");
        let spinner = vec![b'.'; OUTPUT_LIMIT + 10];
        assert!(!output.push(&spinner).needs_code);
        assert!(output.text.len() <= OUTPUT_LIMIT);
        let parsed = output.push(b"\nInvalid code. Paste the code again: ");
        assert!(parsed.needs_code, "a prompt after the code is seen");
        assert!(output.failure().starts_with("Invalid code."));
    }

    #[test]
    fn a_failure_message_leaves_out_tokens_and_pasted_codes() {
        let mut output = Output::default();
        output.push(b"Paste the code: ");
        output.submitted("secret-code#state");
        output.push(b"secret-code#state\nError: invalid_grant\nrefresh token: abc\n");
        assert_eq!(output.failure(), "Error: invalid_grant");
    }

    /// A login Pi writes for the provider, as its fake does.
    const WRITE_LOGIN: &str = r#"mkdir -p "$HOME/.pi/agent"
printf '{"%s":{"type":"oauth","access":"a-%s","refresh":"r","expires":4102444800000,"email":"me@example.com"}}' "$3" "$code" > "$HOME/.pi/agent/auth.json"
"#;

    /// An engine on a fresh database whose Pi is the given script.
    async fn engine_with_pi(name: &str, script: &str, timeout: Duration) -> (Arc<Engine>, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let directory =
            std::env::temp_dir().join(format!("hulahoop-signin-{name}-{}", Uuid::now_v7()));
        let db = crate::db::Db::open(&directory).await.unwrap();
        db.seed_local().await.unwrap();
        let pi = directory.join("fake-pi");
        std::fs::write(&pi, format!("#!/bin/sh\n{script}")).unwrap();
        std::fs::set_permissions(&pi, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut engine = Engine::new(db);
        engine.pi_bin = pi;
        engine.signins.timeout = timeout;
        (Arc::new(engine), directory)
    }

    async fn wait_until_finished(engine: &Engine, id: &ID) -> SignIn {
        for _ in 0..100 {
            let view = status(engine, id).unwrap().unwrap();
            if view.is_finished() {
                return view;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the sign-in did not finish");
    }

    fn scratch_homes_left(directory: &Path) -> usize {
        std::fs::read_dir(directory.join("signin"))
            .map(|entries| entries.count())
            .unwrap_or(0)
    }

    #[tokio::test]
    async fn a_pasted_code_completes_a_claude_sign_in() {
        let script = format!(
            r#"[ "$1 $2 $3 $4" = "auth login anthropic --no-browser" ] || exit 9
[ -e "$HOME/.pi/agent/auth.json" ] && exit 8
echo "Open this URL to sign in:"
echo "  https://claude.ai/oauth/authorize?code=true&state=abc"
printf "Paste the code Anthropic shows after you sign in: "
read code
{WRITE_LOGIN}"#
        );
        let (engine, directory) = engine_with_pi("claude", &script, SIGN_IN_TIMEOUT).await;
        let started = start(&engine, "claude-sub").await.unwrap();
        assert_eq!(started.state, "waiting");
        assert_eq!(
            started.url.as_deref(),
            Some("https://claude.ai/oauth/authorize?code=true&state=abc")
        );
        assert!(started.needs_code);

        let answered = submit_code(&engine, &started.id, "the-code#abc")
            .await
            .unwrap();
        assert_eq!(answered.state, "done", "{:?}", answered.error);
        let connection = engine
            .connections()
            .await
            .unwrap()
            .into_iter()
            .find(|connection| connection.id == "claude-sub")
            .unwrap();
        assert!(connection.connected);
        assert_eq!(connection.label.as_deref(), Some("me@example.com"));
        let entry = crate::connectors::entry("claude-sub").unwrap();
        let oauth = engine.secrets(entry).await.unwrap().unwrap().oauth.unwrap();
        assert!(oauth.contains("a-the-code#abc"));
        assert_eq!(scratch_homes_left(&directory), 0);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn a_device_code_sign_in_shows_the_code_and_finishes_alone() {
        let script = format!(
            r#"echo "Open https://auth.openai.com/codex/device and enter code: 1MRK-O9PDA"
echo "Waiting for authentication..."
sleep 1
code=device
{WRITE_LOGIN}"#
        );
        let (engine, directory) = engine_with_pi("codex", &script, SIGN_IN_TIMEOUT).await;
        let started = start(&engine, "codex-sub").await.unwrap();
        assert_eq!(started.user_code.as_deref(), Some("1MRK-O9PDA"));
        assert_eq!(
            started.url.as_deref(),
            Some("https://auth.openai.com/codex/device")
        );
        assert!(!started.needs_code);

        let finished = wait_until_finished(&engine, &started.id).await;
        assert_eq!(finished.state, "done", "{:?}", finished.error);
        let entry = crate::connectors::entry("codex-sub").unwrap();
        let oauth = engine.secrets(entry).await.unwrap().unwrap().oauth.unwrap();
        assert!(oauth.contains("a-device"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn a_failed_sign_in_reports_pi_and_stores_nothing() {
        let script = "echo 'Open https://auth.openai.com/codex/device and enter code: AB-CD'\necho 'Error: device code expired'\nexit 1\n";
        let (engine, directory) = engine_with_pi("failed", script, SIGN_IN_TIMEOUT).await;
        let started = start(&engine, "codex-sub").await.unwrap();
        let finished = wait_until_finished(&engine, &started.id).await;
        assert_eq!(finished.state, "failed");
        assert_eq!(
            finished.error.as_deref(),
            Some("Error: device code expired")
        );
        let entry = crate::connectors::entry("codex-sub").unwrap();
        assert!(engine.secrets(entry).await.unwrap().is_none());
        assert_eq!(scratch_homes_left(&directory), 0);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn a_sign_in_can_be_cancelled_or_time_out() {
        let script =
            "echo 'Open https://auth.openai.com/codex/device and enter code: AB-CD'\nsleep 30\n";
        let (engine, directory) = engine_with_pi("cancel", script, SIGN_IN_TIMEOUT).await;
        let started = start(&engine, "codex-sub").await.unwrap();
        assert!(cancel(&engine, &started.id).unwrap());
        assert_eq!(
            wait_until_finished(&engine, &started.id).await.state,
            "cancelled"
        );
        assert!(!cancel(&engine, &started.id).unwrap());
        assert!(submit_code(&engine, &started.id, "late").await.is_err());
        let _ = std::fs::remove_dir_all(directory);

        let (engine, directory) =
            engine_with_pi("timeout", script, Duration::from_millis(300)).await;
        let started = start(&engine, "codex-sub").await.unwrap();
        let finished = wait_until_finished(&engine, &started.id).await;
        assert_eq!(finished.state, "failed");
        assert_eq!(finished.error.as_deref(), Some("Sign-in timed out"));
        assert_eq!(scratch_homes_left(&directory), 0);
        let _ = std::fs::remove_dir_all(directory);
    }

    /// Two starts for one connector leave one live sign-in: the older one is
    /// cancelled, and its login is never stored even when Pi finishes it.
    #[tokio::test]
    async fn a_newer_sign_in_replaces_the_older_one() {
        let script = format!(
            r#"echo "Open https://auth.openai.com/codex/device and enter code: AB-CD"
sleep 1
code=$$
{WRITE_LOGIN}"#
        );
        let (engine, directory) = engine_with_pi("replace", &script, SIGN_IN_TIMEOUT).await;
        let (first, second) =
            tokio::join!(start(&engine, "codex-sub"), start(&engine, "codex-sub"));
        let (first, second) = (first.unwrap(), second.unwrap());
        let first = wait_until_finished(&engine, &first.id).await;
        let second = wait_until_finished(&engine, &second.id).await;
        let states = [first.state.as_str(), second.state.as_str()];
        assert!(
            states == ["cancelled", "done"] || states == ["done", "cancelled"],
            "{states:?}"
        );
        assert_eq!(scratch_homes_left(&directory), 0);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn leftover_homes_are_swept_and_new_ones_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let directory = std::env::temp_dir().join(format!("hulahoop-sweep-{}", Uuid::now_v7()));
        let home = scratch_home(&directory, Uuid::now_v7()).unwrap();
        let refresh = private_home(&directory, "refresh").unwrap();
        for dir in [directory.join("signin"), home.clone(), refresh] {
            let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "{}", dir.display());
        }
        std::fs::create_dir_all(home.join(".pi/agent")).unwrap();
        std::fs::write(home.join(".pi/agent/auth.json"), "{}").unwrap();
        sweep(&directory);
        assert_eq!(scratch_homes_left(&directory), 0);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn only_subscription_connectors_sign_in() {
        let (engine, directory) = engine_with_pi("keys", "exit 0\n", SIGN_IN_TIMEOUT).await;
        for connector in ["claude-api", "opencode-go", "github", "pi", "nope"] {
            assert!(start(&engine, connector).await.is_err(), "{connector}");
        }
        assert!(status(&engine, &ID("not-a-uuid".into())).is_err());
        let _ = std::fs::remove_dir_all(directory);
    }
}
