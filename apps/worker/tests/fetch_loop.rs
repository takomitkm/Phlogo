//! End-to-end fetch loop test (needs DATABASE_URL; skipped otherwise):
//! spawn a local HTTP server serving a clash YAML, create a source pointing at it,
//! run `run_fetch`, and assert nodes/reconciliation landed.
use std::net::SocketAddr;

use axum::{routing::get, Router};
use database::SourceRepository;
use uuid::Uuid;
use worker::{run_fetch, FetchJob};

const CLASH_YAML: &str = r#"proxies:
  - name: "e2e-vmess"
    type: vmess
    server: e2e.example.com
    port: 443
    uuid: 99999999-9999-9999-9999-999999999999
    alterId: 0
    cipher: auto
  - name: "e2e-trojan"
    type: trojan
    server: e2e2.example.com
    port: 443
    password: "e2e-pass"
"#;

async fn spawn_subscription_server() -> String {
    let app = Router::new().route("/sub", get(|| async { CLASH_YAML }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}/sub")
}

#[tokio::test]
async fn fetch_loop_end_to_end() {
    let Ok(dsn) = std::env::var("DATABASE_URL") else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    // Allowlist loopback for the SSRF guard (the subscription server is local).
    std::env::set_var("PHLOGO_FETCH_ALLOW_LOOPBACK", "1");
    let pool = database::connect(&dsn).await.expect("connect");
    database::migrate(&pool).await.expect("migrate");

    let url = spawn_subscription_server().await;
    let repo = SourceRepository { pool: pool.clone() };
    let (source_id, _) = repo
        .create_dedup("subscribe", "", "", &url)
        .await
        .expect("create source");

    run_fetch(&pool, FetchJob { source_id, run_id: Uuid::new_v4() }).await;

    // Reconciliation landed the two parsed nodes with fingerprints.
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM node_sources WHERE source_id=$1",
    )
    .bind(source_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 2, "both parsed nodes must be linked to the source");

    let hashed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM nodes n JOIN node_sources ns ON ns.node_id=n.id \
         WHERE ns.source_id=$1 AND n.identity_hash IS NOT NULL AND length(n.identity_hash)=32",
    )
    .bind(source_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(hashed, 2, "nodes must carry P2-03 fingerprints");

    // A fetch-run row was recorded (finished).
    let run: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM source_fetch_runs WHERE source_id=$1 AND status='finished'",
    )
    .bind(source_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(run >= 1, "fetch run must be recorded");

    // Second run is idempotent: same two mappings, no duplicates.
    run_fetch(&pool, FetchJob { source_id, run_id: Uuid::new_v4() }).await;
    let count2: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM node_sources WHERE source_id=$1",
    )
    .bind(source_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count2, 2, "re-fetch must not duplicate mappings");
}
