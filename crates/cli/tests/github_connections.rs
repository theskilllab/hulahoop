//! Acceptance tests for the GitHub connection: the app is stored encrypted and
//! listed, the loop mints it a token that only the app's own key can vouch for,
//! each role authors its commit under its own name, and a stored key that cannot
//! produce a signature is reported as not connected.
//!
//! Every criterion drives the real server and runner over their public surfaces
//! (GraphQL, the run spec, the working copy), so the base revision fails here
//! because the connection does not exist yet, not because a helper is missing.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// A value that cannot appear by accident in a SQLite page.
const APP_ID: &str = "271828182845";

/// A real, complete 2048-bit RSA key: it can sign, so the app it belongs to is
/// connected.
const PRIVATE_KEY: &str = "-----BEGIN RSA PRIVATE KEY-----
MIIEpAIBAAKCAQEAr0Kzi8yfE49NHxfclF8l+fscRnWVb0irZx0WLG5Y+lReU5b4
6noosgqBl9ddadI5sOlhWewk9CEWXgUxISA6qUjn+WNNbFHqlofL7vZa5g6zZonq
6X651L45LVlXi1vplzJ3LsIkD9Y0a0bNktHC04X6jswkk9UHcUqdBL5FWZtgTmue
/FYwZEyu2xVYiCPbDvApnS+knHCGjGw2eRoskc76m4u/E22Xce3SwATc0DBt4DG4
8ZubP6PW2rbA41EVpSGXr+9fno5hZrVJfc3mF2+nHi5uGi0YR+5Et9MY/p19TisT
4QYEoSqEf3XOJIM5V7+SioBqK7r3dJW2KZTnTwIDAQABAoIBACeoSrNoDljhNLzD
6/xA2XmK2jeRVMqVIX/YI5PTNlwSqjN09ocsNDgxS2i96VvQH5DKBmbQGf6J3CNT
Pm41+kAjCKa0UykfEhuUx88fPKtRCUQ7EgZa0Bb1IkACNt6IYj99wl04ves9d6gM
oejGgTTW3L7iL6ap7iCzFvujSFMBhvxqJh8PYuOjYa+r9ySLzwv90613MbSU5Rjs
gZvPcKqVlJB7VTa/gFBULxev9njDUybth7a0Mrvaa15CdJq/OwRi/G3OmqrC7oLI
utEdXjAdC1fdoDjAKh49atZLG+LuLeHwzPkiZ4nC+O7KxL8U3+fFt8B6lWku5eDE
GcHDqBkCgYEA405mlnQ6Y/30ai3CDCJ+AZpT2QH7TXKXylf9f7Tytax7trGqcTX7
mj2cHi5olDrVYm+J7vMPOfW1pvwUyu7T1EZqnaIUk49vwdlV8rX3u+UDHJqfwQi6
LiPuMMfDbIjuoE+XfF++TvqaSu3VwQsQ6Q2Crx5OF/RV++DG1OA/vBkCgYEAxWJm
BjTKBDFpf+nqXoFxIrbKQdL934Ssb33d9JZ5yG2RftSspiMszGEU9WbbpOsws0Yt
TDrrDzYk/iqtwO/tFGyNB/i5bar8Cn36eF5FVSW5Hrej1r4u7U9dM4IHTE4DBefJ
TokwShvN2ZvH600e840ctb3cizSIOU5/fNPRK6cCgYAL7Jr5HZPKlxHxWAEr/cs6
bkwjwDTctTw9QyxrPKTW3vuGc/+K/Nf8GWtNMgYOcJ+tEEjeHiYYVkl3PxYgJzM5
mZASQORvcUDdnHcjymP6mWL2Zy4mRDzwGrZuHKylzvElu8cooSUso3zgnWVvQRNc
BndqlF5oZ0rzcbE8jZ8KUQKBgQCVdendPzHNNQE225AdWZSqfZDvgO29x7+ueLeN
0jeLHfRy/mVxM+djJwvwup8M5Q4uYbBBLcf577zZPMXu934k3gF4jM0UDsnkw/na
FUAYtoTTjqigHch454SiUQ5nqBa/7QBQ2K7ZHu6jnUMnmHEcaLjZoVeTjQatphQB
GH8ltwKBgQCyErmb02Z/JiQVd/9n3nkITDSfB0oz++LBltPXIYps00+kSGSZqc+y
2tHY7DN/e52Nnw50tCbkXPqbLIBSx0LGu7COSyoT7JJFIykpuw7PQSVMkG7KTKAc
gerU/uBGgcEkg/OmBjuFYTwt9IjFkJ5LEmcYTk5c6mZ59ARsKA8ldg==
-----END RSA PRIVATE KEY-----";

/// The same key with its DER body cut short: a structurally broken key that
/// still has valid PEM syntax, but cannot produce a signature.
fn unable_to_sign_key() -> String {
    let mut lines = PRIVATE_KEY.lines();
    let begin = lines.next().expect("pem begin");
    let first = lines.next().expect("pem body");
    let second = lines.next().expect("pem body");
    format!("{begin}\n{first}\n{second}\n-----END RSA PRIVATE KEY-----")
}

/// The fake adapter: one script for every role. It records the environment the
/// loop handed it, so a token the loop mints is visible to the test even when the
/// loop only passes it in-process.
const FAKE_AGENT: &str = r#"#!/usr/bin/env bash
set -uo pipefail
cd "$HULAHOOP_CHECKOUT"
env > "$HULAHOOP_ARTIFACT_DIR/agent-env.txt" 2>/dev/null || true
case "$HULAHOOP_ROLE" in
  test_author)
    printf '\ndef test_ac1_subtracts():\n    from calc import subtract\n    assert subtract(2, 1) == 1\n' > tests.py
    ;;
  implementer)
    printf '\ndef subtract(a, b):\n    return a - b\n' >> calc.py
    ;;
  reviewer)
    echo "HULAHOOP_VERDICT: approved"
    ;;
esac
jj st >/dev/null 2>&1 || true
jj commit -m checkpoint >/dev/null 2>&1 || true
exit 0
"#;

const JUSTFILE: &str = "setup:\n\t@true\n\ntest:\n\t@python3 run_tests.py\n";

const RUN_TESTS: &str = r#"import pathlib
import xml.etree.ElementTree as ET

ok = "def subtract" in pathlib.Path("calc.py").read_text()
suite = ET.Element("testsuite", name="subtract", tests="1", failures="0" if ok else "1")
case = ET.SubElement(suite, "testcase", classname="calc", name="test_ac1_subtracts")
if not ok:
    ET.SubElement(case, "failure", message="subtract is missing").text = "AssertionError: subtract is missing"
pathlib.Path(".hulahoop").mkdir(exist_ok=True)
ET.ElementTree(suite).write(".hulahoop/junit.xml", encoding="utf-8", xml_declaration=True)
raise SystemExit(0 if ok else 1)
"#;

/// `run` finds its data dir through process-wide environment variables, so one
/// fixture at a time may launch a runner; the guard rides along with the fixture
/// that set those variables.
static RUNNER_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Fixture {
    _root: PathBuf,
    _runner_env: Option<std::sync::MutexGuard<'static, ()>>,
    data: PathBuf,
    repo: PathBuf,
    url: String,
    token: String,
    client: reqwest::Client,
    workspace: String,
    project: String,
    repo_id: String,
}

impl Fixture {
    async fn start(tag: &str, with_runner: bool) -> Fixture {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let runner_env = if with_runner {
            Some(
                RUNNER_ENV
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            )
        } else {
            None
        };
        let root = std::env::temp_dir().join(format!(
            "hulahoop-github-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        let data = root.join("data");
        std::fs::create_dir_all(&repo).expect("create fixture repo");
        std::fs::create_dir_all(&data).expect("create data dir");
        write_fixture_repo(&repo);

        let fake = root.join("fake-agent.sh");
        std::fs::write(&fake, FAKE_AGENT).expect("write fake agent");
        make_executable(&fake);
        std::env::set_var("HULAHOOP_FAKE_SCRIPT", &fake);

        let serve_data = data.clone();
        tokio::spawn(async move {
            let _ = hulahoop_server::serve(vec![
                "--data-dir".to_string(),
                serve_data.to_string_lossy().into_owned(),
                "--port".to_string(),
                "0".to_string(),
                "--listen".to_string(),
                "127.0.0.1".to_string(),
            ])
            .await;
        });

        let connection = wait_for_connection(&data);
        let url = connection["url"]
            .as_str()
            .expect("connection url")
            .to_string();
        let token = connection["token"]
            .as_str()
            .expect("connection token")
            .to_string();

        if with_runner {
            // `run` derives its data dir from HULAHOOP_DATA/XDG_DATA_HOME before
            // parsing --data-dir, and the direnv allow-list follows that value;
            // pin it so the test's data dir is the one the runner uses.
            std::env::set_var("HULAHOOP_DATA", &data);
            let run_data = data.clone();
            tokio::spawn(async move {
                let _ = hulahoop_runner::run(vec![
                    "--data-dir".to_string(),
                    run_data.to_string_lossy().into_owned(),
                ])
                .await;
            });
        }

        let mut fixture = Fixture {
            _root: root,
            _runner_env: runner_env,
            data,
            repo,
            url,
            token,
            client: reqwest::Client::new(),
            workspace: String::new(),
            project: String::new(),
            repo_id: String::new(),
        };
        fixture.workspace = fixture.gql("{ workspaces { id } }", json!({})).await["workspaces"][0]
            ["id"]
            .as_str()
            .expect("seeded workspace")
            .to_string();
        fixture
    }

    async fn gql(&self, query: &str, variables: Value) -> Value {
        let response = self
            .client
            .post(format!("{}/graphql", self.url))
            .bearer_auth(&self.token)
            .json(&json!({ "query": query, "variables": variables }))
            .send()
            .await
            .unwrap_or_else(|error| panic!("graphql request failed: {error}\nquery: {query}"));
        let status = response.status();
        let body = response.text().await.expect("graphql body");
        let value: Value = serde_json::from_str(&body).unwrap_or_else(|error| {
            panic!("graphql returned non-json ({status}): {error}: {body}")
        });
        if let Some(errors) = value.get("errors") {
            panic!("graphql error for {query}: {errors}");
        }
        value["data"].clone()
    }

    async fn create_project(&mut self) {
        let data = self
            .gql(
                r#"mutation ($workspace: UUID!) {
                    createProject(workspaceId: $workspace, name: "Github project") { id }
                }"#,
                json!({ "workspace": self.workspace }),
            )
            .await;
        self.project = data["createProject"]["id"]
            .as_str()
            .expect("created project id")
            .to_string();
    }

    async fn add_repo(&mut self) {
        let data = self
            .gql(
                r#"mutation ($project: UUID!, $path: String!) {
                    addRepo(projectId: $project, name: "github-fixture", path: $path) { id }
                }"#,
                json!({ "project": self.project, "path": self.repo.to_string_lossy() }),
            )
            .await;
        self.repo_id = data["addRepo"]["id"]
            .as_str()
            .expect("added repo id")
            .to_string();
    }

    async fn create_app(&self, private_key: &str) {
        self.gql(
            r#"mutation ($appId: String!, $privateKey: String!) {
                createGithubApp(appId: $appId, privateKey: $privateKey) { __typename }
            }"#,
            json!({ "appId": APP_ID, "privateKey": private_key }),
        )
        .await;
    }

    async fn connections(&self) -> Vec<Value> {
        self.gql("{ connections { provider connected } }", json!({}))
            .await["connections"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    async fn github_connection(&self) -> Value {
        let connections = self.connections().await;
        connections
            .iter()
            .find(|entry| {
                entry["provider"]
                    .as_str()
                    .is_some_and(|provider| provider.eq_ignore_ascii_case("github"))
            })
            .cloned()
            .unwrap_or_else(|| panic!("connections should list github, got {connections:?}"))
    }

    /// Prepare a project with three bound, preflighted fake agents and a ready
    /// issue whose suite the fake test author can author.
    async fn prepare_issue(&mut self) -> String {
        self.create_project().await;
        self.add_repo().await;

        let mut agents = Vec::new();
        for (role, name) in [
            ("test_author", "fixture-test-author"),
            ("implementer", "fixture-implementer"),
            ("reviewer", "fixture-reviewer"),
        ] {
            let data = self
                .gql(
                    r#"mutation ($workspace: UUID!, $name: String!) {
                        createAgent(workspaceId: $workspace, name: $name, adapter: "fake", model: "fixture-model") { id }
                    }"#,
                    json!({ "workspace": self.workspace, "name": name }),
                )
                .await;
            agents.push((
                role,
                data["createAgent"]["id"].as_str().unwrap().to_string(),
            ));
        }

        let bindings: Vec<Value> = agents
            .iter()
            .map(|(role, id)| json!({ "role": role, "agentId": id }))
            .collect();
        self.gql(
            r#"mutation ($project: UUID!, $bindings: [RoleBinding!]!) {
                setProjectAgents(projectId: $project, bindings: $bindings) { role agentId }
            }"#,
            json!({ "project": self.project, "bindings": bindings }),
        )
        .await;

        for (_, id) in &agents {
            self.gql(
                r#"mutation ($agent: UUID!, $repo: UUID!) {
                    runPreflight(agentId: $agent, repoId: $repo)
                }"#,
                json!({ "agent": id, "repo": self.repo_id }),
            )
            .await;
        }
        self.wait_for_preflight().await;

        let data = self
            .gql(
                r#"mutation ($project: UUID!, $repo: UUID!) {
                    createIssue(
                        projectId: $project,
                        repoId: $repo,
                        title: "subtract",
                        body: "Add a subtract function.",
                        criteria: ["subtract(2, 1) returns 1"]
                    ) { id }
                }"#,
                json!({ "project": self.project, "repo": self.repo_id }),
            )
            .await;
        let issue = data["createIssue"]["id"]
            .as_str()
            .expect("created issue id")
            .to_string();
        self.gql(
            r#"mutation ($issue: UUID!) { approveCriteria(issueId: $issue) { id } }"#,
            json!({ "issue": issue }),
        )
        .await;
        issue
    }

    async fn wait_for_preflight(&self) {
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            let data = self
                .gql(
                    r#"query ($project: UUID!) { preflight(projectId: $project) { ok detail } }"#,
                    json!({ "project": self.project }),
                )
                .await;
            let rows = data["preflight"].as_array().cloned().unwrap_or_default();
            if rows.len() == 3 && rows.iter().all(|row| row["ok"].as_bool() == Some(true)) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "preflight did not pass within 90s: {data}"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    async fn issue_state(&self, issue: &str) -> Value {
        self.gql(
            r#"query ($issue: UUID!) {
                issue(id: $issue) {
                    status
                    runs { id role status artifactDir }
                    replays { kind status }
                }
            }"#,
            json!({ "issue": issue }),
        )
        .await["issue"]
            .clone()
    }

    /// Drive a test-author run, wait for its base replay, freeze the suite, and
    /// return the authoring run id.
    async fn author_suite(&self, issue: &str) -> String {
        self.gql(
            r#"mutation ($issue: UUID!) { startRun(issueId: $issue, role: "test_author") { id } }"#,
            json!({ "issue": issue }),
        )
        .await;
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let state = self.issue_state(issue).await;
            let author = state["runs"]
                .as_array()
                .and_then(|runs| {
                    runs.iter()
                        .find(|run| run["role"] == "test_author" && run["status"] == "succeeded")
                })
                .map(|run| run["id"].as_str().unwrap().to_string());
            let base = state["replays"]
                .as_array()
                .and_then(|replays| replays.iter().find(|replay| replay["kind"] == "base"))
                .and_then(|replay| replay["status"].as_str())
                .map(str::to_string);
            if let Some(run_id) = author {
                if matches!(base.as_deref(), Some("succeeded") | Some("failed")) {
                    self.gql(
                        r#"mutation ($issue: UUID!) { freezeSuite(issueId: $issue) { status } }"#,
                        json!({ "issue": issue }),
                    )
                    .await;
                    return run_id;
                }
            }
            assert!(
                Instant::now() < deadline,
                "test-author run/replay did not settle within 120s: {state}"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// Run the implementer until the issue is in review.
    async fn implement(&self, issue: &str) {
        self.gql(
            r#"mutation ($issue: UUID!) { startRun(issueId: $issue, role: "implementer") { id } }"#,
            json!({ "issue": issue }),
        )
        .await;
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let state = self.issue_state(issue).await;
            match state["status"].as_str() {
                Some("in_review") => return,
                Some("blocked") | Some("cancelled") | Some("done") => {
                    panic!("implementer did not reach review: {state}")
                }
                _ => {}
            }
            assert!(
                Instant::now() < deadline,
                "implementer run did not settle within 120s: {state}"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// The authors of the run's commit and of its parent, with `<unset>` marking
    /// a commit that carries no identity at all.
    fn commit_authors(&self, bookmark: &str) -> Vec<String> {
        let output = Command::new("jj")
            .arg("-R")
            .arg(&self.repo)
            .args([
                "log",
                "-r",
                &format!("{bookmark} | {bookmark}-"),
                "--no-graph",
                "-T",
                r#"author.name() ++ "|" ++ author.email() ++ "|" ++ committer.email() ++ "\n""#,
            ])
            .output()
            .expect("run jj log");
        assert!(
            output.status.success(),
            "jj log failed for {bookmark}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| {
                let name = line.split('|').next().unwrap_or_default();
                if name.is_empty() {
                    "<unset>".to_string()
                } else {
                    name.to_string()
                }
            })
            .collect()
    }

    /// Everything the implementer run left behind, keyed by file path.
    fn run_artifacts(&self, run: &Value) -> Vec<(String, String)> {
        let dir = PathBuf::from(
            run["artifactDir"]
                .as_str()
                .expect("a run exposes its artifact dir"),
        );
        let mut files = Vec::new();
        collect_files(&dir, &mut files);
        assert!(
            !files.is_empty(),
            "the run should leave artifacts under {}",
            dir.display()
        );
        files
            .into_iter()
            .filter_map(|file| {
                std::fs::read(&file)
                    .ok()
                    .map(|bytes| (file.display().to_string(), String::from_utf8_lossy(&bytes).into_owned()))
            })
            .collect()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ac1_connections_query_lists_github_after_app_created() {
    let fixture = Fixture::start("ac1", false).await;
    fixture.create_app(PRIVATE_KEY).await;
    let connections = fixture.connections().await;
    let github = connections
        .iter()
        .filter(|entry| {
            entry["provider"]
                .as_str()
                .is_some_and(|provider| provider.eq_ignore_ascii_case("github"))
        })
        .count();
    assert_eq!(
        github, 1,
        "connections should list github exactly once after the app is created, got {connections:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ac2_app_id_and_private_key_are_stored_encrypted() {
    let fixture = Fixture::start("ac2", false).await;
    fixture.create_app(PRIVATE_KEY).await;

    let connections = fixture.connections().await;
    assert!(
        connections.iter().any(|entry| entry["provider"]
            .as_str()
            .is_some_and(|provider| provider.eq_ignore_ascii_case("github"))),
        "the github app should be stored and listed, got {connections:?}"
    );

    // Byte sequences that would show up on disk if the row were plaintext: the
    // app id, and the key both whole and in slices (line wrapping and column
    // layout are storage details, the base64 body is not).
    let body: String = PRIVATE_KEY
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect();
    let slices = [&body[..40], &body[200..240], &body[600..640]];

    let mut files = Vec::new();
    collect_files(&fixture.data, &mut files);
    let mut bytes = Vec::new();
    for file in files {
        if let Ok(contents) = std::fs::read(&file) {
            bytes.extend_from_slice(&contents);
        }
    }
    let stored = String::from_utf8_lossy(&bytes);
    assert!(
        !stored.contains(APP_ID),
        "the github app id is stored as plaintext under {}",
        fixture.data.display()
    );
    assert!(
        !stored.contains(&body),
        "the github private key body is stored as plaintext under {}",
        fixture.data.display()
    );
    for slice in slices {
        assert!(
            !stored.contains(slice),
            "the github private key is stored as plaintext under {} (found {slice:?})",
            fixture.data.display()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ac3_the_minted_github_token_verifies_against_the_app_key() {
    let mut fixture = Fixture::start("ac3", true).await;
    let issue = fixture.prepare_issue().await;
    fixture.author_suite(&issue).await;
    fixture.create_app(PRIVATE_KEY).await;
    fixture.implement(&issue).await;

    let state = fixture.issue_state(&issue).await;
    let implementer = state["runs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|run| run["role"] == "implementer" && run["status"] == "succeeded")
        .unwrap_or_else(|| panic!("the implementer run should have succeeded: {state}"));

    // The token the loop mints for the app has to reach the implementer, so the
    // run's own artifacts (including the environment the loop handed it) are
    // where the test reads it back. Anything the loop keeps for itself has to
    // surface somewhere under the data directory for the same reason.
    let mut artifacts = fixture.run_artifacts(&implementer);
    if !artifacts
        .iter()
        .any(|(_, text)| !jwt_tokens(text).is_empty())
    {
        let mut files = Vec::new();
        collect_files(&fixture.data, &mut files);
        artifacts.extend(files.into_iter().filter_map(|file| {
            std::fs::read(&file)
                .ok()
                .map(|bytes| (file.display().to_string(), String::from_utf8_lossy(&bytes).into_owned()))
        }));
    }
    let tokens: Vec<(String, String)> = artifacts
        .iter()
        .flat_map(|(source, text)| {
            jwt_tokens(text)
                .into_iter()
                .map(move |token| (source.clone(), token))
        })
        .collect();
    assert!(
        !tokens.is_empty(),
        "no JWT-shaped token appears in the implementer run's artifacts ({}) or under {}; the \
         loop should mint a token for the github app and hand it to the implementer",
        implementer["artifactDir"],
        fixture.data.display()
    );

    let work = std::env::temp_dir().join(format!("hulahoop-ac3-key-{}", std::process::id()));
    let mut rejected = Vec::new();
    for (source, token) in &tokens {
        match verify_rsa_signature(token, PRIVATE_KEY, &work) {
            Ok(()) => return,
            Err(reason) => rejected.push(format!("{source}: {reason}")),
        }
    }
    panic!(
        "no token in the implementer run verifies against the public key of the stored github \
         app key; rejected: {rejected:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ac4_test_author_commit_author_is_hulahoop_test_author() {
    let mut fixture = Fixture::start("ac4", true).await;
    let issue = fixture.prepare_issue().await;
    let run_id = fixture.author_suite(&issue).await;

    let names = fixture.commit_authors(&format!("tests/{issue}/cand/{run_id}"));
    assert!(
        names.iter().any(|name| name == "hulahoop-test-author"),
        "the test author commit author should be hulahoop-test-author, got {names:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ac5_implementer_commit_author_is_hulahoop_implementer() {
    let mut fixture = Fixture::start("ac5", true).await;
    let issue = fixture.prepare_issue().await;
    fixture.author_suite(&issue).await;
    fixture.implement(&issue).await;

    let names = fixture.commit_authors(&format!("issue/{issue}/head/1"));
    assert!(
        names.iter().any(|name| name == "hulahoop-implementer"),
        "the implementer commit author should be hulahoop-implementer, got {names:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ac6_connections_show_github_not_connected_when_the_stored_key_cannot_sign() {
    // A key that can sign is reported as connected, so the flag carries the
    // signability of what is stored rather than a constant.
    let signable = Fixture::start("ac6-signable", false).await;
    signable.create_app(PRIVATE_KEY).await;
    let github = signable.github_connection().await;
    assert_eq!(
        github["connected"].as_bool(),
        Some(true),
        "a stored key that can sign should be connected, got {github}"
    );

    let broken = Fixture::start("ac6-broken", false).await;
    broken.create_app(&unable_to_sign_key()).await;
    let github = broken.github_connection().await;
    assert_eq!(
        github["connected"].as_bool(),
        Some(false),
        "a stored key that cannot sign should be listed as not connected, got {github}"
    );
}

// ---------------------------------------------------------------- JWT helpers

/// Every `header.payload.signature` run in `text`, where the header decodes to an
/// object. The scan is deliberately about shape, so the loop is free to name the
/// variable or the file it ships the token in.
fn jwt_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let flush = |candidate: &mut String, out: &mut Vec<String>| {
        let parts: Vec<&str> = candidate.split('.').collect();
        if parts.len() == 3
            && parts[0].starts_with("eyJ")
            && parts[1].len() > 8
            && parts[2].len() > 16
            && base64url_decode(parts[0])
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .and_then(|header| header.get("alg").cloned())
                .is_some()
        {
            out.push(candidate.clone());
        }
        candidate.clear();
    };
    for character in text.chars() {
        if character.is_ascii_alphanumeric() || character == '-' || character == '_' || character == '.'
        {
            current.push(character);
        } else {
            flush(&mut current, &mut tokens);
        }
    }
    flush(&mut current, &mut tokens);
    tokens.sort();
    tokens.dedup();
    tokens
}

fn base64url_decode(input: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut accumulator: u32 = 0;
    let mut bits: u32 = 0;
    for character in input.chars() {
        let value = match character {
            'A'..='Z' => character as u32 - 'A' as u32,
            'a'..='z' => character as u32 - 'a' as u32 + 26,
            '0'..='9' => character as u32 - '0' as u32 + 52,
            '-' => 62,
            '_' => 63,
            '=' => continue,
            other => return Err(format!("`{other}` is not base64url")),
        };
        accumulator = (accumulator << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((accumulator >> bits) as u8);
        }
    }
    Ok(out)
}

/// RSASSA-PKCS1-v1_5 over the JWT signing input, with the public half of the
/// stored app key. `openssl` is the only verifier in the dev shell, and the
/// product already depends on it for TLS.
fn verify_rsa_signature(token: &str, private_key: &str, work: &Path) -> Result<(), String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err("a JWT has three dot-separated parts".into());
    }
    let header = base64url_decode(parts[0])?;
    let header: Value = serde_json::from_slice(&header).map_err(|error| error.to_string())?;
    let digest = match header["alg"].as_str().unwrap_or_default() {
        "RS256" => "sha256",
        "RS384" => "sha384",
        "RS512" => "sha512",
        other => return Err(format!("`{other}` is not an RSA signature over the app key")),
    };
    let signature = base64url_decode(parts[2])?;

    std::fs::create_dir_all(work).map_err(|error| error.to_string())?;
    let key_path = work.join("app-key.pem");
    let public_path = work.join("app-public.pem");
    let signature_path = work.join("signature.bin");
    let input_path = work.join("signing-input.txt");
    std::fs::write(&key_path, private_key).map_err(|error| error.to_string())?;
    std::fs::write(&signature_path, &signature).map_err(|error| error.to_string())?;
    std::fs::write(&input_path, format!("{}.{}", parts[0], parts[1]))
        .map_err(|error| error.to_string())?;

    let public = Command::new("openssl")
        .args(["rsa", "-in"])
        .arg(&key_path)
        .args(["-pubout", "-out"])
        .arg(&public_path)
        .output()
        .map_err(|error| format!("running openssl: {error}"))?;
    if !public.status.success() {
        return Err(format!(
            "the app key has no public half: {}",
            String::from_utf8_lossy(&public.stderr)
        ));
    }

    let verified = Command::new("openssl")
        .arg("dgst")
        .arg(format!("-{digest}"))
        .arg("-verify")
        .arg(&public_path)
        .arg("-signature")
        .arg(&signature_path)
        .arg(&input_path)
        .output()
        .map_err(|error| format!("running openssl: {error}"))?;
    if verified.status.success() {
        Ok(())
    } else {
        Err("the signature does not verify against the app's public key".to_string())
    }
}

fn write_fixture_repo(repo: &Path) {
    std::fs::write(repo.join(".envrc"), "export HULAHOOP_FIXTURE=1\n").unwrap();
    std::fs::write(repo.join("flake.nix"), "{ }\n").unwrap();
    std::fs::write(repo.join("justfile"), JUSTFILE).unwrap();
    std::fs::write(repo.join("run_tests.py"), RUN_TESTS).unwrap();
    std::fs::write(repo.join("calc.py"), "def add(a, b):\n    return a + b\n").unwrap();
    std::fs::create_dir_all(repo.join(".hulahoop")).unwrap();

    let git = |args: &[&str]| {
        let output = Command::new("git")
            .current_dir(repo)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["add", "-A"]);
    git(&[
        "-c",
        "user.name=Fixture Base",
        "-c",
        "user.email=base@example.com",
        "commit",
        "-q",
        "-m",
        "base",
    ]);

    let jj = |args: &[&str]| {
        let output = Command::new("jj")
            .current_dir(repo)
            .args(args)
            .output()
            .expect("run jj");
        assert!(
            output.status.success(),
            "jj {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    jj(&["git", "init", "--colocate"]);
    jj(&["bookmark", "set", "main", "-r", "@-"]);
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

fn wait_for_connection(data: &Path) -> Value {
    let path = data.join("connection.json");
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(contents) = std::fs::read(&path) {
            if let Ok(value) = serde_json::from_slice::<Value>(&contents) {
                if value.get("url").and_then(Value::as_str).is_some() {
                    return value;
                }
            }
        }
        assert!(
            Instant::now() < deadline,
            "server did not write {} within 60s",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn collect_files(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out);
        } else {
            out.push(path);
        }
    }
}
