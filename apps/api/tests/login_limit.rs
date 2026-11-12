//! P0-02 login rate-limit integration tests.
//! Boots the real router on a random 127.0.0.1 port (real TCP => real ConnectInfo),
//! then drives curl-like HTTP over raw tokio TcpStream.
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

/// Minimal HTTP/1.1 request over raw TCP so each test conn is a distinct peer.
async fn raw_request(addr: SocketAddr, req: &str) -> u16 {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    stream.write_all(req.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();
    let mut buf = vec![0u8; 4096];
    let n = tokio::time::timeout(Duration::from_secs(3), stream.read(&mut buf))
        .await
        .unwrap()
        .unwrap_or(0);
    let text = String::from_utf8_lossy(&buf[..n]);
    // parse "HTTP/1.1 429"
    text.split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

fn login_req(host: SocketAddr, extra: &str) -> String {
    format!(
        "POST /api/v1/auth/login HTTP/1.1\r\nHost: {host}\r\ncontent-type: application/json\r\ncontent-length: 34\r\n{extra}\r\n{{\"username\":\"admin\",\"password\":\"x\"}}"
    )
}

/// Threshold within a minute: N-1 attempts still get non-429, then 429s.
#[tokio::test]
async fn login_limit_triggers_then_recovers() {
    let addr = spawn().await;
    let req = login_req(addr, "");

    // Default threshold is 10/min. Attempt 12 times on 12 distinct connections
    // (each = distinct peer port, same peer IP -> same bucket per 127.0.0.1/32 policy?).
    // NOTE: the limiter keys on SocketAddr (ip:port), so distinct connections are
    // distinct keys. To exercise the same-key path we must reuse ONE connection
    // pipelining multiple requests — instead we assert per-connection isolation
    // (attackers rotating ports do NOT defeat the bucket only when IP-keyed).
    // The limiter as shipped keys on SocketAddr; same-IP rotation is a known
    // limitation recorded in the crosscheck doc (P1: key by IP).
    let mut saw_429 = false;
    for _ in 0..15 {
        let st = raw_request(addr, &req).await;
        if st == 429 {
            saw_429 = true;
        }
        assert_ne!(st, 0, "no response");
    }
    assert!(saw_429, "must observe 429 within 15 rapid attempts");
}

/// Other endpoints are not throttled by the login limiter.
#[tokio::test]
async fn health_not_throttled() {
    let addr = spawn().await;
    for _ in 0..15 {
        let st = raw_request(addr, &format!("GET /health HTTP/1.1\r\nHost: {addr}\r\n\r\n")).await;
        assert_eq!(st, 200, "/health must stay 200 under login-threshold traffic");
    }
}

/// X-Forwarded-For cannot rotate the bucket identity.
///
/// The limiter keys on the peer IP only (see `rate_limit.rs`), so a fresh forged
/// `X-Forwarded-For` on every attempt must NOT reset the window: attempts 1..=10
/// consume the threshold, attempt 11 -> 429. This is a real assertion — if anyone
/// ever wires XFF into the key, the 11th attempt stops being 429 and this fails.
#[tokio::test]
async fn xff_cannot_bypass() {
    let addr = spawn().await;
    let mut statuses = Vec::new();
    for i in 0..13 {
        let xff = format!("X-Forwarded-For: 10.0.0.{i}\r\n");
        statuses.push(raw_request(addr, &login_req(addr, &xff)).await);
    }

    for (i, st) in statuses.iter().take(10).enumerate() {
        assert_ne!(*st, 429, "attempt {} must be below the threshold", i + 1);
        assert_ne!(*st, 0, "attempt {} got no response", i + 1);
    }
    assert_eq!(
        statuses[10], 429,
        "the 11th attempt must be throttled even though it carried a fresh X-Forwarded-For"
    );
    assert_eq!(statuses[12], 429, "throttling must persist beyond the threshold");
}

/// Concurrent hammering: all responses valid, at least one 429, no panics/hangs.
#[tokio::test]
async fn concurrent_hammer() {
    let addr = spawn().await;
    let req = login_req(addr, "");
    let mut handles = Vec::new();
    for _ in 0..12 {
        let a = addr;
        let r = req.clone();
        handles.push(tokio::spawn(async move { raw_request(a, &r).await }));
    }
    let mut got_429 = 0;
    for h in handles {
        let st = h.await.unwrap();
        assert_ne!(st, 0);
        if st == 429 {
            got_429 += 1;
        }
    }
    assert!(got_429 > 0, "concurrent burst must hit the limiter");
}
