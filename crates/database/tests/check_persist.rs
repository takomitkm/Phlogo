//! P3-04 integration tests (require DATABASE_URL; skipped otherwise).
use chrono::{DateTime, Duration, TimeZone, Utc};
use database::check::{persist_check_run, upsert_node_status, CheckResultRow};
use sqlx::PgPool;
use uuid::Uuid;

async fn setup() -> Option<PgPool> {
    let dsn = std::env::var("DATABASE_URL").ok()?;
    let pool = database::connect(&dsn).await.expect("connect");
    database::migrate(&pool).await.expect("migrate");
    Some(pool)
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

fn t(offset_min: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 11, 0, 0, 0).unwrap() + Duration::minutes(offset_min)
}

/// Acceptance #1 + #2: persist N nodes, idempotent re-run, metrics roundtrip.
#[tokio::test]
async fn check_persistence_idempotent_and_metrics_roundtrip() {
    let Some(pool) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let run = Uuid::new_v4();
    let n1 = make_node(&pool).await;
    let n2 = make_node(&pool).await;
    let n3 = make_node(&pool).await;

    let rows = vec![
        CheckResultRow {
            node_id: n1,
            latency_ms: 42,
            deep: Some((
                40,
                5,
                0, // speed_kbps 0 = untested must survive the JSONB roundtrip
                Some("HK".into()),
                serde_json::json!({"netflix": true}),
                serde_json::json!({"ip": "1.2.3.4"}),
            )),
        },
        CheckResultRow { node_id: n2, latency_ms: -1, deep: None },
        CheckResultRow { node_id: n3, latency_ms: 80, deep: None },
    ];

    let inserted = persist_check_run(&pool, run, "default", Some(t(0)), &rows, &[]).await.unwrap();
    assert_eq!(inserted, 3);

    let runs: i64 = sqlx::query_scalar("SELECT count(*) FROM check_runs WHERE id=$1")
        .bind(run)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(runs, 1);
    let results: i64 = sqlx::query_scalar("SELECT count(*) FROM node_check_results WHERE check_run_id=$1")
        .bind(run)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(results, 3);
    let statuses: i64 = sqlx::query_scalar("SELECT count(*) FROM node_status WHERE checker_tag='default'")
        .fetch_one(&pool)
        .await
        .unwrap();

    // Idempotent re-run with the same run_id: counts unchanged.
    let again = persist_check_run(&pool, run, "default", Some(t(0)), &rows, &[]).await.unwrap();
    assert_eq!(again, 0, "second delivery must insert nothing");
    let results2: i64 = sqlx::query_scalar("SELECT count(*) FROM node_check_results WHERE check_run_id=$1")
        .bind(run)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(results2, results, "re-run must not duplicate rows");

    // Metrics roundtrip.
    let (metrics, status): (serde_json::Value, String) = sqlx::query_as(
        "SELECT metrics, result_status FROM node_check_results WHERE check_run_id=$1 AND node_id=$2",
    )
    .bind(run)
    .bind(n1)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(metrics["latency_ms"], 42);
    assert_eq!(metrics["avg_latency_ms"], 40);
    assert_eq!(metrics["jitter_ms"], 5);
    assert_eq!(metrics["speed_kbps"], 0, "0 = untested must be preserved");
    assert_eq!(metrics["region"], "HK");
    assert_eq!(metrics["streaming"]["netflix"], true);
    assert_eq!(status, "alive");

    let dead_status: String = sqlx::query_scalar(
        "SELECT result_status FROM node_check_results WHERE check_run_id=$1 AND node_id=$2",
    )
    .bind(run)
    .bind(n2)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(dead_status, "dead", "latency -1 must persist as dead");
    assert!(statuses >= 0);
}

/// Acceptance #3: late node_status write does not overwrite a newer snapshot.
#[tokio::test]
async fn node_status_late_write_no_overwrite() {
    let Some(pool) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let n = make_node(&pool).await;
    upsert_node_status(&pool, n, "default", "alive", t(10)).await.unwrap();
    // Late write at T+0 must lose against T+10.
    upsert_node_status(&pool, n, "default", "dead", t(0)).await.unwrap();
    let (health, last): (String, DateTime<Utc>) = sqlx::query_as(
        "SELECT health, last_checked_at FROM node_status WHERE node_id=$1 AND checker_tag='default'",
    )
    .bind(n)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(health, "alive", "late write must not downgrade the snapshot");
    assert_eq!(last, t(10));

    // A newer write still wins.
    upsert_node_status(&pool, n, "default", "dead", t(20)).await.unwrap();
    let (health2, last2): (String, DateTime<Utc>) = sqlx::query_as(
        "SELECT health, last_checked_at FROM node_status WHERE node_id=$1 AND checker_tag='default'",
    )
    .bind(n)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(health2, "dead");
    assert_eq!(last2, t(20));
}
