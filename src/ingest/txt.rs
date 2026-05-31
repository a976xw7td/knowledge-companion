//! Plain text parser.

use super::{DocumentParser, ParsedDocument, Section};
use anyhow::Result;
use std::path::Path;

pub struct TxtParser;

impl DocumentParser for TxtParser {
    fn supports(&self, extension: &str) -> bool {
        extension == "txt"
    }

    fn parse(&self, path: &Path) -> Result<ParsedDocument> {
        let content = std::fs::read_to_string(path)?;
        let title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("untitled")
            .to_string();

        let line_count = content.lines().count();

        Ok(ParsedDocument {
            title,
            sections: vec![Section {
                heading: String::new(),
                heading_path: vec![],
                content: content.clone(),
                start_line: 1,
                end_line: line_count.max(1),
            }],
            plain_text: content,
            tags: vec![],
            aliases: vec![],
            wikilinks: vec![],
            metadata: serde_json::json!({}),
            warnings: vec![],
        })
    }
}
