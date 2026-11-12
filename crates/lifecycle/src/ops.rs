//! Atomic DB cleanup operations (P4-01 Step 3), mirroring Phlogo
//! `MarkDeadAndClean` (conditional UPDATE + only-the-winner-cleans) and
//! `removeSourceMappings` (atomic NOT EXISTS orphan guards).
use sqlx::PgPool;
use uuid::Uuid;

/// `MarkDeadAndClean`: conditional UPDATE (`status != 'dead'`) so concurrent callers
/// do not double-clean; returns true iff this caller won and performed the cleanup.
pub async fn mark_dead_and_clean(pool: &PgPool, source_id: Uuid) -> anyhow::Result<bool> {
    let mut tx = pool.begin().await?;
    let n = sqlx::query("UPDATE sources SET status='dead', updated_at=now() WHERE id=$1 AND status <> 'dead'")
        .bind(source_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if n == 0 {
        return Ok(false); // already dead elsewhere; skip cleanup
    }
    let removed = sqlx::query_scalar("DELETE FROM node_sources WHERE source_id=$1 RETURNING node_id")
        .bind(source_id)
        .fetch_all(&mut *tx)
        .await?;
    for node_id in &removed {
        mark_orphaned_if_lonely(&mut tx, *node_id).await?;
    }
    tx.commit().await?;
    Ok(true)
}

/// Atomic orphan guard: `Orphaned` iff no remaining mapping and not already orphaned.
pub async fn mark_orphaned_if_lonely(
    tx: &mut sqlx::PgConnection,
    node_id: Uuid,
) -> sqlx::Result<bool> {
    let n = sqlx::query(
        "UPDATE nodes SET membership='Orphaned', orphaned_at=now() \
         WHERE id=$1 AND membership <> 'Orphaned' \
           AND NOT EXISTS (SELECT 1 FROM node_sources WHERE node_id=$1)",
    )
    .bind(node_id)
    .execute(tx)
    .await?
    .rows_affected();
    Ok(n > 0)
}

/// `Orphaned -> Retired` once the retention window (`orphan_retention_days`) expires.
pub async fn retire_expired_orphans(pool: &PgPool, retention_days: i64) -> anyhow::Result<u64> {
    let n = sqlx::query(
        "UPDATE nodes SET membership='Retired', retired_at=now(), updated_at=now() \
         WHERE membership='Orphaned' AND orphaned_at < now() - make_interval(days => $1)",
    )
    .bind(retention_days as i32)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n)
}

/// Physically delete `Retired` nodes plus their check history. Guarded by
/// `NOT EXISTS node_sources` so a last-moment re-reference wins.
pub async fn delete_retired_nodes(pool: &PgPool) -> anyhow::Result<u64> {
    let mut tx = pool.begin().await?;
    let victims: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM nodes WHERE membership='Retired' \
         AND NOT EXISTS (SELECT 1 FROM node_sources WHERE node_id=nodes.id)",
    )
    .fetch_all(&mut *tx)
    .await?;
    let mut deleted = 0u64;
    for id in &victims {
        let n = sqlx::query(
            "DELETE FROM nodes WHERE id=$1 \
             AND NOT EXISTS (SELECT 1 FROM node_sources WHERE node_id=$1)",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        deleted += n;
    }
    tx.commit().await?;
    Ok(deleted)
}

/// Read the `orphan_retention_days` config (seeded default 14 in 0006).
pub async fn orphan_retention_days(pool: &PgPool) -> anyhow::Result<i64> {
    let v: String = sqlx::query_scalar("SELECT value FROM configs WHERE key='orphan_retention_days'")
        .fetch_optional(pool)
        .await?
        .unwrap_or_else(|| "14".to_string());
    Ok(v.parse().unwrap_or(14))
}

/// One cleanup tick (plan §2.1 LIFE/cleanup): retire expired orphans, physically
/// delete retired nodes (guarded), then prune check history past its retention
/// window (A5: `check_run_history_days`, default 14). Returns what happened, for logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanupReport {
    pub retired: u64,
    pub deleted: u64,
    pub pruned: u64,
}

pub async fn run_cleanup_cycle(pool: &PgPool) -> anyhow::Result<CleanupReport> {
    let retention = orphan_retention_days(pool).await?;
    let retired = retire_expired_orphans(pool, retention).await?;
    let deleted = delete_retired_nodes(pool).await?;
    let history_days = check_history_days(pool).await?;
    let pruned = prune_check_history(pool, history_days).await?;
    Ok(CleanupReport {
        retired,
        deleted,
        pruned,
    })
}

/// Read `check_run_history_days` (A5: seeded 14 since migration 0010).
pub async fn check_history_days(pool: &PgPool) -> anyhow::Result<i64> {
    let v: Option<String> =
        sqlx::query_scalar("SELECT value FROM configs WHERE key='check_run_history_days'")
            .fetch_optional(pool)
            .await?;
    Ok(v.and_then(|s| s.trim().parse().ok()).unwrap_or(14))
}

/// Read `cleanup_interval_minutes` (seeded 30); the worker's scheduler uses it.
pub async fn cleanup_interval_minutes(pool: &PgPool) -> anyhow::Result<i64> {
    let v: Option<String> =
        sqlx::query_scalar("SELECT value FROM configs WHERE key='cleanup_interval_minutes'")
            .fetch_optional(pool)
            .await?;
    Ok(v.and_then(|s| s.trim().parse().ok()).unwrap_or(30).max(1))
}

/// Delete check history older than the retention window. `retention_days <= 0` keeps
/// history forever (explicit opt-out) and is the only supported "disable" switch.
pub async fn prune_check_history(pool: &PgPool, retention_days: i64) -> anyhow::Result<u64> {
    if retention_days <= 0 {
        return Ok(0);
    }
    let done = sqlx::query(
        "DELETE FROM node_check_results \
         WHERE checked_at < now() - make_interval(days => $1)",
    )
    .bind(retention_days as i32)
    .execute(pool)
    .await?;
    Ok(done.rows_affected())
}

/// Cross-tag alive materialization (rewrites `batchAliveDecision`):
/// for each node, ANY tag whose latest snapshot is alive/degraded within the window
/// makes the node alive. Returns (node_id -> alive) for all nodes having snapshots.
pub async fn cross_tag_alive_map(
    pool: &PgPool,
    window_minutes: i64,
) -> anyhow::Result<std::collections::HashMap<Uuid, bool>> {
    let rows: Vec<(Uuid, bool)> = sqlx::query_as(
        "SELECT node_id, bool_or(health IN ('alive','degraded')) AS alive \
         FROM node_status \
         WHERE last_checked_at >= now() - make_interval(mins => $1) \
         GROUP BY node_id",
    )
    .bind(window_minutes as i32)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}

/// Read `node_alive_check_window_minutes` (seeded 240 in 0001).
pub async fn alive_window_minutes(pool: &PgPool) -> anyhow::Result<i64> {
    let v: String =
        sqlx::query_scalar("SELECT value FROM configs WHERE key='node_alive_check_window_minutes'")
            .fetch_optional(pool)
            .await?
            .unwrap_or_else(|| "240".to_string());
    Ok(v.parse().unwrap_or(240))
}
