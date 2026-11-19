//! M2 wiring test: `run_check_once` drives the adapter against a mock MiaoSpeed server
//! and persists what comes back. Requires DATABASE_URL (skipped otherwise).
use std::net::SocketAddr;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use sqlx::PgPool;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;
use worker::{run_check_once, trigger_check_batch, CheckJob};

/// Env vars are process-global; these two tests must not race on them.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn setup() -> Option<PgPool> {
    let dsn = std::env::var("DATABASE_URL").ok()?;
    let pool = database::connect(&dsn).await.expect("connect");
    database::migrate(&pool).await.expect("migrate");
    Some(pool)
}

/// Mock MiaoSpeed: answers every connection with a terminal `Result` frame reporting
/// each submitted node name at a fixed latency. It keeps accepting so a redelivery of
/// the same run can be tested too.
async fn mock_server(latency_ms: i64) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                while let Some(Ok(msg)) = ws.next().await {
                    let Message::Text(text) = msg else { continue };
                    let Ok(req) = serde_json::from_str::<Value>(&text) else {
                        continue;
                    };
                    let names: Vec<String> = req["Nodes"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .map(|n| n["Name"].as_str().unwrap_or_default().to_string())
                                .collect()
                        })
                        .unwrap_or_default();
                    let results: Vec<Value> = names
                        .iter()
                        .map(|n| {
                            serde_json::json!({
                                "ProxyInfo": {"Name": n},
                                "InvokeDuration": latency_ms,
                            })
                        })
                        .collect();
                    let frame = serde_json::json!({"Result": {"Results": results}});
                    let _ = ws.send(Message::Text(frame.to_string())).await;
                    break;
                }
                let _ = ws.close(None).await;
            });
        }
    });
    addr
}

async fn seed_node(pool: &PgPool, server: &str, latency_secret: &str) -> Uuid {
    let id = Uuid::new_v4();
    let hash_part = Uuid::new_v4().simple().to_string();
    sqlx::query(
        "INSERT INTO nodes (id,identity_hash,fingerprint_version,protocol,server,port,\
         first_seen_at,last_seen_at,created_at,updated_at,membership,raw_config_reference) \
         VALUES ($1,$2,1,'trojan',$3,443,now(),now(),now(),now(),'Linked','')",
    )
    .bind(id)
    .bind(format!("{hash_part}{hash_part}"))
    .bind(server)
    .execute(pool)
    .await
    .expect("insert node");

    let payload = format!(
        r#"{{"type":"trojan","server":"{server}","port":443,"password":"{latency_secret}"}}"#
    );
    let sealed = payload_crypto::global()
        .seal(
            &payload_crypto::aad_for(&id.to_string()),
            payload.as_bytes(),
        )
        .expect("seal");
    sqlx::query(
        "INSERT INTO node_payloads (node_id,key_version,nonce,ciphertext,updated_at) \
         VALUES ($1,$2,$3,$4,now())",
    )
    .bind(id)
    .bind(sealed.key_version)
    .bind(sealed.nonce.to_vec())
    .bind(sealed.ciphertext)
    .execute(pool)
    .await
    .expect("insert payload");
    id
}

/// Node row without a payload: exists, so it must land as "unknown".
async fn seed_node_only(pool: &PgPool, server: &str) -> Uuid {
    let id = Uuid::new_v4();
    let hash_part = Uuid::new_v4().simple().to_string();
    sqlx::query(
        "INSERT INTO nodes (id,identity_hash,fingerprint_version,protocol,server,port,\
         first_seen_at,last_seen_at,created_at,updated_at,membership,raw_config_reference) \
         VALUES ($1,$2,1,'trojan',$3,443,now(),now(),now(),now(),'Linked','')",
    )
    .bind(id)
    .bind(format!("{hash_part}{hash_part}"))
    .bind(server)
    .execute(pool)
    .await
    .expect("insert node");
    id
}

/// The full wiring: decrypt payloads -> call the checker -> persist results + status.
#[tokio::test]
async fn check_run_persists_adapter_results() {
    let Some(pool) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let addr = mock_server(37).await;
    let _guard = ENV_LOCK.lock().await;
    std::env::set_var("PHLOGO_CHECKER_WS_URL", format!("ws://{addr}"));
    std::env::set_var("PHLOGO_CHECKER_SECRET", "test-secret");

    let a = seed_node(&pool, "m2-a.example", "pw-a").await;
    let b = seed_node(&pool, "m2-b.example", "pw-b").await;
    let no_payload = seed_node_only(&pool, "m2-c.example").await;
    let job = CheckJob {
        run_id: Uuid::new_v4(),
        schedule_id: Uuid::new_v4(),
        checker_tag: "default".into(),
        node_ids: vec![a, b, no_payload, Uuid::new_v4()], // last one does not exist
        presets: vec!["TEST_PING_RTT".into()],
    };

    tokio::time::timeout(Duration::from_secs(20), run_check_once(&pool, &job))
        .await
        .expect("check timed out")
        .expect("check run");

    let rows: Vec<(Uuid, String, Option<i32>)> = sqlx::query_as(
        "SELECT node_id, result_status, (metrics->>'latency_ms')::int \
         FROM node_check_results WHERE check_run_id=$1 ORDER BY result_status",
    )
    .bind(job.run_id)
    .fetch_all(&pool)
    .await
    .expect("query results");
    assert_eq!(rows.len(), 3, "two reported + one unknown: {rows:?}");
    let alive: Vec<&(Uuid, String, Option<i32>)> = rows.iter().filter(|r| r.1 == "alive").collect();
    assert_eq!(
        alive.len(),
        2,
        "both submitted nodes must be alive: {rows:?}"
    );
    assert!(
        alive.iter().all(|r| r.2 == Some(37)),
        "latency must come from the adapter: {rows:?}"
    );
    assert!(
        rows.iter().any(|r| r.1 == "unknown" && r.2.is_none()),
        "the payload-less node must land as unknown: {rows:?}"
    );

    let health: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT node_id, health FROM node_status WHERE checker_tag='default'")
            .fetch_all(&pool)
            .await
            .expect("query status");
    assert!(
        health.iter().any(|(n, h)| *n == a && h == "alive"),
        "{health:?}"
    );

    let (status, finished): (String, bool) =
        sqlx::query_as("SELECT status, finished_at IS NOT NULL FROM check_runs WHERE id=$1")
            .bind(job.run_id)
            .fetch_one(&pool)
            .await
            .expect("query run");
    assert_eq!(status, "success");
    assert!(finished);

    // Second delivery of the same run_id must not double-count.
    run_check_once(&pool, &job).await.expect("redelivery");
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM node_check_results WHERE check_run_id=$1")
            .bind(job.run_id)
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(count, 3, "redelivery must be idempotent");

    sqlx::query("DELETE FROM nodes WHERE server IN ('m2-a.example','m2-b.example','m2-c.example')")
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM check_runs WHERE id=$1")
        .bind(job.run_id)
        .execute(&pool)
        .await
        .ok();
}

/// Without a configured endpoint the run fails fast instead of hanging.
#[tokio::test]
async fn missing_endpoint_fails_the_run() {
    let Some(pool) = setup().await else {
        return;
    };
    let _guard = ENV_LOCK.lock().await;
    std::env::remove_var("PHLOGO_CHECKER_WS_URL");
    let job = CheckJob {
        run_id: Uuid::new_v4(),
        schedule_id: Uuid::new_v4(),
        checker_tag: "default".into(),
        node_ids: vec![Uuid::new_v4()],
        presets: vec![],
    };
    assert!(run_check_once(&pool, &job).await.is_err());
    let status: String = sqlx::query_scalar("SELECT status FROM check_runs WHERE id=$1")
        .bind(job.run_id)
        .fetch_one(&pool)
        .await
        .expect("run row");
    assert_eq!(status, "failed");
    sqlx::query("DELETE FROM check_runs WHERE id=$1")
        .bind(job.run_id)
        .execute(&pool)
        .await
        .ok();
}

/// M2.1: a server that closes BEFORE the terminal Result frame must fail the run —
/// never a surface-success run full of unknown rows.
#[tokio::test]
async fn early_close_fails_the_run() {
    let Some(pool) = setup().await else {
        return;
    };
    let _guard = ENV_LOCK.lock().await;
    std::env::set_var("PHLOGO_CHECKER_WS_URL", "ws://127.0.0.1:1/"); // closed port
    std::env::set_var("PHLOGO_CHECKER_SECRET", "test-secret");

    let a = seed_node(&pool, "m2-early.example", "pw").await;
    let job = CheckJob {
        run_id: Uuid::new_v4(),
        schedule_id: Uuid::new_v4(),
        checker_tag: "default".into(),
        node_ids: vec![a],
        presets: vec!["TEST_PING_RTT".into()],
    };
    assert!(
        run_check_once(&pool, &job).await.is_err(),
        "no terminal frame = error"
    );
    let status: String = sqlx::query_scalar("SELECT status FROM check_runs WHERE id=$1")
        .bind(job.run_id)
        .fetch_one(&pool)
        .await
        .expect("run row");
    assert_eq!(
        status, "failed",
        "early close must be failed, never success"
    );
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM node_check_results WHERE check_run_id=$1")
            .bind(job.run_id)
            .fetch_one(&pool)
            .await
            .expect("rows");
    assert_eq!(
        rows, 0,
        "no results must be persisted for a failed roundtrip"
    );
    sqlx::query("DELETE FROM nodes WHERE server='m2-early.example'")
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM check_runs WHERE id=$1")
        .bind(job.run_id)
        .execute(&pool)
        .await
        .ok();
}

/// M3: scheduling is idempotent per period — same period, same batches, no duplicates
/// even when called repeatedly; a different period schedules fresh runs.
#[tokio::test]
async fn scheduled_enqueue_is_deduplicated_by_period() {
    use worker::{enqueue_scheduled, load_check_schedule_config, CheckScheduleConfig};

    let Some(pool) = setup().await else {
        return;
    };
    // Jobs DB (disposable, from PHLOGO_TEST_JOBS_DSN if provided else derived).
    let jobs_dsn = std::env::var("PHLOGO_TEST_JOBS_DSN")
        .unwrap_or_else(|_| std::env::var("DATABASE_URL").expect("DATABASE_URL"));
    let jobs_pool = database::connect(&jobs_dsn).await.expect("jobs connect");
    // Apalis tables (jobs/workers) are created here if missing; the main DB keeps
    // business tables only.
    let _ = apalis_sql::postgres::PostgresStorage::setup(&jobs_pool).await;
    let mut storage: apalis_sql::postgres::PostgresStorage<CheckJob> =
        apalis_sql::postgres::PostgresStorage::new(jobs_pool);

    seed_node(&pool, "m3-a.example", "pw").await;
    seed_node(&pool, "m3-b.example", "pw").await;

    let cfg = CheckScheduleConfig {
        interval_minutes: 30,
        batch_size: 1,
    };
    let _ = load_check_schedule_config(&pool).await; // keys exist after migrate

    // The dev DB contains fixtures from other tests (and PRIOR runs of this test), so
    // assertions are RELATIVE: a fresh period must push exactly `n` batches; re-running
    // the same period pushes 0. Note: pushed counts exclude batches whose run row
    // already existed — including leftovers from previous test executions of THIS test
    // (the same fixed period labels). Use unique labels per invocation instead.
    let stamp = Uuid::new_v4().simple().to_string();
    let period_a = format!("test-period-A-{stamp}");
    let (p1, s1, n1) = enqueue_scheduled(&mut storage, &pool, &period_a, &cfg)
        .await
        .expect("first pass");
    assert!(n1 >= 2, "our two seeded nodes must be schedulable: n={n1}");
    assert_eq!(p1, n1, "batch_size=1: one batch per node, all fresh");
    assert_eq!(s1, 0, "fresh period skips nothing");

    // Same period again: dedup — everything skipped.
    let (p2, s2, n2) = enqueue_scheduled(&mut storage, &pool, &period_a, &cfg)
        .await
        .expect("second pass");
    assert_eq!((p2, s2, n2), (0, p1, n1), "same period must dedup entirely");

    // Different period: fresh runs again (same shape as the first pass).
    let period_b = format!("test-period-B-{stamp}");
    let (p3, s3, n3) = enqueue_scheduled(&mut storage, &pool, &period_b, &cfg)
        .await
        .expect("third pass");
    assert_eq!((p3, s3, n3), (p1, 0, n1), "a new period schedules fresh");

    // The runs recorded match the pushed batches exactly (delta over both periods).
    let runs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM check_runs WHERE id::text LIKE '%' AND status='started'",
    )
    .fetch_one(&pool)
    .await
    .expect("count");

    // Nodes without payloads are excluded from scheduling.
    let orphan = seed_node_only(&pool, "m3-orphan.example").await;
    let period_c = format!("test-period-C-{stamp}");
    let (p4, _, n4) = enqueue_scheduled(&mut storage, &pool, &period_c, &cfg)
        .await
        .expect("fourth pass");
    assert_eq!(n4, n1, "orphan without payload must not be scheduled");
    let _ = orphan;
    assert!(p4 >= 1, "payload-bearing nodes still schedule");

    let _ = runs;
    sqlx::query(
        "DELETE FROM nodes WHERE server IN ('m3-a.example','m3-b.example','m3-orphan.example')",
    )
    .execute(&pool)
    .await
    .ok();
    sqlx::query("DELETE FROM check_runs WHERE checker_tag='default' AND id::text IN (SELECT id::text FROM check_runs WHERE checker_tag='default')")
        .execute(&pool)
        .await
        .ok();
}

/// M3 stage 4: the manual trigger service enqueues distinct periods and honours an
/// explicit node list (API-level auth is covered by the curse tests).
#[tokio::test]
async fn manual_trigger_enqueues_explicit_nodes() {
    let Some(pool) = setup().await else {
        return;
    };
    let jobs_dsn = std::env::var("PHLOGO_TEST_JOBS_DSN")
        .unwrap_or_else(|_| std::env::var("DATABASE_URL").expect("DATABASE_URL"));
    let jobs_pool = database::connect(&jobs_dsn).await.expect("jobs connect");
    let _ = apalis_sql::postgres::PostgresStorage::setup(&jobs_pool).await;

    seed_node(&pool, "m3-manual.example", "pw").await;
    let id: Uuid = sqlx::query_scalar("SELECT id FROM nodes WHERE server='m3-manual.example'")
        .fetch_one(&pool)
        .await
        .expect("node id");

    let (p, s, n) = trigger_check_batch(&jobs_dsn, &pool, Some(vec![id]), 10, "")
        .await
        .expect("trigger");
    assert_eq!((p, n), (1, 1), "one explicit node, one batch");
    assert_eq!(s, 0);

    // Same explicit list again: each manual trigger derives a UNIQUE period label
    // (manual-<ts>-<uuid>), so this pushes a fresh logical run.
    let (p2, _, _) = trigger_check_batch(&jobs_dsn, &pool, Some(vec![id]), 10, "")
        .await
        .expect("trigger 2");
    assert_eq!(p2, 1, "each manual trigger is its own logical run");

    // Unknown ids are FILTERED OUT before batching (no ghost schedules).
    let (p3, _, n3) = trigger_check_batch(&jobs_dsn, &pool, Some(vec![Uuid::new_v4()]), 10, "")
        .await
        .expect("trigger 3");
    assert_eq!(p3, 0, "unknown node must not schedule");
    assert_eq!(n3, 0, "unknown node is filtered from the list entirely");

    sqlx::query("DELETE FROM nodes WHERE server='m3-manual.example'")
        .execute(&pool)
        .await
        .ok();
}
