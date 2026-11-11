use api::{router, telemetry};
use app_config::Config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = Config::load(None).map_err(|e| anyhow::anyhow!("config load failed: {e}"))?;
    telemetry::init(if cfg.server.verbose { "debug" } else { "info" });

    let addr = format!("0.0.0.0:{}", cfg.server.port);
    tracing::info!(%addr, "paily-core-rust api starting");
    let app = router::build(cfg);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    // ConnectInfo gives the rate limiter the real peer address (XFF is not trusted).
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}
