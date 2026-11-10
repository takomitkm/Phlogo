//! Fetch-result reconciliation (plan §5.1/§6.3, mirrors `internal/node/upsert.go`).
//! Replace/Accumulate by `sources.fetch_policy`; dead-source guard; orphan marking;
//! all mapping changes happen inside ONE transaction owned by the caller's pool call.
use database::reconcile as txops;
use domain::{NormalizedNode, SourceContext};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum ReconcileError {
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),
    #[error("source not found: {0}")]
    SourceNotFound(Uuid),
}
#[derive(Debug, Clone, serde::Serialize)]
pub struct FetchRunSummary {
    pub run_id: Uuid,
    pub status: String,
    pub fetched: usize,
    pub upserted: usize,
    pub removed: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn blocked(status: &str) -> bool {
    status == "dead" || status == "suspended"
}

/// Run one fetch result through the reconciliation pipeline.
pub async fn reconcile_fetch_result(
    pool: &PgPool,
    source_id: Uuid,
    success: bool,
    nodes: &[NormalizedNode],
    error: Option<String>,
) -> Result<FetchRunSummary, ReconcileError> {
    let mut tx = pool.begin().await?;

    let run_id = txops::insert_fetch_run(&mut tx, source_id).await?;

    if !success {
        txops::finish_fetch_run(
            &mut tx,
            run_id,
            "failed",
            json!({"fetched": 0}),
            error.clone(),
        )
        .await?;
        tx.commit().await?;
        return Ok(FetchRunSummary {
            run_id,
            status: "failed".into(),
            fetched: 0,
            upserted: 0,
            removed: 0,
            error,
        });
    }

    let status = txops::source_status(&mut tx, source_id)
        .await?
        .ok_or(ReconcileError::SourceNotFound(source_id))?;
    if blocked(&status) {
        txops::finish_fetch_run(&mut tx, run_id, "finished", json!({"noop": true}), None).await?;
        tx.commit().await?;
        return Ok(FetchRunSummary {
            run_id,
            status: "finished".into(),
            fetched: 0,
            upserted: 0,
            removed: 0,
            error: None,
        });
    }

    let policy = txops::fetch_policy_of(&mut tx, source_id).await?;
    let replace = policy != "accumulate";
    let fetched = nodes.len();

    let mut keep: Vec<Uuid> = Vec::with_capacity(nodes.len());
    let mut upserted: Vec<Uuid> = Vec::with_capacity(nodes.len());
    if !nodes.is_empty() {
        for nn in nodes {
            let Some(hash) = nn.identity_hash.as_deref() else {
                continue; // nodes without identity cannot be reconciled
            };
            let node_id = txops::upsert_node_by_hash(&mut tx, hash, nn).await?;
            txops::link_node_source(&mut tx, node_id, source_id).await?;
            txops::revive_if_orphaned(&mut tx, node_id).await?;
            keep.push(node_id);
            upserted.push(node_id);
        }
        let upserted_count = upserted.len();
        let mut removed = 0usize;
        if replace {
            let removed_ids = txops::remove_stale_mappings(&mut tx, source_id, &keep).await?;
            removed = removed_ids.len();
            for node_id in &removed_ids {
                txops::mark_orphaned_if_lonely(&mut tx, *node_id).await?;
            }
        }
        txops::finish_fetch_run(
            &mut tx,
            run_id,
            "finished",
            json!({"fetched": fetched, "upserted": upserted_count, "removed": removed}),
            None,
        )
        .await?;
        tx.commit().await?;
        return Ok(FetchRunSummary {
            run_id,
            status: "finished".into(),
            fetched,
            upserted: upserted_count,
            removed,
            error: None,
        });
    }

    // Empty but successful result.
    let mut removed = 0usize;
    if replace {
        let removed_ids = txops::remove_source_mappings(&mut tx, source_id).await?;
        removed = removed_ids.len();
        for node_id in &removed_ids {
            txops::mark_orphaned_if_lonely(&mut tx, *node_id).await?;
        }
    }
    txops::finish_fetch_run(
        &mut tx,
        run_id,
        "finished",
        json!({"fetched": 0, "upserted": 0, "removed": removed}),
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(FetchRunSummary {
        run_id,
        status: "finished".into(),
        fetched: 0,
        upserted: 0,
        removed,
        error: None,
    })
}

/// Delete a source: mappings first (orphan marking), then the source row itself.
pub async fn delete_source(pool: &PgPool, source_id: Uuid) -> Result<bool, ReconcileError> {
    let mut tx = pool.begin().await?;
    let removed = txops::remove_source_mappings(&mut tx, source_id).await?;
    for node_id in &removed {
        txops::mark_orphaned_if_lonely(&mut tx, *node_id).await?;
    }
    let n = sqlx::query("DELETE FROM sources WHERE id=$1")
        .bind(source_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    Ok(n > 0)
}

/// Build a SourceContext for adapter output (used by the future fetch worker).
pub fn source_ctx(source_id: Uuid, fetch_run_id: Uuid) -> SourceContext {
    SourceContext {
        source_id,
        fetch_run_id,
    }
}
