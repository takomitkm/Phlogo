//! P4-02 integration tests (require DATABASE_URL; skipped otherwise).
use chrono::{DateTime, Duration, TimeZone, Utc};
use database::check::upsert_node_status;
use lifecycle::{cross_tag_alive_map, run_cleanup_cycle};
use sqlx::PgPool;
use uuid::Uuid;

async fn setup() -> Option<PgPool> {
    let dsn = std::env::var("DATABASE_URL").ok()?;
    let pool = database::connect(&dsn).await.expect("connect");
    database::migrate(&pool).await.expect("migrate");
    Some(pool)
}

async fn new_source(pool: &PgPool) -> Uuid {
    let repo = database::SourceRepository { pool: pool.clone() };
    let (id, _) = repo
        .create_dedup("subscribe", "", "", &format!("https://x/{}", Uuid::new_v4()))
        .await
        .unwrap();
    id
}

async fn make_node(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO nodes (id,identity_hash,fingerprint_version,protocol,server,port,first_seen_at,last_seen_at,created_at,updated_at) \
         VALUES ($1,$2,1,'trojan','t.example.com',443,now(),now(),now(),now())",
    )
    .bind(id)
    .bind(Uuid::new_v4().simple().to_string())
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn link(pool: &PgPool, node: Uuid, source: Uuid) {
    sqlx::query("INSERT INTO node_sources (node_id,source_id) VALUES ($1,$2) ON CONFLICT DO NOTHING")
        .bind(node)
        .bind(source)
        .execute(pool)
        .await
        .unwrap();
}

fn t(offset_min: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 11, 0, 0, 0).unwrap() + Duration::minutes(offset_min)
}

/// Acceptance A: per-tag independence — (N,A)=Alive and (N,B)=Dead coexist.
#[tokio::test]
async fn per_tag_health_independent() {
    let Some(pool) = setup().await else { return; };
    let n = make_node(&pool).await;
    upsert_node_status(&pool, n, "a", "alive", t(10)).await.unwrap();
    upsert_node_status(&pool, n, "b", "dead", t(10)).await.unwrap();
    let ha: String = sqlx::query_scalar(
        "SELECT health FROM node_status WHERE node_id=$1 AND checker_tag='a'",
    )
    .bind(n)
    .fetch_one(&pool)
    .await
    .unwrap();
    let hb: String = sqlx::query_scalar(
        "SELECT health FROM node_status WHERE node_id=$1 AND checker_tag='b'",
    )
    .bind(n)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(ha, "alive");
    assert_eq!(hb, "dead");
}

/// Acceptance B: unreachable (dead) node keeps its source mappings and its row.
#[tokio::test]
async fn unreachable_node_keeps_source_mapping() {
    let Some(pool) = setup().await else { return; };
    let s1 = new_source(&pool).await;
    let s2 = new_source(&pool).await;
    let n = make_node(&pool).await;
    link(&pool, n, s1).await;
    link(&pool, n, s2).await;

    upsert_node_status(&pool, n, "a", "dead", t(0)).await.unwrap();

    let maps: i64 = sqlx::query_scalar("SELECT count(*) FROM node_sources WHERE node_id=$1")
        .bind(n)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(maps, 2, "dead report must not remove mappings");
    let exists: i64 = sqlx::query_scalar("SELECT count(*) FROM nodes WHERE id=$1")
        .bind(n)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(exists, 1, "node must not be physically deleted");
}

/// Acceptance C: orphan detection + retention window (not deleted immediately),
/// cleanup cycle retires then deletes, and check history goes with it.
#[tokio::test]
async fn orphan_detection_and_retention() {
    let Some(pool) = setup().await else { return; };
    let a = new_source(&pool).await;
    let n = make_node(&pool).await;
    link(&pool, n, a).await;
    upsert_node_status(&pool, n, "a", "alive", t(0)).await.unwrap();

    // Lose the last mapping -> Orphaned (node still exists).
    let mut tx = pool.begin().await.unwrap();
    let removed = sqlx::query_scalar("DELETE FROM node_sources WHERE source_id=$1 RETURNING node_id")
        .bind(a)
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    for node_id in &removed {
        lifecycle::ops::mark_orphaned_if_lonely(&mut tx, *node_id).await.unwrap();
    }
    tx.commit().await.unwrap();
    let (membership, orphaned_at): (String, Option<DateTime<Utc>>) = sqlx::query_as(
        "SELECT membership, orphaned_at FROM nodes WHERE id=$1",
    )
    .bind(n)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(membership, "Orphaned");
    assert!(orphaned_at.is_some(), "orphaned_at must be recorded");
    let still_there: i64 = sqlx::query_scalar("SELECT count(*) FROM nodes WHERE id=$1")
        .bind(n)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(still_there, 1, "orphan must NOT be deleted immediately");

    // Cleanup cycle with a fresh orphan must NOT retire it yet (within 14d window).
    let report = run_cleanup_cycle(&pool).await.unwrap();
    let membership2: String = sqlx::query_scalar("SELECT membership FROM nodes WHERE id=$1")
        .bind(n)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(membership2, "Orphaned", "fresh orphan stays within retention");
    assert_eq!(report.retired, 0, "no fresh orphan qualifies");

    // Backdate beyond retention -> one cleanup tick retires AND deletes (same cycle),
    // taking the check history with it.
    sqlx::query("UPDATE nodes SET orphaned_at = now() - interval '30 days' WHERE id=$1")
        .bind(n)
        .execute(&pool)
        .await
        .unwrap();
    let report = run_cleanup_cycle(&pool).await.unwrap();
    assert!(report.retired >= 1 || report.deleted >= 1, "cycle must act on the backdated orphan");
    let gone: i64 = sqlx::query_scalar("SELECT count(*) FROM nodes WHERE id=$1")
        .bind(n)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(gone, 0, "backdated orphan retired+deleted within one cycle");
    let history: i64 = sqlx::query_scalar("SELECT count(*) FROM node_status WHERE node_id=$1")
        .bind(n)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(history, 0, "check history cleaned with the node");
}

/// Cross-tag alive map honors the window (in-window dead only -> false; any alive -> true).
#[tokio::test]
async fn cross_tag_map_window() {
    let Some(pool) = setup().await else { return; };
    let n = make_node(&pool).await;
    upsert_node_status(&pool, n, "a", "dead", t(-10)).await.unwrap();
    let m = cross_tag_alive_map(&pool, 240).await.unwrap();
    assert_eq!(m.get(&n), Some(&false), "in-window dead only => false");

    upsert_node_status(&pool, n, "b", "alive", t(-5)).await.unwrap();
    let m = cross_tag_alive_map(&pool, 240).await.unwrap();
    assert_eq!(m.get(&n), Some(&true), "any in-window alive => true");
}
