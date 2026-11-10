//! P4-01 integration tests (require DATABASE_URL; skipped otherwise).
use database::{connect, migrate, SourceRepository};
use lifecycle::{delete_retired_nodes, mark_dead_and_clean, retire_expired_orphans};
use sqlx::PgPool;
use uuid::Uuid;

async fn setup() -> Option<PgPool> {
    let dsn = std::env::var("DATABASE_URL").ok()?;
    let pool = connect(&dsn).await.expect("connect");
    migrate(&pool).await.expect("migrate");
    Some(pool)
}

async fn new_source(pool: &PgPool) -> Uuid {
    let repo = SourceRepository { pool: pool.clone() };
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

async fn membership(pool: &PgPool, node: Uuid) -> String {
    sqlx::query_scalar("SELECT membership FROM nodes WHERE id=$1")
        .bind(node)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Dead-source cleanup: mappings removed, lonely node orphaned, second call is a no-op.
#[tokio::test]
async fn mark_dead_and_clean_is_idempotent() {
    let Some(pool) = setup().await else { return; };
    let a = new_source(&pool).await;
    let b = new_source(&pool).await;
    let n1 = make_node(&pool).await; // only referenced by a
    let n2 = make_node(&pool).await; // referenced by a + b
    link(&pool, n1, a).await;
    link(&pool, n2, a).await;
    link(&pool, n2, b).await;

    assert!(mark_dead_and_clean(&pool, a).await.unwrap(), "first caller wins");
    assert_eq!(membership(&pool, n1).await, "Orphaned");
    assert_eq!(membership(&pool, n2).await, "Linked", "still referenced by b");

    let again = mark_dead_and_clean(&pool, a).await.unwrap();
    assert!(!again, "second caller must skip (conditional UPDATE)");
    let maps: i64 = sqlx::query_scalar("SELECT count(*) FROM node_sources WHERE source_id=$1")
        .bind(a)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(maps, 0);
    let status: String = sqlx::query_scalar("SELECT status FROM sources WHERE id=$1")
        .bind(a)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "dead");
}

/// Orphan -> Retired after retention window -> physically deleted; revivals win.
#[tokio::test]
async fn orphan_retire_delete_lifecycle() {
    let Some(pool) = setup().await else { return; };
    let a = new_source(&pool).await;
    let n = make_node(&pool).await;
    link(&pool, n, a).await;

    // Empty Replace fetch orphans it (directly via lifecycle guard here).
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
    assert_eq!(membership(&pool, n).await, "Orphaned");

    // Backdated orphaned_at => retire immediately.
    sqlx::query("UPDATE nodes SET orphaned_at = now() - interval '30 days' WHERE id=$1")
        .bind(n)
        .execute(&pool)
        .await
        .unwrap();
    let retired = retire_expired_orphans(&pool, 14).await.unwrap();
    assert!(retired >= 1);
    assert_eq!(membership(&pool, n).await, "Retired");

    // Revival before deletion wins: re-link resets to Linked.
    let b = new_source(&pool).await;
    link(&pool, n, b).await;
    sqlx::query("UPDATE nodes SET membership='Linked', orphaned_at=NULL WHERE id=$1 AND membership='Retired'")
        .bind(n)
        .execute(&pool)
        .await
        .unwrap();
    let deleted = delete_retired_nodes(&pool).await.unwrap();
    let alive: i64 = sqlx::query_scalar("SELECT count(*) FROM nodes WHERE id=$1")
        .bind(n)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(alive, 1, "revived node must survive cleanup");
    assert_eq!(membership(&pool, n).await, "Linked");
    let _ = deleted;
}
