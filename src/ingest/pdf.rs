//! Basic PDF text extraction.
//!
//! Uses simple text extraction — no OCR, no layout preservation.
//! Returns text content or a clear error for scanned/image PDFs.

use super::{DocumentParser, ParsedDocument, Section};
use anyhow::Result;
use std::path::Path;

pub struct PdfParser;

impl DocumentParser for PdfParser {
    fn supports(&self, extension: &str) -> bool {
        extension == "pdf"
    }

    fn parse(&self, path: &Path) -> Result<ParsedDocument> {
        let bytes = std::fs::read(path)?;
        let text = pdf_extract::extract_text_from_mem(&bytes).map_err(|e| {
            anyhow::anyhow!(
                "PDF extraction failed: {}. This may be a scanned/image PDF requiring OCR.",
                e
            )
        })?;

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
                "PDF text extraction is basic — scanned/image PDFs are not supported.".into(),
            ],
        })
    }
}
