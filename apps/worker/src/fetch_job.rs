//! FetchJob: the real P2 closed loop — pull a source's subscription, parse, fingerprint,
//! reconcile. Previously implemented piecewise (crates/fetch, vproxy-adapter, registry)
//! but never invoked; this job is the missing orchestration.
use apalis::prelude::Data;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FetchJob {
    pub source_id: Uuid,
    /// check-style idempotency key for the source_fetch_runs row.
    pub run_id: Uuid,
}

/// Apalis handler.
pub async fn handle_fetch(job: FetchJob, pool: Data<PgPool>) {
    let pool: &PgPool = &pool;
    run_fetch(pool, job).await;
}

/// Run the fetch loop for one source (also used directly by integration tests).
pub async fn run_fetch(pool: &PgPool, job: FetchJob) {
    let Some((url,)) = sqlx::query_as::<_, (String,)>(
        "SELECT content FROM sources WHERE id=$1",
    )
    .bind(job.source_id)
    .fetch_optional(pool)
    .await
    .unwrap_or(None) else {
        tracing::warn!(source_id = %job.source_id, "fetch job for missing source");
        return;
    };

    // 1) fetch (SSRF guard + size/timeout limits, P2-01)
    match fetch::fetch(&url, &fetch::FetchPolicy::default()).await {
        Ok(bytes) => {
            // 2) parse (P2-02) — SourceContext carries source + run for tracking
            let ctx = registry::source_ctx(job.source_id, job.run_id);
            match vproxy_adapter::parse_subscription(&bytes, &ctx) {
                Ok(mut nodes) => {
                    // 3) fingerprint (P2-03)
                    for n in &mut nodes {
                        domain::stamp(n);
                    }
                    // 4) reconcile (P2-04): upsert nodes, replace/accumulate mappings
                    match registry::reconcile_fetch_result(pool, job.source_id, true, &nodes, None)
                        .await
                    {
                        Ok(sum) => tracing::info!(
                            source_id = %job.source_id,
                            fetched = sum.fetched,
                            upserted = sum.upserted,
                            removed = sum.removed,
                            "fetch loop complete"
                        ),
                        Err(e) => tracing::error!(source_id = %job.source_id, error = %e, "reconcile failed"),
                    }
                }
                Err(e) => {
                    tracing::warn!(source_id = %job.source_id, error = %e, "parse failed; recording failed run");
                    let _ = registry::reconcile_fetch_result(
                        pool,
                        job.source_id,
                        false,
                        &[],
                        Some(format!("parse: {e}")),
                    )
                    .await;
                }
            }
        }
        Err(e) => {
            // Failed fetch keeps old mappings and records the run (P2-04 invariant).
            tracing::warn!(source_id = %job.source_id, error = %e, "fetch failed");
            let _ = registry::reconcile_fetch_result(
                pool,
                job.source_id,
                false,
                &[],
                Some(format!("fetch: {e}")),
            )
            .await;
        }
    }
}

/// Enqueue FetchJobs for every active source into the Apalis jobs storage.
/// Used by the periodic scheduler (worker) and the API trigger endpoint.
pub async fn enqueue_for_active_sources(
    storage: &mut apalis_sql::postgres::PostgresStorage<FetchJob>,
    pool: &PgPool,
) -> anyhow::Result<usize> {
    use apalis::prelude::Storage as _;
    let ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM sources WHERE status='active' ORDER BY created_at")
            .fetch_all(pool)
            .await?;
    let mut n = 0;
    for id in ids {
        storage
            .push(FetchJob {
                source_id: id,
                run_id: Uuid::new_v4(),
            })
            .await?;
        n += 1;
    }
    Ok(n)
}

/// API-facing trigger: connect to the jobs DSN and enqueue for all active sources.
pub async fn trigger_fetch_batch(jobs_dsn: &str, pool: &PgPool) -> anyhow::Result<usize> {
    let jobs_pool = database::connect_lazy(jobs_dsn)?;
    let mut storage: apalis_sql::postgres::PostgresStorage<FetchJob> =
        apalis_sql::postgres::PostgresStorage::new(jobs_pool);
    enqueue_for_active_sources(&mut storage, pool).await
}
