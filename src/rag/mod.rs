//! RAG (Retrieval-Augmented Generation) engine.
//!
//! Combines hybrid retrieval with LLM answers, producing cited responses.
//! When LLM is configured and available, generates answers with [S1], [S2] citations.
//! When unavailable, returns degraded mode with retrieval results.

pub mod citations;
pub mod llm;

use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;

use crate::retrieve;

/// A RAG answer with citations.
#[derive(Debug, Serialize)]
pub struct RagAnswer {
    pub question: String,
    pub answer: String,
    pub citations: Vec<citations::Citation>,
    pub sources: Vec<SourceRef>,
    pub degraded: bool,
    pub degraded_reason: Option<String>,
    pub diagnostics: Option<retrieve::RetrievalDiagnostics>,
}

/// A source reference linked to a citation.
#[derive(Debug, Clone, Serialize)]
pub struct SourceRef {
    pub source_id: String,
    pub chunk_id: String,
    pub doc_id: String,
    pub title: String,
    pub source_path: String,
    pub heading_path: Vec<String>,
    pub start_line: usize,
    pub end_line: usize,
    pub excerpt: String,
}

/// LLM configuration for RAG.
pub struct LlmConfig {
    pub enabled: bool,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub timeout_seconds: u64,
}

/// Ask a knowledge question using retrieval + LLM.
pub fn ask_question(
    conn: &Connection,
    question: &str,
    top_k: usize,
    llm_config: Option<&LlmConfig>,
    fts_available: bool,
    query_embedding: Option<&[f32]>,
) -> Result<RagAnswer> {
    let (mut items, mut diagnostics) =
        retrieve::hybrid_search(conn, question, top_k, fts_available, query_embedding, true)?;

    // If degraded or too few results, retry with just the entity keywords
    if diagnostics.degraded || items.len() < 2 {
        let simple = question
            .split(['的', '是', '在', '？', '?'])
            .next()
            .unwrap_or(question)
            .trim();
        if simple.len() < question.len() {
            let (items2, diag2) =
                retrieve::hybrid_search(conn, simple, top_k, fts_available, None, true)?;
            items.extend(items2);
            diagnostics = diag2;
        }
    }

    let sources: Vec<SourceRef> = items
        .iter()
        .map(|item| SourceRef {
            source_id: format!("S-{}", item.chunk_id),
            chunk_id: item.chunk_id.clone(),
            doc_id: item.doc_id.clone(),
            title: item.title.clone(),
            source_path: item.source_path.clone(),
            heading_path: item.heading_path.clone(),
            start_line: item.start_line,
            end_line: item.end_line,
            excerpt: item.snippet.clone(),
        })
        .collect();

    let citations: Vec<citations::Citation> = sources
        .iter()
        .map(|s| citations::Citation::new(&s.source_id, &s.title, &s.source_path))
        .collect();

    // Build context for LLM — limit chunks and chars
    const MAX_CONTEXT_CHARS: usize = 12000;
    const MAX_CONTEXT_CHUNKS: usize = 15;
    let sources = if sources.len() > MAX_CONTEXT_CHUNKS {
        sources[..MAX_CONTEXT_CHUNKS].to_vec()
    } else {
        sources
    };
    let context: String = {
        let mut ctx = String::new();
        for s in &sources {
            if ctx.len() >= MAX_CONTEXT_CHARS {
                break;
            }
            let content = conn
                .query_row(
                    "SELECT coalesce(content, '') FROM chunks WHERE id = ?1",
                    rusqlite::params![&s.chunk_id],
                    |row| row.get::<_, String>(0),
                )
                .unwrap_or_else(|_| "[...]".to_string());
            let block = format!("[{}] ({}): {}\n\n", s.source_id, s.source_path, content);
            if ctx.len() + block.len() > MAX_CONTEXT_CHARS {
                ctx.push_str(&block[..(MAX_CONTEXT_CHARS - ctx.len()).min(100)]);
                ctx.push_str("...");
                break;
            }
            ctx.push_str(&block);
        }
        ctx
    };

    // Try LLM if configured
    if let Some(cfg) = llm_config {
        if cfg.enabled && !cfg.api_key.is_empty() {
            let llm_cfg = llm::LlmConfig {
                base_url: cfg.base_url.clone(),
                api_key: cfg.api_key.clone(),
                model: cfg.model.clone(),
                timeout_seconds: cfg.timeout_seconds,
            };

            match llm::ask_sync(&llm_cfg, question, &context) {
                Ok(answer) => {
                    // Validate citations in the answer
                    let valid_ids: Vec<String> =
                        sources.iter().map(|s| s.source_id.clone()).collect();
                    let invalid = citations::validate_citations(&answer, &valid_ids);
                    let mut degraded = false;
                    let mut degraded_reason = None;

                    if !invalid.is_empty() {
                        degraded = true;
                        degraded_reason = Some(format!(
                            "LLM generated {} invalid citation(s): {}. Answer may contain fabricated references.",
                            invalid.len(),
                            invalid.join(", ")
                        ));
                    }

                    return Ok(RagAnswer {
                        question: question.to_string(),
                        answer,
                        citations,
                        sources,
                        degraded,
                        degraded_reason,
                        diagnostics: Some(diagnostics),
                    });
                }
                Err(e) => {
                    tracing::warn!(error = %e, "LLM call failed, returning degraded result");
                    return Ok(RagAnswer {
                        question: question.to_string(),
                        answer: format!(
                            "LLM unavailable ({}). Retrieved {} relevant documents:\n\n{}",
                            e,
                            sources.len(),
                            context
                        ),
                        citations,
                        sources,
                        degraded: true,
                        degraded_reason: Some(format!("LLM call failed: {}", e)),
                        diagnostics: Some(diagnostics),
                    });
                }
            }
        }
    }

    // No LLM configured — return retrieved context
    Ok(RagAnswer {
        question: question.to_string(),
        answer: format!(
            "LLM not configured. Retrieved {} relevant documents:\n\n{}",
            sources.len(),
            context,
        ),
        citations,
        sources,
        degraded: true,
        degraded_reason: Some("LLM is not enabled or API key is missing".to_string()),
        diagnostics: Some(diagnostics),
    })
}

/// Get detailed source information for citation IDs.
pub fn get_sources(conn: &Connection, source_ids: &[String]) -> Result<Vec<SourceRef>> {
    let mut sources = Vec::new();

    for sid in source_ids {
        let chunk_id = sid.trim_start_matches("S-");
        if chunk_id.is_empty() {
            continue;
        }

        if let Ok(row) = conn.query_row(
            "SELECT c.id, c.doc_id, c.heading_path, c.content, c.start_line, c.end_line,
                    d.title, d.source_path
             FROM chunks c JOIN documents d ON c.doc_id = d.id
             WHERE c.id = ?1",
            [chunk_id],
            |row| {
                Ok(SourceRef {
                    source_id: sid.clone(),
                    chunk_id: row.get(0)?,
                    doc_id: row.get(1)?,
                    title: row.get(6)?,
                    source_path: row.get(7)?,
                    heading_path: row
                        .get::<_, Option<String>>(2)?
                        .map(|s| s.split('/').map(String::from).collect())
                        .unwrap_or_default(),
                    start_line: row.get::<_, Option<i64>>(4)?.unwrap_or(0) as usize,
                    end_line: row.get::<_, Option<i64>>(5)?.unwrap_or(0) as usize,
                    excerpt: row.get::<_, String>(3)?,
                })
            },
        ) {
            sources.push(row);
        }
    }

    Ok(sources)
}
