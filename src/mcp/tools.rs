//! Tool trait and registry.
//!
//! The `Tool` trait is the critical architecture boundary. Tool implementations
//! know nothing about MCP transport — they only see typed arguments and return
//! typed results. This allows the same tools to be exposed over stdio MCP,
//! HTTP MCP, or any future transport without code duplication.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Result of calling a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub content: Vec<ToolContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

/// A single content block in a tool result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolContent {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: String,
}

impl ToolResult {
    /// Create a successful text result.
    pub fn text(content: String) -> Self {
        Self {
            content: vec![ToolContent {
                content_type: "text".to_string(),
                text: content,
            }],
            is_error: None,
        }
    }

    /// Create a successful structured (JSON) result.
    pub fn json(value: &impl Serialize) -> Self {
        let text = serde_json::to_string(value)
            .unwrap_or_else(|e| format!(r#"{{"error":"failed to serialize result: {}"}}"#, e));
        Self::text(text)
    }

    /// Create an error result.
    #[allow(dead_code)] // Will be used in future phases for error handling
    pub fn error(message: String) -> Self {
        Self {
            content: vec![ToolContent {
                content_type: "text".to_string(),
                text: message,
            }],
            is_error: Some(true),
        }
    }
}

/// Abstraction for a single MCP tool.
///
/// Implementors provide metadata (name, description, JSON Schema input)
/// and the actual execution logic via `call()`.
///
/// # Requirements
/// - Must be `Send + Sync` for future concurrent use.
/// - Must not panic on invalid input — return `ToolResult::error` instead.
/// - Must not depend on any MCP transport types.
pub trait Tool: Send + Sync {
    /// MCP tool name, e.g. "health_check".
    fn name(&self) -> &str;

    /// Human-readable description shown to LLM agents.
    fn description(&self) -> &str;

    /// JSON Schema for the tool's input parameters.
    /// Return `{"type": "object", "properties": {}, "required": []}` for no-arg tools.
    fn input_schema(&self) -> Value;

    /// Execute the tool with the given arguments.
    fn call(&self, arguments: Option<Value>) -> ToolResult;
}

/// Registry that holds all registered tools and supports lookup by name.
pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    /// Register a tool. The registry takes ownership.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Find a tool by name.
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|t| t.name() == name)
            .map(|t| t.as_ref())
    }

    /// List all registered tools.
    pub fn list(&self) -> &[Box<dyn Tool>] {
        &self.tools
    }

    /// Serialize all tools to the MCP `tools/list` response format.
    pub fn to_tools_list(&self) -> Value {
        let tools: Vec<Value> = self
            .tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name(),
                    "description": t.description(),
                    "inputSchema": t.input_schema(),
                })
            })
            .collect();

        serde_json::json!({ "tools": tools })
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyTool;

    impl Tool for DummyTool {
        fn name(&self) -> &str {
            "dummy"
        }
        fn description(&self) -> &str {
            "A dummy tool for testing"
        }
        fn input_schema(&self) -> Value {
            serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            })
        }
        fn call(&self, _arguments: Option<Value>) -> ToolResult {
            ToolResult::text("dummy result".to_string())
        }
    }

    #[test]
    fn test_tool_registry_register_and_get() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(DummyTool));

        assert!(registry.get("dummy").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_tool_registry_to_tools_list() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(DummyTool));

        let list = registry.to_tools_list();
        let tools = list["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "dummy");
    }

    #[test]
    fn test_tool_result_text() {
        let result = ToolResult::text("hello".to_string());
        assert_eq!(result.content.len(), 1);
        assert_eq!(result.content[0].content_type, "text");
        assert_eq!(result.content[0].text, "hello");
        assert!(result.is_error.is_none());
    }

    #[test]
    fn test_tool_result_error() {
        let result = ToolResult::error("something went wrong".to_string());
        assert_eq!(result.content[0].text, "something went wrong");
        assert_eq!(result.is_error, Some(true));
    }

    #[test]
    fn test_tool_result_json() {
        let result = ToolResult::json(&serde_json::json!({"status": "ok"}));
        assert!(result.content[0].text.contains("\"status\""));
        assert!(result.content[0].text.contains("\"ok\""));
    }
}
