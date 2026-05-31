//! Mock HTTP server tests for embedding provider.
//! Does NOT require a real API key — uses axum test server.

use knowledge_companion::index::embed::{EmbedConfig, RemoteEmbedder};

/// Build a mock embed config pointing to a fake URL.
fn mock_config(port: u16) -> EmbedConfig {
    EmbedConfig {
        base_url: format!("http://127.0.0.1:{}", port),
        api_key: "fake-key".into(),
        model: "test-model".into(),
        dimensions: 4,
        timeout_seconds: 5,
        batch_size: 16,
    }
}

#[tokio::test]
async fn test_embedder_sync_batch() {
    // Start a mock axum server
    let app = axum::Router::new().route(
        "/embeddings",
        axum::routing::post(
            |axum::Json(body): axum::Json<serde_json::Value>| async move {
                let input = body["input"].as_array().unwrap();
                let embeddings: Vec<serde_json::Value> = input
                    .iter()
                    .map(|_| {
                        serde_json::json!({
                            "embedding": [0.1, 0.2, 0.3, 0.4],
                            "index": 0
                        })
                    })
                    .collect();
                axum::Json(serde_json::json!({
                    "data": embeddings,
                    "model": "test-model",
                    "usage": {"total_tokens": input.len()}
                }))
            },
        ),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Test embed_sync with two texts (in spawn_blocking to avoid runtime conflict)
    let config = mock_config(port);
    let result = tokio::task::spawn_blocking(move || {
        let embedder = RemoteEmbedder::new(config);
        embedder.embed_sync(&["hello world".into(), "test text".into()])
    })
    .await
    .unwrap()
    .unwrap();

    assert_eq!(result.len(), 2);
    assert_eq!(result[0].dimensions, 4);
    assert_eq!(result[0].model, "test-model");
    assert!((result[0].vector[0] - 0.1).abs() < 0.01);
}

#[tokio::test]
async fn test_embedder_api_error() {
    // Server that returns 500
    let app = axum::Router::new().route(
        "/embeddings",
        axum::routing::post(|| async {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "internal error",
            )
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let config = mock_config(port);
    let result = tokio::task::spawn_blocking(move || {
        let embedder = RemoteEmbedder::new(config);
        embedder.embed_sync(&["test".into()])
    })
    .await
    .unwrap();
    assert!(result.is_err(), "Expected error, got {:?}", result);
}

#[tokio::test]
async fn test_embedder_timeout() {
    // Server that hangs
    let app = axum::Router::new().route(
        "/embeddings",
        axum::routing::post(|| async {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            "ok"
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let config = mock_config(port);
    let result = tokio::task::spawn_blocking(move || {
        let embedder = RemoteEmbedder::new(config);
        embedder.embed_sync(&["test".into()])
    })
    .await
    .unwrap();
    assert!(result.is_err(), "Timeout should produce error");
}
