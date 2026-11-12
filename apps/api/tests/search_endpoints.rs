//! B3 gap #1/#3 acceptance: AST search + filter preview + format templates.
//! Exercises the real router over real TCP so the auth matrix and JSON body handling
//! are covered. The happy paths need a database; the unauthenticated boundary does not.
use std::net::SocketAddr;
use std::time::Duration;

use app_config::Config;

async fn spawn() -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = api::router::build(Config::default());
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    });
    addr
}

async fn raw(addr: SocketAddr, req: &str) -> (u16, String) {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    stream.write_all(req.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();
    let mut buf = vec![0u8; 65536];
    let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
        .await
        .unwrap()
        .unwrap_or(0);
    let text = String::from_utf8_lossy(&buf[..n]).to_string();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (status, body)
}

async fn post(addr: SocketAddr, path: &str, body: &str) -> (u16, String) {
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\ncontent-type: application/json\r\n\
         content-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    raw(addr, &req).await
}

async fn get(addr: SocketAddr, path: &str) -> (u16, String) {
    let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nconnection: close\r\n\r\n");
    raw(addr, &req).await
}

#[tokio::test]
async fn search_and_preview_require_auth() {
    let addr = spawn().await;
    for path in [
        "/api/v1/nodes/search",
        "/api/v1/sources/search",
        "/api/v1/filter/preview",
    ] {
        let (status, _) = post(addr, path, "{}").await;
        assert_eq!(status, 401, "{path} must require credentials");
    }
}

#[tokio::test]
async fn format_configs_require_auth() {
    let addr = spawn().await;
    let (status, _) = get(addr, "/api/v1/format-configs").await;
    assert_eq!(status, 401);
    let (status, _) = get(addr, "/api/v1/format-configs/clash").await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn unknown_format_is_rejected_after_auth_only() {
    // Unauthenticated -> 401 (the matrix answers first); the 404/400 for an unknown
    // format is covered by the handler unit tests with a database.
    let addr = spawn().await;
    let (status, _) = get(addr, "/api/v1/format-configs/nope").await;
    assert_eq!(status, 401);
}
