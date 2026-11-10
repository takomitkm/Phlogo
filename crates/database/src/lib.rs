//! Database layer: PostgreSQL pool + SQLx migrations + repositories.
//! Migrations live at the repo root `migrations/` (embedded at compile time; no DB needed to build).
pub mod models;
pub mod repositories;

pub use models::{NodeRow, SourceRow};
pub use repositories::{NodeRepository, SourceRepository};

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

/// Create a PostgreSQL pool (max_open_conns mirrors config default 6).
pub async fn connect(dsn: &str) -> anyhow::Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(6)
        .idle_timeout(std::time::Duration::from_secs(120))
        .connect(dsn)
        .await?;
    Ok(pool)
}

/// Create a lazy pool (no connection until first query). Useful for API startup/tests without DB.
pub fn connect_lazy(dsn: &str) -> anyhow::Result<PgPool> {
    Ok(PgPoolOptions::new().max_connections(6).connect_lazy(dsn)?)
}

/// Apply the embedded migrations (`migrations/` at repo root).
pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::migrate!("../../migrations").run(pool).await?;
    Ok(())
}

/// Number of rows seeded into `configs` (used by the migrate example / integration checks).
pub async fn count_configs(pool: &PgPool) -> anyhow::Result<i64> {
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM configs")
        .fetch_one(pool)
        .await?;
    Ok(n)
}
