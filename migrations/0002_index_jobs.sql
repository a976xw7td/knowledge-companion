-- Migration 0002: Index jobs enhancement
-- Adds new columns for background embedding processing.

-- Add missing columns if table exists from 0001
ALTER TABLE index_jobs ADD COLUMN chunk_id TEXT;
ALTER TABLE index_jobs ADD COLUMN started_at TEXT;
ALTER TABLE index_jobs ADD COLUMN finished_at TEXT;
ALTER TABLE index_jobs ADD COLUMN next_retry_at TEXT;

-- Add indexes for job processing
CREATE INDEX IF NOT EXISTS idx_jobs_status_next_retry ON index_jobs(status, next_retry_at);
CREATE INDEX IF NOT EXISTS idx_jobs_doc_id ON index_jobs(doc_id);
CREATE INDEX IF NOT EXISTS idx_jobs_chunk_id ON index_jobs(chunk_id);
