//! Citation builder and validator.
//!
//! Citations link RAG answers back to source documents and chunks.

use serde::Serialize;

/// A citation linking to a source.
#[derive(Debug, Clone, Serialize)]
pub struct Citation {
    pub source_id: String,
    pub title: String,
    pub source_path: String,
}

impl Citation {
    pub fn new(source_id: &str, title: &str, source_path: &str) -> Self {
        Self {
            source_id: source_id.to_string(),
            title: title.to_string(),
            source_path: source_path.to_string(),
        }
    }
}

/// Validate that citations in an answer refer to real source IDs.
/// Source IDs are in format `S-<uuid>` (e.g. S-550e8400-e29b-41d4-a716-446655440000).
pub fn validate_citations(answer: &str, valid_source_ids: &[String]) -> Vec<String> {
    let mut invalid = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // Find all [S-xxxx] patterns in the answer
    let mut remaining = answer;
    while let Some(start) = remaining.find("[S-") {
        let after_start = &remaining[start + 3..]; // after "S-"
        if let Some(end) = after_start.find(']') {
            let sid = format!("S-{}", &after_start[..end].trim());
            seen.insert(sid);
            remaining = &remaining[start + 3 + end + 1..];
        } else {
            break;
        }
    }

    for sid in &seen {
        if !valid_source_ids.contains(sid) {
            invalid.push(sid.clone());
        }
    }

    invalid
}
