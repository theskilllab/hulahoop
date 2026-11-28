//! Who is calling. Every request resolves to a user: the server token is the
//! local owner (the machine credential, no login), a session token is the person
//! who signed in, and a personal API token is its owner. Runners keep the server
//! token and never reach this module's user routes.
//!
//! Library choice: no Rust crate covers what BetterAuth does, and the
//! session-cookie stack (`axum-login`, `tower-sessions`) assumes browser cookies,
//! while the desktop and a future MCP client present a bearer token. Sessions are
//! therefore random bearer tokens kept hashed in SQLite, and passwords are
//! PBKDF2-HMAC-SHA256 from `ring`, which the server already ships. Argon2 and
//! `oauth2`/`openidconnect` can slot in behind `hash_password` and `resolve`.

use crate::api::{same_token, ApiState};
use crate::db::{sha256_hex, Db};
use crate::engine::Engine;
use anyhow::Context;
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rand::RngCore;
use ring::pbkdf2;
use serde::Deserialize;
use serde_json::json;
use std::num::NonZeroU32;
use std::sync::Arc;
use uuid::Uuid;

const PBKDF2_ITERATIONS: u32 = 600_000;
const SETUP_TTL_SECONDS: i64 = 30 * 60;
const INVITE_TTL_SECONDS: i64 = 7 * 24 * 60 * 60;
const SESSION_TTL_SECONDS: i64 = 30 * 24 * 60 * 60;
const MIN_PASSWORD_LENGTH: usize = 8;
// Caps keep an unauthenticated form from handing the KDF or the database an
// unbounded value.
const MAX_PASSWORD_LENGTH: usize = 1024;
const MAX_NAME_LENGTH: usize = 200;
const MAX_EMAIL_LENGTH: usize = 254;

/// A well-formed hash nobody's password matches. Verifying against it when an
/// email is unknown makes a failed sign-in cost the same as one with a wrong
/// password.
const DUMMY_HASH: &str = "pbkdf2-sha256$600000$00000000000000000000000000000000$0000000000000000000000000000000000000000000000000000000000000000";

pub const REGISTRATION_MODES: [&str; 3] = ["closed", "invite", "open"];
pub const ROLES: [&str; 2] = ["owner", "member"];

/// The user a request acts as, put into the GraphQL context and the request
/// extensions once its bearer has been checked.
#[derive(Debug, Clone, Copy)]
pub struct CurrentUser {
    pub id: Uuid,
}

/// Why an auth call failed, in the HTTP status a client can act on.
#[derive(Debug)]
pub enum AuthError {
    Invalid(String),
    Unauthorized,
    Forbidden(String),
    Internal(anyhow::Error),
}

impl<E: Into<anyhow::Error>> From<E> for AuthError {
    fn from(error: E) -> Self {
        Self::Internal(error.into())
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::Invalid(message) => (StatusCode::BAD_REQUEST, message),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".to_string()),
            Self::Forbidden(message) => (StatusCode::FORBIDDEN, message),
            Self::Internal(error) => {
                tracing::error!(%error, "auth request failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error".into())
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

/// Resolves a bearer token to a user.
pub struct Authenticator {
    engine: Arc<Engine>,
    server_token: String,
}

impl Authenticator {
    pub fn new(engine: Arc<Engine>, server_token: String) -> Self {
        Self {
            engine,
            server_token,
        }
    }

    /// Whether `bearer` is the machine credential. Runners need exactly this.
    pub fn is_server_token(&self, bearer: &str) -> bool {
        same_token(&self.server_token, bearer)
    }

    pub async fn resolve(&self, bearer: &str) -> anyhow::Result<Option<CurrentUser>> {
        if bearer.is_empty() {
            return Ok(None);
        }
        let db = &self.engine.db;
        if self.is_server_token(bearer) {
            return Ok(Some(CurrentUser {
                id: db.local_user_id().await?,
            }));
        }
        let hash = sha256_hex(bearer.as_bytes());
        // A person who is no longer in any workspace has no access, whatever
        // credential they still hold.
        let session: Option<Uuid> = sqlx::query_scalar(
            "select s.user_id from sessions s \
             where s.token_hash = ? and s.expires_at > ? \
             and exists (select 1 from workspace_members m where m.user_id = s.user_id)",
        )
        .bind(&hash)
        .bind(unix_now())
        .fetch_optional(&db.pool)
        .await?;
        let user = match session {
            Some(id) => Some(id),
            None => {
                sqlx::query_scalar(
                    "select t.user_id from api_tokens t \
                     where t.token_hash = ? and t.revoked_at is null \
                     and exists (select 1 from workspace_members m where m.user_id = t.user_id)",
                )
                .bind(&hash)
                .fetch_optional(&db.pool)
                .await?
            }
        };
        Ok(user.map(|id| CurrentUser { id }))
    }
}

pub fn bearer(headers: &HeaderMap) -> &str {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default()
}

fn unix_now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn new_token(prefix: &str) -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    format!("{prefix}{}", hex::encode(bytes))
}

// --- passwords -------------------------------------------------------------

fn hash_password(password: &str) -> String {
    let mut salt = [0u8; 16];
    rand::rng().fill_bytes(&mut salt);
    let mut hash = [0u8; 32];
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(PBKDF2_ITERATIONS).expect("non-zero"),
        &salt,
        password.as_bytes(),
        &mut hash,
    );
    format!(
        "pbkdf2-sha256${PBKDF2_ITERATIONS}${}${}",
        hex::encode(salt),
        hex::encode(hash)
    )
}

/// The KDF is CPU-bound for a good fraction of a second, so it runs off the
/// async workers that serve claims, heartbeats and the log socket.
async fn hash_password_off_thread(password: String) -> anyhow::Result<String> {
    Ok(tokio::task::spawn_blocking(move || hash_password(&password)).await?)
}

async fn verify_password_off_thread(password: String, stored: String) -> anyhow::Result<bool> {
    Ok(tokio::task::spawn_blocking(move || verify_password(&password, &stored)).await?)
}

fn verify_password(password: &str, stored: &str) -> bool {
    let mut parts = stored.split('$');
    let (Some("pbkdf2-sha256"), Some(iterations), Some(salt), Some(hash), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return false;
    };
    let (Some(iterations), Ok(salt), Ok(hash)) = (
        iterations.parse().ok().and_then(NonZeroU32::new),
        hex::decode(salt),
        hex::decode(hash),
    ) else {
        return false;
    };
    pbkdf2::verify(
        pbkdf2::PBKDF2_HMAC_SHA256,
        iterations,
        &salt,
        password.as_bytes(),
        &hash,
    )
    .is_ok()
}

// --- the person a form describes ------------------------------------------

struct Profile {
    name: String,
    email: String,
    password: String,
}

fn profile(name: &str, email: &str, password: &str) -> Result<Profile, AuthError> {
    let name = name.trim();
    let email = email.trim().to_lowercase();
    if name.is_empty() {
        return Err(AuthError::Invalid("a name is required".into()));
    }
    if name.chars().count() > MAX_NAME_LENGTH || email.len() > MAX_EMAIL_LENGTH {
        return Err(AuthError::Invalid("that name or email is too long".into()));
    }
    if !email.contains('@') {
        return Err(AuthError::Invalid("that is not an email address".into()));
    }
    if password.chars().count() < MIN_PASSWORD_LENGTH {
        return Err(AuthError::Invalid(format!(
            "a password needs at least {MIN_PASSWORD_LENGTH} characters"
        )));
    }
    if password.chars().count() > MAX_PASSWORD_LENGTH {
        return Err(AuthError::Invalid(format!(
            "a password can have at most {MAX_PASSWORD_LENGTH} characters"
        )));
    }
    Ok(Profile {
        name: name.to_string(),
        email,
        password: password.to_string(),
    })
}

// --- setup link ------------------------------------------------------------

/// True until somebody has a password: the server is waiting for its owner.
pub async fn needs_setup(db: &Db) -> anyhow::Result<bool> {
    let with_password: i64 =
        sqlx::query_scalar("select count(*) from users where password_hash is not null")
            .fetch_one(&db.pool)
            .await?;
    Ok(with_password == 0)
}

/// Mints the single-use setup token (replacing any earlier one) and returns it.
/// Only its hash is stored; the link carries the raw value.
pub async fn issue_setup_token(db: &Db) -> anyhow::Result<String> {
    let token = new_token("hsetup_");
    set_meta(db, "setup_token", &sha256_hex(token.as_bytes())).await?;
    set_meta(
        db,
        "setup_expires_at",
        &(unix_now() + SETUP_TTL_SECONDS).to_string(),
    )
    .await?;
    Ok(token)
}

async fn set_meta(db: &Db, key: &str, value: &str) -> anyhow::Result<()> {
    sqlx::query("insert into meta (key, value) values (?, ?) on conflict(key) do update set value = excluded.value")
        .bind(key)
        .bind(value)
        .execute(&db.pool)
        .await?;
    Ok(())
}

async fn get_meta(db: &Db, key: &str) -> anyhow::Result<Option<String>> {
    Ok(sqlx::query_scalar("select value from meta where key = ?")
        .bind(key)
        .fetch_optional(&db.pool)
        .await?)
}

async fn setup_token_is_live(db: &Db, token: &str) -> anyhow::Result<bool> {
    let stored = get_meta(db, "setup_token").await?;
    let expires: i64 = get_meta(db, "setup_expires_at")
        .await?
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(0);
    Ok(needs_setup(db).await?
        && expires > unix_now()
        && stored.is_some_and(|hash| same_token(&hash, &sha256_hex(token.as_bytes()))))
}

/// Spends the setup token: the local `you` user becomes the first owner, with a
/// name, email and password, and keeps its id so its issues and comments stay.
async fn complete_setup(
    db: &Db,
    token: &str,
    person: Profile,
) -> Result<(Uuid, String), AuthError> {
    if !setup_token_is_live(db, token).await? {
        return Err(AuthError::Forbidden(
            "the setup link is invalid, used or expired".into(),
        ));
    }
    // Deleting the stored hash is the single-use latch: only one request wins it.
    let spent = sqlx::query("delete from meta where key = 'setup_token' and value = ?")
        .bind(sha256_hex(token.as_bytes()))
        .execute(&db.pool)
        .await?;
    if spent.rows_affected() != 1 {
        return Err(AuthError::Forbidden("the setup link was just used".into()));
    }
    let user = db.local_user_id().await?;
    sqlx::query("update users set name = ?, email = ?, password_hash = ? where id = ?")
        .bind(&person.name)
        .bind(&person.email)
        .bind(hash_password_off_thread(person.password.clone()).await?)
        .bind(user)
        .execute(&db.pool)
        .await?;
    let session = create_session(db, user).await?;
    Ok((user, session))
}

// --- sessions, sign-in, sign-up --------------------------------------------

async fn create_session(db: &Db, user: Uuid) -> anyhow::Result<String> {
    let token = new_token("hs_");
    sqlx::query(
        "insert into sessions (token_hash, user_id, created_at, expires_at) values (?, ?, ?, ?)",
    )
    .bind(sha256_hex(token.as_bytes()))
    .bind(user)
    .bind(crate::db::now_rfc3339())
    .bind(unix_now() + SESSION_TTL_SECONDS)
    .execute(&db.pool)
    .await?;
    Ok(token)
}

async fn login(db: &Db, email: &str, password: &str) -> Result<String, AuthError> {
    if email.len() > MAX_EMAIL_LENGTH || password.chars().count() > MAX_PASSWORD_LENGTH {
        return Err(AuthError::Invalid(
            "that email or password is too long".into(),
        ));
    }
    let row: Option<(Uuid, Option<String>)> =
        sqlx::query_as("select id, password_hash from users where email = ?")
            .bind(email.trim().to_lowercase())
            .fetch_optional(&db.pool)
            .await?;
    let stored = row
        .as_ref()
        .and_then(|(_, hash)| hash.clone())
        .unwrap_or_else(|| DUMMY_HASH.to_string());
    let verified = verify_password_off_thread(password.to_string(), stored).await?;
    match row {
        Some((id, _)) if verified => Ok(create_session(db, id).await?),
        _ => Err(AuthError::Unauthorized),
    }
}

pub async fn registration(db: &Db) -> anyhow::Result<String> {
    Ok(get_meta(db, "registration")
        .await?
        .unwrap_or_else(|| "closed".into()))
}

pub async fn set_registration(db: &Db, mode: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        REGISTRATION_MODES.contains(&mode),
        "registration is one of closed, invite or open"
    );
    set_meta(db, "registration", mode).await
}

/// The workspace new people join. A server has one workspace in practice; the
/// oldest is the one every existing client already shows.
pub async fn primary_workspace(db: &Db) -> anyhow::Result<Uuid> {
    sqlx::query_scalar("select id from workspaces order by created_at, id limit 1")
        .fetch_one(&db.pool)
        .await
        .context("the server has no workspace")
}

pub async fn role_in(db: &Db, user: Uuid, workspace: Uuid) -> anyhow::Result<Option<String>> {
    Ok(sqlx::query_scalar(
        "select role from workspace_members where workspace_id = ? and user_id = ?",
    )
    .bind(workspace)
    .bind(user)
    .fetch_optional(&db.pool)
    .await?)
}

/// Fails unless `user` owns `workspace`.
pub async fn require_owner(db: &Db, user: Uuid, workspace: Uuid) -> Result<(), AuthError> {
    match role_in(db, user, workspace).await?.as_deref() {
        Some("owner") => Ok(()),
        _ => Err(AuthError::Forbidden("only an owner can do that".into())),
    }
}

async fn signup(db: &Db, person: Profile, invite: Option<&str>) -> Result<String, AuthError> {
    let workspace = primary_workspace(db).await?;
    let open = registration(db).await? == "open";
    let mut tx = db.pool.begin().await?;
    let role = match invite {
        Some(invite) => spend_invite(&mut tx, invite).await?,
        None if open => "member".to_string(),
        None => return Err(AuthError::Forbidden("registration is closed".into())),
    };
    let taken: Option<Uuid> = sqlx::query_scalar("select id from users where email = ?")
        .bind(&person.email)
        .fetch_optional(&mut *tx)
        .await?;
    if taken.is_some() {
        // The same answer a closed door gives, so the form does not say who
        // already has an account.
        return Err(AuthError::Forbidden(
            "could not sign up with those details".into(),
        ));
    }
    let id = Uuid::now_v7();
    let password_hash = hash_password_off_thread(person.password.clone()).await?;
    sqlx::query(
        "insert into users (id, name, email, password_hash, created_at) values (?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(&person.name)
    .bind(&person.email)
    .bind(password_hash)
    .bind(crate::db::now_rfc3339())
    .execute(&mut *tx)
    .await?;
    sqlx::query("insert into workspace_members (workspace_id, user_id, role) values (?, ?, ?)")
        .bind(workspace)
        .bind(id)
        .bind(&role)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(create_session(db, id).await?)
}

// --- invites ---------------------------------------------------------------

/// A single-use invite link token; only an owner may mint one.
pub async fn create_invite(db: &Db, owner: Uuid, role: &str) -> Result<String, AuthError> {
    require_owner(db, owner, primary_workspace(db).await?).await?;
    if !ROLES.contains(&role) {
        return Err(AuthError::Invalid("a role is owner or member".into()));
    }
    let token = new_token("hinv_");
    sqlx::query(
        "insert into invites (token_hash, role, created_by, created_at, expires_at) values (?, ?, ?, ?, ?)",
    )
    .bind(sha256_hex(token.as_bytes()))
    .bind(role)
    .bind(owner)
    .bind(crate::db::now_rfc3339())
    .bind(unix_now() + INVITE_TTL_SECONDS)
    .execute(&db.pool)
    .await?;
    Ok(token)
}

/// Marks an invite used and returns the role it grants. The `used_at is null`
/// condition makes the spend atomic, so two sign-ups cannot share one invite.
async fn spend_invite(tx: &mut sqlx::SqliteConnection, invite: &str) -> Result<String, AuthError> {
    let hash = sha256_hex(invite.as_bytes());
    let role: Option<String> = sqlx::query_scalar(
        "update invites set used_at = ? where token_hash = ? and used_at is null and expires_at > ? returning role",
    )
    .bind(crate::db::now_rfc3339())
    .bind(&hash)
    .bind(unix_now())
    .fetch_optional(&mut *tx)
    .await?;
    role.ok_or_else(|| AuthError::Forbidden("the invite is invalid, used or expired".into()))
}

// --- members and tokens ----------------------------------------------------

/// Changes a member's role. A workspace never loses its last owner; making an
/// owner an owner again is a no-op, not a refusal.
pub async fn set_member_role(
    db: &Db,
    actor: Uuid,
    workspace: Uuid,
    user: Uuid,
    role: &str,
) -> Result<(), AuthError> {
    require_owner(db, actor, workspace).await?;
    if !ROLES.contains(&role) {
        return Err(AuthError::Invalid("a role is owner or member".into()));
    }
    let changed = sqlx::query(&format!(
        "update workspace_members set role = ? where workspace_id = ? and user_id = ? \
         and (? = 'owner' or {NOT_LAST_OWNER})"
    ))
    .bind(role)
    .bind(workspace)
    .bind(user)
    .bind(role)
    .bind(workspace)
    .bind(user)
    .execute(&db.pool)
    .await?;
    if changed.rows_affected() == 0 {
        return Err(refused_change(db, workspace, user).await?);
    }
    Ok(())
}

pub async fn remove_member(
    db: &Db,
    actor: Uuid,
    workspace: Uuid,
    user: Uuid,
) -> Result<(), AuthError> {
    require_owner(db, actor, workspace).await?;
    let removed = sqlx::query(&format!(
        "delete from workspace_members where workspace_id = ? and user_id = ? and {NOT_LAST_OWNER}"
    ))
    .bind(workspace)
    .bind(user)
    .bind(workspace)
    .bind(user)
    .execute(&db.pool)
    .await?;
    if removed.rows_affected() == 0 {
        return Err(refused_change(db, workspace, user).await?);
    }
    revoke_credentials(db, user).await?;
    Ok(())
}

/// Guards a role change or removal in the same statement that performs it, so
/// two concurrent demotions cannot both see "another owner exists". Binds
/// `workspace` and `user` again, after the statement's own.
const NOT_LAST_OWNER: &str = "(role != 'owner' or exists (select 1 from workspace_members \
     where workspace_id = ? and role = 'owner' and user_id != ?))";

/// Explains why a guarded change touched no row.
async fn refused_change(db: &Db, workspace: Uuid, user: Uuid) -> anyhow::Result<AuthError> {
    let is_member: bool = sqlx::query_scalar(
        "select exists (select 1 from workspace_members where workspace_id = ? and user_id = ?)",
    )
    .bind(workspace)
    .bind(user)
    .fetch_one(&db.pool)
    .await?;
    Ok(if is_member {
        AuthError::Forbidden("a workspace needs at least one owner".into())
    } else {
        AuthError::Invalid("that person is not a member".into())
    })
}

/// Ends every session and personal token of `user`, so leaving the workspace
/// takes effect now and not when a credential happens to expire.
async fn revoke_credentials(db: &Db, user: Uuid) -> anyhow::Result<()> {
    sqlx::query("delete from sessions where user_id = ?")
        .bind(user)
        .execute(&db.pool)
        .await?;
    sqlx::query("update api_tokens set revoked_at = ? where user_id = ? and revoked_at is null")
        .bind(crate::db::now_rfc3339())
        .bind(user)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// Signs out: forgets the session `bearer` names. Anything else changes nothing.
pub async fn sign_out(db: &Db, bearer: &str) -> anyhow::Result<()> {
    sqlx::query("delete from sessions where token_hash = ?")
        .bind(sha256_hex(bearer.as_bytes()))
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// Drops sessions past their expiry, so the table does not grow for ever.
pub async fn purge_expired_sessions(db: &Db) -> anyhow::Result<u64> {
    let purged = sqlx::query("delete from sessions where expires_at <= ?")
        .bind(unix_now())
        .execute(&db.pool)
        .await?;
    Ok(purged.rows_affected())
}

/// Creates a personal API token; the raw value is shown once, only its hash is kept.
pub async fn create_api_token(
    db: &Db,
    user: Uuid,
    name: &str,
) -> Result<(Uuid, String), AuthError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AuthError::Invalid("a token needs a name".into()));
    }
    let id = Uuid::now_v7();
    let token = new_token("hat_");
    sqlx::query(
        "insert into api_tokens (id, user_id, name, token_hash, created_at) values (?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(user)
    .bind(name)
    .bind(sha256_hex(token.as_bytes()))
    .bind(crate::db::now_rfc3339())
    .execute(&db.pool)
    .await?;
    Ok((id, token))
}

/// Revokes one of the user's own tokens; someone else's id changes nothing.
pub async fn revoke_api_token(db: &Db, user: Uuid, id: Uuid) -> Result<bool, AuthError> {
    let revoked = sqlx::query(
        "update api_tokens set revoked_at = ? where id = ? and user_id = ? and revoked_at is null",
    )
    .bind(crate::db::now_rfc3339())
    .bind(id)
    .bind(user)
    .execute(&db.pool)
    .await?;
    Ok(revoked.rows_affected() == 1)
}

// --- HTTP ------------------------------------------------------------------

/// The public sign-in routes, and the owner-only invite route.
pub fn router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/setup", get(setup_page))
        .route("/auth/setup", post(setup))
        .route("/auth/login", post(login_route))
        .route("/auth/signup", post(signup_route))
        .route("/auth/logout", post(logout_route))
        .route("/auth/invites", post(invite_route))
        .with_state(state)
}

#[derive(Deserialize)]
struct SetupQuery {
    token: String,
}

#[derive(Deserialize)]
struct SetupBody {
    token: String,
    name: String,
    email: String,
    password: String,
}

#[derive(Deserialize)]
struct LoginBody {
    email: String,
    password: String,
}

#[derive(Deserialize)]
struct SignupBody {
    name: String,
    email: String,
    password: String,
    invite: Option<String>,
}

#[derive(Deserialize)]
struct InviteBody {
    role: Option<String>,
}

fn session_response(token: String) -> Response {
    Json(json!({ "token": token })).into_response()
}

async fn setup_page(
    State(state): State<Arc<ApiState>>,
    Query(query): Query<SetupQuery>,
) -> Result<Html<String>, AuthError> {
    if !setup_token_is_live(&state.engine.db, &query.token).await? {
        return Err(AuthError::Forbidden(
            "the setup link is invalid, used or expired".into(),
        ));
    }
    Ok(Html(
        SETUP_PAGE.replace("__TOKEN__", &html_escape(&query.token)),
    ))
}

async fn setup(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<SetupBody>,
) -> Result<Response, AuthError> {
    let person = profile(&body.name, &body.email, &body.password)?;
    let (_, session) = complete_setup(&state.engine.db, &body.token, person).await?;
    Ok(session_response(session))
}

async fn login_route(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<LoginBody>,
) -> Result<Response, AuthError> {
    Ok(session_response(
        login(&state.engine.db, &body.email, &body.password).await?,
    ))
}

async fn signup_route(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<SignupBody>,
) -> Result<Response, AuthError> {
    let person = profile(&body.name, &body.email, &body.password)?;
    Ok(session_response(
        signup(&state.engine.db, person, body.invite.as_deref()).await?,
    ))
}

async fn logout_route(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<StatusCode, AuthError> {
    sign_out(&state.engine.db, bearer(&headers)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn invite_route(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(body): Json<InviteBody>,
) -> Result<Response, AuthError> {
    let authenticator = Authenticator::new(state.engine.clone(), state.token.clone());
    let user = authenticator
        .resolve(bearer(&headers))
        .await?
        .ok_or(AuthError::Unauthorized)?;
    let role = body.role.as_deref().unwrap_or("member");
    let token = create_invite(&state.engine.db, user.id, role).await?;
    Ok(session_response(token))
}

fn html_escape(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

const SETUP_PAGE: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>Set up Hulahoop</title></head>
<body>
<h1>Set up Hulahoop</h1>
<p>Create the owner of this server.</p>
<form id="setup">
  <input type="hidden" name="token" value="__TOKEN__">
  <p><label>Name <input name="name" required></label></p>
  <p><label>Email <input name="email" type="email" required></label></p>
  <p><label>Password <input name="password" type="password" minlength="8" required></label></p>
  <button type="submit">Create owner</button>
</form>
<p id="result"></p>
<script>
document.getElementById("setup").addEventListener("submit", async (event) => {
  event.preventDefault();
  const body = Object.fromEntries(new FormData(event.target));
  const response = await fetch("/auth/setup", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  document.getElementById("result").textContent = response.ok
    ? "Done. Sign in from the Hulahoop app."
    : "Could not set up: " + ((await response.json()).error || response.status);
});
</script>
</body></html>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_verifies_only_against_its_own_hash() {
        let stored = hash_password("correct horse");
        assert!(verify_password("correct horse", &stored));
        assert!(!verify_password("wrong horse", &stored));
        assert!(!verify_password("correct horse", "garbage"));
    }

    #[test]
    fn the_dummy_hash_is_well_formed_and_matches_nothing() {
        assert!(!verify_password("", DUMMY_HASH));
        assert!(!verify_password("anything at all", DUMMY_HASH));
        assert!(DUMMY_HASH.starts_with(&format!("pbkdf2-sha256${PBKDF2_ITERATIONS}$")));
    }

    #[test]
    fn a_profile_is_validated_and_the_email_normalised() {
        let person = profile(" Ada ", " Ada@Example.COM ", "long enough").unwrap();
        assert_eq!(
            (person.name.as_str(), person.email.as_str()),
            ("Ada", "ada@example.com")
        );
        assert!(profile("", "a@b.c", "long enough").is_err());
        assert!(profile("Ada", "nope", "long enough").is_err());
        assert!(profile("Ada", "a@b.c", "short").is_err());
        assert!(profile("Ada", "a@b.c", &"x".repeat(MAX_PASSWORD_LENGTH + 1)).is_err());
        assert!(profile(&"n".repeat(MAX_NAME_LENGTH + 1), "a@b.c", "long enough").is_err());
        assert!(profile(
            "Ada",
            &format!("{}@b.c", "e".repeat(MAX_EMAIL_LENGTH)),
            "long enough"
        )
        .is_err());
    }
}
