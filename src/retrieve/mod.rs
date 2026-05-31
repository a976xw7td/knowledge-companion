//! Hybrid retrieval engine.

use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct SearchItem {
    pub chunk_id: String,
    pub doc_id: String,
    pub title: String,
    pub source_path: String,
    pub heading_path: Vec<String>,
    pub snippet: String,
    pub score: f32,
    pub matched_by: Vec<String>,
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct RetrievalDiagnostics {
    pub retrieval_mode: String,
    pub semantic_available: bool,
    pub keyword_available: bool,
    pub graph_expanded: bool,
    pub degraded: bool,
    pub warnings: Vec<String>,
}

pub fn hybrid_search(
    conn: &Connection,
    query: &str,
    top_k: usize,
    fts_available: bool,
    query_embedding: Option<&[f32]>,
    graph_available: bool,
) -> Result<(Vec<SearchItem>, RetrievalDiagnostics)> {
    let mut items: Vec<SearchItem> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // Keyword search via FTS
    let keyword_used = if fts_available {
        match crate::index::fts::search(conn, query, top_k * 2) {
            Ok(fts_results) => {
                for fr in &fts_results {
                    if let Ok(Some(ci)) = get_chunk_info(conn, &fr.chunk_id) {
                        items.push(SearchItem {
                            chunk_id: fr.chunk_id.clone(),
                            doc_id: fr.doc_id.clone(),
                            title: fr.title.clone(),
                            source_path: ci.source_path,
                            heading_path: ci.heading_path,
                            snippet: fr.snippet.clone(),
                            score: 1.0 / (1.0 + fr.rank as f32),
                            matched_by: vec!["keyword".to_string()],
                            start_line: ci.start_line,
                            end_line: ci.end_line,
                        });
                    }
                }
                true
            }
            Err(e) => {
                warnings.push(format!("FTS: {}", e));
                false
            }
        }
    } else {
        false
    };

    // Semantic search via cosine if embedding provided
    let semantic_used = if let Some(q_emb) = query_embedding {
        let qe = crate::index::vector::Embedding {
            model: "query".into(),
            dimensions: q_emb.len(),
            vector: q_emb.to_vec(),
        };
        match crate::index::vector::cosine_search(conn, &qe, top_k) {
            Ok(vec_results) => {
                for vr in &vec_results {
                    if let Ok(Some(ci)) = get_chunk_info(conn, &vr.chunk_id) {
                        items.push(SearchItem {
                            chunk_id: vr.chunk_id.clone(),
                            doc_id: ci.doc_id.clone(),
                            title: String::new(),
                            source_path: ci.source_path,
                            heading_path: ci.heading_path,
                            snippet: String::new(),
                            score: vr.score,
                            matched_by: vec!["semantic".to_string()],
                            start_line: ci.start_line,
                            end_line: ci.end_line,
                        });
                    }
                }
                true
            }
            Err(e) => {
                warnings.push(format!("Cosine search: {}", e));
                false
            }
        }
    } else {
        false
    };

    // Deduplicate by chunk_id, merge matched_by
    let mut seen: std::collections::HashMap<String, SearchItem> = std::collections::HashMap::new();
    for item in items {
        if let Some(existing) = seen.get_mut(&item.chunk_id) {
            let mut m = existing.matched_by.clone();
            for mb in &item.matched_by {
                if !m.contains(mb) {
                    m.push(mb.clone());
                }
            }
            existing.matched_by = m;
            existing.score = existing.score.max(item.score);
        } else {
            seen.insert(item.chunk_id.clone(), item);
        }
    }
    let mut merged: Vec<SearchItem> = seen.into_values().collect();
    merged.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    merged.truncate(top_k);

    let diagnostics = RetrievalDiagnostics {
        retrieval_mode: if keyword_used && semantic_used {
            "hybrid".into()
        } else if keyword_used {
            "keyword".into()
        } else if semantic_used {
            "semantic".into()
        } else {
            "degraded".into()
        },
        semantic_available: semantic_used,
        keyword_available: keyword_used,
        graph_expanded: graph_available,
        degraded: merged.is_empty(),
        warnings,
    };

    Ok((merged, diagnostics))
}

struct ChunkInfo {
    source_path: String,
    heading_path: Vec<String>,
    start_line: usize,
    end_line: usize,
    #[allow(dead_code)]
    doc_id: String,
}

fn get_chunk_info(conn: &Connection, chunk_id: &str) -> Result<Option<ChunkInfo>> {
    conn.query_row(
        "SELECT c.heading_path, c.start_line, c.end_line, d.source_path, c.doc_id
         FROM chunks c JOIN documents d ON c.doc_id = d.id WHERE c.id = ?1",
        [chunk_id],
        |row| {
            Ok(ChunkInfo {
                heading_path: row
                    .get::<_, Option<String>>(0)?
                    .map(|s| s.split('/').map(|s| s.to_string()).collect())
                    .unwrap_or_default(),
                start_line: row.get::<_, Option<i64>>(1)?.unwrap_or(0) as usize,
                end_line: row.get::<_, Option<i64>>(2)?.unwrap_or(0) as usize,
                source_path: row.get(3)?,
                doc_id: row.get::<_, String>(4)?,
            })
        },
    )
    .map(Some)
    .or_else(|e| {
        tracing::warn!(chunk_id=%chunk_id, error=%e, "Chunk info");
        Ok(None)
    })
}
