//! Example: apply migrations to DATABASE_URL and report the seeded `configs` row count.
//! Usage: DATABASE_URL=postgres://... cargo run -p database --example migrate
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let dsn = std::env::var("DATABASE_URL")
        .map_err(|_| anyhow::anyhow!("DATABASE_URL not set"))?;
    let pool = database::connect(&dsn).await?;
    database::migrate(&pool).await?;
    let n = database::count_configs(&pool).await?;
    println!("MIGRATE_OK configs_count={}", n);
    pool.close().await;
    Ok(())
}
