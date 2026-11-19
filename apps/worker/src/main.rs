//! phlogo worker binary: Apalis PG storage + Fetch/Check workers + periodic jobs.
//! Apalis storage lives in its own `phlogo_jobs` database (official apalis migrations);
//! business tables (incl. `check_runs`) stay in the main database.
use std::time::Duration;

use apalis::prelude::*;
use apalis_sql::postgres::PostgresStorage;
use sqlx::PgPool;

use worker::{handle_check, handle_fetch, CheckJob, FetchJob};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    observability::init("phlogo-gnosis-worker");
    let cfg = app_config::Config::load(None)?;

    // M1: node payloads are sealed at rest. A configured key must be valid — an
    // invalid `PHLOGO_PAYLOAD_KEY` is a hard error, never a silent ephemeral fallback.
    match payload_crypto::init_from_env() {
        Ok(true) => tracing::info!("payload key loaded from PHLOGO_PAYLOAD_KEY"),
        Ok(false) => tracing::warn!(
            "PHLOGO_PAYLOAD_KEY not set: using an ephemeral key; stored payloads will \
             not be readable after a restart"
        ),
        Err(e) => return Err(anyhow::anyhow!("PHLOGO_PAYLOAD_KEY invalid: {e}")),
    }

    let pool: PgPool = database::connect(&cfg.database.dsn).await?;
    database::migrate(&pool).await?;

    let jobs_dsn = worker::jobs_dsn(&cfg);
    let jobs_pool: PgPool = database::connect(&jobs_dsn).await?;
    let check_storage: PostgresStorage<CheckJob> = PostgresStorage::new(jobs_pool.clone());
    let fetch_storage: PostgresStorage<FetchJob> = PostgresStorage::new(jobs_pool.clone());
    PostgresStorage::setup(&jobs_pool)
        .await
        .expect("unable to run apalis postgres migrations (create the jobs database first)");

    // Periodic scheduling: batch-enqueue fetches for active sources, run one lifecycle
    // cleanup tick and one check-scheduling pass — all on fixed intervals (plan §2.1).
    // First-tick behavior is EXPLICIT: `interval_at(now + period)` (M3 ruling) so a
    // worker restart does not immediately re-run every scheduler pass; the next tick
    // happens one full period after startup.
    let cleanup_secs = lifecycle::cleanup_interval_minutes(&pool)
        .await
        .unwrap_or(30)
        .max(1) as u64
        * 60;
    let check_cfg =
        worker::load_check_schedule_config(&pool)
            .await
            .unwrap_or(worker::CheckScheduleConfig {
                interval_minutes: 30,
                batch_size: 50,
            });
    let check_secs = check_cfg.interval_minutes.max(1) as u64 * 60;
    let sched_pool = pool.clone();
    let mut sched_fetch = fetch_storage.clone();
    let mut sched_check = check_storage.clone();
    tokio::spawn(async move {
        let now = tokio::time::Instant::now();
        // All three start one full period AFTER startup (no immediate-fire first tick).
        // NOTE on the period label: it is minute-granular, so two ticks within the same
        // minute would share a label and the second would be skipped as duplicate —
        // that is the desired behavior for sub-minute restarts, and the interval is
        // always >= 60s by config.
        let mut fetch_tick =
            tokio::time::interval_at(now + Duration::from_secs(300), Duration::from_secs(300));
        let mut cleanup_tick = tokio::time::interval_at(
            now + Duration::from_secs(cleanup_secs),
            Duration::from_secs(cleanup_secs),
        );
        let mut check_tick = tokio::time::interval_at(
            now + Duration::from_secs(check_secs),
            Duration::from_secs(check_secs),
        );
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
                _ = check_tick.tick() => {
                    // Period label = UTC date + slot index so the label changes every
                    // period even across worker restarts within the same minute.
                    let period = format!(
                        "period-{}",
                        chrono::Utc::now().format("%Y%m%dT%H%M")
                    );
                    match worker::enqueue_scheduled(&mut sched_check, &sched_pool, &period, &check_cfg).await {
                        Ok((pushed, skipped, nodes)) if pushed > 0 => {
                            tracing::info!(pushed, skipped, nodes, %period, "check batch scheduled")
                        }
                        Ok(_) => {}
                        Err(e) => tracing::error!(error = %e, "check scheduling failed"),
                    }
                }
            }
        }
    });

    tracing::info!(jobs = %jobs_dsn, "worker started (fetch + check + scheduler)");

    Monitor::new()
        .register({
            let p = pool.clone();
            WorkerBuilder::new("phlogo-fetch-worker")
                .enable_tracing()
                .data(p)
                .backend(fetch_storage)
                .build_fn(handle_fetch)
        })
        .register({
            let p = pool.clone();
            WorkerBuilder::new("phlogo-check-worker")
                .enable_tracing()
                .data(p)
                .backend(check_storage)
                .build_fn(handle_check)
        })
        .run()
        .await?;
    Ok(())
}
