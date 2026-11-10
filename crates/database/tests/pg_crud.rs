//! P1-04 integration tests (require DATABASE_URL). Skipped automatically when DATABASE_URL is unset.
use database::{connect, migrate, NodeRepository, SourceRepository};
use sqlx::PgPool;
use uuid::Uuid;

async fn setup() -> Option<PgPool> {
    let dsn = std::env::var("DATABASE_URL").ok()?;
    let pool = connect(&dsn).await.expect("connect DATABASE_URL");
    migrate(&pool).await.expect("apply migrations");
    Some(pool)
}

/// Invariant #1: same (type, content) returns the existing row (already_existed=true), 1 row total.
#[tokio::test]
async fn same_type_content_returns_existing() {
    let Some(pool) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let repo = SourceRepository { pool: pool.clone() };
    let content = format!("https://x/{}", Uuid::new_v4());
    let (id1, e1) = repo
        .create_dedup("subscribe", "", "", &content)
        .await
        .unwrap();
    let (id2, e2) = repo
        .create_dedup("subscribe", "", "", &content)
        .await
        .unwrap();
    assert!(!e1, "first create must be new");
    assert!(e2, "second create must hit dedup");
    assert_eq!(id1, id2, "dedup must return same id");
    let cnt: i64 = sqlx::query_scalar(
        r#"SELECT count(*) FROM sources WHERE "type"='subscribe' AND content=$1"#,
    )
    .bind(&content)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(cnt, 1, "must not create a second sources row");
}

/// Invariant #2: one node linked to two sources => 1 node row, 2 relation rows.
#[tokio::test]
async fn node_linked_to_two_sources_one_node_two_relations() {
    let Some(pool) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let srepo = SourceRepository { pool: pool.clone() };
    let nrepo = NodeRepository { pool: pool.clone() };
    let c1 = format!("s1-{}", Uuid::new_v4());
    let c2 = format!("s2-{}", Uuid::new_v4());
    let (sid1, _) = srepo.create_dedup("node", "", "", &c1).await.unwrap();
    let (sid2, _) = srepo.create_dedup("node", "", "", &c2).await.unwrap();
    let hash = format!("hash{}", Uuid::new_v4().simple());
    let (nid, existed) = nrepo
        .upsert_by_hash(&hash, 1, "vmess", "1.2.3.4", 443)
        .await
        .unwrap();
    assert!(!existed);
    nrepo.link(nid, sid1).await.unwrap();
    nrepo.link(nid, sid2).await.unwrap();
    let rel = nrepo.count_node_sources(nid).await.unwrap();
    let node_cnt: i64 = sqlx::query_scalar("SELECT count(*) FROM nodes WHERE identity_hash=$1")
        .bind(&hash)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(node_cnt, 1, "exactly one node row");
    assert_eq!(rel, 2, "exactly two node_sources rows");
}
