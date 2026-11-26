//! Backend tests for the Artifacts revamp: documents that own several files, and
//! old single-file rows that survive the migration.

use hulahoop_server::db::Db;
use hulahoop_server::engine::Engine;
use hulahoop_server::gql::build_schema;
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;

struct Fixture {
    engine: Arc<Engine>,
    workspace: Uuid,
    milestone: Uuid,
    dir: std::path::PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn fixture() -> Fixture {
    let dir = std::env::temp_dir().join(format!("hulahoop-artifacts-{}", Uuid::now_v7()));
    let db = Db::open(&dir).await.unwrap();
    let workspace = db.seed_local().await.unwrap();
    let engine = Arc::new(Engine::new(db));
    let milestone = engine
        .create_milestone(workspace, None, "Beta".into(), String::new(), None)
        .await
        .unwrap()
        .id;
    Fixture {
        engine,
        workspace,
        milestone,
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

async fn create_document(f: &Fixture, extra: &str) -> Result<Value, String> {
    let (w, m) = (f.workspace, f.milestone);
    run(
        f,
        format!(
            r#"mutation {{ createArtifactDocument(workspaceId: "{w}", milestoneId: "{m}", {extra}) {{ id type content author authorIsAgent files {{ id filename size }} }} }}"#
        ),
    )
    .await
    .map(|d| d["createArtifactDocument"].clone())
}

#[tokio::test]
async fn a_document_keeps_its_type_content_author_and_files() {
    let f = fixture().await;
    let doc = create_document(&f, r###"title: "Plan", type: "plan", content: "## Goal""###)
        .await
        .unwrap();
    assert_eq!(doc["type"], "plan");
    assert_eq!(doc["content"], "## Goal");
    assert_eq!(doc["author"], "You");
    let id = doc["id"].as_str().unwrap().to_string();
    for _ in 0..2 {
        run(
            &f,
            format!(
                r#"mutation {{ addArtifactFile(artifactId: "{id}", filename: "a.txt", contentBase64: "aGk=") {{ id }} }}"#
            ),
        )
        .await
        .unwrap();
    }
    let data = run(
        &f,
        format!(r#"{{ artifact(id: "{id}") {{ files {{ filename size }} }} }}"#),
    )
    .await
    .unwrap();
    let names: Vec<_> = data["artifact"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["filename"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, ["a.txt", "a (2).txt"]);
}

#[tokio::test]
async fn an_artifact_needs_a_title_a_known_type_and_an_owner() {
    let f = fixture().await;
    assert!(create_document(&f, r#"title: " ", type: "plan""#)
        .await
        .is_err());
    assert!(create_document(&f, r#"title: "x", type: "poem""#)
        .await
        .is_err());
    let w = f.workspace;
    let orphan = run(
        &f,
        format!(r#"mutation {{ createArtifactDocument(workspaceId: "{w}", title: "x", type: "plan") {{ id }} }}"#),
    )
    .await;
    assert!(orphan.is_err());
}

#[tokio::test]
async fn an_older_single_file_row_lists_as_a_file_with_its_file_attached() {
    let f = fixture().await;
    let legacy = f
        .engine
        .store_artifact(
            f.workspace,
            None,
            Some(f.milestone),
            None,
            "Spec".into(),
            None,
            "spec.md".into(),
            None,
            b"# Spec",
        )
        .await
        .unwrap();
    assert_eq!(legacy.artifact_type, "file");
    let files = f.engine.artifact_files(legacy.id).await.unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].id, legacy.id, "the file keeps the artifact's id");
    assert!(f.engine.artifact_file_path(&files[0]).exists());
}

#[tokio::test]
async fn removing_a_file_and_deleting_the_artifact_clean_up() {
    let f = fixture().await;
    let doc = create_document(&f, r#"title: "R", type: "report""#)
        .await
        .unwrap();
    let id: Uuid = doc["id"].as_str().unwrap().parse().unwrap();
    let file = f
        .engine
        .add_artifact_file(id, "r.txt".into(), None, b"hi")
        .await
        .unwrap();
    let path = f.engine.artifact_file_path(&file);
    assert!(f.engine.remove_artifact_file(file.id).await.unwrap());
    assert!(!path.exists());
    assert!(f.engine.artifact_files(id).await.unwrap().is_empty());
    assert!(
        f.engine.artifact(id).await.unwrap().filename.is_empty(),
        "no file left, so the row no longer mirrors one"
    );
    assert!(f.engine.delete_artifact(id).await.unwrap());
}
