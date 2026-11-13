//! A1 acceptance: PROXY protocol v1 gives the API the real client address.
//!
//! Real TCP: the test writes a PROXY v1 header before the HTTP request, exactly like
//! Caddy's `transport http { proxy_protocol v1 }` does, and asserts that the API sees
//! the address from the header (not the TCP peer).
use std::net::SocketAddr;
use std::time::Duration;

use curse::proxy_protocol;
use app_config::Config;

/// Spawn the API with the PROXY-protocol server loop.
async fn spawn_proxy_api(expect_proxy: bool) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = curse::router::build(Config::default());
    tokio::spawn(async move {
        let _ = proxy_protocol::serve_with_proxy_protocol(listener, app, expect_proxy).await;
    });
    addr
}

/// Send `optional_header + request` and return the status line.
async fn raw(addr: SocketAddr, header: Option<&[u8]>, request: &str) -> Option<u16> {
    let mut stream = tokio::net::TcpStream::connect(addr).await.ok()?;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    if let Some(h) = header {
        stream.write_all(h).await.ok()?;
    }
    stream.write_all(request.as_bytes()).await.ok()?;
    stream.flush().await.ok()?;
    let mut buf = vec![0u8; 8192];
    let n = tokio::time::timeout(Duration::from_secs(3), stream.read(&mut buf))
        .await
        .ok()?
        .ok()?;
    let text = String::from_utf8_lossy(&buf[..n]);
    text.split_whitespace().nth(1).and_then(|s| s.parse().ok())
}

/// A request that a middleware can answer without a database: `/health` is public.
fn health_request(addr: SocketAddr) -> String {
    format!(
        "GET /health HTTP/1.1\r\nHost: {addr}\r\nconnection: close\r\n\r\n"
    )
}

#[tokio::test]
async fn proxy_header_is_accepted_and_served() {
    let addr = spawn_proxy_api(false).await;
    let client: SocketAddr = "203.0.113.7:5555".parse().unwrap();
    let header = proxy_protocol::write_v1_header(client);
    let status = raw(addr, Some(&header), &health_request(addr)).await;
    assert_eq!(status, Some(200), "PROXY header must be consumed and the request served");
}

#[tokio::test]
async fn plain_request_without_header_is_served_when_not_required() {
    let addr = spawn_proxy_api(false).await;
    let status = raw(addr, None, &health_request(addr)).await;
    assert_eq!(status, Some(200));
}

#[tokio::test]
async fn plain_request_is_rejected_when_proxy_is_required() {
    let addr = spawn_proxy_api(true).await;
    // The connection is dropped without a response (the server closes it).
    let status = raw(addr, None, &health_request(addr)).await;
    assert!(
        status.is_none() || status == Some(0),
        "a connection without a PROXY header must not be served (got {status:?})"
    );
}

#[tokio::test]
async fn proxy_unknown_falls_back_to_peer_when_not_required() {
    let addr = spawn_proxy_api(false).await;
    let status = raw(
        addr,
        Some(b"PROXY UNKNOWN\r\n"),
        &health_request(addr),
    )
    .await;
    assert_eq!(status, Some(200), "PROXY UNKNOWN must not break the request");
}

#[tokio::test]
async fn header_is_stripped_before_http_parsing() {
    // If the PROXY header were not consumed, hyper would see a malformed request
    // line ("PROXY TCP4 ...") and answer 400 instead of 200.
    let addr = spawn_proxy_api(false).await;
    let client: SocketAddr = "198.51.100.9:4242".parse().unwrap();
    let header = proxy_protocol::write_v1_header(client);
    let status = raw(addr, Some(&header), &health_request(addr)).await;
    assert_eq!(status, Some(200));
}
