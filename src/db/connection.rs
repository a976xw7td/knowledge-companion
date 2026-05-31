//! SQLite connection management.
//!
//! Opens the database with PRAGMAs for WAL, foreign keys, and busy timeout.
//! Runs migrations on first open.

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

use super::migrations;

/// Open (or create) the SQLite database and run migrations.
pub fn open(db_path: &Path) -> Result<Connection> {
    let conn = Connection::open(db_path)
        .with_context(|| format!("Failed to open database: {}", db_path.display()))?;

    // Essential PRAGMAs for portable deployment
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;",
    )
    .context("Failed to set PRAGMAs")?;

    // Run migrations (idempotent)
    migrations::run(&conn)?;

    tracing::info!(path = %db_path.display(), "Database opened");
    Ok(conn)
}

/// Run PRAGMA integrity_check on a database.
pub fn check_integrity(db_path: &Path) -> Result<()> {
    let conn = Connection::open(db_path)?;
    let result: String = conn
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .context("integrity_check failed")?;
    if result == "ok" {
        Ok(())
    } else {
        Err(anyhow::anyhow!("integrity_check: {}", result))
    }
}
