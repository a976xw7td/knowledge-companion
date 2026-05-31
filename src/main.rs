//! KnowledgeCompanion MCP Server — Portable Personal Knowledge Management
//!
//! A local-first MCP (Model Context Protocol) server that indexes
//! knowledge folders and provides search, RAG, graph exploration,
//! and translation tools to AI agents.
//!
//! Phase 0: Minimal MCP server with health_check and get_knowledge_stats.
//! Future phases will add sync, indexing, RAG, and more.

mod config;
mod mcp;
mod services;

use anyhow::Result;
use mcp::adapter::StdioAdapter;
use mcp::server::McpServer;
use mcp::tools::{Tool, ToolRegistry, ToolResult};
use serde_json::Value;
use tracing_subscriber::EnvFilter;

/// Tool: health_check
struct HealthCheckTool;

impl Tool for HealthCheckTool {
    fn name(&self) -> &str {
        "health_check"
    }

    fn description(&self) -> &str {
        "检查知识库系统的健康状态。返回 bundle root 位置、配置加载状态、\
         knowledge/ 目录可访问性、data/ 目录可写性。\
         当 LLM 或 embedding 不可用时，此工具会报告降级状态。"
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    fn call(&self, _arguments: Option<Value>) -> ToolResult {
        let status = services::health::check_health();
        ToolResult::json(&status)
    }
}

/// Tool: get_knowledge_stats
struct KnowledgeStatsTool;

impl Tool for KnowledgeStatsTool {
    fn name(&self) -> &str {
        "get_knowledge_stats"
    }

    fn description(&self) -> &str {
        "返回知识库的统计信息，包括文档数量、chunks 数量、标签数、wikilink 数、\
         存储占用。Phase 0 仅统计 knowledge/ 目录中的文件数量；\
         完整的索引统计将在后续阶段提供。"
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    fn call(&self, _arguments: Option<Value>) -> ToolResult {
        let stats = services::stats::get_stats();
        ToolResult::json(&stats)
    }
}

fn main() -> Result<()> {
    // Initialize logging to stderr (does not interfere with MCP stdio)
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "KnowledgeCompanion starting"
    );

    // Detect bundle root and log it
    match config::bundle::detect_bundle_root() {
        Ok(root) => tracing::info!(bundle_root = %root.display(), "Bundle root detected"),
        Err(e) => tracing::warn!("Bundle root detection failed: {}", e),
    }

    // Build the tool registry
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(HealthCheckTool));
    registry.register(Box::new(KnowledgeStatsTool));

    tracing::info!(tool_count = registry.list().len(), "Tools registered");

    // Build the MCP server
    let server = McpServer::new(
        registry,
        "knowledge-companion".to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
    );

    // Run with stdio adapter
    let adapter = StdioAdapter::new(server);
    adapter.run()
}
