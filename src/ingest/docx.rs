//! Basic DOCX text extraction.
//!
//! DOCX files are ZIP archives containing XML. This extracts text from
//! word/document.xml without preserving formatting, tables, or images.

use super::{DocumentParser, ParsedDocument, Section};
use anyhow::{Context, Result};
use std::io::Read;
use std::path::Path;

pub struct DocxParser;

impl DocumentParser for DocxParser {
    fn supports(&self, extension: &str) -> bool {
        extension == "docx"
    }

    fn parse(&self, path: &Path) -> Result<ParsedDocument> {
        let file = std::fs::File::open(path)
            .with_context(|| format!("Failed to open DOCX: {}", path.display()))?;
        let mut archive = zip::ZipArchive::new(file).context("Failed to read DOCX as ZIP")?;

        let mut doc_xml = String::new();
        if let Ok(mut entry) = archive.by_name("word/document.xml") {
            entry
                .read_to_string(&mut doc_xml)
                .context("Failed to read word/document.xml")?;
        } else {
            return Err(anyhow::anyhow!("word/document.xml not found in DOCX"));
        }

        // Extract text from XML paragraphs
        let text = extract_text(&doc_xml);

        let title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("untitled")
            .to_string();

        let line_count = text.lines().count();

        Ok(ParsedDocument {
            title,
            sections: vec![Section {
                heading: String::new(),
                heading_path: vec![],
                content: text.clone(),
                start_line: 1,
                end_line: line_count.max(1),
            }],
            plain_text: text,
            tags: vec![],
            aliases: vec![],
            wikilinks: vec![],
            metadata: serde_json::json!({}),
            warnings: vec![
                "DOCX parsing is basic — formatting, tables, and images are not preserved.".into(),
            ],
        })
    }
}

/// Simple XML text extraction: strip all tags, keep text content.
fn extract_text(xml: &str) -> String {
    strip_xml_tags(xml)
}

fn strip_xml_tags(xml: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    let chars = xml.chars().peekable();

    for ch in chars {
        if ch == '<' {
            in_tag = true;
        } else if ch == '>' {
            in_tag = false;
            // Add paragraph break after </w:p> equivalent
            result.push('\n');
        } else if !in_tag {
            result.push(ch);
        }
    }

    // Clean up: collapse whitespace
    result
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}
