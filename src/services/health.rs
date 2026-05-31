//! Health check service.
//!
//! Provides system health diagnostics: bundle root, config loading,
//! database status (future), and directory accessibility.

use crate::config::bundle::detect_bundle_root;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct HealthStatus {
    pub status: String,
    pub version: String,
    pub bundle_root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub db_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_loaded: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub knowledge_dir_exists: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_dir_writable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<Vec<String>>,
}

/// Run the health check and return a structured status.
///
/// This is intentionally synchronous and simple in Phase 0.
/// Future phases will add DB, FTS, embedding, and LLM provider checks.
pub fn check_health() -> HealthStatus {
    let errors: Vec<String> = Vec::new();
    let version = env!("CARGO_PKG_VERSION").to_string();

    // Detect bundle root
    let bundle_root = match detect_bundle_root() {
        Ok(path) => path,
        Err(e) => {
            return HealthStatus {
                status: "error".to_string(),
                version,
                bundle_root: "unknown".to_string(),
                db_status: None,
                config_loaded: None,
                knowledge_dir_exists: None,
                data_dir_writable: None,
                errors: Some(vec![format!("Failed to detect bundle root: {}", e)]),
            };
        }
    };

    let bundle_root_str = bundle_root.display().to_string();

    // Check knowledge/ directory
    let knowledge_dir = bundle_root.join("knowledge");
    let knowledge_exists = knowledge_dir.is_dir();

    // Check data/ directory writability
    let data_dir = bundle_root.join("data");
    let data_writable = check_writable(&data_dir);

    // Attempt config loading
    let config_loaded = crate::config::bundle::load_config(&bundle_root).is_ok();

    // DB status — check if DB exists and is healthy
    let cfg = crate::config::bundle::load_config(&bundle_root).unwrap_or_default();
    let db_path = crate::config::bundle::resolve_path(&bundle_root, &cfg.storage.db_path);
    let db_status = if db_path.exists() {
        match crate::db::connection::check_integrity(&db_path) {
            Ok(()) => {
                // Quick stats: count documents
                if let Ok(conn) = crate::db::connection::open(&db_path) {
                    let doc_count: i64 = conn
                        .query_row(
                            "SELECT COUNT(*) FROM documents WHERE status != 'deleted'",
                            [],
                            |r| r.get(0),
                        )
                        .unwrap_or(0);
                    let chunk_count: i64 = conn
                        .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
                        .unwrap_or(0);
                    format!("ok ({} docs, {} chunks)", doc_count, chunk_count)
                } else {
                    "integrity_ok".to_string()
                }
            }
            Err(e) => format!("integrity_failed: {}", e),
        }
    } else {
        "not_initialized".to_string()
    };

    // Determine overall status
    let status = if knowledge_exists && data_writable && config_loaded && errors.is_empty() {
        "ok"
    } else if errors.is_empty() {
        "degraded"
    } else {
        "error"
    };

    HealthStatus {
        status: status.to_string(),
        version,
        bundle_root: bundle_root_str,
        db_status: Some(db_status),
        config_loaded: Some(config_loaded),
        knowledge_dir_exists: Some(knowledge_exists),
        data_dir_writable: Some(data_writable),
        errors: if errors.is_empty() {
            None
        } else {
            Some(errors)
        },
    }
}

fn check_writable(dir: &std::path::Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    // Try to create and remove a temp file
    let test_file = dir.join(".health_check_test");
    match std::fs::write(&test_file, b"test") {
        Ok(_) => {
            let _ = std::fs::remove_file(&test_file);
            true
        }
        Err(_) => false,
    }
}
