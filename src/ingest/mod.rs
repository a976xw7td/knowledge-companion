//! Document ingestion pipeline.
//!
//! Parsers extract structured data from source files.
//! The chunker splits documents into searchable chunks.

pub mod chunker;
pub mod docx;
pub mod markdown;
pub mod pdf;
pub mod txt;

use std::path::Path;

/// Structured result of parsing a document.
#[derive(Debug, Clone)]
pub struct ParsedDocument {
    pub title: String,
    pub plain_text: String,
    pub sections: Vec<Section>,
    pub tags: Vec<String>,
    pub aliases: Vec<String>,
    pub wikilinks: Vec<String>,
    pub metadata: serde_json::Value,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Section {
    pub heading: String,
    pub heading_path: Vec<String>,
    pub content: String,
    pub start_line: usize,
    pub end_line: usize,
}

/// Unified document parser trait.
pub trait DocumentParser: Send + Sync {
    fn supports(&self, extension: &str) -> bool;
    fn parse(&self, path: &Path) -> anyhow::Result<ParsedDocument>;
}

/// Registry of available parsers.
pub struct ParserRegistry {
    parsers: Vec<Box<dyn DocumentParser>>,
}

impl ParserRegistry {
    pub fn new() -> Self {
        Self {
            parsers: Vec::new(),
        }
    }

    pub fn register(&mut self, parser: Box<dyn DocumentParser>) {
        self.parsers.push(parser);
    }

    pub fn find(&self, extension: &str) -> Option<&dyn DocumentParser> {
        self.parsers
            .iter()
            .find(|p| p.supports(extension))
            .map(|p| p.as_ref())
    }

    pub fn default_registry() -> Self {
        let mut reg = Self::new();
        reg.register(Box::new(markdown::MarkdownParser));
        reg.register(Box::new(txt::TxtParser));
        reg.register(Box::new(pdf::PdfParser));
        reg.register(Box::new(docx::DocxParser));
        reg
    }
}

impl Default for ParserRegistry {
    fn default() -> Self {
        Self::default_registry()
    }
}
