//! paily worker binary: Apalis PG storage + Fetch/Check workers + periodic jobs.
//! Apalis storage lives in its own `paily_jobs` database (official apalis migrations);
//! business tables (incl. `check_runs`) stay in the main database.
use std::time::Duration;

use apalis::prelude::*;
use apalis_sql::postgres::PostgresStorage;
use sqlx::PgPool;

use worker::{handle_check, handle_fetch, CheckJob, FetchJob};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    observability::init("paily-worker");
    let cfg = app_config::Config::load(None)?;

    let pool: PgPool = database::connect(&cfg.database.dsn).await?;
    database::migrate(&pool).await?;

    let jobs_dsn = worker::jobs_dsn(&cfg);
    let jobs_pool: PgPool = database::connect(&jobs_dsn).await?;
    let check_storage: PostgresStorage<CheckJob> = PostgresStorage::new(jobs_pool.clone());
    let fetch_storage: PostgresStorage<FetchJob> = PostgresStorage::new(jobs_pool.clone());
    PostgresStorage::setup(&jobs_pool)
        .await
        .expect("unable to run apalis postgres migrations (create the jobs database first)");

    // Periodic scheduling: batch-enqueue fetches for active sources, then run one
    // lifecycle cleanup tick — both on fixed intervals (plan §2.1 worker loop).
    // The cleanup interval is config-driven (`cleanup_interval_minutes`, default 30).
    let cleanup_secs = lifecycle::cleanup_interval_minutes(&pool)
        .await
        .unwrap_or(30)
        .max(1) as u64
        * 60;
    let sched_pool = pool.clone();
    let mut sched_fetch = fetch_storage.clone();
    tokio::spawn(async move {
        let mut fetch_tick = tokio::time::interval(Duration::from_secs(300));
        let mut cleanup_tick = tokio::time::interval(Duration::from_secs(cleanup_secs));
        loop {
            tokio::select! {
                _ = fetch_tick.tick() => {
                    match worker::fetch_job::enqueue_for_active_sources(&mut sched_fetch, &sched_pool).await {
                        Ok(n) if n > 0 => tracing::info!(enqueued = n, "fetch batch scheduled"),
                        Ok(_) => {}
                        Err(e) => tracing::error!(error = %e, "fetch scheduling failed"),
                    }
                }                _ = cleanup_tick.tick() => {
                    match lifecycle::run_cleanup_cycle(&sched_pool).await {
                        Ok(r) if r.retired > 0 || r.deleted > 0 || r.pruned > 0 => {
                            tracing::info!(
                                retired = r.retired,
                                deleted = r.deleted,
                                pruned = r.pruned,
                                "cleanup tick acted"
                            )
                        }
                        Ok(_) => {}
                        Err(e) => tracing::error!(error = %e, "cleanup tick failed"),
                    }
                }
            }
        }
    });

    tracing::info!(jobs = %jobs_dsn, "worker started (fetch + check + scheduler)");

    Monitor::new()
        .register({
            let p = pool.clone();
            WorkerBuilder::new("paily-fetch-worker")
                .enable_tracing()
                .data(p)
                .backend(fetch_storage)
                .build_fn(handle_fetch)
        })
        .register({
            let p = pool.clone();
            WorkerBuilder::new("paily-check-worker")
                .enable_tracing()
                .data(p)
                .backend(check_storage)
                .build_fn(handle_check)
        })
        .run()
        .await?;
    Ok(())
}
