//! Vector embeddings and cosine similarity search.
//!
//! Supports remote embedding providers and local Rust cosine comparison.
//! Default: Rust cosine over stored BLOB embeddings — no dynamic extensions.

use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;

/// An embedding vector.
#[derive(Debug, Clone)]
pub struct Embedding {
    pub model: String,
    pub dimensions: usize,
    pub vector: Vec<f32>,
}

impl Embedding {
    /// Encode to BLOB for storage.
    pub fn to_blob(&self) -> Vec<u8> {
        self.vector.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    /// Decode from BLOB.
    pub fn from_blob(data: &[u8], model: &str, dimensions: usize) -> Self {
        let vector: Vec<f32> = data
            .chunks(4)
            .take(dimensions)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        Self {
            model: model.to_string(),
            dimensions,
            vector,
        }
    }
}

/// Store an embedding for a chunk.
pub fn store_embedding(conn: &Connection, chunk_id: &str, embedding: &Embedding) -> Result<()> {
    let blob = embedding.to_blob();
    conn.execute(
        "INSERT OR REPLACE INTO chunk_embeddings (chunk_id, model, dimensions, embedding, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![chunk_id, embedding.model, embedding.dimensions as i64, blob, chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// Cosine similarity between two f32 slices.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

/// Search embeddings by cosine similarity (in-memory Rust comparison).
pub fn cosine_search(
    conn: &Connection,
    query_embedding: &Embedding,
    limit: usize,
) -> Result<Vec<VectorResult>> {
    let mut stmt =
        conn.prepare("SELECT chunk_id, model, dimensions, embedding FROM chunk_embeddings")?;

    let mut scored: Vec<(String, f32)> = Vec::new();

    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, Vec<u8>>(3)?,
        ))
    })?;

    for row in rows {
        let (chunk_id, model, dims, blob) = row?;
        let emb = Embedding::from_blob(&blob, &model, dims as usize);
        let score = cosine_similarity(&query_embedding.vector, &emb.vector);
        scored.push((chunk_id, score));
    }

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);

    Ok(scored
        .into_iter()
        .map(|(chunk_id, score)| VectorResult { chunk_id, score })
        .collect())
}

#[derive(Debug, Clone, Serialize)]
pub struct VectorResult {
    pub chunk_id: String,
    pub score: f32,
}

/// Embedder trait — implementors call remote APIs or local models.
#[async_trait::async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Embedding>>;
    fn dimensions(&self) -> usize;
    fn model_name(&self) -> &str;
}

/// A no-op embedder for when embeddings are disabled.
pub struct NoopEmbedder;

#[async_trait::async_trait]
impl Embedder for NoopEmbedder {
    async fn embed(&self, _texts: &[String]) -> Result<Vec<Embedding>> {
        Err(anyhow::anyhow!("Embedding is disabled"))
    }
    fn dimensions(&self) -> usize {
        0
    }
    fn model_name(&self) -> &str {
        "noop"
    }
}
