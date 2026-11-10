//! CheckJob definition + idempotent run recording (P3-01).
//! Apalis is at-least-once: the same `run_id` may be delivered repeatedly, so every
//! side effect keyed by `run_id` must be conflict-safe (`ON CONFLICT DO NOTHING` / guarded UPDATE).
use apalis::prelude::Data;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CheckJob {
    /// == `check_runs.id`; the retry idempotency key (plan §5.1).
    pub run_id: Uuid,
    /// Checker view tag (Phlogo `checker_tag`, e.g. "default" / "miaospeed").
    pub checker_tag: String,
    /// Nodes to check in this batch (internal Uuids).
    pub node_ids: Vec<Uuid>,
    /// Check presets (real MiaoSpeed mapping lands in p3-02).
    pub presets: Vec<String>,
}

/// Record a run start; returns true iff this call created the row (first delivery).
pub async fn record_check_run(pool: &PgPool, job: &CheckJob) -> sqlx::Result<bool> {
    let n = sqlx::query(
        "INSERT INTO check_runs (id,checker_tag,node_count,status,started_at) \
         VALUES ($1,$2,$3,'started',now()) ON CONFLICT (id) DO NOTHING",
    )
    .bind(job.run_id)
    .bind(&job.checker_tag)
    .bind(job.node_ids.len() as i32)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n > 0)
}

/// Mark the run finished (idempotent: only the first transition wins).
pub async fn finish_check_run(pool: &PgPool, run_id: Uuid) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE check_runs SET status='finished', finished_at=now() \
         WHERE id=$1 AND finished_at IS NULL",
    )
    .bind(run_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Apalis handler: orchestration entry only (real MiaoSpeed call is p3-02+).
pub async fn handle_check(job: CheckJob, pool: Data<PgPool>) {
    let pool: &PgPool = &pool;
    if let Err(e) = record_check_run(pool, &job).await {
        tracing::error!(run_id = %job.run_id, error = %e, "record_check_run failed");
        return; // leave job to apalis retry semantics
    }
    if let Err(e) = finish_check_run(pool, job.run_id).await {
        tracing::error!(run_id = %job.run_id, error = %e, "finish_check_run failed");
    }
}
