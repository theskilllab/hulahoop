//! Backend acceptance tests for the Projects page revamp: a colour stored per
//! project, and a way to unlink a repository that never destroys run history.

use hulahoop_server::db::Db;
use hulahoop_server::engine::Engine;
use hulahoop_server::gql::build_schema;
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;

const PALETTE: [&str; 8] = [
    "#2f7ad8", "#1f9d7a", "#c2692b", "#6a5acd", "#d4558a", "#b88100", "#d03b3b", "#64748b",
];

struct Fixture {
    engine: Arc<Engine>,
    workspace: Uuid,
    dir: std::path::PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn fixture(name: &str) -> Fixture {
    let dir = std::env::temp_dir().join(format!("hulahoop-{name}-{}", Uuid::now_v7()));
    let db = Db::open(&dir).await.unwrap();
    let workspace = db.seed_local().await.unwrap();
    Fixture {
        engine: Arc::new(Engine::new(db)),
        workspace,
        dir,
    }
}

async fn run(f: &Fixture, query: String) -> Result<Value, String> {
    let response = build_schema(f.engine.clone()).execute(query).await;
    if let Some(e) = response.errors.first() {
        return Err(e.message.clone());
    }
    Ok(response.data.into_json().unwrap())
}

async fn create(f: &Fixture, name: &str, prefix: &str, extra: &str) -> Result<Value, String> {
    let w = f.workspace;
    run(
        f,
        format!(
            r#"mutation {{ createProject(workspaceId: "{w}", name: "{name}", prefix: "{prefix}" {extra}) {{ id color prefix repos {{ id name remoteUrl }} }} }}"#
        ),
    )
    .await
    .map(|d| d["createProject"].clone())
}

fn id(v: &Value) -> Uuid {
    v["id"].as_str().unwrap().parse().unwrap()
}

// Criterion: a project's colour is saved and survives a reload.
#[tokio::test]
async fn a_chosen_colour_is_saved_and_read_back() {
    let f = fixture("pj-color").await;
    let p = create(&f, "Alpha", "ALP", r##", color: "#d4558a""##)
        .await
        .unwrap();
    assert_eq!(p["color"], "#d4558a");
    let w = f.workspace;
    let listed = run(
        &f,
        format!(r#"{{ projects(workspaceId: "{w}") {{ name color }} }}"#),
    )
    .await
    .unwrap();
    assert_eq!(listed["projects"][0]["color"], "#d4558a");
}

// Criterion: the first unused colour is preselected / picked for a new project.
#[tokio::test]
async fn a_project_without_a_colour_gets_the_first_unused_one() {
    let f = fixture("pj-first-free").await;
    let a = create(&f, "Alpha", "ALP", "").await.unwrap();
    let b = create(&f, "Beta", "BET", "").await.unwrap();
    let c = create(&f, "Gamma", "GAM", r##", color: "#2f7ad8""##)
        .await
        .unwrap();
    assert_eq!(a["color"], PALETTE[0]);
    assert_eq!(b["color"], PALETTE[1]);
    assert_eq!(c["color"], PALETTE[0], "an explicit colour may repeat");
    let d = create(&f, "Delta", "DEL", "").await.unwrap();
    assert_eq!(d["color"], PALETTE[2]);
}

#[tokio::test]
async fn only_the_eight_palette_colours_are_accepted() {
    let f = fixture("pj-bad-color").await;
    for bad in ["red", "#ffffff", "#2F7AD8 ", ""] {
        let err = create(&f, "Alpha", "ALP", &format!(r#", color: "{bad}""#))
            .await
            .expect_err(bad);
        assert!(err.contains("eight project colours"), "{bad}: {err}");
    }
}

#[tokio::test]
async fn update_project_changes_name_and_colour() {
    let f = fixture("pj-update").await;
    let p = create(&f, "Alpha", "ALP", "").await.unwrap();
    let pid = id(&p);
    let updated = run(
        &f,
        format!(
            r##"mutation {{ updateProject(id: "{pid}", name: "Alpha Two", color: "#64748b") {{ name color prefix }} }}"##
        ),
    )
    .await
    .unwrap();
    assert_eq!(updated["updateProject"]["name"], "Alpha Two");
    assert_eq!(updated["updateProject"]["color"], "#64748b");
    assert_eq!(updated["updateProject"]["prefix"], "ALP");
    let err = run(
        &f,
        format!(r##"mutation {{ updateProject(id: "{pid}", color: "#123456") {{ id }} }}"##),
    )
    .await
    .unwrap_err();
    assert!(err.contains("eight project colours"), "{err}");
}

// Criterion: the dialog links the chosen repositories as part of the same save.
#[tokio::test]
async fn create_project_links_its_repositories() {
    let f = fixture("pj-create-repos").await;
    let p = create(
        &f,
        "Alpha",
        "ALP",
        r#", repos: [{ name: "app", remoteUrl: "https://github.com/acme/app" }, { name: "lib", remoteUrl: "https://github.com/acme/lib", baseBookmark: "trunk" }]"#,
    )
    .await
    .unwrap();
    let names: Vec<&str> = p["repos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"app") && names.contains(&"lib"));
    let bases: Vec<(String,)> =
        sqlx::query_as("select base_bookmark from repos where name = 'lib'")
            .fetch_all(&f.engine.db.pool)
            .await
            .unwrap();
    assert_eq!(bases[0].0, "trunk");
}

#[tokio::test]
async fn a_bad_repository_url_creates_no_project() {
    let f = fixture("pj-create-bad-repo").await;
    let err = create(
        &f,
        "Alpha",
        "ALP",
        r#", repos: [{ name: "x", remoteUrl: "-uhack" }]"#,
    )
    .await;
    assert!(err.is_err());
    let (n,): (i64,) = sqlx::query_as("select count(*) from projects")
        .fetch_one(&f.engine.db.pool)
        .await
        .unwrap();
    assert_eq!(
        n, 0,
        "the project and its repositories save together or not at all"
    );
}

async fn task_on(f: &Fixture, project: Uuid, repo: Uuid, status: &str, n: i64) -> Uuid {
    let t = Uuid::now_v7();
    sqlx::query("insert into issues (id, workspace_id, project_id, repo_id, issue_key, number, title, status, created_at, updated_at) values (?,?,?,?,?,?,'Task',?,?,?)")
        .bind(t)
        .bind(f.workspace)
        .bind(project)
        .bind(repo)
        .bind(format!("ALP-{n}"))
        .bind(n)
        .bind(status)
        .bind(chrono::Utc::now())
        .bind(chrono::Utc::now())
        .execute(&f.engine.db.pool)
        .await
        .unwrap();
    t
}

async fn one_repo(f: &Fixture) -> (Uuid, Uuid) {
    let p = create(
        f,
        "Alpha",
        "ALP",
        r#", repos: [{ name: "app", remoteUrl: "https://github.com/acme/app" }]"#,
    )
    .await
    .unwrap();
    (id(&p), id(&p["repos"][0]))
}

// Criterion: a repository can be unlinked; history is kept.
#[tokio::test]
async fn unlinking_removes_the_repo_and_keeps_finished_tasks() {
    let f = fixture("pj-unlink").await;
    let (project, repo) = one_repo(&f).await;
    let done = task_on(&f, project, repo, "done", 1).await;
    let out = run(
        &f,
        format!(r#"mutation {{ unlinkRepo(id: "{repo}") {{ id repos {{ id }} }} }}"#),
    )
    .await
    .unwrap();
    assert_eq!(out["unlinkRepo"]["id"], project.to_string());
    assert!(out["unlinkRepo"]["repos"].as_array().unwrap().is_empty());
    let (left,): (i64,) = sqlx::query_as("select count(*) from repos where id = ?")
        .bind(repo)
        .fetch_one(&f.engine.db.pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
    let row: (Option<Uuid>,) = sqlx::query_as("select repo_id from issues where id = ?")
        .bind(done)
        .fetch_one(&f.engine.db.pool)
        .await
        .expect("the finished task survives the unlink");
    assert_eq!(row.0, None);
}

#[tokio::test]
async fn unlinking_is_refused_while_open_tasks_use_the_repo() {
    let f = fixture("pj-unlink-open").await;
    let (project, repo) = one_repo(&f).await;
    task_on(&f, project, repo, "in_progress", 1).await;
    let err = run(
        &f,
        format!(r#"mutation {{ unlinkRepo(id: "{repo}") {{ id }} }}"#),
    )
    .await
    .unwrap_err();
    assert!(err.contains("1 open task still uses"), "{err}");
    let (left,): (i64,) = sqlx::query_as("select count(*) from repos where id = ?")
        .bind(repo)
        .fetch_one(&f.engine.db.pool)
        .await
        .unwrap();
    assert_eq!(left, 1);
}

// Deleting a repository cascades to its runs, so a repository with runs stays linked.
#[tokio::test]
async fn unlinking_is_refused_for_a_repo_with_run_history() {
    let f = fixture("pj-unlink-runs").await;
    let (project, repo) = one_repo(&f).await;
    let done = task_on(&f, project, repo, "done", 1).await;
    let agent = Uuid::now_v7();
    sqlx::query("insert into agents (id, workspace_id, name, adapter, model, created_at, updated_at) values (?,?,'builder','pi','m',?,?)")
        .bind(agent).bind(f.workspace).bind(chrono::Utc::now()).bind(chrono::Utc::now())
        .execute(&f.engine.db.pool).await.unwrap();
    let run_id = Uuid::now_v7();
    sqlx::query("insert into runs (id, issue_id, repo_id, agent_id, attempt, base_bookmark, prompt_revision, artifact_dir, status, model, created_at) values (?,?,?,?,1,'main','prompt','/tmp','succeeded','m',?)")
        .bind(run_id).bind(done).bind(repo).bind(agent).bind(chrono::Utc::now())
        .execute(&f.engine.db.pool).await.unwrap();
    let err = run(
        &f,
        format!(r#"mutation {{ unlinkRepo(id: "{repo}") {{ id }} }}"#),
    )
    .await
    .unwrap_err();
    assert!(err.contains("run history"), "{err}");
    let (kept,): (i64,) = sqlx::query_as("select count(*) from runs where id = ?")
        .bind(run_id)
        .fetch_one(&f.engine.db.pool)
        .await
        .unwrap();
    assert_eq!(kept, 1);
}
