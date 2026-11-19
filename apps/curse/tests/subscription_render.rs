//! M1b acceptance: `/sub/{token}.{fmt}` renders REAL nodes from sealed payloads.
//!
//! Requires DATABASE_URL (same gate as the other DB integration tests). The payload key
//! is the process-wide key, so seeding and rendering share it without any env setup.
use std::net::SocketAddr;
use std::time::Duration;

use app_config::Config;
use distribution::token::hash_token;
use sqlx::PgPool;
use uuid::Uuid;

async fn setup() -> Option<PgPool> {
    let dsn = std::env::var("DATABASE_URL").ok()?;
    let pool = database::connect(&dsn).await.expect("connect");
    database::migrate(&pool).await.expect("migrate");
    Some(pool)
}

async fn insert_node(pool: &PgPool, server: &str, secret: &str) {
    let node_id = Uuid::new_v4();
    // identity_hash is varchar(64): two 32-char simple uuids concatenated.
    let hash_part = Uuid::new_v4().simple().to_string();
    sqlx::query(
        "INSERT INTO nodes (id,identity_hash,fingerprint_version,protocol,server,port,\
         first_seen_at,last_seen_at,created_at,updated_at,membership,raw_config_reference) \
         VALUES ($1,$2,1,'trojan',$3,443,now(),now(),now(),now(),'Linked','')",
    )
    .bind(node_id)
    .bind(format!("{hash_part}{hash_part}"))
    .bind(server)
    .execute(pool)
    .await
    .expect("insert node");

    let payload =
        format!(r#"{{"type":"trojan","server":"{server}","port":443,"password":"{secret}"}}"#);
    let sealed = payload_crypto::global()
        .seal(
            &payload_crypto::aad_for(&node_id.to_string()),
            payload.as_bytes(),
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
    .execute(pool)
    .await
    .expect("insert payload");

    sqlx::query(
        "INSERT INTO node_scores (node_id,checker_tag,score,components,version,calculated_at) \
         VALUES ($1,'default',0.95,'{}','ewma-v1',now())",
    )
    .bind(node_id)
    .execute(pool)
    .await
    .expect("insert score");

    // The profile's health bar gates on the node's ACTUAL health (node_status); seed
    // one alive row so the seeded node passes the default 'alive' bar.
    sqlx::query(
        "INSERT INTO node_status (node_id,checker_tag,health,last_checked_at) \
         VALUES ($1,'default','alive',now())",
    )
    .bind(node_id)
    .execute(pool)
    .await
    .expect("insert status");
}

async fn insert_profile_and_token(pool: &PgPool, filter_ast: &str) -> String {
    let profile_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO subscription_profiles \
         (id,name,filter_ast,region,health,min_score,limit_count,order_strategy,preset,\
          output_format,naming_rule,enabled,version) \
         VALUES ($1,'m1b-test',$2::jsonb,NULL,'alive',0,200,'score_desc','all','yaml',NULL,true,1)",
    )
    .bind(profile_id)
    .bind(filter_ast)
    .execute(pool)
    .await
    .expect("insert profile");

    let token = format!("tok-{}", Uuid::new_v4());
    sqlx::query(
        "INSERT INTO subscription_tokens (id,profile_id,token_hash,status) \
         VALUES ($1,$2,$3,'active')",
    )
    .bind(Uuid::new_v4())
    .bind(profile_id)
    .bind(hash_token(&token, ""))
    .execute(pool)
    .await
    .expect("insert token");
    token
}

async fn spawn() -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = curse::router::build(Config::default());
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    });
    addr
}

async fn get(addr: SocketAddr, path: &str) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nconnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();
    let mut buf = vec![0u8; 262144];
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

/// Raw GET that returns the whole response, headers included (for header assertions).
async fn get_raw(addr: SocketAddr, path: &str) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nconnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();
    let mut buf = vec![0u8; 262144];
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
    (status, text.to_lowercase())
}

/// Every /sub response — success AND failure — must carry `Cache-Control: no-store`
/// (S64 review finding #1; real-header assertion demanded by the S68 ruling).
#[tokio::test]
async fn every_sub_response_is_no_store() {
    let Some(pool) = setup().await else {
        return;
    };
    insert_node(&pool, "nocache.example", "pw").await;
    let token = insert_profile_and_token(&pool, r#"{"operator":"and","conditions":[]}"#).await;
    let addr = spawn().await;

    for path in [
        format!("/sub/{token}.yaml"),
        format!("/sub/{token}.json"),
        format!("/sub/{token}.base64"),
        // failure shapes: bad format, unknown token
        "/sub/not-a-token.bogus".to_string(),
        "/sub/definitely-unknown-token.yaml".to_string(),
    ] {
        let (status, raw) = get_raw(addr, &path).await;
        assert!(
            raw.contains("cache-control: no-store"),
            "{path} (status {status}) must carry Cache-Control: no-store; got:\n{raw}"
        );
    }

    sqlx::query("DELETE FROM nodes WHERE server='nocache.example'")
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM subscription_profiles WHERE name='m1b-test'")
        .execute(&pool)
        .await
        .ok();
}

/// YAML and sing-box output must carry the real node, and the profile's AST must filter.
#[tokio::test]
async fn renders_real_nodes_and_applies_the_profile_filter() {
    const SECRET: &str = "TOP-SECRET-PW";
    let Some(pool) = setup().await else {
        return;
    };
    insert_node(&pool, "jp.example", SECRET).await;
    insert_node(&pool, "us.example", SECRET).await;
    let ast = r#"{"operator":"and","conditions":[{"field":"server","operator":"eq","value":"jp.example"}]}"#;
    let token = insert_profile_and_token(&pool, ast).await;

    let addr = spawn().await;

    let (status, yaml) = get(addr, &format!("/sub/{token}.yaml")).await;
    assert_eq!(status, 200, "yaml: {yaml}");
    assert!(yaml.contains("jp.example"), "real server missing: {yaml}");
    assert!(yaml.contains(SECRET), "real credential missing: {yaml}");
    assert!(
        !yaml.contains("us.example"),
        "profile AST not applied: {yaml}"
    );

    let (status, json) = get(addr, &format!("/sub/{token}.json")).await;
    assert_eq!(status, 200, "json: {json}");
    let v: serde_json::Value = serde_json::from_str(&json).expect("sing-box json");
    let outbounds = v["outbounds"].as_array().expect("outbounds");
    assert!(
        outbounds.iter().any(|o| o["server"] == "jp.example"),
        "sing-box outbound missing: {json}"
    );
    assert!(
        !outbounds.iter().any(|o| o["server"] == "us.example"),
        "profile AST not applied to json: {json}"
    );

    // base64 now carries a real share link (M1c).
    let (status, body) = get(addr, &format!("/sub/{token}.base64")).await;
    assert_eq!(status, 200, "base64: {body}");
    let decoded = {
        use base64::Engine;
        let raw = base64::engine::general_purpose::STANDARD
            .decode(body.trim())
            .expect("base64 body");
        String::from_utf8(raw).expect("utf8")
    };
    assert!(
        decoded.contains("trojan://") && decoded.contains("jp.example:443"),
        "real URI missing: {decoded}"
    );
    assert!(
        !decoded.contains("us.example"),
        "profile AST not applied: {decoded}"
    );
    assert!(
        decoded.contains(SECRET),
        "credential missing from URI: {decoded}"
    );

    // Revoked token -> 401.
    sqlx::query(
        "UPDATE subscription_tokens SET status='revoked', revoked_at=now() WHERE token_hash=$1",
    )
    .bind(hash_token(&token, ""))
    .execute(&pool)
    .await
    .expect("revoke");
    let (status, _) = get(addr, &format!("/sub/{token}.yaml")).await;
    assert_eq!(status, 401, "revoked token must be rejected");

    // Clean up the fixtures so they never leak into other subscriptions on the dev DB.
    sqlx::query("DELETE FROM nodes WHERE server IN ('jp.example','us.example')")
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM subscription_profiles WHERE name='m1b-test'")
        .execute(&pool)
        .await
        .ok();
}

/// A node whose health is below the profile bar must be excluded even though its
/// payload, score and AST conditions all match.
#[tokio::test]
async fn profile_health_bar_excludes_unhealthy_nodes() {
    let Some(pool) = setup().await else {
        return;
    };
    insert_node(&pool, "healthy.example", "pw").await;
    let dead_id = Uuid::new_v4();
    let hash_part = Uuid::new_v4().simple().to_string();
    sqlx::query(
        "INSERT INTO nodes (id,identity_hash,fingerprint_version,protocol,server,port,\
         first_seen_at,last_seen_at,created_at,updated_at,membership,raw_config_reference) \
         VALUES ($1,$2,1,'trojan','dead.example',443,now(),now(),now(),now(),'Linked','')",
    )
    .bind(dead_id)
    .bind(format!("{hash_part}{hash_part}"))
    .execute(&pool)
    .await
    .expect("insert dead node");
    let payload = r#"{"type":"trojan","server":"dead.example","port":443,"password":"pw"}"#;
    let sealed = payload_crypto::global()
        .seal(
            &payload_crypto::aad_for(&dead_id.to_string()),
            payload.as_bytes(),
        )
        .expect("seal");
    sqlx::query(
        "INSERT INTO node_payloads (node_id,key_version,nonce,ciphertext,updated_at) \
         VALUES ($1,$2,$3,$4,now())",
    )
    .bind(dead_id)
    .bind(sealed.key_version)
    .bind(sealed.nonce.to_vec())
    .bind(sealed.ciphertext)
    .execute(&pool)
    .await
    .expect("insert payload");
    sqlx::query(
        "INSERT INTO node_scores (node_id,checker_tag,score,components,version,calculated_at) \
         VALUES ($1,'default',0.95,'{}','ewma-v1',now())",
    )
    .bind(dead_id)
    .execute(&pool)
    .await
    .expect("insert score");
    sqlx::query(
        "INSERT INTO node_status (node_id,checker_tag,health,last_checked_at) \
         VALUES ($1,'default','dead',now())",
    )
    .bind(dead_id)
    .execute(&pool)
    .await
    .expect("insert dead status");

    let token = insert_profile_and_token(&pool, r#"{"operator":"and","conditions":[]}"#).await; // empty AST: only the bar filters
    let addr = spawn().await;
    let (status, yaml) = get(addr, &format!("/sub/{token}.yaml")).await;
    assert_eq!(status, 200, "yaml: {yaml}");
    assert!(yaml.contains("healthy.example"), "healthy node in: {yaml}");
    assert!(
        !yaml.contains("dead.example"),
        "dead node must be excluded: {yaml}"
    );

    sqlx::query("DELETE FROM nodes WHERE server IN ('healthy.example','dead.example')")
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM subscription_profiles WHERE name='m1b-test'")
        .execute(&pool)
        .await
        .ok();
}
