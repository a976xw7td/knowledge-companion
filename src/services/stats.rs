//! Knowledge statistics service.
//!
//! Queries SQLite for real document counts, chunks, tags, wikilinks,
//! and storage usage.

use crate::config::bundle::{detect_bundle_root, load_config, resolve_path};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct KnowledgeStats {
    pub total_documents: u64,
    pub total_chunks: u64,
    pub total_tags: u64,
    pub total_wikilinks: u64,
    pub storage_bytes: u64,
    pub last_sync_at: Option<String>,
}

/// Get knowledge base statistics from SQLite.
pub fn get_stats() -> KnowledgeStats {
    let bundle_root = detect_bundle_root().ok();
    let cfg = bundle_root
        .as_ref()
        .map(|b| load_config(b).unwrap_or_default());
    let db_path = bundle_root
        .as_ref()
        .zip(cfg.as_ref())
        .map(|(b, c)| resolve_path(b, &c.storage.db_path));

    let (docs, chunks, tags, wikilinks, last_sync, db_size) = db_path
        .filter(|p| p.exists())
        .and_then(|p| crate::db::connection::open(&p).ok())
        .map(|conn| {
            let docs: u64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM documents WHERE status != 'deleted'",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            let chunks: u64 = conn
                .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
                .unwrap_or(0);
            let tags: u64 = conn
                .query_row(
                    "SELECT COUNT(DISTINCT label) FROM graph_nodes WHERE node_type = 'tag'",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            let wikilinks: u64 = conn
                .query_row(
                    "SELECT COUNT(DISTINCT label) FROM graph_nodes WHERE node_type = 'wikilink'",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            let last_sync: Option<String> = conn
                .query_row(
                    "SELECT created_at FROM sync_events ORDER BY created_at DESC LIMIT 1",
                    [],
                    |r| r.get(0),
                )
                .ok();
            let db_size = (docs * 2048) + (chunks * 4096); // rough estimate
            (docs, chunks, tags, wikilinks, last_sync, db_size)
        })
        .unwrap_or((0, 0, 0, 0, None, 0));

    KnowledgeStats {
        total_documents: docs,
        total_chunks: chunks,
        total_tags: tags,
        total_wikilinks: wikilinks,
        storage_bytes: db_size,
        last_sync_at: last_sync,
    }
}
