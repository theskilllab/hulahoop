//! Acceptance tests for the schema version `hulahoop doctor` reports: how many
//! migrations the data directory has applied, and the newest one this binary
//! knows. A data directory that is ahead of the binary is a failure that names
//! both numbers.
//!
//! The binary is run for real against a temporary data directory. Prerequisite
//! probes (nix, jj, ...) may fail on a bare machine, so the passing case asserts
//! on the output, never on the exit code.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

/// The newest migration shipped in `crates/server/migrations`, read from the
/// directory so a new migration moves this test with it.
fn newest_known() -> i64 {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../server/migrations");
    std::fs::read_dir(dir)
        .expect("read migrations")
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            name.split('_').next()?.parse::<i64>().ok()
        })
        .max()
        .expect("at least one migration")
}

fn data_dir(tag: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "hulahoop-doctor-{tag}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Open the database the way the server does, so every known migration is applied.
async fn migrated(dir: &Path) -> hulahoop_server::db::Db {
    hulahoop_server::db::Db::open(dir).await.expect("open db")
}

async fn applied(db: &hulahoop_server::db::Db) -> i64 {
    sqlx::query_scalar("select count(*) from _sqlx_migrations")
        .fetch_one(&db.pool)
        .await
        .expect("count migrations")
}

fn doctor(dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hulahoop"))
        .arg("doctor")
        .env("HULAHOOP_DATA", dir)
        .output()
        .expect("run hulahoop doctor")
}

fn text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Criterion 1.
#[tokio::test]
async fn doctor_prints_the_applied_count_and_the_newest_known_migration() {
    let dir = data_dir("report");
    let db = migrated(&dir).await;
    let count = applied(&db).await;
    let newest = newest_known();
    // The two numbers must be told apart: a gap in the numbering makes them differ.
    assert_ne!(
        count, newest,
        "migration numbering has no gap to tell them apart"
    );
    db.pool.close().await;

    let out = text(&doctor(&dir));
    assert!(
        out.contains(&count.to_string()),
        "applied count {count} missing:\n{out}"
    );
    assert!(
        out.contains(&newest.to_string()),
        "newest known {newest} missing:\n{out}"
    );
    assert!(out.contains("migration"), "no schema line:\n{out}");
    // The existing lines survive.
    assert!(out.contains("nix"), "prerequisite lines are gone:\n{out}");
}

/// Criteria 2 and 3.
#[tokio::test]
async fn doctor_fails_when_the_data_dir_has_a_migration_the_binary_lacks() {
    let dir = data_dir("unknown");
    let db = migrated(&dir).await;
    let newest = newest_known();
    // Two unknown migrations, or three when two would make the applied count
    // equal the newest known number: the numbering has gaps, so the count
    // can land on it.
    let unknown = if applied(&db).await + 2 == newest {
        3
    } else {
        2
    };
    let versions: Vec<i64> = (1..=unknown).map(|n| newest + 23 + n).collect();
    let second = versions[versions.len() - 1];
    for &version in &versions {
        sqlx::query(
            "insert into _sqlx_migrations \
             (version, description, success, checksum, execution_time) \
             values (?, 'from a newer binary', 1, x'00', 0)",
        )
        .bind(version)
        .execute(&db.pool)
        .await
        .expect("insert unknown migration");
    }
    let count = applied(&db).await;
    db.pool.close().await;
    // Distinct numbers, so each one in the output is attributable.
    assert!(count != newest && count != second && newest != second);

    let output = doctor(&dir);
    let out = text(&output);
    assert!(!output.status.success(), "doctor exited 0:\n{out}");
    assert!(
        out.contains(&newest.to_string()),
        "newest known {newest} missing:\n{out}"
    );
    assert!(
        out.contains(&second.to_string()),
        "database's newest {second} missing:\n{out}"
    );
    assert!(
        out.contains(&count.to_string()),
        "applied count {count} missing:\n{out}"
    );
}
