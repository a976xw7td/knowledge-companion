//! Embedded migration runner.
//!
//! Migrations are compiled into the binary. Each migration runs in a
//! transaction and is recorded in `schema_migrations`. Already-applied
//! migrations are skipped (idempotent).

use anyhow::{Context, Result};
use rusqlite::Connection;

/// A migration with its version number, name, and SQL content.
struct Migration {
    version: i32,
    name: &'static str,
    sql: &'static str,
}

/// All migrations in order. Add new entries at the end.
const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "0001_initial",
    sql: include_str!("../../migrations/0001_initial.sql"),
}];

/// Run pending migrations. Skips already-applied ones.
pub fn run(conn: &Connection) -> Result<()> {
    // Ensure migration table exists (pre-migration step)
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at TEXT NOT NULL
        );",
    )
    .context("Failed to create schema_migrations table")?;

    for m in MIGRATIONS {
        let already_applied: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM schema_migrations WHERE version = ?1",
                [m.version],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if already_applied {
            tracing::debug!(
                version = m.version,
                name = m.name,
                "Migration already applied"
            );
            continue;
        }

        let txn = conn
            .unchecked_transaction()
            .context("Failed to begin migration transaction")?;

        txn.execute_batch(m.sql)
            .with_context(|| format!("Migration {} ({}) failed", m.version, m.name))?;

        txn.execute(
            "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![m.version, m.name, chrono::Utc::now().to_rfc3339()],
        )
        .context("Failed to record migration")?;

        txn.commit().context("Failed to commit migration")?;

        tracing::info!(version = m.version, name = m.name, "Migration applied");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_migrations_run_idempotent() {
        let tmp = NamedTempFile::new().unwrap();
        let conn = Connection::open(tmp.path()).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;",
        )
        .unwrap();

        // First run
        run(&conn).unwrap();

        // Check tables exist
        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(count >= 1);

        // Second run should be idempotent
        run(&conn).unwrap();

        // Verify table exists (use query_row: execute is for non-returning statements)
        let _: i64 = conn
            .query_row("SELECT COUNT(*) FROM watched_roots", [], |r| r.get(0))
            .unwrap();
        let _: i64 = conn
            .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
            .unwrap();
        let _: i64 = conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
            .unwrap();
        let _: i64 = conn
            .query_row("SELECT COUNT(*) FROM sync_events", [], |r| r.get(0))
            .unwrap();
    }
}
