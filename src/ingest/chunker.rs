//! Document chunker.
//!
//! Splits parsed documents into overlapping chunks for indexing.
//! Prioritizes natural boundaries: headings, then paragraphs.

use crate::ingest::ParsedDocument;

/// A text chunk with positional metadata.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub content: String,
    pub heading_path: Vec<String>,
    pub start_line: usize,
    pub end_line: usize,
    pub token_estimate: usize,
}

/// Chunking configuration.
pub struct ChunkConfig {
    pub target_tokens: usize,
    pub overlap_tokens: usize,
}

impl Default for ChunkConfig {
    fn default() -> Self {
        Self {
            target_tokens: 650,
            overlap_tokens: 80,
        }
    }
}

/// Split a parsed document into chunks.
///
/// Strategy: split by section first. If section exceeds target, split by paragraph.
pub fn chunk_document(doc: &ParsedDocument, config: &ChunkConfig) -> Vec<Chunk> {
    let mut chunks = Vec::new();

    for section in &doc.sections {
        let section_tokens = estimate_tokens(&section.content);

        if section_tokens <= config.target_tokens {
            chunks.push(Chunk {
                content: section.content.clone(),
                heading_path: section.heading_path.clone(),
                start_line: section.start_line,
                end_line: section.end_line,
                token_estimate: section_tokens,
            });
        } else {
            // Split by paragraph; if no paragraphs, split by lines; if no lines, split by words
            let paragraphs = split_paragraphs(&section.content);
            let paragraphs = if paragraphs.len() <= 1 {
                let lines = split_lines(&section.content);
                if lines.len() <= 1 {
                    split_words(&section.content, config.target_tokens)
                } else {
                    lines
                }
            } else {
                paragraphs
            };
            let mut current = String::new();
            let mut current_start = section.start_line;
            let mut para_start = section.start_line;

            for (i, para) in paragraphs.iter().enumerate() {
                let para_tokens = estimate_tokens(para);
                let combined_tokens = estimate_tokens(&current) + para_tokens;

                if combined_tokens > config.target_tokens && !current.is_empty() {
                    chunks.push(Chunk {
                        content: current.trim().to_string(),
                        heading_path: section.heading_path.clone(),
                        start_line: current_start,
                        end_line: para_start,
                        token_estimate: estimate_tokens(&current),
                    });

                    // Overlap: keep last N tokens of previous chunk
                    let words: Vec<&str> = current.split_whitespace().collect();
                    let overlap_count = (config.overlap_tokens / 2).min(words.len());
                    current = words[words.len() - overlap_count..].join(" ");
                    current_start = para_start
                        .saturating_sub(overlap_count / 10)
                        .max(section.start_line);
                }

                if !current.is_empty() {
                    current.push('\n');
                }
                current.push_str(para);
                para_start = section.start_line + i + 1;
            }

            if !current.trim().is_empty() {
                chunks.push(Chunk {
                    content: current.trim().to_string(),
                    heading_path: section.heading_path.clone(),
                    start_line: current_start,
                    end_line: section.end_line,
                    token_estimate: estimate_tokens(&current),
                });
            }
        }
    }

    chunks
}

/// Rough token count: split by whitespace, divide by 0.75.
fn estimate_tokens(text: &str) -> usize {
    (text.split_whitespace().count() as f64 / 0.75) as usize
}

/// Split text by fixed-size word groups (last-resort fallback).
fn split_words(text: &str, target_tokens: usize) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let words_per_chunk = (target_tokens as f64 * 0.75) as usize;
    words
        .chunks(words_per_chunk.max(1))
        .map(|chunk| chunk.join(" "))
        .collect()
}

/// Split text by lines (fallback when no paragraph breaks).
fn split_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.to_string())
        .filter(|l| !l.trim().is_empty())
        .collect()
}

/// Split text into paragraphs (separated by blank lines).
fn split_paragraphs(text: &str) -> Vec<String> {
    text.split("\n\n")
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_short_section() {
        let doc = ParsedDocument {
            title: "test".into(),
            plain_text: "hello world".into(),
            sections: vec![crate::ingest::Section {
                heading: "Intro".into(),
                heading_path: vec!["Intro".into()],
                content: "hello world".into(),
                start_line: 1,
                end_line: 2,
            }],
            tags: vec![],
            aliases: vec![],
            wikilinks: vec![],
            metadata: serde_json::json!({}),
            warnings: vec![],
        };

        let chunks = chunk_document(&doc, &ChunkConfig::default());
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].content, "hello world");
    }

    #[test]
    fn test_chunk_long_section() {
        let long_text = "word ".repeat(1000);
        let doc = ParsedDocument {
            title: "test".into(),
            plain_text: long_text.clone(),
            sections: vec![crate::ingest::Section {
                heading: "Long".into(),
                heading_path: vec!["Long".into()],
                content: long_text,
                start_line: 1,
                end_line: 1,
            }],
            tags: vec![],
            aliases: vec![],
            wikilinks: vec![],
            metadata: serde_json::json!({}),
            warnings: vec![],
        };

        let chunks = chunk_document(&doc, &ChunkConfig::default());
        assert!(
            chunks.len() > 1,
            "Long section should be split into multiple chunks, got {}",
            chunks.len()
        );
    }
}
