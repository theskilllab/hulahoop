use anyhow::Context;
use hulahoop_core::contract;
use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Copies kept beside the data, so a migration that rebuilds a parent table
/// cannot cost a workspace.
const DB_BACKUPS_KEPT: usize = 5;

/// A migration that rebuilds a parent table (`drop table repos`) cascades
/// through its children while foreign keys are enforced, and the rows go with
/// it. Nothing here can undo that, but a copy taken before the migration runs
/// can.
fn backup_before_migrations(file: &Path, data_dir: &Path) {
    let Ok(meta) = std::fs::metadata(file) else {
        return; // first start: nothing to lose
    };
    if meta.len() == 0 {
        return;
    }
    let dir = data_dir.join("backups");
    if std::fs::create_dir_all(&dir).is_err() {
        eprintln!(
            "warning: could not create {}; skipping the backup",
            dir.display()
        );
        return;
    }
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let target = dir.join(format!("hulahoop-{stamp}.sqlite"));
    if target.exists() {
        return; // one copy per second is enough
    }
    if let Err(error) = std::fs::copy(file, &target) {
        eprintln!("warning: could not back up the database: {error}");
        return;
    }
    for suffix in ["-wal", "-shm"] {
        let side = PathBuf::from(format!("{}{suffix}", file.display()));
        if side.exists() {
            let _ = std::fs::copy(&side, dir.join(format!("hulahoop-{stamp}.sqlite{suffix}")));
        }
    }
    prune_backups(&dir, DB_BACKUPS_KEPT);
}

fn prune_backups(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut copies: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().map(|ext| ext == "sqlite").unwrap_or(false))
        .collect();
    copies.sort();
    if copies.len() <= keep {
        return;
    }
    for old in &copies[..copies.len() - keep] {
        let _ = std::fs::remove_file(old);
        for suffix in ["-wal", "-shm"] {
            let _ = std::fs::remove_file(PathBuf::from(format!("{}{suffix}", old.display())));
        }
    }
}

/// What a data directory's database says about its schema, next to what this
/// binary ships.
pub struct SchemaVersion {
    /// Migrations the database has applied.
    pub applied: i64,
    /// The highest version among them, if any.
    pub newest_applied: Option<i64>,
    /// The highest migration this binary knows.
    pub newest_known: i64,
    /// Whether the database holds a migration this binary does not have.
    pub ahead: bool,
}

/// Reads the schema version without opening the database for writing and without
/// migrating it, so a database from a newer binary can be reported on. `None`
/// when the data directory has no database yet.
pub async fn schema_version(data_dir: &Path) -> anyhow::Result<Option<SchemaVersion>> {
    let file = contract::database_file(data_dir);
    if !file.exists() {
        return Ok(None);
    }
    let options =
        SqliteConnectOptions::from_str(&format!("sqlite://{}", file.display()))?.read_only(true);
    let pool = SqlitePool::connect_with(options)
        .await
        .context("opening the SQLite database")?;
    let table: Option<(String,)> =
        sqlx::query_as("select name from sqlite_master where name = '_sqlx_migrations'")
            .fetch_optional(&pool)
            .await?;
    let versions: Vec<i64> = if table.is_some() {
        sqlx::query_scalar("select version from _sqlx_migrations")
            .fetch_all(&pool)
            .await?
    } else {
        Vec::new()
    };
    pool.close().await;

    let known: Vec<i64> = sqlx::migrate!("./migrations")
        .iter()
        .map(|migration| migration.version)
        .collect();
    Ok(Some(SchemaVersion {
        applied: versions.len() as i64,
        newest_applied: versions.iter().copied().max(),
        newest_known: known.iter().copied().max().unwrap_or(0),
        ahead: versions.iter().any(|version| !known.contains(version)),
    }))
}

/// Migrations run on their own connection with foreign keys off. sqlx wraps
/// every SQLite migration in a transaction, where `PRAGMA foreign_keys = OFF`
/// is ignored, so a migration that rebuilds a parent table (`drop table
/// issues`) would otherwise either cascade-delete its children or fail on them.
/// The rebuilt schema is checked afterwards, so a migration that leaves a
/// dangling reference still refuses to start.
async fn migrate(options: &SqliteConnectOptions) -> anyhow::Result<()> {
    // One connection, so the pragma holds for every statement the migrator runs.
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options.clone().foreign_keys(false))
        .await
        .context("opening the SQLite database")?;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("running migrations")?;
    let dangling: Vec<(String, Option<i64>, String)> =
        sqlx::query_as("select \"table\", rowid, parent from pragma_foreign_key_check")
            .fetch_all(&pool)
            .await
            .context("checking foreign keys after migrations")?;
    pool.close().await;
    if let Some((table, rowid, parent)) = dangling.first() {
        anyhow::bail!(
            "after migrations, {} row(s) point at missing parents (first: {table} row {rowid:?} -> {parent})",
            dangling.len()
        );
    }
    Ok(())
}

pub struct Db {
    pub pool: SqlitePool,
    pub data_dir: PathBuf,
}

impl Db {
    pub async fn open(data_dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        std::fs::create_dir_all(data_dir.join("runs"))?;
        std::fs::create_dir_all(data_dir.join("checkouts"))?;
        std::fs::create_dir_all(data_dir.join("home"))?;

        let file = contract::database_file(data_dir);
        backup_before_migrations(&file, data_dir);

        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", file.display()))?
            .create_if_missing(true)
            .busy_timeout(std::time::Duration::from_millis(
                hulahoop_core::SQLITE_BUSY_TIMEOUT_MS as u64,
            ))
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);

        migrate(&options).await?;

        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options.foreign_keys(true))
            .await
            .context("opening the SQLite database")?;

        Ok(Self {
            pool,
            data_dir: data_dir.to_path_buf(),
        })
    }

    /// One user, one workspace, one membership: local mode with rows that are
    /// real, so remote mode does not need a migration.
    pub async fn seed_local(&self) -> anyhow::Result<uuid::Uuid> {
        // Ids are stored as 16-byte blobs, which is what sqlx's Uuid decoder
        // expects and what makes an index on `id` compact.
        let existing: Option<(uuid::Uuid,)> = sqlx::query_as("select id from workspaces limit 1")
            .fetch_optional(&self.pool)
            .await?;
        if let Some((id,)) = existing {
            return Ok(id);
        }

        let now = chrono::Utc::now().to_rfc3339();
        let user_id = uuid::Uuid::now_v7();
        let workspace_id = uuid::Uuid::now_v7();
        // `Local` gives `LOC`, so an issue filed with no project reads `LOC-3`
        // rather than `WS-3`. The name is the input; the prefix follows it.
        let prefix: String = "Local".chars().take(3).collect::<String>().to_uppercase();
        sqlx::query("insert into users (id, name, email, created_at) values (?, 'you', null, ?)")
            .bind(user_id)
            .bind(&now)
            .execute(&self.pool)
            .await?;
        sqlx::query(
            "insert into workspaces (id, name, slug, default_prefix, created_at) values (?, 'Local', 'local', ?, ?)",
        )
        .bind(workspace_id)
        .bind(&prefix)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "insert into workspace_members (workspace_id, user_id, role) values (?, ?, 'owner')",
        )
        .bind(workspace_id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(workspace_id)
    }

    pub async fn local_user_id(&self) -> anyhow::Result<uuid::Uuid> {
        let (id,): (uuid::Uuid,) = sqlx::query_as("select id from users limit 1")
            .fetch_one(&self.pool)
            .await?;
        Ok(id)
    }
}

/// `sha256` of a byte slice, hex encoded. Used for the prompt revision and the
/// diff pin, so both are comparable strings instead of opaque blobs.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Keep a process group alive only as long as its owner: the server records the
/// pgid at claim time and kills the whole group on cancel or timeout.
pub fn kill_process_group(pgid: i32) {
    if pgid <= 0 {
        return;
    }
    // Negative pid targets the group, which is what a `sh -c` wrapper leaves
    // behind after the agent itself exits.
    unsafe {
        libc::kill(-pgid, libc::SIGTERM);
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    }
}

pub fn json_list(raw: &str) -> Vec<String> {
    serde_json::from_str(raw).unwrap_or_default()
}

pub fn json_list_of_flags(raw: &str) -> Vec<hulahoop_core::RunFlag> {
    serde_json::from_str::<Vec<String>>(raw)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| hulahoop_core::RunFlag::parse(&value))
        .collect()
}

pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[cfg(test)]
mod backup_tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hulahoop-{name}-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn a_copy_is_taken_before_the_database_is_opened() {
        let dir = scratch("backup");
        let file = dir.join("hulahoop.sqlite");
        std::fs::write(&file, b"rows").expect("write");
        backup_before_migrations(&file, &dir);
        let copies: Vec<_> = std::fs::read_dir(dir.join("backups"))
            .expect("backups")
            .filter_map(|entry| entry.ok())
            .collect();
        assert_eq!(copies.len(), 1, "one copy per start");
        assert_eq!(
            copies[0].path().extension().and_then(|ext| ext.to_str()),
            Some("sqlite")
        );
        assert_eq!(std::fs::read(copies[0].path()).expect("read"), b"rows");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn a_first_start_has_nothing_to_copy() {
        let dir = scratch("first");
        backup_before_migrations(&dir.join("hulahoop.sqlite"), &dir);
        assert!(!dir.join("backups").exists());
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn only_the_newest_copies_are_kept() {
        let dir = scratch("prune");
        let backups = dir.join("backups");
        std::fs::create_dir_all(&backups).expect("backups");
        for day in 0..8 {
            std::fs::write(
                backups.join(format!("hulahoop-2026010{day}-000000.sqlite")),
                b"x",
            )
            .expect("write");
        }
        prune_backups(&backups, 5);
        assert_eq!(std::fs::read_dir(&backups).expect("list").count(), 5);
        assert!(backups.join("hulahoop-20260107-000000.sqlite").exists());
        assert!(!backups.join("hulahoop-20260100-000000.sqlite").exists());
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 0023 rebuilds `issues` and `runs`, which have children. Applied to a
    /// database that already holds a task with runs and comments, it used to
    /// fail with `FOREIGN KEY constraint failed`, because sqlx runs it inside a
    /// transaction where `PRAGMA foreign_keys = OFF` does nothing.
    #[tokio::test]
    async fn rebuilding_a_parent_table_keeps_its_children() {
        let dir = scratch("rebuild");
        let file = contract::database_file(&dir);
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", file.display()))
            .expect("options")
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .expect("pool");
        let full = sqlx::migrate!("./migrations");
        let before = sqlx::migrate::Migrator {
            migrations: std::borrow::Cow::Owned(
                full.iter()
                    .filter(|migration| migration.version < 23)
                    .cloned()
                    .collect(),
            ),
            ..full
        };
        before.run(&pool).await.expect("migrations before 0023");
        let db = Db {
            pool,
            data_dir: dir.clone(),
        };
        let workspace = db.seed_local().await.expect("seed");
        let (project, repo, agent, issue, run, comment) = (
            uuid::Uuid::now_v7(),
            uuid::Uuid::now_v7(),
            uuid::Uuid::now_v7(),
            uuid::Uuid::now_v7(),
            uuid::Uuid::now_v7(),
            uuid::Uuid::now_v7(),
        );
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "insert into projects (id, workspace_id, name, created_at) values (?,?, 'Keep',?)",
        )
        .bind(project)
        .bind(workspace)
        .bind(&now)
        .execute(&db.pool)
        .await
        .expect("project");
        sqlx::query("insert into repos (id, project_id, name, remote_url, contract_version, created_at) values (?,?, 'repo','/tmp/repo','1',?)")
            .bind(repo).bind(project).bind(&now).execute(&db.pool).await.expect("repo");
        sqlx::query("insert into agents (id, workspace_id, name, adapter, model, created_at, updated_at) values (?,?, 'builder','pi','model-x',?,?)")
            .bind(agent).bind(workspace).bind(&now).bind(&now).execute(&db.pool).await.expect("agent");
        sqlx::query("insert into issues (id, workspace_id, project_id, repo_id, issue_key, number, title, status, created_at, updated_at) values (?,?,?,?, 'KEP-1',1,'Task','done',?,?)")
            .bind(issue).bind(workspace).bind(project).bind(repo).bind(&now).bind(&now).execute(&db.pool).await.expect("issue");
        sqlx::query("insert into runs (id, issue_id, repo_id, agent_id, attempt, base_bookmark, prompt_revision, artifact_dir, status, model, created_at) values (?,?,?,?,1,'main','prompt','/tmp','succeeded','model-x',?)")
            .bind(run).bind(issue).bind(repo).bind(agent).bind(&now).execute(&db.pool).await.expect("run");
        sqlx::query("insert into comments (id, issue_id, author_type, body, created_at, run_id) values (?,?, 'agent','done',?,?)")
            .bind(comment).bind(issue).bind(&now).bind(run).execute(&db.pool).await.expect("comment");
        db.pool.close().await;

        let db = Db::open(&dir)
            .await
            .expect("0023 applies over existing rows");
        let counts: (i64, i64, i64) = sqlx::query_as(
            "select (select count(*) from issues), (select count(*) from runs), (select count(*) from comments)",
        )
        .fetch_one(&db.pool)
        .await
        .expect("counts");
        assert_eq!(
            counts,
            (1, 1, 1),
            "the task, its run and its comment survive"
        );
        sqlx::query("update issues set repo_id = null where id = ?")
            .bind(issue)
            .execute(&db.pool)
            .await
            .expect("repo_id is optional after 0023");
        let enforced: (i64,) = sqlx::query_as("pragma foreign_keys")
            .fetch_one(&db.pool)
            .await
            .expect("pragma");
        assert_eq!(enforced.0, 1, "the pool still enforces foreign keys");
        db.pool.close().await;
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 0030 turns single-file artifacts into documents that own files. Applied
    /// over a row written before it, the row keeps its id, becomes type `file`
    /// and gets its file attached, still at the same path on disk.
    #[tokio::test]
    async fn an_artifact_from_before_0030_keeps_its_file() {
        let dir = scratch("artifact-docs");
        let file = contract::database_file(&dir);
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", file.display()))
            .expect("options")
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .expect("pool");
        let full = sqlx::migrate!("./migrations");
        let before = sqlx::migrate::Migrator {
            migrations: std::borrow::Cow::Owned(
                full.iter()
                    .filter(|migration| migration.version < 30)
                    .cloned()
                    .collect(),
            ),
            ..full
        };
        before.run(&pool).await.expect("migrations before 0030");
        let db = Db {
            pool,
            data_dir: dir.clone(),
        };
        let workspace = db.seed_local().await.expect("seed");
        let id = uuid::Uuid::now_v7();
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query("insert into artifacts (id, workspace_id, title, filename, mime, size, sha256, created_at) values (?,?, 'Old spec','spec.md','text/markdown',6,'abc',?)")
            .bind(id).bind(workspace).bind(&now).execute(&db.pool).await.expect("artifact");
        let on_disk = dir.join("artifacts").join(id.to_string());
        std::fs::create_dir_all(&on_disk).expect("artifact dir");
        std::fs::write(on_disk.join("spec.md"), b"# Spec").expect("artifact bytes");
        db.pool.close().await;

        let db = Db::open(&dir)
            .await
            .expect("0030 applies over existing rows");
        let row: (String, String, String) =
            sqlx::query_as("select artifact_type, content, updated_at from artifacts where id = ?")
                .bind(id)
                .fetch_one(&db.pool)
                .await
                .expect("row");
        assert_eq!(row, ("file".into(), String::new(), now));
        let attached: Vec<(uuid::Uuid, String, i64)> =
            sqlx::query_as("select id, filename, size from artifact_files where artifact_id = ?")
                .bind(id)
                .fetch_all(&db.pool)
                .await
                .expect("files");
        assert_eq!(attached, vec![(id, "spec.md".to_string(), 6)]);
        assert!(on_disk.join("spec.md").exists());
        db.pool.close().await;
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
