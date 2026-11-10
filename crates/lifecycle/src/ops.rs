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
