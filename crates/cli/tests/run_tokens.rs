//! Acceptance tests for the tokens a run spent.
//!
//! Every criterion drives the real server over its public surfaces: GraphQL
//! creates the project, repository, agent and issue; the internal API records
//! the preflight, starts nothing extra, and completes the run with the stream a
//! runner would have written. The stream is pi's JSONL shape, where `usage` sits
//! on every `message_update` and is cumulative within one assistant message.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

struct Fixture {
    _root: PathBuf,
    url: String,
    token: String,
    client: reqwest::Client,
    workspace: String,
    project: String,
    repo_id: String,
}

impl Fixture {
    async fn start(tag: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!(
            "hulahoop-tokens-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        let data = root.join("data");
        std::fs::create_dir_all(&repo).expect("create fixture repo");
        std::fs::create_dir_all(&data).expect("create data dir");
        std::fs::write(repo.join("flake.nix"), "{ }\n").expect("write flake.nix");
        std::fs::write(repo.join("justfile"), "test:\n\t@true\n").expect("write justfile");

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

        let mut fixture = Fixture {
            _root: root,
            url,
            token,
            client: reqwest::Client::new(),
            workspace: String::new(),
            project: String::new(),
            repo_id: String::new(),
        };
        fixture.workspace = fixture
            .gql("{ workspaces { id } }", json!({}))
            .await["workspaces"][0]["id"]
            .as_str()
            .expect("seeded workspace")
            .to_string();
        fixture.create_project_and_repo().await;
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

    async fn post(&self, path: &str, body: Value) -> Value {
        let response = self
            .client
            .post(format!("{}{path}", self.url))
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .unwrap_or_else(|error| panic!("POST {path} failed: {error}"));
        let status = response.status();
        let text = response.text().await.expect("response body");
        assert!(
            status.is_success(),
            "POST {path} returned {status}: {text}"
        );
        serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("POST {path} returned non-json: {error}: {text}"))
    }

    async fn create_project_and_repo(&mut self) {
        let data = self
            .gql(
                r#"mutation ($workspace: UUID!) {
                    createProject(workspaceId: $workspace, name: "Tokens project") { id }
                }"#,
                json!({ "workspace": self.workspace }),
            )
            .await;
        self.project = data["createProject"]["id"]
            .as_str()
            .expect("created project id")
            .to_string();

        let data = self
            .gql(
                r#"mutation ($project: UUID!, $path: String!) {
                    addRepo(projectId: $project, name: "tokens-fixture", path: $path) { id }
                }"#,
                json!({
                    "project": self.project,
                    "path": self._root.join("repo").to_string_lossy(),
                }),
            )
            .await;
        self.repo_id = data["addRepo"]["id"]
            .as_str()
            .expect("added repo id")
            .to_string();
    }

    /// A project, a repository, one bound fake agent, a green preflight for it,
    /// and an issue whose criteria a test author may start on.
    async fn prepare_issue(&self) -> String {
        let data = self
            .gql(
                r#"mutation ($workspace: UUID!) {
                    createAgent(
                        workspaceId: $workspace,
                        name: "fixture-test-author",
                        adapter: "fake",
                        model: "fixture-model"
                    ) { id }
                }"#,
                json!({ "workspace": self.workspace }),
            )
            .await;
        let agent = data["createAgent"]["id"]
            .as_str()
            .expect("created agent id")
            .to_string();

        self.gql(
            r#"mutation ($project: UUID!, $bindings: [RoleBinding!]!) {
                setProjectAgents(projectId: $project, bindings: $bindings) { role agentId }
            }"#,
            json!({
                "project": self.project,
                "bindings": [{ "role": "test_author", "agentId": agent }],
            }),
        )
        .await;

        // No runner process in these tests, so the jobs the server queues are
        // claimed and answered here, which is exactly what the runner would do.
        self.gql(
            r#"mutation ($agent: UUID!, $repo: UUID!) {
                runPreflight(agentId: $agent, repoId: $repo)
            }"#,
            json!({ "agent": agent, "repo": self.repo_id }),
        )
        .await;
        self.drain_jobs().await;

        let data = self
            .gql(
                r#"mutation ($project: UUID!, $repo: UUID!) {
                    createIssue(
                        projectId: $project,
                        repoId: $repo,
                        title: "tokens",
                        body: "Record what the run spent.",
                        criteria: ["a run reports the tokens it spent"]
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

    async fn drain_jobs(&self) {
        for _ in 0..8 {
            let envelope = self.post("/internal/jobs/claim", json!({})).await;
            if envelope.is_null() {
                return;
            }
            let job = &envelope["job"];
            let id = job["id"].as_str().expect("claimed job id");
            let kind = job["kind"].as_str().expect("claimed job kind");
            let payload: Value = job["payload"]
                .as_str()
                .and_then(|raw| serde_json::from_str(raw).ok())
                .unwrap_or_else(|| json!({}));
            let result = if kind == "preflight" {
                json!({
                    "agent_id": payload["agent_id"],
                    "ok": true,
                    "detail": "the fixture answered the probe",
                })
            } else {
                json!({})
            };
            self.post(&format!("/internal/jobs/{id}/complete"), json!({ "result": result }))
                .await;
        }
        panic!("jobs kept appearing; the server never ran dry");
    }

    async fn start_author_run(&self, issue: &str) -> Value {
        self.gql(
            r#"mutation ($issue: UUID!) {
                startRun(issueId: $issue, role: "test_author") { id artifactDir }
            }"#,
            json!({ "issue": issue }),
        )
        .await["startRun"]
            .clone()
    }

    /// Write the stream where a runner writes it and complete the run as one
    /// would, then read the stored row back.
    async fn finish_run(&self, issue: &str, run: &Value, stream: &str) -> Value {
        let artifact_dir = PathBuf::from(
            run["artifactDir"]
                .as_str()
                .expect("a run exposes its artifact dir"),
        );
        std::fs::write(artifact_dir.join("agent.jsonl"), stream).expect("write agent.jsonl");
        let id = run["id"].as_str().expect("run id");
        let completed = self
            .post(
                &format!("/internal/runs/{id}/complete"),
                json!({
                    "status": "succeeded",
                    "exit_code": 0,
                    "diff": "",
                    "changed_paths": ["tests.py"],
                    "transcript": stream,
                }),
            )
            .await;
        assert_eq!(
            completed["run"]["id"].as_str(),
            Some(id),
            "the completion was refused: {completed}"
        );
        self.run_row(issue, id).await
    }

    async fn run_row(&self, issue: &str, run: &str) -> Value {
        let data = self
            .gql(
                r#"query ($issue: UUID!) {
                    issue(id: $issue) {
                        runs { id status tokensIn tokensOut }
                    }
                }"#,
                json!({ "issue": issue }),
            )
            .await;
        data["issue"]["runs"]
            .as_array()
            .and_then(|runs| runs.iter().find(|row| row["id"] == run).cloned())
            .unwrap_or_else(|| panic!("run {run} missing from {data}"))
    }
}

fn usage(input: i64, output: i64) -> Value {
    json!({
        "input": input,
        "output": output,
        "cacheRead": 0,
        "cacheWrite": 0,
        "totalTokens": input + output,
        "cost": {
            "input": 0.0,
            "output": 0.0,
            "cacheRead": 0.0,
            "cacheWrite": 0.0,
            "total": 0.0,
        },
    })
}

fn update(input: i64, output: i64, delta: &str) -> Value {
    json!({
        "type": "message_update",
        "usage": usage(input, output),
        "assistantMessageEvent": {
            "type": "text_delta",
            "contentIndex": 0,
            "delta": delta,
        },
    })
}

fn message_end(input: i64, output: i64) -> Value {
    json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [{ "type": "text", "text": "done" }],
            "usage": usage(input, output),
            "stopReason": "stop",
            "timestamp": 1733234402000u64,
        },
    })
}

fn transcript(events: Vec<Value>) -> String {
    let mut out = String::new();
    for event in events {
        out.push_str(&event.to_string());
        out.push('\n');
    }
    out
}

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_ac1_run_stores_tokens_summed_over_finished_messages() {
    let fixture = Fixture::start("ac1").await;
    let issue = fixture.prepare_issue().await;
    let run = fixture.start_author_run(&issue).await;

    // Two finished assistant messages. The first ends at 120/25, the second at
    // 350/60, so the run spent 470 input and 85 output tokens.
    let stream = transcript(vec![
        json!({ "type": "session", "version": 3, "id": "fixture", "timestamp": "2026-11-20T07:00:00.000Z", "cwd": "/tmp" }),
        json!({ "type": "agent_start" }),
        json!({ "type": "turn_start" }),
        json!({ "type": "message_start", "message": { "role": "assistant", "content": [], "timestamp": 1733234400000u64 } }),
        update(100, 10, "first "),
        update(120, 25, "message"),
        message_end(120, 25),
        json!({ "type": "turn_end", "message": { "role": "assistant" }, "toolResults": [] }),
        json!({ "type": "turn_start" }),
        json!({ "type": "message_start", "message": { "role": "assistant", "content": [], "timestamp": 1733234401000u64 } }),
        update(300, 40, "second "),
        update(350, 60, "message"),
        message_end(350, 60),
        json!({ "type": "turn_end", "message": { "role": "assistant" }, "toolResults": [] }),
        json!({ "type": "agent_end", "messages": [], "willRetry": false }),
        json!({ "type": "agent_settled", "aborted": false }),
    ]);

    let row = fixture.finish_run(&issue, &run, &stream).await;

    assert_eq!(
        row["tokensIn"].as_i64(),
        Some(470),
        "input tokens should be the sum of each finished message's final usage, got {row}"
    );
    assert_eq!(
        row["tokensOut"].as_i64(),
        Some(85),
        "output tokens should be the sum of each finished message's final usage, got {row}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_ac2_repeated_usage_counts_once_at_final_value() {
    let fixture = Fixture::start("ac2").await;
    let issue = fixture.prepare_issue().await;
    let run = fixture.start_author_run(&issue).await;

    // One message whose cumulative usage repeats on later updates. Counting the
    // repeated snapshots would add up to 460/70; the message cost 180/30.
    let stream = transcript(vec![
        json!({ "type": "session", "version": 3, "id": "fixture", "timestamp": "2026-11-20T07:00:00.000Z", "cwd": "/tmp" }),
        json!({ "type": "agent_start" }),
        json!({ "type": "turn_start" }),
        json!({ "type": "message_start", "message": { "role": "assistant", "content": [], "timestamp": 1733234400000u64 } }),
        update(100, 10, "one "),
        update(180, 30, "message "),
        update(180, 30, "that "),
        update(180, 30, "streams"),
        message_end(180, 30),
        json!({ "type": "turn_end", "message": { "role": "assistant" }, "toolResults": [] }),
        json!({ "type": "agent_end", "messages": [], "willRetry": false }),
    ]);

    let row = fixture.finish_run(&issue, &run, &stream).await;

    assert_eq!(
        row["tokensIn"].as_i64(),
        Some(180),
        "a repeated cumulative usage is one message's cost, not a sum of snapshots: {row}"
    );
    assert_eq!(
        row["tokensOut"].as_i64(),
        Some(30),
        "a repeated cumulative usage is one message's cost, not a sum of snapshots: {row}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_ac3_run_without_usage_reports_zero_and_keeps_status() {
    let fixture = Fixture::start("ac3").await;
    let issue = fixture.prepare_issue().await;
    let run = fixture.start_author_run(&issue).await;

    // A stream from an adapter that reports no usage at all: the run still ends
    // successfully, and its token totals are zero rather than absent.
    let stream = transcript(vec![
        json!({ "type": "session", "version": 3, "id": "fixture", "timestamp": "2026-11-20T07:00:00.000Z", "cwd": "/tmp" }),
        json!({ "type": "agent_start" }),
        json!({ "type": "turn_start" }),
        json!({ "type": "message_start", "message": { "role": "assistant", "content": [], "timestamp": 1733234400000u64 } }),
        json!({ "type": "message_update", "assistantMessageEvent": { "type": "text_delta", "contentIndex": 0, "delta": "no usage here" } }),
        json!({ "type": "message_end", "message": { "role": "assistant", "content": [{ "type": "text", "text": "done" }], "stopReason": "stop", "timestamp": 1733234402000u64 } }),
        json!({ "type": "turn_end", "message": { "role": "assistant" }, "toolResults": [] }),
        json!({ "type": "agent_end", "messages": [], "willRetry": false }),
        json!({ "type": "agent_settled", "aborted": false }),
    ]);

    let row = fixture.finish_run(&issue, &run, &stream).await;

    assert_eq!(
        row["tokensIn"].as_i64(),
        Some(0),
        "a stream with no usage spent no recorded input tokens: {row}"
    );
    assert_eq!(
        row["tokensOut"].as_i64(),
        Some(0),
        "a stream with no usage spent no recorded output tokens: {row}"
    );
    assert_eq!(
        row["status"].as_str(),
        Some("succeeded"),
        "a missing usage report is not a run failure: {row}"
    );
}
