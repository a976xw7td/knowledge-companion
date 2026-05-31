//! Transport-agnostic MCP Server.
//!
//! Handles JSON-RPC request dispatch for the MCP protocol.
//! The server knows nothing about stdio, HTTP, or any IO mechanism —
//! it takes parsed JSON-RPC requests and returns JSON-RPC responses.
//!
//! This is the critical architecture decision: by keeping transport out of
//! the server, we enable future HTTP MCP support without code changes here.

use crate::mcp::protocol::{error_codes, JsonRpcRequest, JsonRpcResponse};
use crate::mcp::tools::ToolRegistry;

/// Server information sent in the `initialize` response.
#[derive(Debug, Clone)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

/// MCP protocol version we support.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Lightweight error for JSON-RPC request parsing failures.
///
/// This avoids `clippy::result_large_err` by keeping the error type small
/// (two fields). Callers can convert it into a `JsonRpcResponse` via `From`.
#[derive(Debug, Clone)]
pub struct ParseRequestError {
    pub message: String,
    pub code: i32,
}

impl From<ParseRequestError> for JsonRpcResponse {
    fn from(e: ParseRequestError) -> Self {
        JsonRpcResponse::error(None, e.code, &e.message)
    }
}

/// Transport-agnostic MCP server.
///
/// Holds a tool registry and responds to MCP protocol messages.
pub struct McpServer {
    pub registry: ToolRegistry,
    pub server_info: ServerInfo,
}

impl McpServer {
    /// Create a new MCP server with the given tool registry.
    pub fn new(registry: ToolRegistry, name: String, version: String) -> Self {
        Self {
            registry,
            server_info: ServerInfo { name, version },
        }
    }

    /// Handle an incoming JSON-RPC request.
    ///
    /// Returns:
    /// - `Some(response)` for requests that expect a response
    /// - `None` for notifications (which require no response)
    pub fn handle_request(&self, request: &JsonRpcRequest) -> Option<JsonRpcResponse> {
        tracing::debug!(
            method = %request.method,
            id = ?request.id,
            "Handling MCP request"
        );

        match request.method.as_str() {
            "initialize" => Some(self.handle_initialize(request)),
            "notifications/initialized" => {
                tracing::debug!("Received initialized notification");
                None // Notifications get no response
            }
            "tools/list" => Some(self.handle_tools_list(request)),
            "tools/call" => Some(self.handle_tools_call(request)),
            _ => Some(self.method_not_found(request)),
        }
    }

    /// Handle the `initialize` request.
    fn handle_initialize(&self, request: &JsonRpcRequest) -> JsonRpcResponse {
        JsonRpcResponse::success(
            request.id.clone(),
            serde_json::json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": self.server_info.name,
                    "version": self.server_info.version,
                }
            }),
        )
    }

    /// Handle the `tools/list` request.
    fn handle_tools_list(&self, request: &JsonRpcRequest) -> JsonRpcResponse {
        JsonRpcResponse::success(request.id.clone(), self.registry.to_tools_list())
    }

    /// Handle the `tools/call` request.
    fn handle_tools_call(&self, request: &JsonRpcRequest) -> JsonRpcResponse {
        let params = match &request.params {
            Some(p) => p,
            None => {
                return JsonRpcResponse::error(
                    request.id.clone(),
                    error_codes::INVALID_PARAMS,
                    "Missing params",
                );
            }
        };

        let tool_name = match params.get("name").and_then(|n| n.as_str()) {
            Some(name) => name,
            None => {
                return JsonRpcResponse::error(
                    request.id.clone(),
                    error_codes::INVALID_PARAMS,
                    "Missing tool name in params",
                );
            }
        };

        let arguments = params.get("arguments").cloned();

        tracing::info!(tool = %tool_name, "Executing tool");

        match self.registry.get(tool_name) {
            Some(tool) => {
                let result = tool.call(arguments);
                JsonRpcResponse::success(
                    request.id.clone(),
                    serde_json::to_value(result).unwrap_or_else(|e| {
                        serde_json::json!({
                            "content": [{"type": "text", "text": format!("Serialization error: {}", e)}],
                            "isError": true
                        })
                    }),
                )
            }
            None => JsonRpcResponse::error(
                request.id.clone(),
                error_codes::METHOD_NOT_FOUND,
                &format!("Tool not found: {}", tool_name),
            ),
        }
    }

    /// Handle unknown methods.
    fn method_not_found(&self, request: &JsonRpcRequest) -> JsonRpcResponse {
        JsonRpcResponse::error(
            request.id.clone(),
            error_codes::METHOD_NOT_FOUND,
            &format!("Method not found: {}", request.method),
        )
    }

    /// Parse a JSON-RPC request from a string.
    /// Returns a lightweight `ParseRequestError` on parse failure.
    /// The caller can convert this into a `JsonRpcResponse` via `.into()`.
    pub fn parse_request(line: &str) -> Result<JsonRpcRequest, ParseRequestError> {
        serde_json::from_str::<JsonRpcRequest>(line).map_err(|e| {
            tracing::warn!(error = %e, line = %line, "Failed to parse JSON-RPC request");
            ParseRequestError {
                message: format!("Parse error: {}", e),
                code: error_codes::PARSE_ERROR,
            }
        })
    }

    /// Serialize a response to a JSON string with a trailing newline.
    pub fn serialize_response(response: &JsonRpcResponse) -> Result<String, serde_json::Error> {
        let mut json = serde_json::to_string(response)?;
        json.push('\n');
        Ok(json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::tools::{Tool, ToolRegistry, ToolResult};
    use serde_json::Value;

    struct TestTool;
    impl Tool for TestTool {
        fn name(&self) -> &str {
            "test_tool"
        }
        fn description(&self) -> &str {
            "A test tool"
        }
        fn input_schema(&self) -> Value {
            serde_json::json!({"type": "object", "properties": {}, "required": []})
        }
        fn call(&self, _args: Option<Value>) -> ToolResult {
            ToolResult::text("test result".to_string())
        }
    }

    fn test_server() -> McpServer {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(TestTool));
        McpServer::new(registry, "test-server".to_string(), "0.1.0".to_string())
    }

    #[test]
    fn test_handle_initialize() {
        let server = test_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(1.into())),
            method: "initialize".to_string(),
            params: Some(
                serde_json::json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "test", "version": "1.0"}}),
            ),
        };
        let resp = server.handle_request(&req).unwrap();
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert_eq!(result["serverInfo"]["name"], "test-server");
    }

    #[test]
    fn test_handle_tools_list() {
        let server = test_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(2.into())),
            method: "tools/list".to_string(),
            params: None,
        };
        let resp = server.handle_request(&req).unwrap();
        let tools = &resp.result.unwrap()["tools"];
        assert_eq!(tools.as_array().unwrap().len(), 1);
        assert_eq!(tools[0]["name"], "test_tool");
    }

    #[test]
    fn test_handle_tools_call_success() {
        let server = test_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(3.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({"name": "test_tool", "arguments": {}})),
        };
        let resp = server.handle_request(&req).unwrap();
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let content = &result["content"].as_array().unwrap()[0];
        assert_eq!(content["text"], "test result");
    }

    #[test]
    fn test_handle_tools_call_not_found() {
        let server = test_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(4.into())),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({"name": "nonexistent", "arguments": {}})),
        };
        let resp = server.handle_request(&req).unwrap();
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, error_codes::METHOD_NOT_FOUND);
    }

    #[test]
    fn test_handle_unknown_method() {
        let server = test_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(5.into())),
            method: "unknown/method".to_string(),
            params: None,
        };
        let resp = server.handle_request(&req).unwrap();
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, error_codes::METHOD_NOT_FOUND);
    }

    #[test]
    fn test_notification_no_response() {
        let server = test_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: None,
            method: "notifications/initialized".to_string(),
            params: None,
        };
        let resp = server.handle_request(&req);
        assert!(resp.is_none());
    }

    #[test]
    fn test_serialize_response() {
        let resp = JsonRpcResponse::success(
            Some(Value::Number(1.into())),
            serde_json::json!({"ok": true}),
        );
        let json = McpServer::serialize_response(&resp).unwrap();
        assert!(json.ends_with('\n'));
        assert!(json.contains("\"ok\":true"));
    }
}
