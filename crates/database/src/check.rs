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
    deep: Option<(i32, i32, i32, Option<String>, serde_json::Value, serde_json::Value)>,
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
    pub deep: Option<(i32, i32, i32, Option<String>, serde_json::Value, serde_json::Value)>,
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
    tx.commit().await?;
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
    .execute(pool)
    .await?;
    Ok(())
}
