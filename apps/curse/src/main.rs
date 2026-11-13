use curse::{proxy_protocol, router, telemetry};
use app_config::Config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = Config::load(None).map_err(|e| anyhow::anyhow!("config load failed: {e}"))?;
    telemetry::init(if cfg.server.verbose { "debug" } else { "info" });

    let addr = format!("0.0.0.0:{}", cfg.server.port);
    tracing::info!(%addr, "Phlogo Wish Gnosis api starting");
    let app = router::build(cfg.clone());
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    if cfg.server.proxy_protocol {
        // A1: the reverse proxy prepends a PROXY v1 header carrying the real client
        // address. `expect_proxy` additionally requires it, so the API port must not
        // be reachable directly (otherwise a client could skip the proxy entirely).
        tracing::info!(
            expect_proxy = cfg.server.expect_proxy,
            "serving with PROXY protocol v1"
        );
        proxy_protocol::serve_with_proxy_protocol(listener, app, cfg.server.expect_proxy).await?;
    } else {
        // ConnectInfo gives the rate limiter the real peer address (XFF is not trusted).
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await?;
    }
    Ok(())
}
