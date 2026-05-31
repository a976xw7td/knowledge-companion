//! MCP transport layer.
//!
//! Contains:
//! - `protocol`: JSON-RPC 2.0 types
//! - `tools`: Tool trait and registry (architecture boundary)
//! - `server`: Transport-agnostic MCP server logic
//! - `adapter`: Stdio transport adapter

pub mod adapter;
pub mod protocol;
pub mod server;
pub mod tools;
