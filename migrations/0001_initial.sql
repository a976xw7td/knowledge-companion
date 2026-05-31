-- Migration 0001: Initial schema for KnowledgeCompanion
-- Creates all core tables for Phase 1+ delivery.

-- Watched knowledge roots
CREATE TABLE IF NOT EXISTS watched_roots (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    root_path TEXT NOT NULL UNIQUE,
    enabled INTEGER NOT NULL DEFAULT 1,
    read_only INTEGER NOT NULL DEFAULT 1,
    include_globs TEXT NOT NULL DEFAULT '**/*.md,**/*.txt',
    exclude_globs TEXT NOT NULL DEFAULT '**/.git/**,**/node_modules/**',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Indexed documents
CREATE TABLE IF NOT EXISTS documents (
    id TEXT PRIMARY KEY,
    root_id TEXT NOT NULL REFERENCES watched_roots(id),
    source_path TEXT NOT NULL UNIQUE,
    relative_path TEXT NOT NULL,
    title TEXT NOT NULL,
    file_type TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    source_mtime INTEGER NOT NULL,
    source_size INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    error_message TEXT,
    word_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    indexed_at TEXT,
    deleted_at TEXT
);

-- Document chunks
CREATE TABLE IF NOT EXISTS chunks (
    id TEXT PRIMARY KEY,
    doc_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    chunk_index INTEGER NOT NULL,
    heading_path TEXT,
    content TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    start_line INTEGER,
    end_line INTEGER,
    token_count INTEGER,
    embedding_status TEXT NOT NULL DEFAULT 'pending',
    created_at TEXT NOT NULL,
    UNIQUE(doc_id, chunk_index)
);

-- FTS5 virtual table for keyword search
CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
    chunk_id UNINDEXED,
    doc_id UNINDEXED,
    title,
    heading,
    content,
    tokenize='unicode61'
);

-- Embedding cache
CREATE TABLE IF NOT EXISTS chunk_embeddings (
    chunk_id TEXT PRIMARY KEY REFERENCES chunks(id) ON DELETE CASCADE,
    model TEXT NOT NULL,
    dimensions INTEGER NOT NULL,
    embedding BLOB NOT NULL,
    created_at TEXT NOT NULL
);

-- Deterministic knowledge graph nodes
CREATE TABLE IF NOT EXISTS graph_nodes (
    id TEXT PRIMARY KEY,
    stable_key TEXT NOT NULL UNIQUE,
    node_type TEXT NOT NULL,
    label TEXT NOT NULL,
    doc_id TEXT REFERENCES documents(id) ON DELETE CASCADE,
    metadata TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Graph edges
CREATE TABLE IF NOT EXISTS graph_edges (
    id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES graph_nodes(id) ON DELETE CASCADE,
    target_id TEXT NOT NULL REFERENCES graph_nodes(id) ON DELETE CASCADE,
    relation TEXT NOT NULL,
    weight REAL NOT NULL DEFAULT 1.0,
    metadata TEXT,
    created_at TEXT NOT NULL,
    UNIQUE(source_id, target_id, relation)
);

-- Background index jobs
CREATE TABLE IF NOT EXISTS index_jobs (
    id TEXT PRIMARY KEY,
    job_type TEXT NOT NULL,
    doc_id TEXT,
    source_path TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    attempts INTEGER NOT NULL DEFAULT 0,
    error_message TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Sync event log
CREATE TABLE IF NOT EXISTS sync_events (
    id TEXT PRIMARY KEY,
    root_id TEXT NOT NULL REFERENCES watched_roots(id),
    event_type TEXT NOT NULL,
    source_path TEXT NOT NULL,
    message TEXT,
    created_at TEXT NOT NULL
);

-- Translation memory
CREATE TABLE IF NOT EXISTS translation_memory (
    id TEXT PRIMARY KEY,
    source_hash TEXT NOT NULL UNIQUE,
    source_text TEXT NOT NULL,
    translated_text TEXT NOT NULL,
    source_lang TEXT NOT NULL,
    target_lang TEXT NOT NULL,
    hit_count INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Glossary entries
CREATE TABLE IF NOT EXISTS glossary_entries (
    id TEXT PRIMARY KEY,
    source_term TEXT NOT NULL,
    target_term TEXT NOT NULL,
    source_lang TEXT NOT NULL,
    target_lang TEXT NOT NULL,
    category TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(source_term, source_lang, target_lang)
);
