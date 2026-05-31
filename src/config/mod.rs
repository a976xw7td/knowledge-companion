//! Configuration loading for KnowledgeCompanion.
//!
//! The config is read from `config/knowledge-companion.toml` relative to
//! the bundle root. All paths are resolved against the bundle root.

pub mod bundle;

use serde::{Deserialize, Serialize};

/// Top-level application configuration.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub app: AppSection,
    #[serde(default)]
    pub storage: StorageSection,
    #[serde(default)]
    pub knowledge: KnowledgeRoots,
    #[serde(default)]
    pub llm: LlmSection,
    #[serde(default)]
    pub embedding: EmbeddingSection,
    #[serde(default)]
    pub retrieval: RetrievalSection,
    #[serde(default)]
    pub http_mcp: HttpMcpSection,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AppSection {
    #[serde(default = "default_name")]
    pub name: String,
    #[serde(default = "default_sync_interval")]
    pub sync_interval_seconds: u64,
    #[serde(default = "default_max_file_size")]
    pub max_file_size_mb: u64,
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StorageSection {
    #[serde(default = "default_db_path")]
    pub db_path: String,
    #[serde(default = "default_cache_dir")]
    pub cache_dir: String,
    #[serde(default = "default_log_dir")]
    pub log_dir: String,
}

/// Multi-root knowledge configuration: `[[knowledge.roots]]`.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct KnowledgeRoots {
    #[serde(default)]
    pub roots: Vec<KnowledgeRoot>,
}

/// A single watched knowledge root.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct KnowledgeRoot {
    #[serde(default)]
    pub name: String,
    pub path: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub read_only: bool,
    #[serde(default = "default_include_globs")]
    pub include_globs: Vec<String>,
    #[serde(default = "default_exclude_globs")]
    pub exclude_globs: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LlmSection {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_llm_url")]
    pub base_url: String,
    #[serde(default)]
    pub api_key_env: String,
    #[serde(default = "default_qa_model")]
    pub model_qa: String,
    #[serde(default = "default_qa_model")]
    pub model_translate: String,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EmbeddingSection {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_embed_url")]
    pub base_url: String,
    #[serde(default)]
    pub api_key_env: String,
    #[serde(default = "default_embed_model")]
    pub model: String,
    #[serde(default = "default_dimensions")]
    pub dimensions: usize,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RetrievalSection {
    #[serde(default = "default_vector_backend")]
    pub vector_backend: String,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    #[serde(default = "default_chunk_tokens")]
    pub chunk_target_tokens: usize,
    #[serde(default = "default_overlap_tokens")]
    pub chunk_overlap_tokens: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HttpMcpSection {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub token_env: String,
    #[serde(default = "default_rpm")]
    pub requests_per_minute: u32,
    #[serde(default = "default_http_timeout")]
    pub timeout_seconds: u64,
}

// ── Default values ─────────────────────────────────────────────────────────

fn default_name() -> String {
    "KnowledgeCompanion".into()
}
fn default_sync_interval() -> u64 {
    300
}
fn default_max_file_size() -> u64 {
    50
}
fn default_log_level() -> String {
    "info".into()
}
fn default_db_path() -> String {
    "./data/knowledge.db".into()
}
fn default_cache_dir() -> String {
    "./data/cache".into()
}
fn default_log_dir() -> String {
    "./data/logs".into()
}
fn default_true() -> bool {
    true
}
fn default_include_globs() -> Vec<String> {
    vec![
        "**/*.md".into(),
        "**/*.markdown".into(),
        "**/*.txt".into(),
        "**/*.pdf".into(),
        "**/*.docx".into(),
    ]
}
fn default_exclude_globs() -> Vec<String> {
    vec![
        "**/.git/**".into(),
        "**/.svn/**".into(),
        "**/node_modules/**".into(),
        "**/target/**".into(),
        "**/.env".into(),
        "**/.obsidian/workspace*.json".into(),
    ]
}
fn default_llm_url() -> String {
    "https://api.openai.com/v1".into()
}
fn default_qa_model() -> String {
    "gpt-4.1-mini".into()
}
fn default_embed_url() -> String {
    "https://api.siliconflow.cn/v1".into()
}
fn default_embed_model() -> String {
    "BAAI/bge-m3".into()
}
fn default_dimensions() -> usize {
    1024
}
fn default_timeout() -> u64 {
    60
}
fn default_batch_size() -> usize {
    16
}
fn default_vector_backend() -> String {
    "rust_cosine".into()
}
fn default_top_k() -> usize {
    12
}
fn default_chunk_tokens() -> usize {
    650
}
fn default_overlap_tokens() -> usize {
    80
}
fn default_bind() -> String {
    "127.0.0.1".into()
}
fn default_port() -> u16 {
    18791
}
fn default_rpm() -> u32 {
    60
}
fn default_http_timeout() -> u64 {
    120
}

// ── Default impls for serde ────────────────────────────────────────────────

impl Default for AppSection {
    fn default() -> Self {
        Self {
            name: default_name(),
            sync_interval_seconds: default_sync_interval(),
            max_file_size_mb: default_max_file_size(),
            log_level: default_log_level(),
        }
    }
}
impl Default for StorageSection {
    fn default() -> Self {
        Self {
            db_path: default_db_path(),
            cache_dir: default_cache_dir(),
            log_dir: default_log_dir(),
        }
    }
}
impl Default for LlmSection {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: default_llm_url(),
            api_key_env: String::new(),
            model_qa: default_qa_model(),
            model_translate: default_qa_model(),
            timeout_seconds: default_timeout(),
        }
    }
}
impl Default for EmbeddingSection {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: default_embed_url(),
            api_key_env: String::new(),
            model: default_embed_model(),
            dimensions: default_dimensions(),
            timeout_seconds: default_timeout(),
            batch_size: default_batch_size(),
        }
    }
}
impl Default for RetrievalSection {
    fn default() -> Self {
        Self {
            vector_backend: default_vector_backend(),
            top_k: default_top_k(),
            chunk_target_tokens: default_chunk_tokens(),
            chunk_overlap_tokens: default_overlap_tokens(),
        }
    }
}
impl Default for HttpMcpSection {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: default_bind(),
            port: default_port(),
            token_env: String::new(),
            requests_per_minute: default_rpm(),
            timeout_seconds: default_http_timeout(),
        }
    }
}

// AppConfig Default is derived via #[derive(Default)]
