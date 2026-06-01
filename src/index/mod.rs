//! Indexing layer — FTS5, vector embeddings, and knowledge graph.
//!
//! Each indexer operates on chunks and documents after parsing.

pub mod embed;
pub mod fts;
pub mod graph;
pub mod tokenizer;
pub mod vector;
