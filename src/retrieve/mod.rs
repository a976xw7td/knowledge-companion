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
                            title: ci.doc_title,
                            source_path: ci.source_path,
                            heading_path: ci.heading_path,
                            snippet: ci.content_preview.chars().take(300).collect(),
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
                warnings.push(format!("Cosine: {}", e));
                false
            }
        }
    } else {
        false
    };

    // Graph expansion: follow wikilinks from top documents
    let graph_expanded = if graph_available && !items.is_empty() {
        let doc_ids: Vec<String> = items.iter().take(5).map(|i| i.doc_id.clone()).collect();
        let mut graph_items: Vec<SearchItem> = Vec::new();
        for doc_id in &doc_ids {
            // Collect wikilink labels first (avoids borrow conflict with conn)
            let labels: Vec<String> = {
                let mut stmt = conn.prepare(
                    "SELECT DISTINCT gn.label FROM graph_edges e
                     JOIN graph_nodes gn ON e.target_id = gn.id
                     WHERE e.source_id IN (SELECT id FROM graph_nodes WHERE doc_id = ?1)
                       AND e.relation = 'links_to' LIMIT 10",
                )?;
                let rows = stmt.query_map([doc_id], |r| r.get::<_, String>(0))?;
                rows.flatten().collect()
            };
            for label in labels {
                // Find documents with matching title (since wikilink nodes have NULL doc_id now)
                if let Ok(linked_doc) = conn.query_row(
                    "SELECT d.id, d.title, d.source_path FROM documents d
                     WHERE d.title = ?1 AND d.status = 'indexed' LIMIT 1",
                    [&label],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                        ))
                    },
                ) {
                    if let Ok(chunk) = conn.query_row(
                        "SELECT c.id, c.content, c.heading_path, c.start_line, c.end_line
                         FROM chunks c WHERE c.doc_id = ?1 LIMIT 1",
                        [&linked_doc.0],
                        |r| {
                            Ok((
                                r.get::<_, String>(0)?,
                                r.get::<_, String>(1)?,
                                r.get::<_, Option<String>>(2)?,
                                r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                                r.get::<_, Option<i64>>(4)?.unwrap_or(0),
                            ))
                        },
                    ) {
                        graph_items.push(SearchItem {
                            chunk_id: chunk.0,
                            doc_id: linked_doc.0,
                            title: linked_doc.1,
                            source_path: linked_doc.2,
                            heading_path: chunk
                                .2
                                .map(|s| s.split('/').map(String::from).collect())
                                .unwrap_or_default(),
                            snippet: chunk.1.chars().take(200).collect(),
                            score: 0.3,
                            matched_by: vec!["graph".to_string()],
                            start_line: chunk.3 as usize,
                            end_line: chunk.4 as usize,
                        });
                    }
                }
            }
        }
        let had = !graph_items.is_empty();
        items.extend(graph_items);
        had
    } else {
        false
    };

    // Expand: for each top-scoring document, include ALL its chunks so the LLM gets full context
    let top_docs: std::collections::HashSet<String> =
        items.iter().take(3).map(|i| i.doc_id.clone()).collect();
    for doc_id in &top_docs {
        if let Ok(mut stmt) = conn.prepare("SELECT id, content, heading_path, start_line, end_line FROM chunks WHERE doc_id = ?1 ORDER BY chunk_index")
        {
            if let Ok(rows) = stmt.query_map([doc_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                    row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                ))
            }) {
                for row in rows.flatten() {
                    let (chunk_id, content, heading_path, start_line, end_line) = row;
                    // Only add if this chunk is not already in results (avoid dup)
                    if !items.iter().any(|i| i.chunk_id == chunk_id) {
                        let hp = heading_path.map(|s| s.split('/').map(|s| s.to_string()).collect()).unwrap_or_default();
                        // Get title and source_path for this document (cached)
                        let (doc_title, doc_path): (String, String) = conn.query_row(
                            "SELECT title, source_path FROM documents WHERE id = ?1",
                            [doc_id],
                            |r| Ok((r.get(0)?, r.get(1)?)),
                        ).unwrap_or_default();
                        items.push(SearchItem {
                            chunk_id,
                            doc_id: doc_id.clone(),
                            title: doc_title,
                            source_path: doc_path,
                            heading_path: hp,
                            snippet: content.chars().take(300).collect(),
                            score: 0.1,
                            matched_by: vec!["document_expansion".to_string()],
                            start_line: start_line as usize,
                            end_line: end_line as usize,
                        });
                    }
                }
            }
        }
    }

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
        graph_expanded,
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
    doc_title: String,
    content_preview: String,
}

fn get_chunk_info(conn: &Connection, chunk_id: &str) -> Result<Option<ChunkInfo>> {
    conn.query_row(
        "SELECT c.heading_path, c.start_line, c.end_line, d.source_path, c.doc_id, c.content, d.title
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
                content_preview: row.get::<_, String>(5).unwrap_or_default(),
                doc_title: row.get::<_, String>(6)?,
            })
        },
    )
    .map(Some)
    .or_else(|e| {
        tracing::warn!(chunk_id=%chunk_id, error=%e, "Chunk info");
        Ok(None)
    })
}
