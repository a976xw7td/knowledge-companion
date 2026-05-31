//! Translation memory and glossary.
//!
//! SHA256-based translation cache with glossary-aware terminology injection.
//! Calls LLM on cache miss and stores results in TM.

use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::rag::llm::LlmConfig;

/// A translation request.
#[derive(Debug)]
pub struct TranslationRequest {
    pub source_lang: String,
    pub target_lang: String,
    pub text: String,
}

/// Translation result with cache status.
#[derive(Debug, Serialize)]
pub struct TranslationResult {
    pub translated_text: String,
    pub cache_hit: bool,
    pub glossary_terms_found: Vec<String>,
    pub degraded: bool,
    pub degraded_reason: Option<String>,
}

/// Look up a translation in the Translation Memory.
/// Returns cached translation if available, otherwise calls LLM.
pub fn translate(
    conn: &Connection,
    req: &TranslationRequest,
    llm_config: Option<&LlmConfig>,
) -> Result<TranslationResult> {
    let hash_key = compute_tm_hash(&req.source_lang, &req.target_lang, &req.text);

    // Check cache
    let cached: Option<String> = conn
        .query_row(
            "SELECT translated_text FROM translation_memory WHERE source_hash = ?1",
            [&hash_key],
            |row| row.get(0),
        )
        .ok();

    if let Some(cached_text) = cached {
        let _ = conn.execute(
            "UPDATE translation_memory SET hit_count = hit_count + 1, updated_at = ?1 WHERE source_hash = ?2",
            rusqlite::params![chrono::Utc::now().to_rfc3339(), hash_key],
        );
        return Ok(TranslationResult {
            translated_text: cached_text,
            cache_hit: true,
            glossary_terms_found: vec![],
            degraded: false,
            degraded_reason: None,
        });
    }

    // Query matching glossary terms
    let glossary_terms =
        query_glossary_for_translation(conn, &req.source_lang, &req.target_lang, &req.text)
            .unwrap_or_default();
    let glossary_term_strings: Vec<String> = glossary_terms
        .iter()
        .map(|(s, t)| format!("{}→{}", s, t))
        .collect();

    // Not in cache — call LLM if configured
    if let Some(cfg) = llm_config {
        if !cfg.api_key.is_empty() {
            let terms: Vec<(String, String)> = glossary_terms
                .iter()
                .map(|(s, t)| (s.clone(), t.clone()))
                .collect();
            match crate::rag::llm::translate_sync(
                cfg,
                &req.source_lang,
                &req.target_lang,
                &req.text,
                &terms,
            ) {
                Ok(translated) => {
                    // Store in TM
                    let id = uuid::Uuid::new_v4().to_string();
                    let now = chrono::Utc::now().to_rfc3339();
                    let _ = conn.execute(
                        "INSERT OR REPLACE INTO translation_memory (id, source_hash, source_text, translated_text, source_lang, target_lang, hit_count, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?7)",
                        rusqlite::params![id, hash_key, req.text, translated, req.source_lang, req.target_lang, now],
                    );
                    return Ok(TranslationResult {
                        translated_text: translated,
                        cache_hit: false,
                        glossary_terms_found: glossary_term_strings,
                        degraded: false,
                        degraded_reason: None,
                    });
                }
                Err(e) => {
                    tracing::warn!(error = %e, "LLM translation failed");
                    return Ok(TranslationResult {
                        translated_text: format!("[translation error: {}]", e),
                        cache_hit: false,
                        glossary_terms_found: glossary_term_strings,
                        degraded: true,
                        degraded_reason: Some(format!("LLM translation failed: {}", e)),
                    });
                }
            }
        }
    }

    // No LLM — return degraded placeholder
    Ok(TranslationResult {
        translated_text: format!("[{}→{}] {}", req.source_lang, req.target_lang, req.text),
        cache_hit: false,
        glossary_terms_found: glossary_term_strings,
        degraded: true,
        degraded_reason: Some(
            "LLM is not configured. Install an API key to enable translation.".to_string(),
        ),
    })
}

fn compute_tm_hash(source_lang: &str, target_lang: &str, text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source_lang.as_bytes());
    hasher.update(b":");
    hasher.update(target_lang.as_bytes());
    hasher.update(b":");
    hasher.update(text.as_bytes());
    hex::encode(hasher.finalize())
}

/// Query glossary for terms that appear in the source text.
fn query_glossary_for_translation(
    conn: &Connection,
    source_lang: &str,
    target_lang: &str,
    text: &str,
) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT source_term, target_term FROM glossary_entries WHERE source_lang = ?1 AND target_lang = ?2",
    )?;
    let all_terms: Vec<(String, String)> = stmt
        .query_map(rusqlite::params![source_lang, target_lang], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .filter_map(|r| r.ok())
        .collect();

    // Filter: only include terms that appear in the source text
    let lower_text = text.to_lowercase();
    Ok(all_terms
        .into_iter()
        .filter(|(src, _)| lower_text.contains(&src.to_lowercase()))
        .collect())
}

/// Glossary CRUD.
pub fn add_glossary_entry(
    conn: &Connection,
    source_term: &str,
    target_term: &str,
    source_lang: &str,
    target_lang: &str,
    category: Option<&str>,
) -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT OR REPLACE INTO glossary_entries (id, source_term, target_term, source_lang, target_lang, category, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
        rusqlite::params![id, source_term, target_term, source_lang, target_lang, category.unwrap_or(""), now],
    )?;
    Ok(id)
}

pub fn list_glossary(
    conn: &Connection,
    source_lang: Option<&str>,
    target_lang: Option<&str>,
) -> Result<Vec<serde_json::Value>> {
    let mut results: Vec<serde_json::Value> = Vec::new();
    let row_to_json = |row: &rusqlite::Row<'_>| -> rusqlite::Result<serde_json::Value> {
        Ok(serde_json::json!({
            "id": row.get::<_, String>(0)?, "source_term": row.get::<_, String>(1)?,
            "target_term": row.get::<_, String>(2)?, "source_lang": row.get::<_, String>(3)?,
            "target_lang": row.get::<_, String>(4)?, "category": row.get::<_, Option<String>>(5)?,
        }))
    };
    if let (Some(sl), Some(tl)) = (source_lang, target_lang) {
        let mut stmt = conn.prepare("SELECT id, source_term, target_term, source_lang, target_lang, category FROM glossary_entries WHERE source_lang = ?1 AND target_lang = ?2 ORDER BY source_term")?;
        for v in stmt
            .query_map(rusqlite::params![sl, tl], row_to_json)?
            .flatten()
        {
            results.push(v);
        }
    } else {
        let mut stmt = conn.prepare("SELECT id, source_term, target_term, source_lang, target_lang, category FROM glossary_entries ORDER BY source_term")?;
        for v in stmt.query_map([], row_to_json)?.flatten() {
            results.push(v);
        }
    }
    Ok(results)
}

pub fn delete_glossary_entry(conn: &Connection, entry_id: &str) -> Result<()> {
    conn.execute("DELETE FROM glossary_entries WHERE id = ?1", [entry_id])?;
    Ok(())
}
