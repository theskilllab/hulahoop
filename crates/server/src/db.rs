use anyhow::Context;
use hulahoop_core::contract;
use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};
use std::str::FromStr;

pub struct Db {
    pub pool: SqlitePool,
    pub data_dir: PathBuf,
}

impl Db {
    pub async fn open(data_dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        std::fs::create_dir_all(data_dir.join("runs"))?;
        std::fs::create_dir_all(data_dir.join("replays"))?;
        std::fs::create_dir_all(data_dir.join("checkouts"))?;
        std::fs::create_dir_all(data_dir.join("home"))?;

        let file = contract::database_file(data_dir);
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", file.display()))?
            .create_if_missing(true)
            .busy_timeout(std::time::Duration::from_millis(
                hulahoop_core::SQLITE_BUSY_TIMEOUT_MS as u64,
            ))
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await
            .context("opening the SQLite database")?;

        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("running migrations")?;

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
