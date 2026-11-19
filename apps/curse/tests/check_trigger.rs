//! M3 stage 4 acceptance: POST /api/v1/check/trigger permission matrix (real HTTP).
//!
//! Requires DATABASE_URL + PHLOGO_TEST_JOBS_DSN (same base DSN); skipped otherwise.
//! Enqueues into a DISPOSABLE jobs DB — never the production queue — and inserts a
//! fixture node that is removed at the end.
use std::time::Duration;

use sqlx::PgPool;
use uuid::Uuid;

async fn setup() -> Option<PgPool> {
    let dsn = std::env::var("DATABASE_URL").ok()?;
    let pool = database::connect(&dsn).await.expect("connect");
    database::migrate(&pool).await.expect("migrate");
    Some(pool)
}

fn admin_jwt() -> String {
    // The login endpoint needs ADMIN_PASSWORD; mint the token directly instead.
    use jsonwebtoken::{encode, EncodingKey, Header};
    #[derive(serde::Serialize)]
    struct Claims {
        token_type: &'static str,
        exp: usize,
    }
    let secret = std::env::var("PHLOGO_AUTH_JWT_SECRET").unwrap_or_default();
    encode(
        &Header::default(),
        &Claims {
            token_type: "access",
            exp: (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                + 600) as usize,
        },
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("jwt")
}

/// POST via a real TCP request; returns (status, body).
async fn post(
    addr: std::net::SocketAddr,
    path: &str,
    bearer: Option<&str>,
    body: &str,
) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let auth = bearer
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\n{auth}content-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();
    let mut buf = vec![0u8; 65536];
    let n = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buf))
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

/// AdminJWT -> 202 + stats; ServiceBearer -> 403; anonymous -> 401 (ruling: the check
/// trigger must NOT extend the service whitelist).
#[tokio::test]
async fn check_trigger_permission_matrix() {
    let Some(pool) = setup().await else {
        return;
    };
    std::env::set_var("PHLOGO_AUTH_JWT_SECRET", "test-jwt-secret-m3");

    // Fixture node with a payload so explicit selection has something to schedule.
    let node_id = Uuid::new_v4();
    let hash_part = Uuid::new_v4().simple().to_string();
    sqlx::query(
        "INSERT INTO nodes (id,identity_hash,fingerprint_version,protocol,server,port,\
         first_seen_at,last_seen_at,created_at,updated_at,membership,raw_config_reference) \
         VALUES ($1,$2,1,'trojan','m3trigger.example',443,now(),now(),now(),now(),'Linked','')",
    )
    .bind(node_id)
    .bind(format!("{hash_part}{hash_part}"))
    .execute(&pool)
    .await
    .expect("insert node");
    let sealed = payload_crypto::global()
        .seal(
            &payload_crypto::aad_for(&node_id.to_string()),
            br#"{"type":"trojan","server":"m3trigger.example","port":443,"password":"pw"}"#,
        )
        .expect("seal");
    sqlx::query(
        "INSERT INTO node_payloads (node_id,key_version,nonce,ciphertext,updated_at) \
         VALUES ($1,$2,$3,$4,now())",
    )
    .bind(node_id)
    .bind(sealed.key_version)
    .bind(sealed.nonce.to_vec())
    .bind(sealed.ciphertext)
    .execute(&pool)
    .await
    .expect("insert payload");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // Non-empty secrets so the permission matrix distinguishes 401 vs 403; the handler
    // enqueues into the DISPOSABLE test jobs DB (never the production queue).
    std::env::set_var(
        "PHLOGO_DATABASE_JOBS_DSN",
        std::env::var("PHLOGO_TEST_JOBS_DSN")
            .unwrap_or_else(|_| std::env::var("DATABASE_URL").expect("DATABASE_URL")),
    );
    let mut cfg = app_config::Config::default();
    cfg.auth.service_secret = "test-service-secret-m3".into();
    cfg.auth.jwt_secret = "test-jwt-secret-m3".into();
    let app = curse::router::build(cfg);
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    });

    let body = format!(r#"{{"node_ids":["{node_id}"],"batch_size":5}}"#);

    // Anonymous -> 401.
    let (status, _) = post(addr, "/api/v1/check/trigger", None, &body).await;
    assert_eq!(status, 401, "anonymous must be 401");

    // ServiceBearer -> 403 (fail-closed: not in the whitelist).
    let (status, msg) = post(
        addr,
        "/api/v1/check/trigger",
        Some("test-service-secret-m3"),
        &body,
    )
    .await;
    assert_eq!(status, 403, "service bearer must be 403: {msg}");

    // AdminJWT -> 202 with enqueue stats.
    let token = admin_jwt();
    let (status, resp) = post(addr, "/api/v1/check/trigger", Some(&token), &body).await;
    assert_eq!(status, 202, "admin must get 202: {resp}");
    let v: serde_json::Value = serde_json::from_str(&resp).expect("json");
    assert_eq!(v["enqueued_batches"], 1, "one batch: {v}");
    assert_eq!(v["nodes"], 1, "one node: {v}");

    // Unknown node ids -> 422 (explicit selection has nothing schedulable).
    let (status, _) = post(
        addr,
        "/api/v1/check/trigger",
        Some(&token),
        r#"{"node_ids":["11111111-1111-1111-1111-111111111111"]}"#,
    )
    .await;
    assert_eq!(status, 422, "explicit-but-unschedulable must be 422");

    sqlx::query("DELETE FROM nodes WHERE id=$1")
        .bind(node_id)
        .execute(&pool)
        .await
        .ok();
}
