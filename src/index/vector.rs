//! Vector embeddings and cosine similarity search.
//!
//! Supports remote embedding providers and local Rust cosine comparison.
//! Default: Rust cosine over stored BLOB embeddings — no dynamic extensions.
//!
//! Corrupted or malformed BLOBs are skipped with a warning rather than
//! causing the entire search to fail.

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

    /// Decode from BLOB. Returns None if the BLOB is invalid.
    /// - BLOB length must be a multiple of 4 (f32 = 4 bytes)
    /// - Decoded float count must match `dimensions`
    pub fn from_blob(data: &[u8], model: &str, dimensions: usize) -> Option<Self> {
        // BLOB must be non-empty and aligned to 4-byte f32 boundary
        if !data.len().is_multiple_of(4) {
            tracing::warn!(
                blob_len = data.len(),
                "Embedding BLOB length not a multiple of 4; skipping corrupted entry"
            );
            return None;
        }

        let vector: Vec<f32> = data
            .chunks(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();

        // If dimensions don't match expected, the BLOB is from a different model
        if vector.len() != dimensions {
            tracing::warn!(
                expected = dimensions,
                actual = vector.len(),
                "Embedding dimension mismatch; skipping entry (likely from different model)"
            );
            return None;
        }

        Some(Self {
            model: model.to_string(),
            dimensions,
            vector,
        })
    }

    /// Legacy decode from BLOB. Maintains backward compatibility but
    /// silently truncates/extends to match dimensions.
    #[allow(dead_code)]
    fn from_blob_legacy(data: &[u8], model: &str, dimensions: usize) -> Self {
        let vector: Vec<f32> = data
            .chunks(4)
            .take(dimensions)
            .filter(|c| c.len() == 4)
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
    // Validate: blob length should be dimensions * 4
    let expected_len = embedding.dimensions * 4;
    if blob.len() != expected_len {
        anyhow::bail!(
            "Embedding BLOB length mismatch: expected {} bytes for {} dimensions, got {}",
            expected_len,
            embedding.dimensions,
            blob.len()
        );
    }
    conn.execute(
        "INSERT OR REPLACE INTO chunk_embeddings (chunk_id, model, dimensions, embedding, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![chunk_id, embedding.model, embedding.dimensions as i64, blob, chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// Cosine similarity between two f32 slices.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
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
/// Corrupted BLOBs are skipped with a warning rather than causing errors.
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
        let (chunk_id, model, dims, blob) = match row {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error=%e, "Failed to read embedding row; skipping");
                continue;
            }
        };
        match Embedding::from_blob(&blob, &model, dims as usize) {
            Some(emb) => {
                let score = cosine_similarity(&query_embedding.vector, &emb.vector);
                scored.push((chunk_id, score));
            }
            None => {
                // from_blob already logged a warning with details
                continue;
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blob_roundtrip() {
        let emb = Embedding {
            model: "test".into(),
            dimensions: 4,
            vector: vec![1.0, 2.0, 3.0, 4.0],
        };
        let blob = emb.to_blob();
        let decoded = Embedding::from_blob(&blob, "test", 4).unwrap();
        assert_eq!(decoded.vector, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn test_blob_unaligned_length() {
        // 3 bytes — not divisible by 4
        let data = vec![0u8, 1, 2];
        let result = Embedding::from_blob(&data, "test", 1);
        assert!(result.is_none(), "Unaligned BLOB should return None");
    }

    #[test]
    fn test_blob_dimension_mismatch() {
        // 8 bytes = 2 floats, but we expect 3
        let data = vec![0u8; 8];
        let result = Embedding::from_blob(&data, "test", 3);
        assert!(result.is_none(), "Dimension mismatch should return None");
    }

    #[test]
    fn test_blob_empty() {
        let data = vec![];
        let result = Embedding::from_blob(&data, "test", 1);
        assert!(result.is_none(), "Empty BLOB should return None");
    }

    #[test]
    fn test_cosine_different_lengths() {
        let a = vec![1.0, 2.0];
        let b = vec![1.0, 2.0, 3.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }
}
