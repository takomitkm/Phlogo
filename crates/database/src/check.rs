//! Check-history persistence (p3-04). Kept free of checker-miaospeed types: the
//! persistence layer takes plain parameters, so `crates/database` never depends on
//! any checker/adapter crate (plan §3.1 layering).
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// Merged initial+deep metrics for one row of `node_check_results`.
/// `latency_ms == -1` dead; `speed_kbps == 0` untested (Phlogo conventions).
pub fn build_metrics(
    latency_ms: i32,
    deep: Option<(
        i32,
        i32,
        i32,
        Option<String>,
        serde_json::Value,
        serde_json::Value,
    )>,
) -> serde_json::Value {
    let mut m = serde_json::json!({ "latency_ms": latency_ms });
    if let Some((avg, jitter, speed, region, streaming, metadata)) = deep {
        m["avg_latency_ms"] = serde_json::json!(avg);
        m["jitter_ms"] = serde_json::json!(jitter);
        m["speed_kbps"] = serde_json::json!(speed);
        m["streaming"] = streaming;
        m["metadata"] = metadata;
        if let Some(r) = region.filter(|s| !s.is_empty()) {
            m["region"] = serde_json::json!(r);
        }
    }
    m
}

/// One result row to persist.
pub struct CheckResultRow {
    pub node_id: Uuid,
    pub latency_ms: i32,
    pub deep: Option<(
        i32,
        i32,
        i32,
        Option<String>,
        serde_json::Value,
        serde_json::Value,
    )>,
}

pub fn result_status_of(latency_ms: i32) -> &'static str {
    if latency_ms == -1 {
        "dead"
    } else {
        "alive"
    }
}

/// Persist one check run atomically (idempotent by run_id + unique key per result row).
pub async fn persist_check_run(
    pool: &PgPool,
    run_id: Uuid,
    tag: &str,
    started_at: Option<DateTime<Utc>>,
    results: &[CheckResultRow],
    unreported: &[Uuid],
) -> anyhow::Result<usize> {
    let mut tx = pool.begin().await?;
    let n = persist_check_run_in(tx.as_mut(), run_id, tag, started_at, results, unreported).await?;
    tx.commit().await?;
    Ok(n)
}

/// [`persist_check_run`] inside a caller-owned transaction (M2.1: results + node_status
/// + terminal state commit together).
pub async fn persist_check_run_in(
    tx: &mut sqlx::PgConnection,
    run_id: Uuid,
    tag: &str,
    started_at: Option<DateTime<Utc>>,
    results: &[CheckResultRow],
    unreported: &[Uuid],
) -> anyhow::Result<usize> {
    sqlx::query(
        "INSERT INTO check_runs (id,checker_tag,node_count,status,started_at,finished_at) \
         VALUES ($1,$2,$3,'success',COALESCE($4, now()),now()) \
         ON CONFLICT (id) DO UPDATE SET status='success', finished_at=now(), node_count=$3",
    )
    .bind(run_id)
    .bind(tag)
    .bind((results.len() + unreported.len()) as i32)
    .bind(started_at)
    .execute(&mut *tx)
    .await?;

    let mut inserted = 0usize;
    for r in results {
        let n = sqlx::query(
            "INSERT INTO node_check_results (node_id,check_run_id,checker_tag,metrics,result_status,checked_at) \
             VALUES ($1,$2,$3,$4,$5,now()) \
             ON CONFLICT (node_id,check_run_id,checker_tag) DO NOTHING",
        )
        .bind(r.node_id)
        .bind(run_id)
        .bind(tag)
        .bind(build_metrics(r.latency_ms, r.deep.clone()))
        .bind(result_status_of(r.latency_ms))
        .execute(&mut *tx)
        .await?
        .rows_affected();
        inserted += n as usize;
    }
    for nid in unreported {
        let n = sqlx::query(
            "INSERT INTO node_check_results (node_id,check_run_id,checker_tag,metrics,result_status,checked_at) \
             VALUES ($1,$2,$3,'{}'::jsonb,'unknown',now()) \
             ON CONFLICT (node_id,check_run_id,checker_tag) DO NOTHING",
        )
        .bind(nid)
        .bind(run_id)
        .bind(tag)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        inserted += n as usize;
    }
    Ok(inserted)
}

/// Per-tag snapshot upsert guarded by the late-result rule (only strictly newer wins).
pub async fn upsert_node_status(
    pool: &PgPool,
    node_id: Uuid,
    tag: &str,
    health: &str,
    checked_at: DateTime<Utc>,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    upsert_node_status_in(tx.as_mut(), node_id, tag, health, checked_at).await?;
    tx.commit().await?;
    Ok(())
}

/// [`upsert_node_status`] inside a caller-owned transaction (M2.1 persistence consistency).
pub async fn upsert_node_status_in(
    tx: &mut sqlx::PgConnection,
    node_id: Uuid,
    tag: &str,
    health: &str,
    checked_at: DateTime<Utc>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO node_status (node_id,checker_tag,health,last_checked_at) \
         VALUES ($1,$2,$3,$4) \
         ON CONFLICT (node_id,checker_tag) DO UPDATE \
           SET health=EXCLUDED.health, last_checked_at=EXCLUDED.last_checked_at \
           WHERE EXCLUDED.last_checked_at > node_status.last_checked_at",
    )
    .bind(node_id)
    .bind(tag)
    .bind(health)
    .bind(checked_at)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// Flip a run to `success` inside a caller-owned transaction (M2.1: the terminal write
/// participates in the same commit as the results).
pub async fn finish_check_run_in(tx: &mut sqlx::PgConnection, run_id: Uuid) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE check_runs SET status='success', finished_at=now() \
         WHERE id=$1 AND finished_at IS NULL",
    )
    .bind(run_id)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// Owner-guarded terminal write for the execution lease (M3.1-3).
///
/// Releases `exec_owner`/`exec_lease_until` in the SAME statement that confirms the
/// terminal state, and only if this caller still holds the lease. It deliberately does
/// NOT require `finished_at IS NULL`: `persist_check_run_in` has already flipped the run
/// to `success` inside this very transaction (the results and the terminal state commit
/// together), so such a guard could never match. Ownership is the lease, and a lost lease
/// makes the whole transaction roll back — taking the premature `success` with it.
pub async fn finish_check_run_owned_in(
    tx: &mut sqlx::PgConnection,
    run_id: Uuid,
    exec_owner: Uuid,
) -> anyhow::Result<()> {
    let affected = sqlx::query(
        "UPDATE check_runs \
            SET status='success', finished_at=now(), exec_owner=NULL, exec_lease_until=NULL \
          WHERE id=$1 AND exec_owner=$2",
    )
    .bind(run_id)
    .bind(exec_owner)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    anyhow::ensure!(
        affected == 1,
        "execution lease lost before the terminal write (run {run_id})"
    );
    Ok(())
}
