//! FTS5 full-text keyword search.

use anyhow::Result;
use rusqlite::Connection;

/// Index a chunk in the FTS5 table.
pub fn index_chunk(
    conn: &Connection,
    chunk_id: &str,
    doc_id: &str,
    title: &str,
    heading: &str,
    content: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO chunks_fts (chunk_id, doc_id, title, heading, content) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![chunk_id, doc_id, title, heading, content],
    )?;
    Ok(())
}

/// Delete FTS entries for a document.
pub fn delete_doc(conn: &Connection, doc_id: &str) -> Result<()> {
    conn.execute("DELETE FROM chunks_fts WHERE doc_id = ?1", [doc_id])?;
    Ok(())
}

/// Search FTS5 with BM25 ranking.
pub fn search(conn: &Connection, query: &str, limit: usize) -> Result<Vec<FtsResult>> {
    // Sanitize query for FTS5: strip special chars that cause syntax errors.
    // FTS5 special: * prefix wildcard, " phrase, - NOT, () grouping, : column filter.
    // Strategy: strip all problematic chars, then tokenize with wildcard suffix.
    let sanitized: String = query
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '*' || *c == '-')
        .collect();
    let sanitized = sanitized.trim();
    if sanitized.is_empty() {
        return Ok(Vec::new());
    }
    let fts_query = sanitized
        .split_whitespace()
        .map(|w| {
            if w.contains('*') || w.starts_with('-') {
                w.to_string()
            } else {
                format!("{}*", w)
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    let mut stmt = conn.prepare(
        "SELECT c.id as chunk_id, c.doc_id, fts.title, fts.heading, snippet(chunks_fts, 2, '<b>', '</b>', '...', 32) as snippet,
                rank
         FROM chunks_fts fts
         JOIN chunks c ON fts.chunk_id = c.id
         WHERE chunks_fts MATCH ?1
         ORDER BY rank
         LIMIT ?2",
    )?;

    let results = stmt.query_map(rusqlite::params![fts_query, limit as i64], |row| {
        Ok(FtsResult {
            chunk_id: row.get(0)?,
            doc_id: row.get(1)?,
            title: row.get(2)?,
            heading: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
            snippet: row.get(4)?,
            rank: row.get(5)?,
        })
    })?;

    let mut items = Vec::new();
    for r in results {
        match r {
            Ok(item) => items.push(item),
            Err(e) => tracing::warn!(error = %e, "FTS result row error"),
        }
    }

    Ok(items)
}

#[derive(Debug, Clone)]
pub struct FtsResult {
    pub chunk_id: String,
    pub doc_id: String,
    pub title: String,
    pub heading: String,
    pub snippet: String,
    pub rank: f64,
}
