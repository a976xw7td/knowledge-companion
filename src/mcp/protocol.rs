//! MCP JSON-RPC 2.0 protocol types.
//!
//! Implements the transport-level types for the Model Context Protocol
//! (2024-11-05) over stdio. These types are transport-agnostic — they
//! could be reused for an HTTP MCP adapter in a future phase.

use serde::{Deserialize, Serialize};

/// A JSON-RPC 2.0 request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// A JSON-RPC 2.0 success response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// A JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl JsonRpcResponse {
    /// Create a success response with a result.
    pub fn success(id: Option<serde_json::Value>, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Create an error response.
    pub fn error(id: Option<serde_json::Value>, code: i32, message: &str) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.to_string(),
                data: None,
            }),
        }
    }
}

/// Standard JSON-RPC error codes.
pub mod error_codes {
    pub const PARSE_ERROR: i32 = -32700;
    #[allow(dead_code)]
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    #[allow(dead_code)]
    pub const INTERNAL_ERROR: i32 = -32603;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_initialize_request() {
        let json = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"claw","version":"1.0"}}}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "initialize");
        assert_eq!(req.id, Some(serde_json::Value::Number(1.into())));
    }

    #[test]
    fn test_serialize_initialize_response() {
        let resp = JsonRpcResponse::success(
            Some(serde_json::Value::Number(1.into())),
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": {
                    "name": "knowledge-companion",
                    "version": "0.1.0"
                }
            }),
        );
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"protocolVersion\""));
        assert!(json.contains("\"knowledge-companion\""));
        assert!(json.contains("\"2.0\""));
    }

    #[test]
    fn test_parse_tools_call_request() {
        let json = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"health_check","arguments":{}}}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "tools/call");
        assert_eq!(req.id, Some(serde_json::Value::Number(3.into())));

        let params = req.params.unwrap();
        assert_eq!(params["name"], "health_check");
    }

    #[test]
    fn test_serialize_tools_list_response() {
        let resp = JsonRpcResponse::success(
            Some(serde_json::Value::Number(2.into())),
            serde_json::json!({
                "tools": [
                    {
                        "name": "health_check",
                        "description": "Check system health",
                        "inputSchema": { "type": "object", "properties": {} }
                    }
                ]
            }),
        );
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"tools\""));
        assert!(json.contains("\"health_check\""));
    }

    #[test]
    fn test_error_response() {
        let resp = JsonRpcResponse::error(
            Some(serde_json::Value::Number(1.into())),
            error_codes::METHOD_NOT_FOUND,
            "Method not found",
        );
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"code\":-32601"));
        assert!(json.contains("\"Method not found\""));
    }

    #[test]
    fn test_notification_no_id() {
        let json = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "notifications/initialized");
        assert_eq!(req.id, None);
    }
}
