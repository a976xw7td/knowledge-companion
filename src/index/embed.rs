//! OpenAI-compatible embedding provider.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::vector::Embedding;

/// Configuration for an embedding provider.
#[derive(Debug, Clone)]
pub struct EmbedConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub dimensions: usize,
    pub timeout_seconds: u64,
    pub batch_size: usize,
}

#[derive(Debug, Serialize)]
struct EmbedRequest {
    model: String,
    input: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct EmbedResponse {
    data: Vec<EmbedData>,
}

#[derive(Debug, Deserialize)]
struct EmbedData {
    embedding: Vec<f32>,
}

/// OpenAI-compatible embedding provider.
pub struct RemoteEmbedder {
    config: EmbedConfig,
    client: reqwest::Client,
}

impl RemoteEmbedder {
    pub fn new(config: EmbedConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_seconds))
            .build()
            .expect("Failed to create HTTP client");
        Self { config, client }
    }

    /// Synchronous embed: uses blocking reqwest for use from sync MCP tools and executor.
    pub fn embed_sync(&self, texts: &[String]) -> Result<Vec<Embedding>> {
        let url = format!("{}/embeddings", self.config.base_url.trim_end_matches('/'));
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(self.config.timeout_seconds))
            .build()
            .context("Failed to create blocking HTTP client")?;

        let mut all = Vec::new();
        for chunk in texts.chunks(self.config.batch_size) {
            let req_body = EmbedRequest {
                model: self.config.model.clone(),
                input: chunk.to_vec(),
            };
            let resp = client
                .post(&url)
                .header("Authorization", format!("Bearer {}", self.config.api_key))
                .header("Content-Type", "application/json")
                .json(&req_body)
                .send()?;
            if !resp.status().is_success() {
                let s = resp.status();
                let b = resp.text().unwrap_or_default();
                return Err(anyhow::anyhow!("Embedding API error ({}): {}", s, b));
            }
            let er: EmbedResponse = resp.json()?;
            for d in er.data {
                all.push(Embedding {
                    model: self.config.model.clone(),
                    dimensions: d.embedding.len(),
                    vector: d.embedding,
                });
            }
        }
        Ok(all)
    }
}

#[async_trait::async_trait]
impl super::vector::Embedder for RemoteEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Embedding>> {
        let url = format!("{}/embeddings", self.config.base_url.trim_end_matches('/'));

        let mut all_embeddings = Vec::new();

        // Batch requests
        for chunk in texts.chunks(self.config.batch_size) {
            let request = EmbedRequest {
                model: self.config.model.clone(),
                input: chunk.to_vec(),
            };

            let response = self
                .client
                .post(&url)
                .header("Authorization", format!("Bearer {}", self.config.api_key))
                .header("Content-Type", "application/json")
                .json(&request)
                .send()
                .await?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                return Err(anyhow::anyhow!(
                    "Embedding API error ({}): {}",
                    status,
                    body
                ));
            }

            let embed_response: EmbedResponse = response.json().await?;
            for data in embed_response.data {
                all_embeddings.push(Embedding {
                    model: self.config.model.clone(),
                    dimensions: data.embedding.len(),
                    vector: data.embedding,
                });
            }
        }

        Ok(all_embeddings)
    }

    fn dimensions(&self) -> usize {
        self.config.dimensions
    }

    fn model_name(&self) -> &str {
        &self.config.model
    }
}
