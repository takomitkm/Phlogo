//! Minimal-privilege boundary tests over **real TCP** (user ruling 2026-11-18).
//!
//! These pin the authorization boundary only: which identity may reach which
//! Method+path. Every assertion here is answered by the auth layer itself, so the
//! tests need no database and stay fast. Handler-level proof ("AdminJWT reaches the
//! management surface", "the two service actions still work") runs in the disposable
//! jobs-DB integration test instead, because its requests do reach handlers.
use std::net::SocketAddr;
use std::time::Duration;

use app_config::Config;

const SERVICE: &str = "svc-secret-test";
const JWT_SECRET: &str = "jwt-secret-test";
const UUID: &str = "6f96f9e0-8d1d-4c1a-9d3f-1a2b3c4d5e6f";

fn test_config() -> Config {
    let mut c = Config::default();
    c.auth.admin_password = "admin-pw-test".into();
    c.auth.service_secret = SERVICE.into();
    c.auth.jwt_secret = JWT_SECRET.into();
    // Unroutable DSN: no test may touch a real database.
    c.database.dsn = "postgres://nobody:nobody@127.0.0.1:1/none".into();
    c
}

async fn spawn() -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = curse::router::build(test_config());
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

async fn send(addr: SocketAddr, method: &str, path: &str, auth: Option<&str>, body: &str) -> u16 {
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nconnection: close\r\n");
    if let Some(a) = auth {
        req.push_str(&format!("Authorization: Bearer {a}\r\n"));
    }
    if !body.is_empty() {
        req.push_str("content-type: application/json\r\n");
        req.push_str(&format!("content-length: {}\r\n", body.len()));
    }
    req.push_str("\r\n");
    req.push_str(body);
    raw(addr, &req).await.0
}

#[tokio::test]
async fn unauthenticated_management_access_is_401() {
    let addr = spawn().await;
    for (m, p) in [
        ("GET", "/api/v1/stats"),
        ("GET", "/api/v1/sources"),
        ("GET", "/api/v1/config"),
        ("POST", "/api/v1/sources/search"),
        ("POST", "/api/v1/fetch/trigger"),
        ("PUT", "/api/v1/config"),
    ] {
        let s = send(addr, m, p, None, "{}").await;
        assert_eq!(s, 401, "{m} {p} without credentials must be 401, got {s}");
    }
}

#[tokio::test]
async fn service_bearer_cannot_read_the_management_surface() {
    let addr = spawn().await;
    let paths = [
        ("GET", "/api/v1/stats".to_string()),
        ("GET", "/api/v1/checkers".to_string()),
        ("GET", "/api/v1/config".to_string()),
        ("GET", "/api/v1/logs/check".to_string()),
        ("GET", "/api/v1/logs/fetch".to_string()),
        ("GET", "/api/v1/sources".to_string()),
        ("GET", format!("/api/v1/sources/{UUID}")),
        ("GET", format!("/api/v1/sources/{UUID}/nodes")),
        ("GET", "/api/v1/nodes".to_string()),
        ("GET", format!("/api/v1/nodes/{UUID}")),
        ("GET", format!("/api/v1/nodes/{UUID}/checks/deep")),
        ("GET", "/api/v1/format-configs".to_string()),
        ("GET", "/api/v1/subscriptions".to_string()),
        ("GET", "/api/v1/sponsors".to_string()),
        ("POST", "/api/v1/sources/search".to_string()),
        ("POST", "/api/v1/nodes/search".to_string()),
        ("POST", "/api/v1/filter/preview".to_string()),
    ];
    for (m, p) in &paths {
        let s = send(addr, m, p, Some(SERVICE), "{}").await;
        assert_eq!(s, 403, "{m} {p} with ServiceBearer must be 403, got {s}");
    }
}

#[tokio::test]
async fn extra_segments_and_methods_do_not_inherit_permission() {
    let addr = spawn().await;
    for (m, p) in [
        ("POST", format!("/api/v1/sources/{UUID}")),
        ("POST", format!("/api/v1/sources/{UUID}/anything")),
        ("POST", "/api/v1/fetch/trigger/extra".to_string()),
        ("GET", "/api/v1/sources".to_string()),
        ("HEAD", "/api/v1/sources".to_string()),
        ("GET", "/api/v1/fetch/trigger".to_string()),
    ] {
        let s = send(addr, m, &p, Some(SERVICE), "{}").await;
        // 403 = matched the auth matrix but not whitelisted; 404 = no such route
        // (axum routes before middleware, so an unmatched path never reaches it).
        // Both prove the permission was not inherited; only 2xx would be a breach.
        assert!(
            s == 403 || s == 404,
            "{m} {p} must not inherit a whitelist entry, got {s}"
        );
    }
}

#[tokio::test]
async fn bad_credentials_are_401_and_public_routes_stay_public() {
    let addr = spawn().await;
    for a in ["definitely-wrong", ""] {
        let s = send(addr, "GET", "/api/v1/stats", Some(a), "").await;
        assert_eq!(s, 401, "bearer {a:?} must not authenticate");
    }
    for p in ["/health"] {
        // `/metrics` is deliberately not asserted here: it is public but its handler
        // reads pool statistics, so it would block on the unroutable test DSN.
        assert_eq!(send(addr, "GET", p, None, "").await, 200, "{p}");
    }
}

// ---------------------------------------------------------------------------
// Middleware-level check for the AdminJWT side (no DB, no handlers involved).
// ---------------------------------------------------------------------------

fn admin_jwt() -> String {
    #[derive(serde::Serialize)]
    struct Claims {
        sub: String,
        token_type: String,
        exp: usize,
    }
    let claims = Claims {
        sub: "admin".into(),
        token_type: "access".into(),
        exp: 4_000_000_000,
    };
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(JWT_SECRET.as_bytes()),
    )
    .expect("sign jwt")
}

async fn protected_app() -> axum::Router {
    use axum::{middleware, routing::get, Router};
    Router::new()
        .route("/api/v1/stats", get(|| async { "ok" }))
        .layer(middleware::from_fn_with_state(
            curse::auth::AuthState {
                service_secret: SERVICE.into(),
                jwt_secret: JWT_SECRET.into(),
            },
            curse::auth::require_api_auth,
        ))
}

async fn oneshot_status(auth: Option<&str>) -> u16 {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    let mut builder = Request::builder().uri("/api/v1/stats");
    if let Some(a) = auth {
        builder = builder.header("authorization", format!("Bearer {a}"));
    }
    let req = builder.body(Body::empty()).unwrap();
    protected_app()
        .await
        .oneshot(req)
        .await
        .unwrap()
        .status()
        .as_u16()
}

#[tokio::test]
async fn admin_jwt_passes_the_middleware_service_does_not() {
    assert_eq!(oneshot_status(None).await, 401);
    assert_eq!(oneshot_status(Some(SERVICE)).await, 403);
    assert_eq!(oneshot_status(Some(&admin_jwt())).await, 200);
}
