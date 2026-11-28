//! Acceptance tests for user management: setup link, sign-in, registration modes,
//! roles, per-user attribution, personal API tokens and the runner credential.
//!
//! The contract these tests fix (the implementation has to provide it):
//! - `auth::issue_setup_token(&Db)` returns the raw single-use token that the setup
//!   link carries. `GET /setup?token=…` serves an HTML page.
//! - Public JSON endpoints: `POST /auth/setup {token,name,email,password}`,
//!   `POST /auth/login {email,password}`, `POST /auth/signup {name,email,password,invite?}`.
//!   Each success answers `{"token": <bearer session token>}`; a refusal is 4xx.
//! - `POST /auth/invites {role?}` with an owner's bearer answers `{"token": …}`.
//! - GraphQL: `me {id name email role}`, `registration`, `setRegistration(mode)`,
//!   `setMemberRole(workspaceId,userId,role)`, `createApiToken(name) {id token}`,
//!   `revokeApiToken(id)`, and `Issue.events {kind toStatus actor}` where `actor`
//!   is the user's name.
//! - Credentials: the server token is the local owner; session and API tokens are
//!   their user. Anything else is 401, on every route, loopback or not.

use hulahoop_server::api::ApiState;
use hulahoop_server::db::Db;
use hulahoop_server::engine::Engine;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

const SERVER_TOKEN: &str = "server-token-for-tests";

struct Fixture {
    base: String,
    workspace: Uuid,
    engine: Arc<Engine>,
    http: reqwest::Client,
    dir: std::path::PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn fixture() -> Fixture {
    let dir = std::env::temp_dir().join(format!("hulahoop-users-{}", Uuid::now_v7()));
    let db = Db::open(&dir).await.unwrap();
    let workspace = db.seed_local().await.unwrap();
    let engine = Arc::new(Engine::new(db));
    let state = Arc::new(ApiState {
        engine: engine.clone(),
        token: SERVER_TOKEN.into(),
        runner_id_default: "runner-test".into(),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = hulahoop_server::router(state);
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Fixture {
        base,
        workspace,
        engine,
        http: reqwest::Client::new(),
        dir,
    }
}

impl Fixture {
    async fn post(&self, path: &str, bearer: Option<&str>, body: Value) -> (u16, Value) {
        let mut req = self.http.post(format!("{}{path}", self.base)).json(&body);
        if let Some(t) = bearer {
            req = req.bearer_auth(t);
        }
        let res = req.send().await.unwrap();
        let status = res.status().as_u16();
        (status, res.json().await.unwrap_or(Value::Null))
    }

    /// GraphQL as `bearer`: the HTTP status and the JSON body.
    async fn gql(&self, bearer: &str, query: &str) -> (u16, Value) {
        self.post("/graphql", Some(bearer), json!({ "query": query }))
            .await
    }

    /// GraphQL that has to succeed; returns `data`.
    async fn data(&self, bearer: &str, query: &str) -> Value {
        let (status, body) = self.gql(bearer, query).await;
        assert_eq!(status, 200, "{query}: {body}");
        assert!(body["errors"].is_null(), "{query}: {body}");
        body["data"].clone()
    }

    /// GraphQL that has to be refused: an HTTP error or a GraphQL error.
    async fn refused(&self, bearer: &str, query: &str) {
        let (status, body) = self.gql(bearer, query).await;
        assert!(
            status >= 400 || !body["errors"].is_null(),
            "{query} should be refused: {body}"
        );
    }

    /// Runs the one-time setup and returns the owner's session token.
    async fn setup_owner(&self, email: &str) -> String {
        let setup = hulahoop_server::auth::issue_setup_token(&self.engine.db)
            .await
            .unwrap();
        let (status, body) = self
            .post(
                "/auth/setup",
                None,
                json!({"token": setup, "name": "Ada", "email": email, "password": "correct horse battery"}),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        body["token"].as_str().unwrap().to_string()
    }

    async fn set_registration(&self, owner: &str, mode: &str) {
        self.data(
            owner,
            &format!(r#"mutation {{ setRegistration(mode: "{mode}") }}"#),
        )
        .await;
    }

    async fn signup(&self, name: &str, email: &str, invite: Option<&str>) -> (u16, Value) {
        let mut body = json!({"name": name, "email": email, "password": "another long password"});
        if let Some(i) = invite {
            body["invite"] = json!(i);
        }
        self.post("/auth/signup", None, body).await
    }

    async fn create_issue(&self, bearer: &str, title: &str) -> String {
        let w = self.workspace;
        let d = self
            .data(
                bearer,
                &format!(r#"mutation {{ createIssue(workspaceId: "{w}", title: "{title}", body: "") {{ id }} }}"#),
            )
            .await;
        d["createIssue"]["id"].as_str().unwrap().to_string()
    }

    /// An owner plus a second user who signed up while registration was open.
    async fn owner_and_member(&self) -> (String, String) {
        let owner = self.setup_owner("ada@example.com").await;
        self.set_registration(&owner, "open").await;
        let (status, body) = self.signup("Grace", "grace@example.com", None).await;
        assert_eq!(status, 200, "{body}");
        (owner, body["token"].as_str().unwrap().to_string())
    }
}

#[tokio::test]
async fn setup_link_creates_an_owner_who_can_sign_in_and_only_once() {
    let f = fixture().await;
    let setup = hulahoop_server::auth::issue_setup_token(&f.engine.db)
        .await
        .unwrap();
    let page = f
        .http
        .get(format!("{}/setup?token={setup}", f.base))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200);
    assert!(page.text().await.unwrap().to_lowercase().contains("<form"));

    let body = json!({"token": setup, "name": "Ada", "email": "Ada@Example.com", "password": "correct horse battery"});
    let (status, first) = f.post("/auth/setup", None, body.clone()).await;
    assert_eq!(status, 200, "{first}");
    let session = first["token"].as_str().unwrap();
    let d = f.data(session, "{ me { name email role } }").await;
    assert_eq!(d["me"]["name"], "Ada");
    assert_eq!(d["me"]["email"], "ada@example.com");
    assert_eq!(d["me"]["role"], "owner");

    // The link is single use.
    let (again, _) = f.post("/auth/setup", None, body).await;
    assert!(again >= 400);

    // Signing in afterwards works with the password, and not with a wrong one.
    let (ok, login) = f
        .post(
            "/auth/login",
            None,
            json!({"email": "ada@example.com", "password": "correct horse battery"}),
        )
        .await;
    assert_eq!(ok, 200);
    f.data(login["token"].as_str().unwrap(), "{ me { id } }")
        .await;
    let (bad, _) = f
        .post(
            "/auth/login",
            None,
            json!({"email": "ada@example.com", "password": "nope"}),
        )
        .await;
    assert_eq!(bad, 401);
}

#[tokio::test]
async fn setup_takes_over_the_existing_you_user_and_keeps_its_issues() {
    let f = fixture().await;
    let before = f.engine.db.local_user_id().await.unwrap();
    // A comment written before the upgrade, as the local user.
    let issue = f.create_issue(SERVER_TOKEN, "Old work").await;
    f.data(
        SERVER_TOKEN,
        &format!(r#"mutation {{ addComment(issueId: "{issue}", body: "from before") {{ id }} }}"#),
    )
    .await;

    let owner = f.setup_owner("ada@example.com").await;
    let d = f.data(&owner, "{ me { id name } }").await;
    assert_eq!(
        d["me"]["id"],
        before.to_string(),
        "the same user row is reused"
    );
    assert_eq!(d["me"]["name"], "Ada");

    let w = f.workspace;
    let d = f
        .data(
            &owner,
            &format!(r#"{{ issues(workspaceId: "{w}") {{ id title }} workspaceMembers(workspaceId: "{w}") {{ userId name role }} }}"#),
        )
        .await;
    assert!(d["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["id"] == issue));
    let members = d["workspaceMembers"].as_array().unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0]["userId"], before.to_string());
    assert_eq!(members[0]["role"], "owner");
    // The earlier comment is now attributed to the real person, not to `you`.
    let d = f
        .data(
            &owner,
            &format!(r#"{{ issue(id: "{issue}") {{ comments {{ author }} }} }}"#),
        )
        .await;
    assert_eq!(d["issue"]["comments"][0]["author"], "Ada");
}

#[tokio::test]
async fn local_client_with_the_server_token_is_the_owner_without_login() {
    let f = fixture().await;
    let d = f.data(SERVER_TOKEN, "{ me { name role } }").await;
    assert_eq!(d["me"]["role"], "owner");
    // Still true after a real owner has been set up: the machine credential keeps working.
    let owner = f.setup_owner("ada@example.com").await;
    let d = f.data(SERVER_TOKEN, "{ me { name role } }").await;
    assert_eq!(d["me"]["name"], "Ada");
    assert_eq!(d["me"]["role"], "owner");
    assert_ne!(owner, SERVER_TOKEN);
}

#[tokio::test]
async fn closed_registration_refuses_signup_and_an_invite_works_once() {
    let f = fixture().await;
    let owner = f.setup_owner("ada@example.com").await;
    let d = f.data(&owner, "{ registration }").await;
    assert_eq!(d["registration"], "closed", "closed is the default");

    let (status, _) = f.signup("Mallory", "mallory@example.com", None).await;
    assert!(
        status == 403 || status == 401,
        "closed signup refused, got {status}"
    );

    let (status, invite) = f.post("/auth/invites", Some(&owner), json!({})).await;
    assert_eq!(status, 200, "{invite}");
    let token = invite["token"].as_str().unwrap();
    let (status, body) = f.signup("Grace", "grace@example.com", Some(token)).await;
    assert_eq!(status, 200, "{body}");
    let d = f
        .data(body["token"].as_str().unwrap(), "{ me { role } }")
        .await;
    assert_eq!(d["me"]["role"], "member");

    let (again, _) = f.signup("Heidi", "heidi@example.com", Some(token)).await;
    assert!(again >= 400, "an invite link is single use");
}

#[tokio::test]
async fn open_registration_lets_a_new_person_sign_up_as_member() {
    let f = fixture().await;
    let (_owner, member) = f.owner_and_member().await;
    let d = f.data(&member, "{ me { name role } }").await;
    assert_eq!(d["me"]["name"], "Grace");
    assert_eq!(d["me"]["role"], "member");
}

#[tokio::test]
async fn a_member_cannot_change_registration_or_roles_but_an_owner_can() {
    let f = fixture().await;
    let (owner, member) = f.owner_and_member().await;
    let w = f.workspace;
    let grace = f.data(&member, "{ me { id } }").await["me"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    f.refused(&member, r#"mutation { setRegistration(mode: "closed") }"#)
        .await;
    assert_eq!(
        f.data(&owner, "{ registration }").await["registration"],
        "open"
    );
    f.refused(
        &member,
        &format!(
            r#"mutation {{ setMemberRole(workspaceId: "{w}", userId: "{grace}", role: "owner") }}"#
        ),
    )
    .await;
    assert_eq!(
        f.data(&member, "{ me { role } }").await["me"]["role"],
        "member"
    );

    f.data(
        &owner,
        &format!(
            r#"mutation {{ setMemberRole(workspaceId: "{w}", userId: "{grace}", role: "owner") }}"#
        ),
    )
    .await;
    assert_eq!(
        f.data(&member, "{ me { role } }").await["me"]["role"],
        "owner"
    );
    f.set_registration(&owner, "closed").await;
    assert_eq!(
        f.data(&owner, "{ registration }").await["registration"],
        "closed"
    );
    // And the registration setting really is enforced afterwards.
    let (status, _) = f.signup("Mallory", "mallory@example.com", None).await;
    assert!(status >= 400);
}

#[tokio::test]
async fn comments_and_moves_by_a_second_user_show_that_users_name() {
    let f = fixture().await;
    let (owner, member) = f.owner_and_member().await;
    let issue = f.create_issue(&owner, "Shared").await;

    f.data(
        &member,
        &format!(r#"mutation {{ addComment(issueId: "{issue}", body: "hello") {{ id }} }}"#),
    )
    .await;
    f.data(
        &member,
        &format!(r#"mutation {{ planIssue(issueId: "{issue}", planned: true) {{ id }} }}"#),
    )
    .await;
    let d = f
        .data(
            &owner,
            &format!(r#"{{ issue(id: "{issue}") {{ comments {{ author body }} events {{ kind toStatus actor }} }} }}"#),
        )
        .await;
    assert_eq!(d["issue"]["comments"][0]["author"], "Grace");
    let events = d["issue"]["events"].as_array().unwrap();
    assert!(
        events
            .iter()
            .any(|e| e["toStatus"] == "todo" && e["actor"] == "Grace"),
        "{events:?}"
    );
}

#[tokio::test]
async fn personal_api_token_calls_graphql_as_its_user_until_revoked() {
    let f = fixture().await;
    let (_owner, member) = f.owner_and_member().await;
    let d = f
        .data(
            &member,
            r#"mutation { createApiToken(name: "laptop") { id token } }"#,
        )
        .await;
    let token = d["createApiToken"]["token"].as_str().unwrap().to_string();
    let id = d["createApiToken"]["id"].as_str().unwrap().to_string();

    let d = f.data(&token, "{ me { name role } }").await;
    assert_eq!(d["me"]["name"], "Grace");

    // A personal token is a user, not a runner.
    let (status, _) = f
        .post(
            "/internal/runs/claim",
            Some(&token),
            json!({"runner_id": "x"}),
        )
        .await;
    assert_eq!(status, 401);

    f.data(
        &member,
        &format!(r#"mutation {{ revokeApiToken(id: "{id}") }}"#),
    )
    .await;
    let (status, _) = f.gql(&token, "{ me { id } }").await;
    assert_eq!(status, 401, "a revoked token stops working");
}

#[tokio::test]
async fn requests_without_a_valid_credential_get_401() {
    let f = fixture().await;
    let _ = f.setup_owner("ada@example.com").await;
    for bearer in [None, Some("garbage")] {
        let (status, _) = f
            .post(
                "/graphql",
                bearer,
                json!({"query": "{ workspaces { id } }"}),
            )
            .await;
        assert_eq!(status, 401, "graphql with {bearer:?}");
        let (status, _) = f
            .post("/internal/runs/claim", bearer, json!({"runner_id": "x"}))
            .await;
        assert_eq!(status, 401, "claim with {bearer:?}");
    }
    let res = f
        .http
        .get(format!("{}/artifacts/{}", f.base, Uuid::now_v7()))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn runners_still_claim_with_the_server_token() {
    let f = fixture().await;
    let _ = f.setup_owner("ada@example.com").await;
    let (status, body) = f
        .post(
            "/internal/runs/claim",
            Some(SERVER_TOKEN),
            json!({"runner_id": "r1"}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert!(body.is_null(), "no queued run: {body}");
}

#[tokio::test]
async fn a_member_cannot_change_credentials_agents_or_the_workflow() {
    let f = fixture().await;
    let (owner, member) = f.owner_and_member().await;
    let w = f.workspace;
    for mutation in [
        r#"mutation { setGithubToken(token: "x") { __typename } }"#.to_string(),
        r#"mutation { setAgentToken(id: "claude", token: "x") { __typename } }"#.to_string(),
        r#"mutation { clearGithubToken }"#.to_string(),
        r#"mutation { createWorkspace(name: "Mine") { id } }"#.to_string(),
        format!(r#"mutation {{ setWorkflow(workspaceId: "{w}", source: "x") {{ __typename }} }}"#),
        format!(
            r#"mutation {{ createProject(workspaceId: "{w}", name: "P", repos: [{{ name: "r", remoteUrl: "/srv/private" }}]) {{ id }} }}"#
        ),
    ] {
        let (status, body) = f.gql(&member, &mutation).await;
        assert_eq!(status, 200, "{body}");
        let message = body["errors"][0]["message"].as_str().unwrap_or_default();
        assert_eq!(message, "only an owner can do that", "{mutation}: {body}");
    }
    // The owner passes the gate (and may then fail on the mutation itself).
    let (_, body) = f.gql(&owner, r#"mutation { clearGithubToken }"#).await;
    assert!(body["errors"].is_null(), "{body}");
}

#[tokio::test]
async fn signing_out_ends_the_session_and_only_that_one() {
    let f = fixture().await;
    let owner = f.setup_owner("ada@example.com").await;
    let (status, body) = f
        .post(
            "/auth/login",
            None,
            json!({"email": "ada@example.com", "password": "correct horse battery"}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let second = body["token"].as_str().unwrap().to_string();

    let (status, _) = f.post("/auth/logout", Some(&second), json!({})).await;
    assert_eq!(status, 204);
    let (status, _) = f.gql(&second, "{ me { id } }").await;
    assert_eq!(status, 401, "a signed-out session stops working");
    let (status, _) = f.gql(&owner, "{ me { id } }").await;
    assert_eq!(status, 200, "the other session is untouched");
}

#[tokio::test]
async fn removing_a_member_ends_their_session_and_api_tokens() {
    let f = fixture().await;
    let (owner, member) = f.owner_and_member().await;
    let w = f.workspace;
    let d = f
        .data(
            &member,
            r#"mutation { createApiToken(name: "laptop") { token } }"#,
        )
        .await;
    let token = d["createApiToken"]["token"].as_str().unwrap().to_string();
    let grace = f.data(&member, "{ me { id } }").await["me"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    f.data(
        &owner,
        &format!(r#"mutation {{ removeMember(workspaceId: "{w}", userId: "{grace}") }}"#),
    )
    .await;
    for credential in [&member, &token] {
        let (status, _) = f.gql(credential, "{ me { id } }").await;
        assert_eq!(status, 401);
    }
    // And a login still succeeds but gives no access without a membership.
    let (status, body) = f
        .post(
            "/auth/login",
            None,
            json!({"email": "grace@example.com", "password": "another long password"}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let (status, _) = f
        .gql(body["token"].as_str().unwrap(), "{ me { id } }")
        .await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn the_last_owner_cannot_be_demoted_or_removed() {
    let f = fixture().await;
    let owner = f.setup_owner("ada@example.com").await;
    let w = f.workspace;
    let ada = f.data(&owner, "{ me { id } }").await["me"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    f.refused(
        &owner,
        &format!(
            r#"mutation {{ setMemberRole(workspaceId: "{w}", userId: "{ada}", role: "member") }}"#
        ),
    )
    .await;
    f.refused(
        &owner,
        &format!(r#"mutation {{ removeMember(workspaceId: "{w}", userId: "{ada}") }}"#),
    )
    .await;
    // Reaffirming the sole owner as owner changes nothing and is allowed.
    f.data(
        &owner,
        &format!(
            r#"mutation {{ setMemberRole(workspaceId: "{w}", userId: "{ada}", role: "owner") }}"#
        ),
    )
    .await;
    assert_eq!(
        f.data(&owner, "{ me { role } }").await["me"]["role"],
        "owner"
    );
}

#[tokio::test]
async fn an_expired_setup_link_and_invite_are_refused() {
    let f = fixture().await;
    let setup = hulahoop_server::auth::issue_setup_token(&f.engine.db)
        .await
        .unwrap();
    sqlx::query("update meta set value = '1' where key = 'setup_expires_at'")
        .execute(&f.engine.db.pool)
        .await
        .unwrap();
    let (status, _) = f
        .post(
            "/auth/setup",
            None,
            json!({"token": setup, "name": "Ada", "email": "ada@example.com", "password": "correct horse battery"}),
        )
        .await;
    assert_eq!(status, 403);

    let owner = f.setup_owner("ada@example.com").await;
    let (_, invite) = f.post("/auth/invites", Some(&owner), json!({})).await;
    sqlx::query("update invites set expires_at = 1")
        .execute(&f.engine.db.pool)
        .await
        .unwrap();
    let (status, _) = f
        .signup("Late", "late@example.com", invite["token"].as_str())
        .await;
    assert_eq!(status, 403);
}

#[tokio::test]
async fn signup_does_not_say_whether_an_email_is_taken_and_caps_input() {
    let f = fixture().await;
    let (owner, _member) = f.owner_and_member().await;
    let _ = owner;
    let (status, body) = f.signup("Again", "grace@example.com", None).await;
    assert_eq!(status, 403, "{body}");
    let (status, _) = f
        .signup("Long", &format!("{}@x.io", "a".repeat(300)), None)
        .await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn a_member_who_abandons_or_requests_changes_is_named() {
    let f = fixture().await;
    let (owner, member) = f.owner_and_member().await;
    let issue = f.create_issue(&owner, "Doomed").await;
    f.data(
        &member,
        &format!(r#"mutation {{ abandonIssue(issueId: "{issue}", reason: "nope") {{ id }} }}"#),
    )
    .await;
    let d = f
        .data(
            &owner,
            &format!(r#"{{ issue(id: "{issue}") {{ comments {{ author body }} events {{ toStatus actor }} }} }}"#),
        )
        .await;
    let comments = d["issue"]["comments"].as_array().unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c["body"] == "Abandoned: nope" && c["author"] == "Grace"),
        "{comments:?}"
    );
    let events = d["issue"]["events"].as_array().unwrap();
    assert!(events.iter().any(|e| e["actor"] == "Grace"), "{events:?}");
}

#[tokio::test]
async fn login_refuses_an_oversized_password_before_hashing() {
    let f = fixture().await;
    f.setup_owner("ada@example.com").await;
    let (status, _) = f
        .post(
            "/auth/login",
            None,
            json!({"email": "ada@example.com", "password": "x".repeat(1025)}),
        )
        .await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn the_machine_account_cannot_be_removed_or_demoted_even_with_another_owner() {
    let f = fixture().await;
    let (owner, member) = f.owner_and_member().await;
    let w = f.workspace;
    let ada = f.data(&owner, "{ me { id } }").await["me"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let grace = f.data(&member, "{ me { id } }").await["me"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    f.data(
        &owner,
        &format!(
            r#"mutation {{ setMemberRole(workspaceId: "{w}", userId: "{grace}", role: "owner") }}"#
        ),
    )
    .await;

    // Grace is an owner now, but the user the server token acts as stays one.
    f.refused(
        &member,
        &format!(r#"mutation {{ removeMember(workspaceId: "{w}", userId: "{ada}") }}"#),
    )
    .await;
    f.refused(
        &member,
        &format!(
            r#"mutation {{ setMemberRole(workspaceId: "{w}", userId: "{ada}", role: "member") }}"#
        ),
    )
    .await;
    assert_eq!(
        f.data(SERVER_TOKEN, "{ me { role } }").await["me"]["role"],
        "owner"
    );
}

#[tokio::test]
async fn the_command_line_creates_the_first_owner_once() {
    let f = fixture().await;
    let db = &f.engine.db;
    let created =
        hulahoop_server::auth::create_owner(db, "Ada", "ada@example.com", "correct horse battery")
            .await;
    assert!(created.is_ok(), "{created:?}");
    let (status, body) = f
        .post(
            "/auth/login",
            None,
            json!({"email": "ada@example.com", "password": "correct horse battery"}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let session = body["token"].as_str().unwrap();
    assert_eq!(
        f.data(session, "{ me { role } }").await["me"]["role"],
        "owner"
    );

    // With an owner in place there is nothing left to take over.
    let again =
        hulahoop_server::auth::create_owner(db, "Eve", "eve@example.com", "another long password")
            .await;
    assert!(again.is_err());
    // A weak password is refused before the setup link is spent.
    assert!(
        hulahoop_server::auth::create_owner(db, "Eve", "eve@example.com", "short")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn the_setup_page_is_not_cached_or_leaked_through_the_referrer() {
    let f = fixture().await;
    let token = hulahoop_server::auth::issue_setup_token(&f.engine.db)
        .await
        .unwrap();
    let response = f
        .http
        .get(format!("{}/setup?token={token}", f.base))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["referrer-policy"], "no-referrer");
}

#[tokio::test]
async fn simultaneous_signups_with_one_email_get_one_account_and_no_server_error() {
    let f = fixture().await;
    let owner = f.setup_owner("ada@example.com").await;
    f.set_registration(&owner, "open").await;
    let (a, b) = tokio::join!(
        f.signup("Grace", "grace@example.com", None),
        f.signup("Grace", "grace@example.com", None)
    );
    let mut statuses = [a.0, b.0];
    statuses.sort();
    assert_eq!(statuses, [200, 403], "{a:?} {b:?}");
}

#[tokio::test]
async fn an_api_token_name_is_capped() {
    let f = fixture().await;
    let owner = f.setup_owner("ada@example.com").await;
    let long = "n".repeat(201);
    f.refused(
        &owner,
        &format!(r#"mutation {{ createApiToken(name: "{long}") {{ id }} }}"#),
    )
    .await;
}
