//! HTTP MCP integration tests.
//!
//! Spawns the HTTP MCP server and sends real JSON-RPC requests.

#[cfg(test)]
mod tests {
    use std::time::Duration;

    /// Helper: send a JSON-RPC request to the HTTP MCP server.
    async fn send_mcp(
        client: &reqwest::Client,
        url: &str,
        req: serde_json::Value,
    ) -> serde_json::Value {
        let resp = client
            .post(format!("{}/mcp", url))
            .json(&req)
            .send()
            .await
            .expect("HTTP request failed");
        resp.json().await.expect("JSON parse failed")
    }

    /// Spawn the server in a background task with given token.
    async fn spawn_server(
        port: u16,
        token: Option<String>,
        bind: &str,
    ) -> (String, tokio::task::JoinHandle<()>) {
        // Use unique env var name per test to avoid cross-test contamination
        let token_env_name = format!("TEST_HTTP_TOKEN_{}", port);
        let cfg = knowledge_companion::config::HttpMcpSection {
            port,
            bind: bind.to_string(),
            enabled: true,
            token_env: if token.is_some() {
                token_env_name.clone()
            } else {
                String::new()
            },
            ..Default::default()
        };
        if let Some(ref t) = token {
            std::env::set_var(&token_env_name, t);
        }

        let url = format!("http://{}:{}", bind, port);
        let handle = tokio::spawn(async move {
            let _ = knowledge_companion::http::serve(cfg).await;
        });

        // Wait for server to be ready
        tokio::time::sleep(Duration::from_millis(500)).await;
        (url, handle)
    }

    #[tokio::test]
    async fn test_http_mcp_initialize_and_tools_list() {
        let port = 18801;
        let (url, _handle) = spawn_server(port, None, "127.0.0.1").await;
        let client = reqwest::Client::new();

        // Initialize
        let resp = send_mcp(
            &client,
            &url,
            serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "test", "version": "1.0"}}
            }),
        )
        .await;
        assert_eq!(resp["result"]["serverInfo"]["name"], "knowledge-companion");
        assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");

        // Tools/list
        let resp = send_mcp(
            &client,
            &url,
            serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
        )
        .await;
        let tools = resp["result"]["tools"].as_array().unwrap();
        assert!(
            tools.len() >= 15,
            "Expected >= 15 tools, got {}",
            tools.len()
        );

        // Health check via tools/call
        let resp = send_mcp(
            &client,
            &url,
            serde_json::json!({
                "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": {"name": "health_check", "arguments": {}}
            }),
        )
        .await;
        assert!(resp["error"].is_null());
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        // Health check may return ok, degraded, or error depending on env
        let health: serde_json::Value = serde_json::from_str(text).unwrap();
        assert!(
            health.get("status").is_some(),
            "Health check should return status"
        );
    }

    #[tokio::test]
    async fn test_http_mcp_auth_wrong_token() {
        let port = 18802;
        let (_url, _handle) =
            spawn_server(port, Some("correct-token".to_string()), "127.0.0.1").await;
        let client = reqwest::Client::new();

        // Request without token
        let resp = client
            .post(format!("http://127.0.0.1:{}/mcp", port))
            .json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);

        // Request with wrong token
        let resp = client
            .post(format!("http://127.0.0.1:{}/mcp", port))
            .header("Authorization", "Bearer wrong-token")
            .json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_http_mcp_auth_correct_token() {
        let port = 18803;
        let (url, _handle) = spawn_server(port, Some("my-secret".to_string()), "127.0.0.1").await;
        let client = reqwest::Client::new();

        let resp = client
            .post(format!("{}/mcp", url))
            .header("Authorization", "Bearer my-secret")
            .json(&serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "test", "version": "1.0"}}
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["result"]["serverInfo"]["name"], "knowledge-companion");
    }

    #[tokio::test]
    async fn test_http_mcp_oversized_body() {
        let port = 18804;
        let (url, _handle) = spawn_server(port, None, "127.0.0.1").await;
        let client = reqwest::Client::new();

        // Send a request with a very large body
        let big_string = "x".repeat(11 * 1024 * 1024); // 11 MB > 10 MB limit
        let resp = client
            .post(format!("{}/mcp", url))
            .body(big_string)
            .header("Content-Type", "application/json")
            .send()
            .await
            .unwrap();
        // Should get 413 Payload Too Large or connection reset
        assert!(
            resp.status() == reqwest::StatusCode::PAYLOAD_TOO_LARGE
                || resp.status().is_server_error()
        );
    }

    #[tokio::test]
    async fn test_http_mcp_health_endpoint() {
        let port = 18805;
        let (url, _handle) = spawn_server(port, Some("token".to_string()), "127.0.0.1").await;
        let client = reqwest::Client::new();

        // Health endpoint should NOT require auth
        let resp = client.get(format!("{}/health", url)).send().await.unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["status"], "ok");
    }

    #[tokio::test]
    async fn test_http_mcp_rate_limit_by_ip() {
        let port = 18806;
        let cfg = knowledge_companion::config::HttpMcpSection {
            port,
            bind: "127.0.0.1".to_string(),
            enabled: true,
            token_env: String::new(),
            requests_per_minute: 1,
            ..Default::default()
        };
        let handle = tokio::spawn(async move {
            let _ = knowledge_companion::http::serve(cfg).await;
        });
        tokio::time::sleep(Duration::from_millis(500)).await;

        let client = reqwest::Client::new();
        let request = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});
        let first = client
            .post(format!("http://127.0.0.1:{}/mcp", port))
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(first.status(), reqwest::StatusCode::OK);

        let second = client
            .post(format!("http://127.0.0.1:{}/mcp", port))
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(second.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);
        handle.abort();
    }

    #[tokio::test]
    async fn test_http_mcp_rejects_remote_bind_without_token() {
        let cfg = knowledge_companion::config::HttpMcpSection {
            port: 18807,
            bind: "0.0.0.0".to_string(),
            enabled: true,
            token_env: String::new(),
            ..Default::default()
        };
        let error = knowledge_companion::http::serve(cfg).await.unwrap_err();
        assert!(error.to_string().contains("refuses to bind"));
    }
}
