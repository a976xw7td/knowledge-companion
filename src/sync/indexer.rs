//! Unified document indexer.
//!
//! Provides a single `index_document` function used by create, modify, and reindex.
//! All writes (document, chunks, FTS, graph) happen in one explicit transaction
//! using BEGIN IMMEDIATE / COMMIT / ROLLBACK.
//!
//! Handles: normal documents, empty documents, and unsupported file types
//! consistently — all go through the same soft-delete cleanup + atomic insert.

use anyhow::{Context, Result};
use rusqlite::Connection;
use sha2::Digest;

use crate::index::{fts, graph};
use crate::ingest::{chunker, ParserRegistry};

use super::planner::compute_hash;
use super::scanner::ScannedFile;

/// Index a scanned file into the database. All writes are atomic.
/// Returns (doc_id, chunk_ids) on success.
///
/// `max_file_size_bytes` comes from config `app.max_file_size_mb`.
/// Files exceeding this limit are rejected with a clear error before any IO.
pub fn index_document(
    conn: &Connection,
    root_id: &str,
    file: &ScannedFile,
    parser_registry: &ParserRegistry,
    chunk_config: &chunker::ChunkConfig,
    max_file_size_bytes: u64,
) -> Result<(String, Vec<String>)> {
    // 1. File size check — fail fast before reading content
    if file.size > max_file_size_bytes {
        anyhow::bail!(
            "File exceeds max size limit ({}MB): {}",
            max_file_size_bytes / 1_048_576,
            file.relative_path
        );
    }

    // 2. Determine parser
    let ext = file
        .absolute_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    // 3. Handle unsupported file types — insert as unindexed in transaction
    let parser = match parser_registry.find(&ext) {
        Some(p) => p,
        None => {
            return insert_unindexed_in_transaction(conn, root_id, file, &ext);
        }
    };

    // 4. Parse outside transaction (so parse failures don't need rollback)
    let parsed = parser
        .parse(&file.absolute_path)
        .with_context(|| format!("Parse failed: {}", file.relative_path))?;
    super::executor::check_content_quality(&parsed, &file.relative_path)?;
    let chunks = chunker::chunk_document(&parsed, chunk_config);

    // 5. Transactional writes
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| -> Result<(String, Vec<String>)> {
        // Clean up soft-deleted record with same path (file recovery)
        conn.execute(
            "DELETE FROM documents WHERE root_id=?1 AND relative_path=?2 AND status='deleted'",
            rusqlite::params![root_id, file.relative_path],
        )?;

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
            _ => &ext,
        };

        conn.execute(
            "INSERT INTO documents (id,root_id,source_path,relative_path,title,file_type,content_hash,source_mtime,source_size,status,word_count,created_at,updated_at,indexed_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'indexed',?10,?11,?11,?11)",
            rusqlite::params![doc_id, root_id, source_path, file.relative_path, title, file_type, hash, file.mtime, file.size as i64, word_count, now],
        )?;

        let mut chunk_ids = Vec::new();
        for (i, chunk) in chunks.iter().enumerate() {
            let chunk_id = uuid::Uuid::new_v4().to_string();
            let chunk_hash = {
                let mut h = sha2::Sha256::new();
                sha2::Digest::update(&mut h, chunk.content.as_bytes());
                hex::encode(sha2::Digest::finalize(h))
            };
            conn.execute(
                "INSERT INTO chunks (id,doc_id,chunk_index,heading_path,content,content_hash,start_line,end_line,token_count,embedding_status,created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'pending',?10)",
                rusqlite::params![chunk_id, doc_id, i as i64, chunk.heading_path.join("/"), chunk.content, chunk_hash, chunk.start_line as i64, chunk.end_line as i64, chunk.token_estimate as i64, now],
            )?;
            fts::index_chunk(
                conn,
                &chunk_id,
                &doc_id,
                &title,
                &chunk.heading_path.join(" > "),
                &chunk.content,
            )?;
            chunk_ids.push(chunk_id);
        }

        graph::build_document_graph(conn, &doc_id, &source_path, &parsed)?;

        Ok((doc_id, chunk_ids))
    })();

    match result {
        Ok(r) => {
            conn.execute_batch("COMMIT")?;
            Ok(r)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// Insert a document without parsing (unsupported file type or empty).
/// Uses the same transactional pattern as index_document.
fn insert_unindexed_in_transaction(
    conn: &Connection,
    root_id: &str,
    file: &ScannedFile,
    ext: &str,
) -> Result<(String, Vec<String>)> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| -> Result<(String, Vec<String>)> {
        // Consistent cleanup: same as index_document
        conn.execute(
            "DELETE FROM documents WHERE root_id=?1 AND relative_path=?2 AND status='deleted'",
            rusqlite::params![root_id, file.relative_path],
        )?;

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

        Ok((id, Vec::new()))
    })();
    match result {
        Ok(r) => {
            conn.execute_batch("COMMIT")?;
            Ok(r)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// Soft-delete a document and clean up derived data.
pub fn delete_document(conn: &Connection, doc_id: &str) -> Result<()> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| -> Result<()> {
        let exists: bool = conn.query_row(
            "SELECT COUNT(*)>0 FROM documents WHERE id=?1 AND status!='deleted'",
            [doc_id],
            |r| r.get(0),
        )?;
        if !exists {
            anyhow::bail!("Document not found or already deleted: {}", doc_id);
        }

        let now = chrono::Utc::now().to_rfc3339();
        let affected = conn.execute(
            "UPDATE documents SET status='deleted',deleted_at=?1,updated_at=?1 WHERE id=?2 AND status!='deleted'",
            rusqlite::params![now, doc_id],
        )?;
        if affected == 0 {
            anyhow::bail!("Document not found or already deleted: {}", doc_id);
        }
        fts::delete_doc(conn, doc_id)?;
        conn.execute("DELETE FROM chunk_embeddings WHERE chunk_id IN (SELECT id FROM chunks WHERE doc_id=?1)", [doc_id])?;
        conn.execute("DELETE FROM chunks WHERE doc_id=?1", [doc_id])?;
        conn.execute("DELETE FROM graph_edges WHERE source_id IN (SELECT id FROM graph_nodes WHERE doc_id=?1) OR target_id IN (SELECT id FROM graph_nodes WHERE doc_id=?1)", [doc_id])?;
        conn.execute("DELETE FROM graph_nodes WHERE doc_id=?1", [doc_id])?;
        // Also clean up index_jobs for this doc
        conn.execute("DELETE FROM index_jobs WHERE doc_id=?1", [doc_id])?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// Modify a document: parse new file first, then transactionally replace old with new.
pub fn modify_document(
    conn: &Connection,
    root_id: &str,
    file: &ScannedFile,
    parser_registry: &ParserRegistry,
    chunk_config: &chunker::ChunkConfig,
) -> Result<(String, Vec<String>)> {
    // Get old document ID
    let old_id: String = conn
        .query_row(
            "SELECT id FROM documents WHERE root_id=?1 AND relative_path=?2 AND status!='deleted'",
            rusqlite::params![root_id, file.relative_path],
            |r| r.get(0),
        )
        .context("Old document not found")?;

    // Parse new file FIRST (before touching old data)
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
    super::executor::check_content_quality(&parsed, &file.relative_path)?;
    let chunks = chunker::chunk_document(&parsed, chunk_config);

    // Transactional: delete old + insert new
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| -> Result<(String, Vec<String>)> {
        // Delete old derived data
        fts::delete_doc(conn, &old_id)?;
        conn.execute("DELETE FROM graph_edges WHERE source_id IN (SELECT id FROM graph_nodes WHERE doc_id=?1) OR target_id IN (SELECT id FROM graph_nodes WHERE doc_id=?1)", [&old_id])?;
        conn.execute("DELETE FROM graph_nodes WHERE doc_id=?1", [&old_id])?;
        conn.execute("DELETE FROM chunk_embeddings WHERE chunk_id IN (SELECT id FROM chunks WHERE doc_id=?1)", [&old_id])?;
        conn.execute("DELETE FROM chunks WHERE doc_id=?1", [&old_id])?;
        conn.execute("DELETE FROM index_jobs WHERE doc_id=?1", [&old_id])?;
        conn.execute("DELETE FROM documents WHERE id=?1", [&old_id])?;

        // Insert new
        let hash = compute_hash(&file.absolute_path)?;
        let doc_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let title = parsed.title.clone();
        let source_path = file.absolute_path.display().to_string();
        let file_type = match ext.as_str() {
            "md" | "markdown" => "markdown",
            "txt" => "txt",
            "pdf" => "pdf",
            "docx" => "docx",
            _ => &ext,
        };

        conn.execute(
            "INSERT INTO documents (id,root_id,source_path,relative_path,title,file_type,content_hash,source_mtime,source_size,status,word_count,created_at,updated_at,indexed_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'indexed',?10,?11,?11,?11)",
            rusqlite::params![doc_id, root_id, source_path, file.relative_path, title, file_type, hash, file.mtime, file.size as i64, parsed.plain_text.split_whitespace().count() as i32, now],
        )?;

        let mut chunk_ids = Vec::new();
        for (i, chunk) in chunks.iter().enumerate() {
            let chunk_id = uuid::Uuid::new_v4().to_string();
            let chunk_hash = {
                let mut h = sha2::Sha256::new();
                sha2::Digest::update(&mut h, chunk.content.as_bytes());
                hex::encode(sha2::Digest::finalize(h))
            };
            conn.execute(
                "INSERT INTO chunks (id,doc_id,chunk_index,heading_path,content,content_hash,start_line,end_line,token_count,embedding_status,created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'pending',?10)",
                rusqlite::params![chunk_id, doc_id, i as i64, chunk.heading_path.join("/"), chunk.content, chunk_hash, chunk.start_line as i64, chunk.end_line as i64, chunk.token_estimate as i64, now],
            )?;
            fts::index_chunk(
                conn,
                &chunk_id,
                &doc_id,
                &title,
                &chunk.heading_path.join(" > "),
                &chunk.content,
            )?;
            chunk_ids.push(chunk_id);
        }

        graph::build_document_graph(conn, &doc_id, &source_path, &parsed)?;
        Ok((doc_id, chunk_ids))
    })();
    match result {
        Ok(r) => {
            conn.execute_batch("COMMIT")?;
            Ok(r)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}
