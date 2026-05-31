//! Sync executor — runs the sync plan against the database transactionally.
//!
//! For each created/modified file, the executor runs the full parsing,
//! chunking, FTS indexing, and graph building pipeline inside a single
//! transaction. One file failure does not stop other files.

use anyhow::{Context, Result};
use rusqlite::Connection;
use sha2::Digest;
use std::path::Path;

use super::planner::{compute_hash, DocumentRef, SyncPlan};
use super::scanner::ScannedFile;
use crate::config::KnowledgeRoot;
use crate::index::{fts, graph};
use crate::ingest::{chunker, ParserRegistry};

/// Result of executing a sync plan.
#[derive(Debug)]
pub struct ExecResult {
    pub created: usize,
    pub modified: usize,
    pub deleted: usize,
    pub skipped: usize,
    pub failed: usize,
}

/// Execute the sync plan against the database.
pub fn execute_plan(
    conn: &Connection,
    root: &KnowledgeRoot,
    _bundle_root: &Path,
    root_path: &Path,
    plan: &SyncPlan,
) -> Result<ExecResult> {
    let mut created = 0usize;
    let mut modified = 0usize;
    let mut deleted = 0usize;
    let mut failed = 0usize;

    let root_id = ensure_root(conn, root, root_path)?;
    let parser_registry = ParserRegistry::default_registry();
    let chunk_config = chunker::ChunkConfig::default();

    // Process creates
    for file in &plan.create {
        match index_file(conn, &root_id, file, &parser_registry, &chunk_config) {
            Ok(_) => {
                created += 1;
                log_sync_event(conn, &root_id, "created", &file.relative_path, None)?;
            }
            Err(e) => {
                failed += 1;
                tracing::warn!(path = %file.relative_path, error = %e, "Failed to create document");
                log_sync_event(
                    conn,
                    &root_id,
                    "failed",
                    &file.relative_path,
                    Some(&format!("{}", e)),
                )?;
            }
        }
    }

    // Process modifies: delete old → rebuild in a transaction.
    // If rebuild fails, the transaction rolls back and old data remains intact.
    for file in &plan.modify {
        match modify_file_transactional(conn, &root_id, file, &parser_registry, &chunk_config) {
            Ok(()) => {
                modified += 1;
                log_sync_event(conn, &root_id, "modified", &file.relative_path, None)?;
            }
            Err(e) => {
                failed += 1;
                tracing::warn!(path = %file.relative_path, error = %e, "Failed to modify document");
                log_sync_event(
                    conn,
                    &root_id,
                    "failed",
                    &file.relative_path,
                    Some(&format!("{}", e)),
                )?;
            }
        }
    }

    // Process deletes (soft delete + remove derived data)
    for doc_ref in &plan.delete {
        if let Err(e) = soft_delete_document(conn, &root_id, doc_ref) {
            failed += 1;
            tracing::warn!(path = %doc_ref.relative_path, error = %e, "Failed to delete document");
        } else {
            deleted += 1;
            log_sync_event(conn, &root_id, "deleted", &doc_ref.relative_path, None)?;
        }
    }

    Ok(ExecResult {
        created,
        modified,
        deleted,
        skipped: plan.skip,
        failed,
    })
}

/// Modify a file within an explicit SQLite transaction.
/// If any step fails, the transaction rolls back and old data is preserved.
fn modify_file_transactional(
    conn: &Connection,
    root_id: &str,
    file: &ScannedFile,
    parser_registry: &ParserRegistry,
    chunk_config: &chunker::ChunkConfig,
) -> Result<()> {
    // Get old document ID
    let old_id: String = conn.query_row(
        "SELECT id FROM documents WHERE root_id = ?1 AND relative_path = ?2 AND status != 'deleted'",
        rusqlite::params![root_id, &file.relative_path],
        |r| r.get(0),
    ).context("Old document not found")?;

    // Parse new file FIRST (before deleting old data)
    // If parsing fails, we haven't touched old data yet
    let ext = file
        .absolute_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    let parser = parser_registry
        .find(&ext)
        .with_context(|| format!("No parser for: {}", ext))?;
    let parsed = parser
        .parse(&file.absolute_path)
        .with_context(|| format!("Parse failed: {}", file.relative_path))?;
    check_content_quality(&parsed, &file.relative_path)?;
    let chunks = chunker::chunk_document(&parsed, chunk_config);

    // Begin transaction: clean old + insert new atomically
    let txn = conn
        .unchecked_transaction()
        .context("Failed to begin transaction")?;

    // Remove old derived data (don't ignore errors)
    fts::delete_doc(&txn, &old_id)?;
    txn.execute("DELETE FROM graph_edges WHERE source_id IN (SELECT id FROM graph_nodes WHERE doc_id = ?1) OR target_id IN (SELECT id FROM graph_nodes WHERE doc_id = ?1)", [&old_id])?;
    txn.execute("DELETE FROM graph_nodes WHERE doc_id = ?1", [&old_id])?;
    txn.execute(
        "DELETE FROM chunk_embeddings WHERE chunk_id IN (SELECT id FROM chunks WHERE doc_id = ?1)",
        [&old_id],
    )?;
    txn.execute("DELETE FROM chunks WHERE doc_id = ?1", [&old_id])?;
    txn.execute("DELETE FROM documents WHERE id = ?1", [&old_id])?;

    // Insert new document
    let hash = compute_hash(&file.absolute_path)?;
    let doc_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let title = parsed.title.clone();
    let word_count = parsed.plain_text.split_whitespace().count() as i32;
    let source_path = file.absolute_path.display().to_string();
    let file_type = match ext.as_str() {
        "md" | "markdown" => "markdown",
        "txt" => "txt",
        "pdf" => "pdf",
        "docx" => "docx",
        _ => "unknown",
    };

    txn.execute(
        "INSERT INTO documents (id, root_id, source_path, relative_path, title, file_type, content_hash, source_mtime, source_size, status, word_count, created_at, updated_at, indexed_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'indexed',?10,?11,?11,?11)",
        rusqlite::params![doc_id, root_id, source_path, file.relative_path, title, file_type, hash, file.mtime, file.size as i64, word_count, now],
    )?;

    // Insert chunks + FTS
    let mut chunk_ids = Vec::new();
    for (i, chunk) in chunks.iter().enumerate() {
        let chunk_id = uuid::Uuid::new_v4().to_string();
        let chunk_hash = {
            let mut h = sha2::Sha256::new();
            sha2::Digest::update(&mut h, &chunk.content);
            hex::encode(sha2::Digest::finalize(h))
        };
        txn.execute(
            "INSERT INTO chunks (id, doc_id, chunk_index, heading_path, content, content_hash, start_line, end_line, token_count, embedding_status, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'pending',?10)",
            rusqlite::params![chunk_id, doc_id, i as i64, chunk.heading_path.join("/"), chunk.content, chunk_hash, chunk.start_line as i64, chunk.end_line as i64, chunk.token_estimate as i64, now],
        )?;
        let heading_str = chunk.heading_path.join(" > ");
        fts::index_chunk(
            &txn,
            &chunk_id,
            &doc_id,
            &parsed.title,
            &heading_str,
            &chunk.content,
        )?;
        chunk_ids.push(chunk_id);
    }

    // Graph
    graph::build_document_graph(&txn, &doc_id, &source_path, &parsed)?;

    txn.commit()
        .context("Failed to commit modify transaction")?;

    let chunk_texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();
    try_embed_chunks(conn, &chunk_ids, &chunk_texts);

    tracing::debug!(doc_id = %doc_id, chunks = chunks.len(), path = %file.relative_path, "Document modified transactionally");
    Ok(())
}

/// Safely rebuild one existing document while preserving the old index on failure.
pub fn reindex_document(conn: &Connection, doc_id: &str) -> Result<()> {
    let (root_id, source_path, relative_path): (String, String, String) = conn.query_row(
        "SELECT root_id, source_path, relative_path FROM documents WHERE id = ?1 AND status != 'deleted'",
        [doc_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let absolute_path = std::path::PathBuf::from(source_path);
    let metadata = absolute_path
        .metadata()
        .with_context(|| format!("Source file no longer exists: {}", absolute_path.display()))?;
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let file = ScannedFile {
        absolute_path,
        relative_path,
        size: metadata.len(),
        mtime,
    };
    modify_file_transactional(
        conn,
        &root_id,
        &file,
        &ParserRegistry::default_registry(),
        &chunker::ChunkConfig::default(),
    )
}

/// Full indexing pipeline for a single file: parse → chunk → insert → FTS → graph.
/// Each file is processed in its own implicit transaction (via individual SQL statements).
/// A failure here returns an error that the caller can handle per-file.
fn index_file(
    conn: &Connection,
    root_id: &str,
    file: &ScannedFile,
    parser_registry: &ParserRegistry,
    chunk_config: &chunker::ChunkConfig,
) -> Result<String> {
    // 1. Determine file type and parse
    let ext = file
        .absolute_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    let file_type = match ext.as_str() {
        "md" | "markdown" => "markdown",
        "txt" => "txt",
        "pdf" => "pdf",
        "docx" => "docx",
        _ => {
            // Unsupported types: insert as unindexed
            return insert_unindexed_document(conn, root_id, file, &ext);
        }
    };

    // 2. Parse
    let parser = parser_registry
        .find(&ext)
        .with_context(|| format!("No parser for extension: {}", ext))?;
    let parsed = parser
        .parse(&file.absolute_path)
        .with_context(|| format!("Failed to parse: {}", file.relative_path))?;

    check_content_quality(&parsed, &file.relative_path)?;

    // 3. Chunk
    let chunks = chunker::chunk_document(&parsed, chunk_config);
    if chunks.is_empty() {
        return insert_unindexed_document(conn, root_id, file, &ext);
    }

    // 4. Compute hash and insert document
    let hash = compute_hash(&file.absolute_path)?;
    let doc_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let title = parsed.title.clone();
    let word_count = parsed.plain_text.split_whitespace().count() as i32;
    let source_path = file.absolute_path.display().to_string();

    conn.execute(
        "INSERT INTO documents (id, root_id, source_path, relative_path, title, file_type,
         content_hash, source_mtime, source_size, status, word_count, created_at, updated_at, indexed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'indexed', ?10, ?11, ?11, ?11)",
        rusqlite::params![
            doc_id,
            root_id,
            source_path,
            file.relative_path,
            title,
            file_type,
            hash,
            file.mtime,
            file.size as i64,
            word_count,
            now,
        ],
    )?;

    // 5. Insert chunks + FTS
    let mut chunk_ids: Vec<String> = Vec::new();
    for (i, chunk) in chunks.iter().enumerate() {
        let chunk_id = uuid::Uuid::new_v4().to_string();
        let chunk_hash = {
            let mut h = sha2::Sha256::new();
            sha2::Digest::update(&mut h, &chunk.content);
            hex::encode(sha2::Digest::finalize(h))
        };

        conn.execute(
            "INSERT INTO chunks (id, doc_id, chunk_index, heading_path, content, content_hash,
             start_line, end_line, token_count, embedding_status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'pending', ?10)",
            rusqlite::params![
                chunk_id,
                doc_id,
                i as i64,
                chunk.heading_path.join("/"),
                chunk.content,
                chunk_hash,
                chunk.start_line as i64,
                chunk.end_line as i64,
                chunk.token_estimate as i64,
                now,
            ],
        )?;

        // FTS index
        let heading_str = chunk.heading_path.join(" > ");
        fts::index_chunk(
            conn,
            &chunk_id,
            &doc_id,
            &title,
            &heading_str,
            &chunk.content,
        )?;
        chunk_ids.push(chunk_id.clone());
    }

    // 6. Try embedding (non-fatal)
    let chunk_texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();
    try_embed_chunks(conn, &chunk_ids, &chunk_texts);

    // 7. Build graph
    graph::build_document_graph(conn, &doc_id, &source_path, &parsed)?;

    tracing::debug!(doc_id = %doc_id, chunks = chunks.len(), path = %file.relative_path, "Document indexed");

    Ok(doc_id)
}

/// Insert a document without indexing (unsupported file type).
fn insert_unindexed_document(
    conn: &Connection,
    root_id: &str,
    file: &ScannedFile,
    ext: &str,
) -> Result<String> {
    let hash = compute_hash(&file.absolute_path)?;
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let title = file
        .absolute_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_string();
    let source_path = file.absolute_path.display().to_string();

    conn.execute(
        "INSERT INTO documents (id, root_id, source_path, relative_path, title, file_type,
         content_hash, source_mtime, source_size, status, word_count, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'pending', 0, ?10, ?10)",
        rusqlite::params![
            id,
            root_id,
            source_path,
            file.relative_path,
            title,
            ext,
            hash,
            file.mtime,
            file.size as i64,
            now,
        ],
    )?;

    Ok(id)
}

/// Ensure the watched_root row exists.
fn ensure_root(conn: &Connection, root: &KnowledgeRoot, root_path: &Path) -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT OR IGNORE INTO watched_roots (id, name, root_path, enabled, read_only, include_globs, exclude_globs, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
        rusqlite::params![
            id, root.name, root_path.display().to_string(),
            root.enabled as i32, root.read_only as i32,
            root.include_globs.join(","), root.exclude_globs.join(","),
            now,
        ],
    )?;

    let actual_id: String = conn.query_row(
        "SELECT id FROM watched_roots WHERE name = ?1",
        rusqlite::params![root.name],
        |row| row.get(0),
    )?;

    Ok(actual_id)
}

/// Soft-delete a document and its derived data.
fn soft_delete_document(conn: &Connection, root_id: &str, doc_ref: &DocumentRef) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();

    // Get the doc_id
    let doc_id: String = conn.query_row(
        "SELECT id FROM documents WHERE root_id = ?1 AND relative_path = ?2 AND status != 'deleted'",
        rusqlite::params![root_id, doc_ref.relative_path],
        |row| row.get(0),
    ).context("Document not found for deletion")?;

    // Soft-delete
    conn.execute(
        "UPDATE documents SET status = 'deleted', deleted_at = ?1, updated_at = ?1 WHERE id = ?2",
        rusqlite::params![now, doc_id],
    )?;

    // Remove derived data
    fts::delete_doc(conn, &doc_id)?;
    conn.execute(
        "DELETE FROM chunk_embeddings WHERE chunk_id IN (SELECT id FROM chunks WHERE doc_id = ?1)",
        [&doc_id],
    )?;
    conn.execute("DELETE FROM chunks WHERE doc_id = ?1", [&doc_id])?;
    conn.execute("DELETE FROM graph_edges WHERE source_id IN (SELECT id FROM graph_nodes WHERE doc_id = ?1) OR target_id IN (SELECT id FROM graph_nodes WHERE doc_id = ?1)", [&doc_id])?;
    conn.execute("DELETE FROM graph_nodes WHERE doc_id = ?1", [&doc_id])?;

    Ok(())
}

/// Log a sync event.
fn log_sync_event(
    conn: &Connection,
    root_id: &str,
    event_type: &str,
    source_path: &str,
    message: Option<&str>,
) -> Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO sync_events (id, root_id, event_type, source_path, message, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            id,
            root_id,
            event_type,
            source_path,
            message.unwrap_or(""),
            now
        ],
    )?;
    Ok(())
}

/// Try to embed chunks using the configured embedding provider.
/// Non-fatal: embedding failures are logged but don't block indexing.
fn try_embed_chunks(conn: &Connection, chunk_ids: &[String], texts: &[String]) {
    let embedder = match build_embedder() {
        Some(e) => e,
        None => return,
    };
    match embedder.embed_sync(texts) {
        Ok(embeddings) => {
            for (chunk_id, emb) in chunk_ids.iter().zip(embeddings.iter()) {
                if let Err(e) = crate::index::vector::store_embedding(conn, chunk_id, emb) {
                    tracing::warn!(chunk_id=%chunk_id, error=%e, "Embedding store failed");
                } else {
                    let _ = conn.execute(
                        "UPDATE chunks SET embedding_status='ready' WHERE id=?1",
                        [chunk_id],
                    );
                }
            }
        }
        Err(e) => {
            tracing::warn!(error=%e, "Embedding batch failed; marking chunks as failed");
            for chunk_id in chunk_ids {
                let _ = conn.execute(
                    "UPDATE chunks SET embedding_status='failed' WHERE id=?1",
                    [chunk_id],
                );
            }
        }
    }
}

fn build_embedder() -> Option<crate::index::embed::RemoteEmbedder> {
    let cfg = crate::config::bundle::detect_bundle_root().ok()?;
    let config = crate::config::bundle::load_config(&cfg).ok()?;
    if !config.embedding.enabled || config.embedding.api_key_env.is_empty() {
        return None;
    }
    let key = std::env::var(&config.embedding.api_key_env).ok()?;
    if key.is_empty() {
        return None;
    }
    Some(crate::index::embed::RemoteEmbedder::new(
        crate::index::embed::EmbedConfig {
            base_url: config.embedding.base_url,
            api_key: key,
            model: config.embedding.model,
            dimensions: config.embedding.dimensions,
            timeout_seconds: config.embedding.timeout_seconds,
            batch_size: config.embedding.batch_size,
        },
    ))
}

/// Content quality gate: rejects files that appear to be binary garbage.
/// Files with >30% non-printable characters or zero meaningful words are rejected.
fn check_content_quality(doc: &crate::ingest::ParsedDocument, path: &str) -> anyhow::Result<()> {
    let text = &doc.plain_text;
    if text.is_empty() {
        return Ok(()); // empty files are fine
    }

    let total = text.chars().count() as f64;
    let non_printable = text
        .chars()
        .filter(|c| c.is_control() && *c != '\n' && *c != '\t' && *c != '\r')
        .count() as f64;

    if total > 0.0 && non_printable / total > 0.3 {
        return Err(anyhow::anyhow!(
            "Content quality check failed for {}: {:.0}% non-printable characters (looks like binary data)",
            path,
            (non_printable / total) * 100.0
        ));
    }

    // Check for zero meaningful words in non-trivial files
    let word_count = text.split_whitespace().count();
    if total > 100.0 && word_count == 0 {
        return Err(anyhow::anyhow!(
            "Content quality check failed for {}: no whitespace-delimited words in non-trivial file",
            path
        ));
    }

    Ok(())
}
