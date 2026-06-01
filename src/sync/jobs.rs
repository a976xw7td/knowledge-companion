//! Background embedding job queue.
//!
//! Each chunk queued for embedding gets an index_jobs row.
//! A worker picks up pending jobs, calls the embedding API,
//! stores vectors, and updates chunk status.

use anyhow::Result;
use rusqlite::Connection;
use std::time::Duration;

/// Create embedding jobs for chunks. Called after successful document indexing.
pub fn queue_embedding_jobs(
    conn: &Connection,
    doc_id: &str,
    source_path: &str,
    chunk_ids: &[String],
) {
    let now = chrono::Utc::now().to_rfc3339();
    for chunk_id in chunk_ids {
        let job_id = uuid::Uuid::new_v4().to_string();
        let _ = conn.execute(
            "INSERT INTO index_jobs (id, job_type, doc_id, chunk_id, source_path, status, attempts, created_at, updated_at)
             VALUES (?1, 'generate_embedding', ?2, ?3, ?4, 'pending', 0, ?5, ?5)",
            rusqlite::params![job_id, doc_id, chunk_id, source_path, now],
        );
    }
}

/// Pick up pending embedding jobs (up to `limit`).
pub fn claim_pending_jobs(conn: &Connection, limit: usize) -> Result<Vec<EmbeddingJob>> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut stmt = conn.prepare(
        "SELECT id, doc_id, chunk_id, source_path, attempts
         FROM index_jobs
         WHERE job_type = 'generate_embedding' AND status = 'pending'
           AND (next_retry_at IS NULL OR next_retry_at <= ?1)
         ORDER BY created_at ASC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![now, limit as i64], |row| {
        Ok(EmbeddingJob {
            id: row.get(0)?,
            doc_id: row.get(1)?,
            chunk_id: row.get(2)?,
            source_path: row.get(3)?,
            attempts: row.get(4)?,
        })
    })?;
    Ok(rows.flatten().collect())
}

/// Mark a job as processing.
pub fn mark_job_started(conn: &Connection, job_id: &str) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE index_jobs SET status='processing', started_at=?1, updated_at=?1 WHERE id=?2",
        rusqlite::params![now, job_id],
    )?;
    Ok(())
}

/// Mark a job as completed and update chunk status.
pub fn mark_job_done(conn: &Connection, job_id: &str, chunk_id: &str) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE index_jobs SET status='completed', finished_at=?1, updated_at=?1 WHERE id=?2",
        rusqlite::params![now, job_id],
    )?;
    conn.execute(
        "UPDATE chunks SET embedding_status='ready' WHERE id=?1",
        [chunk_id],
    )?;
    Ok(())
}

/// Mark a job as failed with retry logic.
pub fn mark_job_failed(
    conn: &Connection,
    job_id: &str,
    error_msg: &str,
    max_retries: u32,
) -> Result<bool> {
    let attempts: u32 = conn
        .query_row(
            "SELECT attempts FROM index_jobs WHERE id=?1",
            [job_id],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let new_attempts = attempts + 1;
    let now = chrono::Utc::now().to_rfc3339();

    if new_attempts >= max_retries {
        conn.execute(
            "UPDATE index_jobs SET status='failed', attempts=?1, error_message=?2, finished_at=?3, updated_at=?3 WHERE id=?4",
            rusqlite::params![new_attempts, error_msg, now, job_id],
        )?;
        return Ok(true); // exhausted retries
    }

    // Exponential backoff: 5s, 30s, 2min
    let delay = match new_attempts {
        1 => Duration::from_secs(5),
        2 => Duration::from_secs(30),
        _ => Duration::from_secs(120),
    };
    let retry_at =
        (chrono::Utc::now() + chrono::Duration::from_std(delay).unwrap_or_default()).to_rfc3339();

    conn.execute(
        "UPDATE index_jobs SET status='pending', attempts=?1, error_message=?2, next_retry_at=?3, updated_at=?4 WHERE id=?5",
        rusqlite::params![new_attempts, error_msg, retry_at, now, job_id],
    )?;
    Ok(false)
}

/// Delete jobs for a document.
pub fn delete_doc_jobs(conn: &Connection, doc_id: &str) -> Result<()> {
    conn.execute("DELETE FROM index_jobs WHERE doc_id=?1", [doc_id])?;
    Ok(())
}

/// Get job statistics.
pub fn job_stats(conn: &Connection) -> Result<serde_json::Value> {
    let pending: i64 = conn.query_row(
        "SELECT COUNT(*) FROM index_jobs WHERE job_type='generate_embedding' AND status='pending'",
        [], |r| r.get(0),
    ).unwrap_or(0);
    let processing: i64 = conn.query_row(
        "SELECT COUNT(*) FROM index_jobs WHERE job_type='generate_embedding' AND status='processing'",
        [], |r| r.get(0),
    ).unwrap_or(0);
    let completed: i64 = conn.query_row(
        "SELECT COUNT(*) FROM index_jobs WHERE job_type='generate_embedding' AND status='completed'",
        [], |r| r.get(0),
    ).unwrap_or(0);
    let failed: i64 = conn.query_row(
        "SELECT COUNT(*) FROM index_jobs WHERE job_type='generate_embedding' AND status='failed'",
        [], |r| r.get(0),
    ).unwrap_or(0);
    Ok(
        serde_json::json!({"pending": pending, "processing": processing, "completed": completed, "failed": failed}),
    )
}

#[derive(Debug, Clone)]
pub struct EmbeddingJob {
    pub id: String,
    pub doc_id: String,
    pub chunk_id: String,
    pub source_path: String,
    pub attempts: u32,
}
