//! P2-04 reconciliation integration tests (require DATABASE_URL; skipped otherwise).
use database::{connect, migrate, SourceRepository};
use domain::{Credential, NormalizedNode, TlsSettings, Transport};
use registry::{delete_source, reconcile_fetch_result};
use sqlx::PgPool;
use uuid::Uuid;

async fn setup() -> Option<PgPool> {
    let dsn = std::env::var("DATABASE_URL").ok()?;
    let pool = connect(&dsn).await.expect("connect");
    migrate(&pool).await.expect("migrate");
    Some(pool)
}

async fn new_source(pool: &PgPool, policy: &str) -> Uuid {
    let repo = SourceRepository { pool: pool.clone() };
    let (id, _) = repo
        .create_dedup(
            "subscribe",
            "",
            "",
            &format!("https://x/{}", Uuid::new_v4()),
        )
        .await
        .unwrap();
    sqlx::query("UPDATE sources SET fetch_policy=$2 WHERE id=$1")
        .bind(id)
        .bind(policy)
        .execute(pool)
        .await
        .unwrap();
    id
}

fn node(server: &str) -> NormalizedNode {
    let mut n = NormalizedNode {
        protocol: "trojan".into(),
        server: server.into(),
        port: 443,
        credentials: Credential::Password("pw".into()),
        transport: Some(Transport {
            kind: "ws".into(),
            path: Some("/p".into()),
            host: None,
            service_name: None,
        }),
        tls: Some(TlsSettings {
            enabled: true,
            sni: Some(server.into()),
            alpn: vec![],
            reality: None,
        }),
        network_options: None,
        display_name: Some(server.into()),
        source_context: registry::source_ctx(Uuid::nil(), Uuid::nil()),
        raw_config_reference: "{}".into(),
        identity_hash: None,
        fingerprint_version: None,
    };
    domain::stamp(&mut n);
    n
}

async fn mappings(pool: &PgPool, source_id: Uuid) -> Vec<Uuid> {
    sqlx::query_scalar("SELECT node_id FROM node_sources WHERE source_id=$1")
        .bind(source_id)
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn membership(pool: &PgPool, node_id: Uuid) -> String {
    sqlx::query_scalar("SELECT membership FROM nodes WHERE id=$1")
        .bind(node_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn orphaned_at(pool: &PgPool, node_id: Uuid) -> Option<chrono::DateTime<chrono::Utc>> {
    sqlx::query_scalar("SELECT orphaned_at FROM nodes WHERE id=$1")
        .bind(node_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Scenario 1: Replace success removes vanished mapping; lonely node becomes Orphaned.
#[tokio::test]
async fn replace_removes_vanished_and_marks_orphaned() {
    let Some(pool) = setup().await else {
        return;
    };
    let a = new_source(&pool, "replace").await;
    let n1 = node("r1.example.com");
    let n2 = node("r2.example.com");
    reconcile_fetch_result(&pool, a, true, &[n1.clone(), n2.clone()], None)
        .await
        .unwrap();
    let m1 = mappings(&pool, a).await;
    assert_eq!(m1.len(), 2, "two mappings after first fetch");

    let kept = n1.identity_hash.clone().unwrap();
    reconcile_fetch_result(&pool, a, true, &[n1], None)
        .await
        .unwrap();
    let m2 = mappings(&pool, a).await;
    assert_eq!(m2.len(), 1, "vanished node mapping removed");
    let gone = n2.identity_hash.clone().unwrap();
    let n2_id: Uuid = sqlx::query_scalar("SELECT id FROM nodes WHERE identity_hash=$1")
        .bind(&gone)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(membership(&pool, n2_id).await, "Orphaned");
    assert!(orphaned_at(&pool, n2_id).await.is_some());
    let n1_id: Uuid = sqlx::query_scalar("SELECT id FROM nodes WHERE identity_hash=$1")
        .bind(&kept)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(membership(&pool, n1_id).await, "Linked");
}

/// Scenario 2: failed fetch keeps old mappings and records run=failed.
#[tokio::test]
async fn failed_fetch_keeps_mappings() {
    let Some(pool) = setup().await else {
        return;
    };
    let a = new_source(&pool, "replace").await;
    let n1 = node("f1.example.com");
    let n2 = node("f2.example.com");
    reconcile_fetch_result(&pool, a, true, &[n1.clone(), n2.clone()], None)
        .await
        .unwrap();
    let sum = reconcile_fetch_result(&pool, a, false, &[], Some("boom".into()))
        .await
        .unwrap();
    assert_eq!(sum.status, "failed");
    assert_eq!(mappings(&pool, a).await.len(), 2, "no mapping removed");
    let status: String = sqlx::query_scalar("SELECT status FROM source_fetch_runs WHERE id=$1")
        .bind(sum.run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "failed");
}

/// Scenario 3: accumulate keeps old mappings when nodes vanish.
#[tokio::test]
async fn accumulate_keeps_old() {
    let Some(pool) = setup().await else {
        return;
    };
    let a = new_source(&pool, "accumulate").await;
    let n1 = node("a1.example.com");
    let n2 = node("a2.example.com");
    reconcile_fetch_result(&pool, a, true, std::slice::from_ref(&n1), None)
        .await
        .unwrap();
    reconcile_fetch_result(&pool, a, true, std::slice::from_ref(&n2), None)
        .await
        .unwrap();
    let ids = mappings(&pool, a).await;
    assert_eq!(ids.len(), 2, "both mappings kept under accumulate");
}

/// Scenario 4: deleting a source does not hurt nodes still referenced by others.
#[tokio::test]
async fn delete_source_keeps_shared_nodes() {
    let Some(pool) = setup().await else {
        return;
    };
    let a = new_source(&pool, "replace").await;
    let b = new_source(&pool, "replace").await;
    let n1 = node("d1.example.com");
    reconcile_fetch_result(&pool, a, true, std::slice::from_ref(&n1), None)
        .await
        .unwrap();
    reconcile_fetch_result(&pool, b, true, std::slice::from_ref(&n1), None)
        .await
        .unwrap();

    delete_source(&pool, a).await.unwrap();
    let remaining = mappings(&pool, b).await;
    assert_eq!(remaining.len(), 1, "b keeps its mapping");
    let hash = n1.identity_hash.clone().unwrap();
    let row: (Uuid, String) =
        sqlx::query_as("SELECT id, membership FROM nodes WHERE identity_hash=$1")
            .bind(&hash)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.1, "Linked", "still referenced => stays Linked");
}

/// Scenario 5: removing the last mapping orphans the node (empty replace).
#[tokio::test]
async fn empty_replace_orphans_all() {
    let Some(pool) = setup().await else {
        return;
    };
    let a = new_source(&pool, "replace").await;
    let n1 = node("e1.example.com");
    reconcile_fetch_result(&pool, a, true, std::slice::from_ref(&n1), None)
        .await
        .unwrap();
    let sum = reconcile_fetch_result(&pool, a, true, &[], None)
        .await
        .unwrap();
    assert_eq!(sum.removed, 1);
    let hash = n1.identity_hash.clone().unwrap();
    let n1_id: Uuid = sqlx::query_scalar("SELECT id FROM nodes WHERE identity_hash=$1")
        .bind(&hash)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(membership(&pool, n1_id).await, "Orphaned");
    assert!(orphaned_at(&pool, n1_id).await.is_some());
}

/// Scenario 6: dead/suspended sources never get new mappings.
#[tokio::test]
async fn dead_source_is_guarded() {
    let Some(pool) = setup().await else {
        return;
    };
    let a = new_source(&pool, "replace").await;
    let n1 = node("g1.example.com");
    reconcile_fetch_result(&pool, a, true, std::slice::from_ref(&n1), None)
        .await
        .unwrap();
    sqlx::query("UPDATE sources SET status='dead' WHERE id=$1")
        .bind(a)
        .execute(&pool)
        .await
        .unwrap();
    let n2 = node("g2.example.com");
    let sum = reconcile_fetch_result(&pool, a, true, std::slice::from_ref(&n2), None)
        .await
        .unwrap();
    assert_eq!(sum.upserted, 0, "dead source must not ingest");
    let ids = mappings(&pool, a).await;
    assert_eq!(ids.len(), 1, "old mapping untouched, no revival");
}

/// M1: the vendor payload carries credentials, so it must be sealed at rest, bound to
/// the node id, and never written to `nodes.raw_config_reference`.
#[tokio::test]
async fn payload_is_sealed_and_bound_to_its_node() {
    const SECRET: &str = "TOP-SECRET-PW";
    let Some(pool) = setup().await else {
        return;
    };
    let source = new_source(&pool, "replace").await;
    let mut n = node("payload.example");
    n.raw_config_reference = format!(r#"{{"password":"{SECRET}","server":"payload.example"}}"#);
    domain::stamp(&mut n);
    reconcile_fetch_result(&pool, source, true, &[n], None)
        .await
        .unwrap();

    let id: Uuid = sqlx::query_scalar("SELECT id FROM nodes WHERE server=$1")
        .bind("payload.example")
        .fetch_one(&pool)
        .await
        .unwrap();

    let raw: String = sqlx::query_scalar("SELECT raw_config_reference FROM nodes WHERE id=$1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        raw.is_empty(),
        "plaintext payload must never be stored on nodes"
    );

    let (kv, nonce, ct): (i32, Vec<u8>, Vec<u8>) =
        sqlx::query_as("SELECT key_version,nonce,ciphertext FROM node_payloads WHERE node_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(kv, payload_crypto::KEY_VERSION);
    assert_eq!(nonce.len(), payload_crypto::NONCE_LEN);
    assert!(
        !ct.windows(SECRET.len()).any(|w| w == SECRET.as_bytes()),
        "ciphertext must not contain the secret"
    );

    let opened = payload_crypto::global()
        .open(&payload_crypto::aad_for(&id.to_string()), &nonce, &ct)
        .expect("payload must open with the node id as AAD");
    assert!(
        String::from_utf8(opened).unwrap().contains(SECRET),
        "payload must round-trip"
    );

    // Moving the ciphertext to another node id must fail (AAD binding).
    let other = new_source(&pool, "replace").await;
    let mut n2 = node("other.example");
    n2.raw_config_reference = r#"{"password":"x"}"#.into();
    domain::stamp(&mut n2);
    reconcile_fetch_result(&pool, other, true, &[n2], None)
        .await
        .unwrap();
    let id2: Uuid = sqlx::query_scalar("SELECT id FROM nodes WHERE server=$1")
        .bind("other.example")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        payload_crypto::global()
            .open(&payload_crypto::aad_for(&id2.to_string()), &nonce, &ct)
            .is_err(),
        "a payload sealed for one node must not open for another"
    );
}
