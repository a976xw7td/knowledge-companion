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

    // Tag nodes — shared, doc_id = NULL. Lookup real ID after insert-or-ignore.
    for tag in &doc.tags {
        let tid = uuid::Uuid::new_v4().to_string();
        let stable_key = format!("tag:{}", tag);
        conn.execute(
            "INSERT OR IGNORE INTO graph_nodes (id, stable_key, node_type, label, doc_id, metadata, created_at, updated_at)
             VALUES (?1, ?2, 'tag', ?3, NULL, '{}', ?4, ?4)",
            rusqlite::params![tid, stable_key, tag, now],
        )?;
        // Get the actual node ID (new or existing)
        let actual_tid: String = conn.query_row(
            "SELECT id FROM graph_nodes WHERE stable_key = ?1",
            [&stable_key],
            |r| r.get(0),
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO graph_edges (id, source_id, target_id, relation, weight, metadata, created_at)
             VALUES (?1, ?2, ?3, 'tagged_with', 1.0, '{}', ?4)",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), doc_node_id, actual_tid, now],
        )?;
    }

    // Wikilink nodes — shared, doc_id = NULL. Same pattern.
    for wikilink in &doc.wikilinks {
        let wid = uuid::Uuid::new_v4().to_string();
        let stable_key = format!("wikilink:{}", wikilink);
        conn.execute(
            "INSERT OR IGNORE INTO graph_nodes (id, stable_key, node_type, label, doc_id, metadata, created_at, updated_at)
             VALUES (?1, ?2, 'wikilink', ?3, NULL, '{}', ?4, ?4)",
            rusqlite::params![wid, stable_key, wikilink, now],
        )?;
        // Get the actual node ID
        let actual_wid: String = conn.query_row(
            "SELECT id FROM graph_nodes WHERE stable_key = ?1",
            [&stable_key],
            |r| r.get(0),
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO graph_edges (id, source_id, target_id, relation, weight, metadata, created_at)
             VALUES (?1, ?2, ?3, 'links_to', 1.0, '{}', ?4)",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), doc_node_id, actual_wid, now],
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::ParsedDocument;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS graph_nodes (
                id TEXT PRIMARY KEY, stable_key TEXT UNIQUE NOT NULL,
                node_type TEXT NOT NULL, label TEXT NOT NULL,
                doc_id TEXT, metadata TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL, updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS graph_edges (
                id TEXT PRIMARY KEY, source_id TEXT NOT NULL,
                target_id TEXT NOT NULL, relation TEXT NOT NULL,
                weight REAL NOT NULL DEFAULT 1.0,
                metadata TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL,
                FOREIGN KEY (source_id) REFERENCES graph_nodes(id),
                FOREIGN KEY (target_id) REFERENCES graph_nodes(id)
            );
            CREATE INDEX IF NOT EXISTS idx_gn_doc_id ON graph_nodes(doc_id);
            CREATE INDEX IF NOT EXISTS idx_ge_source ON graph_edges(source_id);
            CREATE INDEX IF NOT EXISTS idx_ge_target ON graph_edges(target_id);",
        )
        .unwrap();
        conn
    }

    fn make_doc(title: &str, tags: Vec<&str>, wikilinks: Vec<&str>) -> ParsedDocument {
        ParsedDocument {
            title: title.to_string(),
            aliases: vec![],
            tags: tags.iter().map(|s| s.to_string()).collect(),
            wikilinks: wikilinks.iter().map(|s| s.to_string()).collect(),
            sections: vec![],
            plain_text: String::new(),
            metadata: serde_json::json!({}),
            warnings: vec![],
        }
    }

    #[test]
    fn test_shared_tags_across_documents() {
        let conn = test_conn();

        // Index doc A with tag "rust"
        let doc_a = make_doc("Doc A", vec!["rust"], vec![]);
        build_document_graph(&conn, "doc-a", "/a.md", &doc_a).unwrap();

        // Index doc B with same tag "rust"
        let doc_b = make_doc("Doc B", vec!["rust"], vec![]);
        build_document_graph(&conn, "doc-b", "/b.md", &doc_b).unwrap();

        // Verify there is exactly ONE "tag:rust" node with doc_id=NULL
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM graph_nodes WHERE stable_key='tag:rust' AND doc_id IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "Shared tag should have exactly one node with NULL doc_id"
        );

        // Verify both docs have tagged_with edges to the same tag node
        let edge_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM graph_edges WHERE relation='tagged_with'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(edge_count, 2, "Both docs should have tagged_with edges");

        // Delete doc A's graph nodes
        conn.execute(
            "DELETE FROM graph_edges WHERE source_id IN (SELECT id FROM graph_nodes WHERE doc_id='doc-a')
             OR target_id IN (SELECT id FROM graph_nodes WHERE doc_id='doc-a')",
            [],
        )
        .unwrap();
        conn.execute("DELETE FROM graph_nodes WHERE doc_id='doc-a'", [])
            .unwrap();

        // The shared tag node should still exist
        let tag_still_exists: bool = conn
            .query_row(
                "SELECT COUNT(*)>0 FROM graph_nodes WHERE stable_key='tag:rust'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(tag_still_exists, "Shared tag should survive doc A deletion");

        // Doc B's tagged_with edge should still exist
        let b_edge_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM graph_edges e
                 JOIN graph_nodes n ON e.source_id = n.id
                 WHERE n.doc_id='doc-b' AND e.relation='tagged_with'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(b_edge_count, 1, "Doc B edge should survive doc A deletion");
    }

    #[test]
    fn test_shared_wikilinks_across_documents() {
        let conn = test_conn();

        // Index doc A with wikilink "HomePage"
        let doc_a = make_doc("Doc A", vec![], vec!["HomePage"]);
        build_document_graph(&conn, "doc-a", "/a.md", &doc_a).unwrap();

        // Index doc B with same wikilink
        let doc_b = make_doc("Doc B", vec![], vec!["HomePage"]);
        build_document_graph(&conn, "doc-b", "/b.md", &doc_b).unwrap();

        // Verify there is exactly ONE "wikilink:HomePage" node with doc_id=NULL
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM graph_nodes WHERE stable_key='wikilink:HomePage' AND doc_id IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "Shared wikilink should have one node with NULL doc_id"
        );

        // Delete doc B
        conn.execute(
            "DELETE FROM graph_edges WHERE source_id IN (SELECT id FROM graph_nodes WHERE doc_id='doc-b')
             OR target_id IN (SELECT id FROM graph_nodes WHERE doc_id='doc-b')",
            [],
        )
        .unwrap();
        conn.execute("DELETE FROM graph_nodes WHERE doc_id='doc-b'", [])
            .unwrap();

        // Wikilink node still exists
        let wl_still_exists: bool = conn
            .query_row(
                "SELECT COUNT(*)>0 FROM graph_nodes WHERE stable_key='wikilink:HomePage'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            wl_still_exists,
            "Shared wikilink should survive doc B deletion"
        );
    }

    #[test]
    fn test_insert_or_ignore_returns_existing_node_id() {
        let conn = test_conn();

        // Index first doc
        let doc_a = make_doc("First", vec!["shared-tag"], vec!["SharedPage"]);
        build_document_graph(&conn, "doc-1", "/1.md", &doc_a).unwrap();

        // Get the node IDs from the first indexing
        let tag_id_1: String = conn
            .query_row(
                "SELECT id FROM graph_nodes WHERE stable_key='tag:shared-tag'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let wl_id_1: String = conn
            .query_row(
                "SELECT id FROM graph_nodes WHERE stable_key='wikilink:SharedPage'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        // Index second doc with same tag and wikilink
        let doc_b = make_doc("Second", vec!["shared-tag"], vec!["SharedPage"]);
        build_document_graph(&conn, "doc-2", "/2.md", &doc_b).unwrap();

        // The node IDs should be the same (INSERT OR IGNORE + stable key lookup)
        let tag_id_2: String = conn
            .query_row(
                "SELECT id FROM graph_nodes WHERE stable_key='tag:shared-tag'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let wl_id_2: String = conn
            .query_row(
                "SELECT id FROM graph_nodes WHERE stable_key='wikilink:SharedPage'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        assert_eq!(
            tag_id_1, tag_id_2,
            "Tag node ID should be stable across insertions"
        );
        assert_eq!(
            wl_id_1, wl_id_2,
            "Wikilink node ID should be stable across insertions"
        );
    }
}
