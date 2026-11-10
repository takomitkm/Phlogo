//! Database layer: PostgreSQL pool + SQLx migrations.
//! Migrations live at the repo root `migrations/` (embedded at compile time; no DB needed to build).
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
