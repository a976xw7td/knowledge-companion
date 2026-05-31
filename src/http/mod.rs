//! HTTP MCP adapter — JSON-RPC over HTTP POST.
//!
//! Compatibility: initialize, tools/list, tools/call, notifications/initialized.
//! NOT full Streamable HTTP MCP (no SSE streaming). This covers the core
//! JSON-RPC request/response protocol that most MCP clients need.
//!
//! Security: Bearer token auth (KC_HTTP_MCP_TOKEN), request size limit,
//! rate limiting, access logging with credential redaction.

use axum::{
    body::Body,
    extract::{ConnectInfo, State},
    http::{Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tower_http::limit::RequestBodyLimitLayer;

use crate::config::HttpMcpSection;
use crate::mcp::protocol::JsonRpcRequest;
use crate::mcp::server::McpServer;

/// Per-IP rate limiter using a simple sliding window.
struct RateLimiter {
    requests: Mutex<HashMap<IpAddr, Vec<Instant>>>,
    max_per_minute: u32,
}

impl RateLimiter {
    fn new(max_per_minute: u32) -> Self {
        Self {
            requests: Mutex::new(HashMap::new()),
            max_per_minute,
        }
    }

    fn check(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let window = now - std::time::Duration::from_secs(60);
        let mut map = self.requests.lock().unwrap();
        let entries = map.entry(ip).or_default();
        entries.retain(|t| *t > window);
        if entries.len() >= self.max_per_minute as usize {
            false
        } else {
            entries.push(now);
            true
        }
    }
}

/// Shared application state.
#[derive(Clone)]
struct AppState {
    server: Arc<McpServer>,
    token: Option<String>,
    rate_limiter: Arc<RateLimiter>,
}

/// Start the HTTP MCP server. Blocks until shutdown.
pub async fn serve(config: HttpMcpSection) -> anyhow::Result<()> {
    let token = if config.token_env.is_empty() {
        None
    } else {
        std::env::var(&config.token_env).ok()
    };

    let bind_addr = format!("{}:{}", config.bind, config.port);
    let is_loopback =
        config.bind == "127.0.0.1" || config.bind == "localhost" || config.bind == "::1";

    if !is_loopback && token.is_none() {
        return Err(anyhow::anyhow!(
            "HTTP MCP refuses to bind to non-loopback address {} without token.\n\
             Set [http_mcp] token_env in config and provide the token via env var to enable remote access.",
            bind_addr
        ));
    }

    let registry = crate::build_registry();
    let server = Arc::new(McpServer::new(
        registry,
        "knowledge-companion".to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
    ));

    let rate_limiter = Arc::new(RateLimiter::new(config.requests_per_minute));
    let state = AppState {
        server,
        token,
        rate_limiter,
    };
    let limit_bytes = 10 * 1024 * 1024;

    let app = Router::new()
        .route("/mcp", post(handle_mcp))
        .route("/health", get(handle_health))
        .layer(RequestBodyLimitLayer::new(limit_bytes))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit_middleware,
        ))
        .layer(middleware::from_fn_with_state(state.clone(), access_log))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .with_state(state);

    let addr: SocketAddr = bind_addr.parse()?;
    tracing::info!(addr = %addr, "HTTP MCP server starting");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;

    Ok(())
}

/// Rate limiting middleware: enforces per-IP RPM.
async fn rate_limit_middleware(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if req.uri().path() == "/health" {
        return next.run(req).await;
    }
    if !state.rate_limiter.check(addr.ip()) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "error": {"code": -32000, "message": "Rate limit exceeded"}
            })),
        )
            .into_response();
    }
    next.run(req).await
}

/// Auth middleware: checks Bearer token if configured.
async fn auth_middleware(
    State(state): State<AppState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if req.uri().path() == "/health" {
        return next.run(req).await;
    }

    if let Some(ref expected) = state.token {
        let auth_header = req
            .headers()
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        let provided = auth_header.strip_prefix("Bearer ").unwrap_or("");
        if provided != expected.as_str() {
            let body = Json(serde_json::json!({
                "jsonrpc": "2.0",
                "error": {"code": -32001, "message": "Unauthorized: invalid or missing Bearer token"}
            }));
            return (StatusCode::UNAUTHORIZED, body).into_response();
        }
    }

    next.run(req).await
}

/// Access log middleware with credential redaction.
async fn access_log(
    State(_state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let start = std::time::Instant::now();

    let response = next.run(req).await;

    let status = response.status();
    let duration = start.elapsed();
    tracing::info!(
        method = %method,
        path = %path,
        status = %status.as_u16(),
        duration_ms = %duration.as_millis(),
        client = %addr,
        "HTTP MCP access"
    );

    response
}

/// POST /mcp — JSON-RPC handler. Accepts single requests and returns JSON responses.
async fn handle_mcp(
    State(state): State<AppState>,
    Json(request): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    let response = state.server.handle_request(&request);

    match response {
        Some(resp) => {
            let json = serde_json::to_string(&resp).unwrap_or_else(|_| {
                r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"Internal error"}}"#
                    .to_string()
            });
            (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                json,
            )
                .into_response()
        }
        None => {
            // Notification — 202 Accepted, empty body
            StatusCode::ACCEPTED.into_response()
        }
    }
}

/// GET /health — liveness check.
async fn handle_health() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status": "ok"}))
}
