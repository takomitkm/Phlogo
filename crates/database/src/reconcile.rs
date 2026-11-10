//! Transaction-scoped reconciliation primitives (P2-04). All functions take an
//! open connection/transaction so the caller controls atomicity.
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

/// Insert a fetch run with status='started'; returns the run id.
pub async fn insert_fetch_run(conn: &mut PgConnection, source_id: Uuid) -> sqlx::Result<Uuid> {
    sqlx::query_scalar(
        "INSERT INTO source_fetch_runs (id,source_id,status,started_at) VALUES ($1,$2,'started',now()) RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(source_id)
    .fetch_one(conn)
    .await
}

/// Finish a run: status finished/failed + counts + optional error.
pub async fn finish_fetch_run(
    conn: &mut PgConnection,
    run_id: Uuid,
    status: &str,
    counts: Value,
    error: Option<String>,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE source_fetch_runs SET status=$2, finished_at=now(), counts=$3, error=$4 WHERE id=$1")
        .bind(run_id)
        .bind(status)
        .bind(counts)
        .bind(error)
        .execute(conn)
        .await?;
    Ok(())
}

pub async fn source_status(
    conn: &mut PgConnection,
    source_id: Uuid,
) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar("SELECT status FROM sources WHERE id=$1")
        .bind(source_id)
        .fetch_optional(conn)
        .await
}

pub async fn fetch_policy_of(conn: &mut PgConnection, source_id: Uuid) -> sqlx::Result<String> {
    sqlx::query_scalar("SELECT fetch_policy FROM sources WHERE id=$1")
        .bind(source_id)
        .fetch_one(conn)
        .await
}

/// Upsert a node by identity_hash; returns the node id.
pub async fn upsert_node_by_hash(
    conn: &mut PgConnection,
    identity_hash: &str,
    nn: &domain::NormalizedNode,
) -> sqlx::Result<Uuid> {
    let raw = nn.raw_config_reference.clone();
    sqlx::query_scalar(
        r#"INSERT INTO nodes (id,identity_hash,fingerprint_version,protocol,server,port,first_seen_at,last_seen_at,created_at,updated_at,membership,raw_config_reference)
           VALUES ($1,$2,$3,$4,$5,$6,now(),now(),now(),now(),'Linked',$7)
           ON CONFLICT (identity_hash) DO UPDATE SET
             last_seen_at = now(),
             protocol = EXCLUDED.protocol,
             server = EXCLUDED.server,
             port = EXCLUDED.port,
             fingerprint_version = EXCLUDED.fingerprint_version,
             raw_config_reference = EXCLUDED.raw_config_reference
           RETURNING id"#,
    )
    .bind(Uuid::new_v4())
    .bind(identity_hash)
    .bind(i32::from(domain::CURRENT_FINGERPRINT_VERSION))
    .bind(&nn.protocol)
    .bind(&nn.server)
    .bind(i32::from(nn.port))
    .bind(raw)
    .fetch_one(conn)
    .await
}

/// Idempotent node<->source link; refreshes last_seen_at.
pub async fn link_node_source(
    conn: &mut PgConnection,
    node_id: Uuid,
    source_id: Uuid,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO node_sources (node_id,source_id,first_seen_at,last_seen_at) VALUES ($1,$2,now(),now()) ON CONFLICT (node_id,source_id) DO UPDATE SET last_seen_at=now()",
    )
    .bind(node_id)
    .bind(source_id)
    .execute(conn)
    .await?;
    Ok(())
}

/// A node that came back must leave the Orphaned state.
pub async fn revive_if_orphaned(conn: &mut PgConnection, node_id: Uuid) -> sqlx::Result<()> {
    sqlx::query("UPDATE nodes SET membership='Linked', orphaned_at=NULL WHERE id=$1 AND membership='Orphaned'")
        .bind(node_id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Delete this source's mappings except `keep`; returns the removed node ids.
pub async fn remove_stale_mappings(
    conn: &mut PgConnection,
    source_id: Uuid,
    keep: &[Uuid],
) -> sqlx::Result<Vec<Uuid>> {
    sqlx::query_scalar(
        "DELETE FROM node_sources WHERE source_id=$1 AND NOT (node_id = ANY($2)) RETURNING node_id",
    )
    .bind(source_id)
    .bind(keep.to_vec())
    .fetch_all(conn)
    .await
}

/// Delete ALL mappings of a source; returns the affected node ids.
pub async fn remove_source_mappings(
    conn: &mut PgConnection,
    source_id: Uuid,
) -> sqlx::Result<Vec<Uuid>> {
    sqlx::query_scalar("DELETE FROM node_sources WHERE source_id=$1 RETURNING node_id")
        .bind(source_id)
        .fetch_all(conn)
        .await
}

/// Mark `Orphaned` iff the node has no remaining source mapping (atomic NOT EXISTS).
pub async fn mark_orphaned_if_lonely(conn: &mut PgConnection, node_id: Uuid) -> sqlx::Result<bool> {
    let n = sqlx::query(
        "UPDATE nodes SET membership='Orphaned', orphaned_at=now() WHERE id=$1 AND membership <> 'Orphaned' AND NOT EXISTS (SELECT 1 FROM node_sources WHERE node_id=$1)",
    )
    .bind(node_id)
    .execute(conn)
    .await?
    .rows_affected();
    Ok(n > 0)
}
