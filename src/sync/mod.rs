//! Folder sync engine.
//!
//! Coordinates scanning, planning, and execution of sync operations.
//! The folder is the source of truth; the database stores derived indexes.

pub mod executor;
pub mod planner;
pub mod scanner;
pub mod watcher;

use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;
use std::path::Path;
use std::time::Instant;

use crate::config::KnowledgeRoot;

/// Result of a sync operation.
#[derive(Debug, Serialize)]
pub struct SyncResult {
    pub root_name: String,
    pub created: usize,
    pub modified: usize,
    pub deleted: usize,
    pub skipped: usize,
    pub failed: usize,
    pub duration_ms: u64,
}

/// Run a full sync for a single watched root.
pub fn sync_root(
    conn: &Connection,
    root: &KnowledgeRoot,
    bundle_root: &Path,
) -> Result<SyncResult> {
    let start = Instant::now();

    // Resolve root path
    let root_path = if Path::new(&root.path).is_absolute() {
        Path::new(&root.path).to_path_buf()
    } else {
        bundle_root.join(root.path.trim_start_matches("./"))
    };

    if !root_path.is_dir() {
        return Err(anyhow::anyhow!(
            "Root path does not exist or is not a directory: {}",
            root_path.display()
        ));
    }

    tracing::info!(
        root = %root.name,
        path = %root_path.display(),
        "Starting sync"
    );

    // 1. Scan
    let files = scanner::scan(&root_path, &root.include_globs, &root.exclude_globs)?;
    tracing::debug!(file_count = files.len(), "Scan complete");

    // 2. Plan
    let plan = planner::build_plan(conn, root, &root_path, &files)?;
    tracing::debug!(
        create = plan.create.len(),
        modify = plan.modify.len(),
        delete = plan.delete.len(),
        skip = plan.skip,
        "Plan built"
    );

    // 3. Execute
    let exec_result = executor::execute_plan(conn, root, bundle_root, &root_path, &plan)?;

    let duration_ms = start.elapsed().as_millis() as u64;

    tracing::info!(
        root = %root.name,
        created = exec_result.created,
        modified = exec_result.modified,
        deleted = exec_result.deleted,
        skipped = exec_result.skipped,
        failed = exec_result.failed,
        duration_ms = duration_ms,
        "Sync complete"
    );

    Ok(SyncResult {
        root_name: root.name.clone(),
        created: exec_result.created,
        modified: exec_result.modified,
        deleted: exec_result.deleted,
        skipped: exec_result.skipped,
        failed: exec_result.failed,
        duration_ms,
    })
}

/// Run sync for all enabled roots.
pub fn sync_all(conn: &Connection, bundle_root: &Path) -> Result<Vec<SyncResult>> {
    let config = crate::config::bundle::load_config(bundle_root).unwrap_or_default();
    let mut results = Vec::new();

    for root in &config.knowledge.roots {
        if !root.enabled {
            continue;
        }
        match sync_root(conn, root, bundle_root) {
            Ok(result) => results.push(result),
            Err(e) => {
                tracing::error!(error = %e, root = %root.name, "Sync failed for root");
                results.push(SyncResult {
                    root_name: root.name.clone(),
                    created: 0,
                    modified: 0,
                    deleted: 0,
                    skipped: 0,
                    failed: 1,
                    duration_ms: 0,
                });
            }
        }
    }

    Ok(results)
}
