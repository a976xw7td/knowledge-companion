//! KnowledgeCompanion MCP Server binary.
//!
//! Stdio MCP server for use by Claw, Claude Code, Hermes Agent, etc.

use anyhow::Result;
use knowledge_companion::{config, init_logging, run_mcp_server};

fn main() -> Result<()> {
    // Detect bundle root early for logging setup
    let bundle_root = config::bundle::detect_bundle_root()
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| "/".into()));

    let cfg = config::bundle::load_config(&bundle_root).unwrap_or_default();
    let log_dir = config::bundle::resolve_path(&bundle_root, &cfg.storage.log_dir);
    std::fs::create_dir_all(&log_dir).ok();

    init_logging(&log_dir);

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "KnowledgeCompanion MCP server starting"
    );

    run_mcp_server()
}
