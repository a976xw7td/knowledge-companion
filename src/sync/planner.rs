//! Sync planner — compares scanned files against the database
//! and produces a plan of create/modify/delete/skip operations.
//!
//! Idempotency: uses `relative_path` as the stable key for comparing
//! scanned files with database records. Absolute `source_path` is stored
//! but not used as the join key, because the same relative path may map
//! to different absolute paths when the USB bundle is mounted at different
//! mount points.

use anyhow::Result;
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::path::Path;

use super::scanner::ScannedFile;
use crate::config::KnowledgeRoot;

/// A sync plan describing what to do with each file.
#[derive(Debug)]
pub struct SyncPlan {
    pub create: Vec<ScannedFile>,
    pub modify: Vec<ScannedFile>,
    pub delete: Vec<DocumentRef>,
    pub skip: usize,
}

/// Reference to a document in the database that needs deletion.
#[derive(Debug, Clone)]
pub struct DocumentRef {
    pub id: String,
    pub relative_path: String,
}

/// Build a sync plan by comparing scanned files with the database.
///
/// Uses `relative_path` as the stable join key. The `source_path` (absolute
/// path) may change between runs (USB mounted at different paths), but
/// `relative_path` is invariant as long as the watched root path is consistent.
///
/// `max_file_size_bytes` skips files that exceed the size limit (from config).
pub fn build_plan(
    conn: &Connection,
    root: &KnowledgeRoot,
    _root_path: &Path,
    scanned: &[ScannedFile],
    max_file_size_bytes: u64,
) -> Result<SyncPlan> {
    let mut create = Vec::new();
    let mut modify = Vec::new();
    let mut delete = Vec::new();
    let mut skip = 0usize;

    // Get existing documents for this root, keyed by relative_path
    let mut stmt = conn.prepare(
        "SELECT id, relative_path, content_hash, source_mtime, source_size
         FROM documents
         WHERE root_id = (SELECT id FROM watched_roots WHERE name = ?1)
           AND status != 'deleted'",
    )?;

    // Key: relative_path → (id, hash, mtime, size)
    let mut db_files: std::collections::HashMap<String, (String, String, i64, i64)> =
        std::collections::HashMap::new();

    let rows = stmt.query_map(rusqlite::params![root.name], |row| {
        Ok((
            row.get::<_, String>(0)?, // id
            row.get::<_, String>(1)?, // relative_path
            row.get::<_, String>(2)?, // content_hash
            row.get::<_, i64>(3)?,    // mtime
            row.get::<_, i64>(4)?,    // size
        ))
    })?;

    for row in rows {
        let (id, relative_path, hash, mtime, size) = row?;
        db_files.insert(relative_path, (id, hash, mtime, size));
    }

    // Build set of scanned relative paths for deletion detection
    let scanned_paths: std::collections::HashSet<&str> =
        scanned.iter().map(|f| f.relative_path.as_str()).collect();

    // Determine creates, modifies, and skips
    for file in scanned {
        // Skip files exceeding size limit — don't even read them for hashing
        if file.size > max_file_size_bytes {
            tracing::warn!(
                path = %file.relative_path,
                size = file.size,
                limit = max_file_size_bytes,
                "File exceeds max size, skipping"
            );
            skip += 1;
            continue;
        }

        match db_files.get(&file.relative_path) {
            None => {
                // Not in DB → create
                create.push(file.clone());
            }
            Some((_id, db_hash, _db_mtime, _db_size)) => {
                // Always compute hash for correctness. mtime may be unchanged
                // on fast equal-length edits within the same second.
                let file_hash = match compute_hash(&file.absolute_path) {
                    Ok(h) => h,
                    Err(_) => {
                        // Can't hash → treat as modify (will fail safely in executor)
                        modify.push(file.clone());
                        continue;
                    }
                };

                if file_hash != *db_hash {
                    modify.push(file.clone());
                } else {
                    // mtime/size changed but hash same — update mtime/size silently? No, skip.
                    // Future: update mtime/size in DB without reindexing
                    skip += 1;
                }
            }
        }
    }

    // Determine deletions: files in DB but not on disk
    for (relative_path, (id, _hash, _mtime, _size)) in &db_files {
        if !scanned_paths.contains(relative_path.as_str()) {
            delete.push(DocumentRef {
                id: id.clone(),
                relative_path: relative_path.clone(),
            });
        }
    }

    tracing::debug!(
        create = create.len(),
        modify = modify.len(),
        delete = delete.len(),
        skip = skip,
        "Sync plan built"
    );

    Ok(SyncPlan {
        create,
        modify,
        delete,
        skip,
    })
}

/// Compute SHA256 hash of a file.
pub fn compute_hash(path: &Path) -> Result<String> {
    let data = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    Ok(hex::encode(hasher.finalize()))
}
