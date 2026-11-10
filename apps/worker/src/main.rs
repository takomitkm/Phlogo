//! paily worker binary: Apalis PG storage + CheckJob worker (P3-01).
//! Apalis storage lives in its own `paily_jobs` database (official apalis migrations);
//! business tables (incl. `check_runs`) stay in the main database.
use apalis::prelude::*;
use apalis_sql::postgres::PostgresStorage;
use sqlx::PgPool;

use worker::{handle_check, CheckJob};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    observability::init("paily-worker");
    let cfg = app_config::Config::load(None)?;

    let pool: PgPool = database::connect(&cfg.database.dsn).await?;
    database::migrate(&pool).await?;

    let jobs_dsn = worker::jobs_dsn(&cfg);
    let jobs_pool: PgPool = database::connect(&jobs_dsn).await?;
    let storage: PostgresStorage<CheckJob> = PostgresStorage::new(jobs_pool.clone());
    PostgresStorage::setup(&jobs_pool)
        .await
        .expect("unable to run apalis postgres migrations (create the jobs database first)");

    let mut producer = storage.clone();
    let demo = CheckJob {
        run_id: uuid::Uuid::new_v4(),
        checker_tag: "default".into(),
        node_ids: vec![],
        presets: vec!["idle".into()],
    };
    producer.push(demo).await?;
    tracing::info!(jobs = %jobs_dsn, "worker started; demo CheckJob enqueued");

    Monitor::new()
        .register({
            WorkerBuilder::new("paily-check-worker")
                .enable_tracing()
                .data(pool)
                .backend(storage)
                .build_fn(handle_check)
        })
        .run()
        .await?;
    Ok(())
}
