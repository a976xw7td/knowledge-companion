//! Deterministic knowledge graph builder.
//!
//! Builds graph nodes and edges from parsed documents:
//! - document nodes
//! - heading nodes
//! - tag nodes
//! - wikilink nodes
//! - alias nodes
//!
//! Edges: contains, links_to, tagged_with, alias_of

use anyhow::Result;
use rusqlite::Connection;

use crate::ingest::ParsedDocument;

/// Build graph nodes and edges for a parsed document.
/// All operations are deterministic — no LLM involved.
pub fn build_document_graph(
    conn: &Connection,
    doc_id: &str,
    source_path: &str,
    doc: &ParsedDocument,
) -> Result<()> {
    // Delete existing nodes/edges for this doc (re-idempotent)
    conn.execute("DELETE FROM graph_edges WHERE source_id IN (SELECT id FROM graph_nodes WHERE doc_id = ?1) OR target_id IN (SELECT id FROM graph_nodes WHERE doc_id = ?1)", [doc_id])?;
    conn.execute("DELETE FROM graph_nodes WHERE doc_id = ?1", [doc_id])?;

    let now = chrono::Utc::now().to_rfc3339();

    // Document node
    let doc_node_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO graph_nodes (id, stable_key, node_type, label, doc_id, metadata, created_at, updated_at)
         VALUES (?1, ?2, 'document', ?3, ?4, ?5, ?6, ?6)",
        rusqlite::params![doc_node_id, format!("doc:{}", source_path), doc.title, doc_id, serde_json::json!({"source_path": source_path}).to_string(), now],
    )?;

    // Heading nodes
    let mut heading_ids: Vec<String> = Vec::new();
    for section in &doc.sections {
        if section.heading.is_empty() {
            continue;
        }
        let hid = uuid::Uuid::new_v4().to_string();
        let stable_key = format!("heading:{}:{}", source_path, section.heading_path.join("/"));
        conn.execute(
            "INSERT OR IGNORE INTO graph_nodes (id, stable_key, node_type, label, doc_id, metadata, created_at, updated_at)
             VALUES (?1, ?2, 'heading', ?3, ?4, ?5, ?6, ?6)",
            rusqlite::params![hid, stable_key, section.heading, doc_id, serde_json::json!({"line": section.start_line}).to_string(), now],
        )?;
        // contains edge: doc -> heading
        conn.execute(
            "INSERT OR IGNORE INTO graph_edges (id, source_id, target_id, relation, weight, metadata, created_at)
             VALUES (?1, ?2, ?3, 'contains', 1.0, '{}', ?4)",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), doc_node_id, hid, now],
        )?;
        heading_ids.push(hid);
    }

    // Tag nodes
    for tag in &doc.tags {
        let tid = uuid::Uuid::new_v4().to_string();
        let stable_key = format!("tag:{}", tag);
        conn.execute(
            "INSERT OR IGNORE INTO graph_nodes (id, stable_key, node_type, label, doc_id, metadata, created_at, updated_at)
             VALUES (?1, ?2, 'tag', ?3, ?4, '{}', ?5, ?5)",
            rusqlite::params![tid, stable_key, tag, doc_id, now],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO graph_edges (id, source_id, target_id, relation, weight, metadata, created_at)
             VALUES (?1, ?2, ?3, 'tagged_with', 1.0, '{}', ?4)",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), doc_node_id, tid, now],
        )?;
    }

    // Wikilink nodes
    for wikilink in &doc.wikilinks {
        let wid = uuid::Uuid::new_v4().to_string();
        let stable_key = format!("wikilink:{}", wikilink);
        conn.execute(
            "INSERT OR IGNORE INTO graph_nodes (id, stable_key, node_type, label, doc_id, metadata, created_at, updated_at)
             VALUES (?1, ?2, 'wikilink', ?3, ?4, '{}', ?5, ?5)",
            rusqlite::params![wid, stable_key, wikilink, doc_id, now],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO graph_edges (id, source_id, target_id, relation, weight, metadata, created_at)
             VALUES (?1, ?2, ?3, 'links_to', 1.0, '{}', ?4)",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), doc_node_id, wid, now],
        )?;
    }

    // Alias nodes
    for alias in &doc.aliases {
        let aid = uuid::Uuid::new_v4().to_string();
        let stable_key = format!("alias:{}:{}", source_path, alias);
        conn.execute(
            "INSERT OR IGNORE INTO graph_nodes (id, stable_key, node_type, label, doc_id, metadata, created_at, updated_at)
             VALUES (?1, ?2, 'alias', ?3, ?4, '{}', ?5, ?5)",
            rusqlite::params![aid, stable_key, alias, doc_id, now],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO graph_edges (id, source_id, target_id, relation, weight, metadata, created_at)
             VALUES (?1, ?2, ?3, 'alias_of', 1.0, '{}', ?4)",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), aid, doc_node_id, now],
        )?;
    }

    Ok(())
}

/// Search graph nodes by label.
pub fn search_nodes(
    conn: &Connection,
    query: &str,
    node_type: Option<&str>,
    limit: usize,
) -> Result<Vec<serde_json::Value>> {
    let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(nt) = node_type
    {
        (
            "SELECT id, stable_key, node_type, label, doc_id FROM graph_nodes
             WHERE (label LIKE ?1 OR stable_key LIKE ?1) AND node_type = ?2
             LIMIT ?3"
                .to_string(),
            vec![
                Box::new(format!("%{}%", query)),
                Box::new(nt.to_string()),
                Box::new(limit as i64),
            ],
        )
    } else {
        (
            "SELECT id, stable_key, node_type, label, doc_id FROM graph_nodes
             WHERE label LIKE ?1 OR stable_key LIKE ?1
             LIMIT ?2"
                .to_string(),
            vec![Box::new(format!("%{}%", query)), Box::new(limit as i64)],
        )
    };

    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let rows = stmt.query_map(param_refs.as_slice(), |row| {
        Ok(serde_json::json!({
            "id": row.get::<_, String>(0)?,
            "stable_key": row.get::<_, String>(1)?,
            "node_type": row.get::<_, String>(2)?,
            "label": row.get::<_, String>(3)?,
            "doc_id": row.get::<_, Option<String>>(4)?,
        }))
    })?;

    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// Explore neighbors of a graph node.
pub fn explore_node(conn: &Connection, node_id: &str, depth: usize) -> Result<serde_json::Value> {
    let node: Option<serde_json::Value> = conn
        .query_row(
            "SELECT id, stable_key, node_type, label, doc_id FROM graph_nodes WHERE id = ?1",
            [node_id],
            |row| {
                Ok(serde_json::json!({
                    "id": row.get::<_, String>(0)?,
                    "stable_key": row.get::<_, String>(1)?,
                    "node_type": row.get::<_, String>(2)?,
                    "label": row.get::<_, String>(3)?,
                    "doc_id": row.get::<_, Option<String>>(4)?,
                }))
            },
        )
        .ok();

    // Get edges
    let mut stmt = conn.prepare(
        "SELECT e.id, e.source_id, e.target_id, e.relation, e.weight,
                s.label as source_label, t.label as target_label
         FROM graph_edges e
         JOIN graph_nodes s ON e.source_id = s.id
         JOIN graph_nodes t ON e.target_id = t.id
         WHERE e.source_id = ?1 OR e.target_id = ?1
         LIMIT 50",
    )?;
    let edges: Vec<serde_json::Value> = stmt
        .query_map([node_id], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "source_id": row.get::<_, String>(1)?,
                "target_id": row.get::<_, String>(2)?,
                "relation": row.get::<_, String>(3)?,
                "weight": row.get::<_, f64>(4)?,
                "source_label": row.get::<_, String>(5)?,
                "target_label": row.get::<_, String>(6)?,
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(serde_json::json!({
        "node": node,
        "edges": edges,
        "depth": depth,
    }))
}
