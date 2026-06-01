//! CJK tokenizer using jieba-rs.
//!
//! Splits Chinese text into words for FTS5 indexing and searching.
//! This fixes the unicode61 limitation where CJK characters are not segmented.

use jieba_rs::Jieba;
use std::sync::OnceLock;

static JIEBA: OnceLock<Jieba> = OnceLock::new();

fn get_jieba() -> &'static Jieba {
    JIEBA.get_or_init(Jieba::new)
}

/// Tokenize text for FTS5: Chinese text is segmented into words separated by spaces.
/// Non-CJK text passes through unchanged.
pub fn tokenize_for_fts(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    let jieba = get_jieba();

    // Check if text contains CJK characters
    let has_cjk = text.chars().any(|c| {
        ('\u{4E00}'..='\u{9FFF}').contains(&c)      // CJK Unified
            || ('\u{3400}'..='\u{4DBF}').contains(&c)  // CJK Extension A
            || ('\u{F900}'..='\u{FAFF}').contains(&c) // CJK Compatibility
    });

    if !has_cjk {
        return text.to_string();
    }

    // Tokenize with jieba: cut the text, join with spaces
    let tokens = jieba.cut(text, true);
    tokens.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_cjk_basic() {
        let result = tokenize_for_fts("全文搜索支持中文");
        // Should produce segmented tokens like "全文 搜索 支持 中文"
        assert!(result.contains("全文"));
        assert!(result.contains("搜索"));
        assert!(result.contains("支持"));
        assert!(result.contains("中文"));
    }

    #[test]
    fn test_tokenize_non_cjk_passthrough() {
        let result = tokenize_for_fts("hello world");
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_tokenize_mixed() {
        let result = tokenize_for_fts("RAG 知识图谱 GraphRAG");
        assert!(result.contains("RAG"));
        assert!(result.contains("知识"));
        assert!(result.contains("图谱"));
        assert!(result.contains("GraphRAG"));
    }

    #[test]
    fn test_tokenize_empty() {
        assert_eq!(tokenize_for_fts(""), "");
    }
}
