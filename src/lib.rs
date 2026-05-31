//! KnowledgeCompanion — Portable Personal Knowledge Management
//!
//! Shared library used by both the MCP server (`knowledge-companion`) and
//! the maintenance CLI (`kcctl`).

pub mod config;
pub mod db;
pub mod http;
pub mod index;
pub mod ingest;
pub mod mcp;
pub mod rag;
pub mod retrieve;
pub mod services;
pub mod sync;
pub mod translate;

use anyhow::Result;
use db::connection;
use index::fts;
use mcp::adapter::StdioAdapter;
use mcp::server::McpServer;
use mcp::tools::{Tool, ToolRegistry, ToolResult};
use serde_json::Value;
use tracing_subscriber::EnvFilter;

// ── Tool definitions (shared between bins) ─────────────────────────────────

struct HealthCheckTool;

impl Tool for HealthCheckTool {
    fn name(&self) -> &str {
        "health_check"
    }
    fn description(&self) -> &str {
        "检查知识库系统的健康状态。返回 bundle root、配置加载、数据库连接、\
         knowledge roots 可访问性、data/ 目录可写性、LLM/embedding 配置状态。\
         当组件不可用时报告降级原因。"
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}, "required": []})
    }
    fn call(&self, _args: Option<Value>) -> ToolResult {
        ToolResult::json(&services::health::check_health())
    }
}

struct KnowledgeStatsTool;

impl Tool for KnowledgeStatsTool {
    fn name(&self) -> &str {
        "get_knowledge_stats"
    }
    fn description(&self) -> &str {
        "返回知识库统计信息：文档总数、chunks 数量、标签/wikilinks 数、存储占用、\
         最后同步时间。数据来自 SQLite，实时反映知识库状态。"
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}, "required": []})
    }
    fn call(&self, _args: Option<Value>) -> ToolResult {
        ToolResult::json(&services::stats::get_stats())
    }
}

// ── Shared helpers ─────────────────────────────────────────────────────────

/// Initialize logging (stderr + file).
pub fn init_logging(log_dir: &std::path::Path) {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    // File appender for app.log
    let file_appender = tracing_appender::rolling::never(log_dir, "app.log");

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr)
        .with_writer(file_appender)
        .with_target(false)
        .init();
}

// ── Sync tools ────────────────────────────────────────────────────────────

struct SyncNowTool;

impl Tool for SyncNowTool {
    fn name(&self) -> &str {
        "sync_now"
    }
    fn description(&self) -> &str {
        "立即扫描所有启用的 watched roots，检测新增、修改、删除的文件，更新数据库索引。返回每个 root 的同步结果。"
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}, "required": []})
    }
    fn call(&self, _args: Option<Value>) -> ToolResult {
        let bundle_root = config::bundle::detect_bundle_root().unwrap_or_else(|_| "/".into());
        let cfg = config::bundle::load_config(&bundle_root).unwrap_or_default();
        let db_path = config::bundle::resolve_path(&bundle_root, &cfg.storage.db_path);
        let conn = match connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("Failed to open DB: {}", e)),
        };
        match sync::sync_all(&conn, &bundle_root) {
            Ok(results) => ToolResult::json(&results),
            Err(e) => ToolResult::error(format!("Sync failed: {}", e)),
        }
    }
}

struct GetSyncStatusTool;

impl Tool for GetSyncStatusTool {
    fn name(&self) -> &str {
        "get_sync_status"
    }
    fn description(&self) -> &str {
        "查看同步状态：已配置的 watched roots、最后同步时间、最近的同步事件。"
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}, "required": []})
    }
    fn call(&self, _args: Option<Value>) -> ToolResult {
        let bundle_root = config::bundle::detect_bundle_root().unwrap_or_else(|_| "/".into());
        let cfg = config::bundle::load_config(&bundle_root).unwrap_or_default();
        let db_path = config::bundle::resolve_path(&bundle_root, &cfg.storage.db_path);
        if !db_path.exists() {
            return ToolResult::json(&serde_json::json!({
                "roots": [], "last_sync_at": null, "running_jobs": 0,
                "queued_jobs": 0, "failed_jobs": 0, "recent_events": []
            }));
        }
        let conn = match connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("Failed to open DB: {}", e)),
        };
        let roots: Vec<Value> = conn
            .prepare("SELECT name, root_path, enabled FROM watched_roots")
            .ok()
            .map(|mut s| {
                s.query_map([], |row| {
                    Ok(serde_json::json!({
                        "name": row.get::<_, String>(0)?,
                        "root_path": row.get::<_, String>(1)?,
                        "enabled": row.get::<_, i32>(2)? == 1,
                    }))
                })
                .ok()
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
                .unwrap_or_default()
            })
            .unwrap_or_default();
        let recent: Vec<Value> = conn
            .prepare("SELECT event_type, source_path, created_at FROM sync_events ORDER BY created_at DESC LIMIT 10")
            .ok()
            .map(|mut s| {
                s.query_map([], |row| {
                    Ok(serde_json::json!({
                        "event_type": row.get::<_, String>(0)?,
                        "source_path": row.get::<_, String>(1)?,
                        "created_at": row.get::<_, String>(2)?,
                    }))
                })
                .ok()
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
                .unwrap_or_default()
            })
            .unwrap_or_default();
        ToolResult::json(&serde_json::json!({
            "roots": roots,
            "last_sync_at": recent.first().and_then(|e| e.get("created_at").cloned()),
            "running_jobs": 0,
            "queued_jobs": 0,
            "failed_jobs": 0,
            "recent_events": recent
        }))
    }
}

struct ListWatchFoldersTool;

impl Tool for ListWatchFoldersTool {
    fn name(&self) -> &str {
        "list_watch_folders"
    }
    fn description(&self) -> &str {
        "列出所有已配置的 watched knowledge roots，包括启用状态、路径和 glob 模式。"
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}, "required": []})
    }
    fn call(&self, _args: Option<Value>) -> ToolResult {
        let bundle_root = config::bundle::detect_bundle_root().unwrap_or_else(|_| "/".into());
        let cfg = config::bundle::load_config(&bundle_root).unwrap_or_default();
        let roots: Vec<Value> = cfg
            .knowledge
            .roots
            .iter()
            .map(|r| {
                serde_json::json!({
                    "name": r.name, "path": r.path, "enabled": r.enabled,
                    "read_only": r.read_only,
                    "include_globs": r.include_globs,
                    "exclude_globs": r.exclude_globs,
                })
            })
            .collect();
        ToolResult::json(&serde_json::json!({"roots": roots}))
    }
}

struct RebuildIndexTool;

impl Tool for RebuildIndexTool {
    fn name(&self) -> &str {
        "rebuild_index"
    }
    fn description(&self) -> &str {
        "强制重建所有文档的索引。删除所有 chunks、FTS、graph nodes/edges、embeddings，然后重新扫描和索引。"
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}, "required": []})
    }
    fn call(&self, _args: Option<Value>) -> ToolResult {
        let bundle_root = config::bundle::detect_bundle_root().unwrap_or_else(|_| "/".into());
        let cfg = config::bundle::load_config(&bundle_root).unwrap_or_default();
        let db_path = config::bundle::resolve_path(&bundle_root, &cfg.storage.db_path);
        let conn = match connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("Failed to open DB: {}", e)),
        };
        // Wipe derived data
        let _ = conn.execute("DELETE FROM chunk_embeddings", []);
        let _ = conn.execute("DELETE FROM graph_edges", []);
        let _ = conn.execute("DELETE FROM graph_nodes", []);
        let _ = conn.execute("DELETE FROM chunks_fts", []);
        let _ = conn.execute("DELETE FROM chunks", []);
        let _ = conn.execute("DELETE FROM documents", []);
        // Rebuild via full sync
        match sync::sync_all(&conn, &bundle_root) {
            Ok(results) => ToolResult::json(&results),
            Err(e) => ToolResult::error(format!("Rebuild failed: {}", e)),
        }
    }
}

// ── Document tools ────────────────────────────────────────────────────────

struct ConfigureWatchFolderTool;

impl Tool for ConfigureWatchFolderTool {
    fn name(&self) -> &str {
        "configure_watch_folder"
    }
    fn description(&self) -> &str {
        "配置新的 watched knowledge root。添加后会自动同步。"
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type":"object",
            "properties": {
                "path":{"type":"string","description":"要监听的文件夹路径"},
                "name":{"type":"string","description":"此 root 的名称"}
            },
            "required":["path"]
        })
    }
    fn call(&self, args: Option<Value>) -> ToolResult {
        let args = args.unwrap_or_default();
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let name = args.get("name").and_then(|v| v.as_str()).unwrap_or(path);
        let bundle_root = config::bundle::detect_bundle_root().unwrap_or_else(|_| "/".into());
        let mut cfg = config::bundle::load_config(&bundle_root).unwrap_or_default();
        cfg.knowledge.roots.push(crate::config::KnowledgeRoot {
            name: name.to_string(),
            path: path.to_string(),
            enabled: true,
            read_only: true,
            include_globs: vec![
                "**/*.md".into(),
                "**/*.markdown".into(),
                "**/*.txt".into(),
                "**/*.pdf".into(),
                "**/*.docx".into(),
            ],
            exclude_globs: vec!["**/.git/**".into(), "**/node_modules/**".into()],
        });
        if let Err(e) = config::bundle::save_config(&bundle_root, &cfg) {
            return ToolResult::error(format!("Failed to save config: {}", e));
        }
        // Sync the new root
        let db_path = config::bundle::resolve_path(&bundle_root, &cfg.storage.db_path);
        let conn = match connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("DB: {}", e)),
        };
        match sync::sync_all(&conn, &bundle_root) {
            Ok(results) => {
                ToolResult::json(&serde_json::json!({"status":"ok","name":name,"sync":results}))
            }
            Err(e) => ToolResult::error(format!("{}", e)),
        }
    }
}

struct ListDocumentsTool;

impl Tool for ListDocumentsTool {
    fn name(&self) -> &str {
        "list_documents"
    }
    fn description(&self) -> &str {
        "列出已同步的文档，支持按状态和关键词筛选。"
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type":"object",
            "properties": {
                "status":{"type":"string","description":"筛选状态: indexed, pending, failed, deleted"},
                "query":{"type":"string","description":"标题搜索关键词"},
                "limit":{"type":"integer","default":20}
            }
        })
    }
    fn call(&self, args: Option<Value>) -> ToolResult {
        let args = args.unwrap_or_default();
        let status = args.get("status").and_then(|v| v.as_str());
        let query = args.get("query").and_then(|v| v.as_str());
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20);
        let bundle_root = config::bundle::detect_bundle_root().unwrap_or_else(|_| "/".into());
        let cfg = config::bundle::load_config(&bundle_root).unwrap_or_default();
        let db_path = config::bundle::resolve_path(&bundle_root, &cfg.storage.db_path);
        let conn = match connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("DB: {}", e)),
        };
        let mut sql = "SELECT id, title, file_type, status, source_path, relative_path, word_count, created_at FROM documents WHERE 1=1".to_string();
        let mut params: Vec<rusqlite::types::Value> = Vec::new();
        if let Some(s) = status {
            sql.push_str(" AND status = ?");
            params.push(s.to_string().into());
        }
        if let Some(q) = query {
            sql.push_str(" AND title LIKE ?");
            params.push(format!("%{}%", q).into());
        }
        sql.push_str(" ORDER BY created_at DESC LIMIT ?");
        params.push((limit.min(500) as i64).into());
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(e) => return ToolResult::error(format!("{}", e)),
        };
        let rows: Vec<Value> = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                Ok(serde_json::json!({
                    "id":row.get::<_,String>(0)?,"title":row.get::<_,String>(1)?,
                    "file_type":row.get::<_,String>(2)?,"status":row.get::<_,String>(3)?,
                    "source_path":row.get::<_,String>(4)?,"relative_path":row.get::<_,String>(5)?,
                    "word_count":row.get::<_,i32>(6)?,"created_at":row.get::<_,String>(7)?
                }))
            })
            .ok()
            .map(|r| r.filter_map(|r2| r2.ok()).collect())
            .unwrap_or_default();
        ToolResult::json(&serde_json::json!({"documents":rows,"total":rows.len()}))
    }
}

struct GetDocumentTool;

impl Tool for GetDocumentTool {
    fn name(&self) -> &str {
        "get_document"
    }
    fn description(&self) -> &str {
        "获取单个文档的详情、chunks 和可引用信息。"
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({"type":"object","properties":{"doc_id":{"type":"string"}},"required":["doc_id"]})
    }
    fn call(&self, args: Option<Value>) -> ToolResult {
        let args = args.unwrap_or_default();
        let doc_id = args.get("doc_id").and_then(|v| v.as_str()).unwrap_or("");
        let bundle_root = config::bundle::detect_bundle_root().unwrap_or_else(|_| "/".into());
        let cfg = config::bundle::load_config(&bundle_root).unwrap_or_default();
        let db_path = config::bundle::resolve_path(&bundle_root, &cfg.storage.db_path);
        let conn = match connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("DB: {}", e)),
        };
        let doc = conn.query_row("SELECT id,title,file_type,status,source_path,relative_path,word_count,content_hash,created_at,indexed_at FROM documents WHERE id=?1",[doc_id], |row| {
            Ok(serde_json::json!({
                "id":row.get::<_,String>(0)?,"title":row.get::<_,String>(1)?,
                "file_type":row.get::<_,String>(2)?,"status":row.get::<_,String>(3)?,
                "source_path":row.get::<_,String>(4)?,"relative_path":row.get::<_,String>(5)?,
                "word_count":row.get::<_,i32>(6)?,"content_hash":row.get::<_,String>(7)?,
                "created_at":row.get::<_,String>(8)?,"indexed_at":row.get::<_,Option<String>>(9)?
            }))
        });
        match doc {
            Ok(d) => ToolResult::json(&d),
            Err(_) => ToolResult::error(format!("Document not found: {}", doc_id)),
        }
    }
}

struct ForgetDocumentTool;

impl Tool for ForgetDocumentTool {
    fn name(&self) -> &str {
        "forget_document"
    }
    fn description(&self) -> &str {
        "从索引中移除文档（不删除源文件）。下次同步时如果文件仍在，会重新入库。"
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({"type":"object","properties":{"doc_id":{"type":"string"}},"required":["doc_id"]})
    }
    fn call(&self, args: Option<Value>) -> ToolResult {
        let args = args.unwrap_or_default();
        let doc_id = args.get("doc_id").and_then(|v| v.as_str()).unwrap_or("");
        let bundle_root = config::bundle::detect_bundle_root().unwrap_or_else(|_| "/".into());
        let cfg = config::bundle::load_config(&bundle_root).unwrap_or_default();
        let db_path = config::bundle::resolve_path(&bundle_root, &cfg.storage.db_path);
        let conn = match connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("DB: {}", e)),
        };
        let now = chrono::Utc::now().to_rfc3339();
        let _ = conn.execute(
            "UPDATE documents SET status='deleted', deleted_at=?1, updated_at=?1 WHERE id=?2",
            rusqlite::params![now, doc_id],
        );
        let _ = fts::delete_doc(&conn, doc_id);
        let _ = conn.execute("DELETE FROM chunk_embeddings WHERE chunk_id IN (SELECT id FROM chunks WHERE doc_id=?1)", [doc_id]);
        let _ = conn.execute("DELETE FROM chunks WHERE doc_id=?1", [doc_id]);
        let _ = conn.execute("DELETE FROM graph_edges WHERE source_id IN (SELECT id FROM graph_nodes WHERE doc_id=?1) OR target_id IN (SELECT id FROM graph_nodes WHERE doc_id=?1)", [doc_id]);
        let _ = conn.execute("DELETE FROM graph_nodes WHERE doc_id=?1", [doc_id]);
        ToolResult::json(&serde_json::json!({"status":"ok","doc_id":doc_id}))
    }
}

struct ReindexDocumentTool;

impl Tool for ReindexDocumentTool {
    fn name(&self) -> &str {
        "reindex_document"
    }
    fn description(&self) -> &str {
        "强制重建单个文档的 chunks、FTS 和 graph。"
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({"type":"object","properties":{"doc_id":{"type":"string"}},"required":["doc_id"]})
    }
    fn call(&self, args: Option<Value>) -> ToolResult {
        let args = args.unwrap_or_default();
        let doc_id = args.get("doc_id").and_then(|v| v.as_str()).unwrap_or("");
        let bundle_root = config::bundle::detect_bundle_root().unwrap_or_else(|_| "/".into());
        let cfg = config::bundle::load_config(&bundle_root).unwrap_or_default();
        let db_path = config::bundle::resolve_path(&bundle_root, &cfg.storage.db_path);
        let conn = match connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("DB: {}", e)),
        };
        match crate::sync::executor::reindex_document(&conn, doc_id) {
            Ok(()) => ToolResult::json(&serde_json::json!({"status":"ok","doc_id":doc_id})),
            Err(e) => ToolResult::error(format!("Reindex failed: {}", e)),
        }
    }
}

struct UpdateGlossaryEntryTool;

impl Tool for UpdateGlossaryEntryTool {
    fn name(&self) -> &str {
        "update_glossary_entry"
    }
    fn description(&self) -> &str {
        "更新术语表条目。"
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type":"object",
            "properties": {
                "id":{"type":"string"},
                "source_term":{"type":"string"},
                "target_term":{"type":"string"},
                "category":{"type":"string"}
            },
            "required":["id"]
        })
    }
    fn call(&self, args: Option<Value>) -> ToolResult {
        let args = args.unwrap_or_default();
        let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let bundle_root = config::bundle::detect_bundle_root().unwrap_or_else(|_| "/".into());
        let cfg = config::bundle::load_config(&bundle_root).unwrap_or_default();
        let db_path = config::bundle::resolve_path(&bundle_root, &cfg.storage.db_path);
        let conn = match connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("DB: {}", e)),
        };
        let now = chrono::Utc::now().to_rfc3339();
        if let Some(st) = args.get("source_term").and_then(|v| v.as_str()) {
            let _ = conn.execute(
                "UPDATE glossary_entries SET source_term=?1, updated_at=?2 WHERE id=?3",
                rusqlite::params![st, now, id],
            );
        }
        if let Some(tt) = args.get("target_term").and_then(|v| v.as_str()) {
            let _ = conn.execute(
                "UPDATE glossary_entries SET target_term=?1, updated_at=?2 WHERE id=?3",
                rusqlite::params![tt, now, id],
            );
        }
        if let Some(cat) = args.get("category").and_then(|v| v.as_str()) {
            let _ = conn.execute(
                "UPDATE glossary_entries SET category=?1, updated_at=?2 WHERE id=?3",
                rusqlite::params![cat, now, id],
            );
        }
        ToolResult::json(&serde_json::json!({"status":"ok","id":id}))
    }
}

// ── Search and RAG tools ─────────────────────────────────────────────────

struct SearchKnowledgeTool;

impl Tool for SearchKnowledgeTool {
    fn name(&self) -> &str {
        "search_knowledge"
    }
    fn description(&self) -> &str {
        "搜索知识库。支持关键词检索（FTS5），可在配置中启用语义和图谱扩展。\
         返回匹配的文档 chunk、标题、路径、行号和相关性分数。"
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "搜索词"},
                "top_k": {"type": "integer", "default": 8, "description": "返回结果数量"}
            },
            "required": ["query"]
        })
    }
    fn call(&self, args: Option<Value>) -> ToolResult {
        let args = args.unwrap_or_default();
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let top_k = args.get("top_k").and_then(|v| v.as_u64()).unwrap_or(8) as usize;

        let bundle_root = config::bundle::detect_bundle_root().unwrap_or_else(|_| "/".into());
        let cfg = config::bundle::load_config(&bundle_root).unwrap_or_default();
        let db_path = config::bundle::resolve_path(&bundle_root, &cfg.storage.db_path);
        let conn = match connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("DB error: {}", e)),
        };

        let query_embedding = if cfg.embedding.enabled && !cfg.embedding.api_key_env.is_empty() {
            std::env::var(&cfg.embedding.api_key_env)
                .ok()
                .and_then(|key| {
                    if key.is_empty() {
                        return None;
                    }
                    let ec = crate::index::embed::EmbedConfig {
                        base_url: cfg.embedding.base_url.clone(),
                        api_key: key,
                        model: cfg.embedding.model.clone(),
                        dimensions: cfg.embedding.dimensions,
                        timeout_seconds: cfg.embedding.timeout_seconds,
                        batch_size: cfg.embedding.batch_size,
                    };
                    let embedder = crate::index::embed::RemoteEmbedder::new(ec);
                    embedder
                        .embed_sync(&[query.to_string()])
                        .ok()
                        .and_then(|e| e.into_iter().next().map(|emb| emb.vector))
                })
        } else {
            None
        };

        match retrieve::hybrid_search(&conn, query, top_k, true, query_embedding.as_deref(), true) {
            Ok((items, diagnostics)) => ToolResult::json(&serde_json::json!({
                "items": items,
                "diagnostics": diagnostics,
            })),
            Err(e) => ToolResult::error(format!("Search failed: {}", e)),
        }
    }
}

struct AskQuestionTool;

impl Tool for AskQuestionTool {
    fn name(&self) -> &str {
        "ask_question"
    }
    fn description(&self) -> &str {
        "基于知识库内容回答问题。调用混合检索获取相关文档，如果有 LLM 配置则生成答案并附带引用。\
         无 LLM 时返回检索到的文档和来源。"
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "question": {"type": "string", "description": "要回答的问题"},
                "top_k": {"type": "integer", "default": 12, "description": "检索数量"}
            },
            "required": ["question"]
        })
    }
    fn call(&self, args: Option<Value>) -> ToolResult {
        let args = args.unwrap_or_default();
        let question = args.get("question").and_then(|v| v.as_str()).unwrap_or("");
        let top_k = args.get("top_k").and_then(|v| v.as_u64()).unwrap_or(12) as usize;

        let bundle_root = config::bundle::detect_bundle_root().unwrap_or_else(|_| "/".into());
        let cfg = config::bundle::load_config(&bundle_root).unwrap_or_default();
        let db_path = config::bundle::resolve_path(&bundle_root, &cfg.storage.db_path);
        let conn = match connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("DB error: {}", e)),
        };

        let llm_config = if cfg.llm.enabled && !cfg.llm.api_key_env.is_empty() {
            let api_key = std::env::var(&cfg.llm.api_key_env).unwrap_or_default();
            if !api_key.is_empty() {
                Some(rag::LlmConfig {
                    enabled: true,
                    base_url: cfg.llm.base_url.clone(),
                    api_key,
                    model: cfg.llm.model_qa.clone(),
                    timeout_seconds: cfg.llm.timeout_seconds,
                })
            } else {
                None
            }
        } else {
            None
        };

        let query_embedding = if cfg.embedding.enabled && !cfg.embedding.api_key_env.is_empty() {
            std::env::var(&cfg.embedding.api_key_env)
                .ok()
                .and_then(|key| {
                    if key.is_empty() {
                        return None;
                    }
                    let ec = crate::index::embed::EmbedConfig {
                        base_url: cfg.embedding.base_url.clone(),
                        api_key: key,
                        model: cfg.embedding.model.clone(),
                        dimensions: cfg.embedding.dimensions,
                        timeout_seconds: cfg.embedding.timeout_seconds,
                        batch_size: cfg.embedding.batch_size,
                    };
                    let embedder = crate::index::embed::RemoteEmbedder::new(ec);
                    embedder
                        .embed_sync(&[question.to_string()])
                        .ok()
                        .and_then(|e| e.into_iter().next().map(|emb| emb.vector))
                })
        } else {
            None
        };

        match rag::ask_question(
            &conn,
            question,
            top_k,
            llm_config.as_ref(),
            true,
            query_embedding.as_deref(),
        ) {
            Ok(answer) => ToolResult::json(&answer),
            Err(e) => ToolResult::error(format!("Question failed: {}", e)),
        }
    }
}

struct GetSourcesTool;

impl Tool for GetSourcesTool {
    fn name(&self) -> &str {
        "get_sources"
    }
    fn description(&self) -> &str {
        "根据引用 ID（如 S1、S2）获取对应的原文 chunk、文件路径和行号。"
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "source_ids": {"type": "array", "items": {"type": "string"}, "description": "引用 ID 列表"}
            },
            "required": ["source_ids"]
        })
    }
    fn call(&self, args: Option<Value>) -> ToolResult {
        let args = args.unwrap_or_default();
        let ids: Vec<String> = args
            .get("source_ids")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let bundle_root = config::bundle::detect_bundle_root().unwrap_or_else(|_| "/".into());
        let cfg = config::bundle::load_config(&bundle_root).unwrap_or_default();
        let db_path = config::bundle::resolve_path(&bundle_root, &cfg.storage.db_path);
        let conn = match connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("DB error: {}", e)),
        };

        match rag::get_sources(&conn, &ids) {
            Ok(sources) => ToolResult::json(&sources),
            Err(e) => ToolResult::error(format!("Failed: {}", e)),
        }
    }
}

struct SearchGraphTool;

impl Tool for SearchGraphTool {
    fn name(&self) -> &str {
        "search_graph"
    }
    fn description(&self) -> &str {
        "搜索知识图谱节点。可按标签、wikilink、文档标题匹配，可选过滤节点类型。"
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "搜索词"},
                "node_type": {"type": "string", "description": "节点类型: tag, wikilink, heading, document"}
            },
            "required": ["query"]
        })
    }
    fn call(&self, args: Option<Value>) -> ToolResult {
        let args = args.unwrap_or_default();
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let node_type = args.get("node_type").and_then(|v| v.as_str());

        let bundle_root = config::bundle::detect_bundle_root().unwrap_or_else(|_| "/".into());
        let cfg = config::bundle::load_config(&bundle_root).unwrap_or_default();
        let db_path = config::bundle::resolve_path(&bundle_root, &cfg.storage.db_path);
        let conn = match connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("DB error: {}", e)),
        };

        match index::graph::search_nodes(&conn, query, node_type, 20) {
            Ok(nodes) => ToolResult::json(&serde_json::json!({"nodes": nodes})),
            Err(e) => ToolResult::error(format!("Graph search failed: {}", e)),
        }
    }
}

struct ExploreGraphNodeTool;

impl Tool for ExploreGraphNodeTool {
    fn name(&self) -> &str {
        "explore_graph_node"
    }
    fn description(&self) -> &str {
        "探索知识图谱中的某个节点，返回其邻居节点和关系。用于发现关联的文档、标签和 wikilink。"
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "node_id": {"type": "string", "description": "图谱节点 ID"},
                "depth": {"type": "integer", "default": 1, "description": "探索深度"}
            },
            "required": ["node_id"]
        })
    }
    fn call(&self, args: Option<Value>) -> ToolResult {
        let args = args.unwrap_or_default();
        let node_id = args.get("node_id").and_then(|v| v.as_str()).unwrap_or("");
        let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(1) as usize;

        let bundle_root = config::bundle::detect_bundle_root().unwrap_or_else(|_| "/".into());
        let cfg = config::bundle::load_config(&bundle_root).unwrap_or_default();
        let db_path = config::bundle::resolve_path(&bundle_root, &cfg.storage.db_path);
        let conn = match connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("DB error: {}", e)),
        };

        match index::graph::explore_node(&conn, node_id, depth) {
            Ok(result) => ToolResult::json(&result),
            Err(e) => ToolResult::error(format!("Graph explore failed: {}", e)),
        }
    }
}

// ── Translation tools ────────────────────────────────────────────────────

struct TranslateTextTool;

impl Tool for TranslateTextTool {
    fn name(&self) -> &str {
        "translate_text"
    }
    fn description(&self) -> &str {
        "翻译文本。利用 Translation Memory 缓存和术语表确保一致翻译。\
         缓存命中时返回已缓存的翻译结果。"
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": {"type": "string", "description": "要翻译的文本"},
                "source_lang": {"type": "string", "default": "zh", "description": "源语言"},
                "target_lang": {"type": "string", "default": "en", "description": "目标语言"}
            },
            "required": ["text"]
        })
    }
    fn call(&self, args: Option<Value>) -> ToolResult {
        let args = args.unwrap_or_default();
        let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let source_lang = args
            .get("source_lang")
            .and_then(|v| v.as_str())
            .unwrap_or("zh");
        let target_lang = args
            .get("target_lang")
            .and_then(|v| v.as_str())
            .unwrap_or("en");

        let bundle_root = config::bundle::detect_bundle_root().unwrap_or_else(|_| "/".into());
        let cfg = config::bundle::load_config(&bundle_root).unwrap_or_default();
        let db_path = config::bundle::resolve_path(&bundle_root, &cfg.storage.db_path);
        let conn = match connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("DB error: {}", e)),
        };

        let llm_config = if cfg.llm.enabled && !cfg.llm.api_key_env.is_empty() {
            let api_key = std::env::var(&cfg.llm.api_key_env).unwrap_or_default();
            if !api_key.is_empty() {
                Some(crate::rag::llm::LlmConfig {
                    base_url: cfg.llm.base_url.clone(),
                    api_key,
                    model: cfg.llm.model_translate.clone(),
                    timeout_seconds: cfg.llm.timeout_seconds,
                })
            } else {
                None
            }
        } else {
            None
        };

        let req = translate::TranslationRequest {
            source_lang: source_lang.to_string(),
            target_lang: target_lang.to_string(),
            text: text.to_string(),
        };

        match translate::translate(&conn, &req, llm_config.as_ref()) {
            Ok(result) => ToolResult::json(&result),
            Err(e) => ToolResult::error(format!("Translation failed: {}", e)),
        }
    }
}

struct AddGlossaryEntryTool;

impl Tool for AddGlossaryEntryTool {
    fn name(&self) -> &str {
        "add_glossary_entry"
    }
    fn description(&self) -> &str {
        "添加术语表条目。用于保证专业术语在翻译中的一致性。"
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "source_term": {"type": "string", "description": "源语言术语"},
                "target_term": {"type": "string", "description": "目标语言对应术语"},
                "source_lang": {"type": "string", "default": "zh"},
                "target_lang": {"type": "string", "default": "en"},
                "category": {"type": "string", "description": "术语分类"}
            },
            "required": ["source_term", "target_term"]
        })
    }
    fn call(&self, args: Option<Value>) -> ToolResult {
        let args = args.unwrap_or_default();
        let st = args
            .get("source_term")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let tt = args
            .get("target_term")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let sl = args
            .get("source_lang")
            .and_then(|v| v.as_str())
            .unwrap_or("zh");
        let tl = args
            .get("target_lang")
            .and_then(|v| v.as_str())
            .unwrap_or("en");
        let cat = args.get("category").and_then(|v| v.as_str());

        let bundle_root = config::bundle::detect_bundle_root().unwrap_or_else(|_| "/".into());
        let cfg = config::bundle::load_config(&bundle_root).unwrap_or_default();
        let db_path = config::bundle::resolve_path(&bundle_root, &cfg.storage.db_path);
        let conn = match connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("DB error: {}", e)),
        };

        match translate::add_glossary_entry(&conn, st, tt, sl, tl, cat) {
            Ok(id) => ToolResult::json(&serde_json::json!({"status": "ok", "id": id})),
            Err(e) => ToolResult::error(format!("Failed: {}", e)),
        }
    }
}

struct ListGlossaryTool;

impl Tool for ListGlossaryTool {
    fn name(&self) -> &str {
        "list_glossary"
    }
    fn description(&self) -> &str {
        "列出术语表条目，可按语言对筛选。"
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "source_lang": {"type": "string", "description": "源语言筛选"},
                "target_lang": {"type": "string", "description": "目标语言筛选"}
            }
        })
    }
    fn call(&self, args: Option<Value>) -> ToolResult {
        let args = args.unwrap_or_default();
        let sl = args.get("source_lang").and_then(|v| v.as_str());
        let tl = args.get("target_lang").and_then(|v| v.as_str());

        let bundle_root = config::bundle::detect_bundle_root().unwrap_or_else(|_| "/".into());
        let cfg = config::bundle::load_config(&bundle_root).unwrap_or_default();
        let db_path = config::bundle::resolve_path(&bundle_root, &cfg.storage.db_path);
        let conn = match connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("DB error: {}", e)),
        };

        match translate::list_glossary(&conn, sl, tl) {
            Ok(entries) => ToolResult::json(&serde_json::json!({"entries": entries})),
            Err(e) => ToolResult::error(format!("Failed: {}", e)),
        }
    }
}

struct DeleteGlossaryEntryTool;

impl Tool for DeleteGlossaryEntryTool {
    fn name(&self) -> &str {
        "delete_glossary_entry"
    }
    fn description(&self) -> &str {
        "删除术语表条目。"
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "术语条目 ID"}
            },
            "required": ["id"]
        })
    }
    fn call(&self, args: Option<Value>) -> ToolResult {
        let args = args.unwrap_or_default();
        let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("");

        let bundle_root = config::bundle::detect_bundle_root().unwrap_or_else(|_| "/".into());
        let cfg = config::bundle::load_config(&bundle_root).unwrap_or_default();
        let db_path = config::bundle::resolve_path(&bundle_root, &cfg.storage.db_path);
        let conn = match connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("DB error: {}", e)),
        };

        match translate::delete_glossary_entry(&conn, id) {
            Ok(()) => ToolResult::json(&serde_json::json!({"status": "ok"})),
            Err(e) => ToolResult::error(format!("Failed: {}", e)),
        }
    }
}

/// Build the tool registry with all MCP tools.
pub fn build_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(HealthCheckTool));
    registry.register(Box::new(KnowledgeStatsTool));
    registry.register(Box::new(SyncNowTool));
    registry.register(Box::new(GetSyncStatusTool));
    registry.register(Box::new(ListWatchFoldersTool));
    registry.register(Box::new(RebuildIndexTool));
    registry.register(Box::new(SearchKnowledgeTool));
    registry.register(Box::new(AskQuestionTool));
    registry.register(Box::new(GetSourcesTool));
    registry.register(Box::new(SearchGraphTool));
    registry.register(Box::new(ExploreGraphNodeTool));
    registry.register(Box::new(TranslateTextTool));
    registry.register(Box::new(AddGlossaryEntryTool));
    registry.register(Box::new(ListGlossaryTool));
    registry.register(Box::new(DeleteGlossaryEntryTool));
    registry.register(Box::new(ConfigureWatchFolderTool));
    registry.register(Box::new(ListDocumentsTool));
    registry.register(Box::new(GetDocumentTool));
    registry.register(Box::new(ForgetDocumentTool));
    registry.register(Box::new(ReindexDocumentTool));
    registry.register(Box::new(UpdateGlossaryEntryTool));
    registry
}

/// Run the MCP stdio server. Blocks until stdin EOF.
pub fn run_mcp_server() -> Result<()> {
    let bundle_root = match config::bundle::detect_bundle_root() {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("Bundle root detection failed: {}. Using cwd.", e);
            std::env::current_dir().unwrap_or_else(|_| "/".into())
        }
    };

    tracing::info!(bundle_root = %bundle_root.display(), "Bundle root detected");

    let registry = build_registry();
    tracing::info!(tool_count = registry.list().len(), "Tools registered");

    let server = McpServer::new(
        registry,
        "knowledge-companion".to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
    );

    let adapter = StdioAdapter::new(server);
    adapter.run()
}
