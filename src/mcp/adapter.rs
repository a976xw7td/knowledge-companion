//! Stdio MCP transport adapter.
//!
//! Reads JSON-RPC requests from stdin line by line, delegates to McpServer,
//! and writes responses to stdout. Logs go to stderr to avoid interfering
//! with the MCP protocol stream.

use crate::mcp::protocol::JsonRpcResponse;
use crate::mcp::server::McpServer;
use anyhow::Result;
use std::io::{BufRead, BufReader, Write};

/// An MCP transport adapter that communicates over stdin/stdout.
///
/// This is a blocking synchronous implementation. Phase 0 does not
/// require concurrency — one request at a time is sufficient.
/// Future phases may add a tokio-based adapter for HTTP MCP.
pub struct StdioAdapter {
    server: McpServer,
}

impl StdioAdapter {
    pub fn new(server: McpServer) -> Self {
        Self { server }
    }

    /// Run the stdio MCP server loop.
    ///
    /// This function blocks indefinitely, reading JSON-RPC requests
    /// from stdin and writing responses to stdout. It returns only
    /// on EOF or fatal I/O error.
    pub fn run(&self) -> Result<()> {
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();

        let reader = BufReader::new(stdin.lock());
        let mut writer = stdout.lock();

        tracing::info!("KnowledgeCompanion MCP stdio server started");

        for line_result in reader.lines() {
            let line = match line_result {
                Ok(l) => l,
                Err(e) => {
                    tracing::error!(error = %e, "Failed to read from stdin");
                    break;
                }
            };

            if line.trim().is_empty() {
                continue;
            }

            tracing::trace!(line = %line, "Received MCP message");

            // Parse the request
            let request = match McpServer::parse_request(&line) {
                Ok(req) => req,
                Err(e) => {
                    let error_response: JsonRpcResponse = e.into();
                    // Write parse error back
                    if let Ok(json) = McpServer::serialize_response(&error_response) {
                        let _ = writer.write_all(json.as_bytes());
                        let _ = writer.flush();
                    }
                    continue;
                }
            };

            // Handle the request
            if let Some(response) = self.server.handle_request(&request) {
                match McpServer::serialize_response(&response) {
                    Ok(json) => {
                        if let Err(e) = writer.write_all(json.as_bytes()) {
                            tracing::error!(error = %e, "Failed to write response to stdout");
                            break;
                        }
                        if let Err(e) = writer.flush() {
                            tracing::error!(error = %e, "Failed to flush stdout");
                            break;
                        }
                        tracing::trace!(response = %json.trim(), "Sent MCP response");
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to serialize response");
                    }
                }
            }
        }

        tracing::info!("KnowledgeCompanion MCP stdio server stopped");
        Ok(())
    }
}
