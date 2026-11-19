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
    // Jobs DB: a dedicated disposable DB is REQUIRED (M3.1 ruling) — no fallback to
    // DATABASE_URL, so test traffic can never land on the business database as a queue.
    let Ok(jobs_dsn) = std::env::var("PHLOGO_TEST_JOBS_DSN") else {
        eprintln!("skip: PHLOGO_TEST_JOBS_DSN not set (refusing to use DATABASE_URL as queue)");
        return;
    };
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
    let (p1, s1, _r1, n1, _u1) = enqueue_scheduled(&mut storage, &pool, &period_a, &cfg)
        .await
        .expect("first pass");
    assert!(n1 >= 2, "our two seeded nodes must be schedulable: n={n1}");
    assert_eq!(p1, n1, "batch_size=1: one batch per node, all fresh");
    assert_eq!(s1, 0, "fresh period skips nothing");

    // Same period again: dedup — everything skipped.
    let (p2, s2, _r2, n2, _u2) = enqueue_scheduled(&mut storage, &pool, &period_a, &cfg)
        .await
        .expect("second pass");
    assert_eq!((p2, s2, n2), (0, p1, n1), "same period must dedup entirely");

    // Different period: fresh runs again (same shape as the first pass).
    let period_b = format!("test-period-B-{stamp}");
    let (p3, s3, _r3, n3, _u3) = enqueue_scheduled(&mut storage, &pool, &period_b, &cfg)
        .await
        .expect("third pass");
    assert_eq!((p3, s3, n3), (p1, 0, n1), "a new period schedules fresh");

    // Nodes without payloads are excluded from scheduling.
    let orphan = seed_node_only(&pool, "m3-orphan.example").await;
    let period_c = format!("test-period-C-{stamp}");
    let (p4, _, _r4, n4, _u4) = enqueue_scheduled(&mut storage, &pool, &period_c, &cfg)
        .await
        .expect("fourth pass");
    assert_eq!(n4, n1, "orphan without payload must not be scheduled");
    let _ = orphan;
    assert!(p4 >= 1, "payload-bearing nodes still schedule");

    sqlx::query(
        "DELETE FROM nodes WHERE server IN ('m3-a.example','m3-b.example','m3-orphan.example')",
    )
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
    let jobs_dsn = std::env::var("PHLOGO_TEST_JOBS_DSN").unwrap_or_else(|_| {
        eprintln!("skip: PHLOGO_TEST_JOBS_DSN not set (refusing to use DATABASE_URL as queue)");
        std::process::exit(0);
    });
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

// ---------------------------------------------------------------------------
// M3.1 fault-injection tests (ruling 2026-11-20): the four behaviors the previous
// report could not prove.
// ---------------------------------------------------------------------------

/// Fault #1: two workers started at DIFFERENT wall-clock moments must derive the SAME
/// logical period identity (aligned boundary), so the dedup constraint actually links
/// their passes.
#[test]
fn aligned_period_label_is_stable_across_start_times() {
    use worker::aligned_period_label;
    let t0 = chrono::DateTime::parse_from_rfc3339("2026-11-20T12:00:30Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let t1 = chrono::DateTime::parse_from_rfc3339("2026-11-20T12:31:30Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    // 30-min interval: 12:00:30 lands in the 12:00 slot, 12:31:30 in the 12:30 slot —
    // different slots, so the SIMULATED restart uses a start one minute after the
    // first worker's tick, still inside the same slot:
    let t_same_slot = chrono::DateTime::parse_from_rfc3339("2026-11-20T12:29:30Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let a = aligned_period_label(t0, 30);
    let b = aligned_period_label(t_same_slot, 30);
    assert_eq!(
        a, b,
        "same slot => same schedule identity regardless of start time"
    );
    let next = aligned_period_label(t1, 30);
    assert_ne!(a, next, "the next slot must get a fresh identity");
    // The label encodes the interval so two intervals can't collide on the same slot.
    assert!(a.ends_with("+30m"), "{a}");
    // Interval change => new identity even for the same wall-clock slot.
    assert_ne!(a, aligned_period_label(t0, 60));
    assert!(aligned_period_label(t1, 30) != a, "different slot differs");
    assert_eq!(
        aligned_period_label(t0, 30),
        aligned_period_label(t_same_slot, 30)
    );
}

/// Fault #2: a node whose ciphertext cannot be decrypted (wrong key) must be EXCLUDED
/// from scheduling, even though its node_payloads row exists.
#[tokio::test]
async fn undecryptable_payloads_are_excluded_from_scheduling() {
    use worker::{enqueue_scheduled, CheckScheduleConfig};
    let Some(pool) = setup().await else {
        return;
    };
    let Ok(jobs_dsn) = std::env::var("PHLOGO_TEST_JOBS_DSN") else {
        eprintln!("skip: PHLOGO_TEST_JOBS_DSN not set (refusing to use DATABASE_URL as queue)");
        return;
    };
    let jobs_pool = database::connect(&jobs_dsn).await.expect("jobs connect");
    let _ = apalis_sql::postgres::PostgresStorage::setup(&jobs_pool).await;
    let mut storage: apalis_sql::postgres::PostgresStorage<CheckJob> =
        apalis_sql::postgres::PostgresStorage::new(jobs_pool);

    // Node with a payload sealed under a DIFFERENT key (simulates key mismatch).
    let bad = Uuid::new_v4();
    let hash_part = Uuid::new_v4().simple().to_string();
    sqlx::query(
        "INSERT INTO nodes (id,identity_hash,fingerprint_version,protocol,server,port,\
         first_seen_at,last_seen_at,created_at,updated_at,membership,raw_config_reference) \
         VALUES ($1,$2,1,'trojan','m31-wrongkey.example',443,now(),now(),now(),now(),'Linked','')",
    )
    .bind(bad)
    .bind(format!("{hash_part}{hash_part}"))
    .execute(&pool)
    .await
    .expect("insert node");
    let wrong_key = payload_crypto::PayloadKey::from_bytes([42u8; 32]);
    let sealed = wrong_key
        .seal(
            &payload_crypto::aad_for(&bad.to_string()),
            br#"{"type":"trojan","server":"m31-wrongkey.example"}"#,
        )
        .expect("seal under wrong key");
    sqlx::query(
        "INSERT INTO node_payloads (node_id,key_version,nonce,ciphertext,updated_at) \
         VALUES ($1,$2,$3,$4,now())",
    )
    .bind(bad)
    .bind(sealed.key_version)
    .bind(sealed.nonce.to_vec())
    .bind(sealed.ciphertext)
    .execute(&pool)
    .await
    .expect("insert payload");

    let cfg = CheckScheduleConfig {
        interval_minutes: 30,
        batch_size: 10,
    };
    let stamp = Uuid::new_v4().simple().to_string();
    let (_, _, _, _, undecryptable) =
        enqueue_scheduled(&mut storage, &pool, &format!("m31-wrongkey-{stamp}"), &cfg)
            .await
            .expect("pass");
    assert!(
        undecryptable >= 1,
        "the wrong-key node must be counted as undecryptable"
    );
    let schedulable: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM nodes n JOIN node_payloads p ON p.node_id=n.id \
         WHERE n.server='m31-wrongkey.example')",
    )
    .fetch_one(&pool)
    .await
    .expect("row exists");
    assert!(schedulable, "precondition: the payload row exists");
    // But its id must NOT be inside any pushed batch: count pushes referencing it.
    let jobs_db = database::connect(&jobs_dsn).await.expect("jobs db");
    let leaked: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE payload LIKE '%m31-wrongkey%'")
            .fetch_one(&jobs_db)
            .await
            .unwrap_or(0);
    assert_eq!(leaked, 0, "undecryptable node must never be scheduled");

    sqlx::query("DELETE FROM nodes WHERE server='m31-wrongkey.example'")
        .execute(&pool)
        .await
        .ok();
}

/// Fault #3 (crash window): a run row claimed but never pushed (scheduler died between
/// the two steps) is recovered by the NEXT pass over the same period — exactly once.
#[tokio::test]
async fn lost_push_is_recovered_by_the_next_pass() {
    use worker::{claim_for_push, enqueue_scheduled, CheckScheduleConfig, ClaimOutcome};
    let Some(pool) = setup().await else {
        return;
    };
    let Ok(jobs_dsn) = std::env::var("PHLOGO_TEST_JOBS_DSN") else {
        eprintln!("skip: PHLOGO_TEST_JOBS_DSN not set (refusing to use DATABASE_URL as queue)");
        return;
    };
    let jobs_pool = database::connect(&jobs_dsn).await.expect("jobs connect");
    let _ = apalis_sql::postgres::PostgresStorage::setup(&jobs_pool).await;
    let mut storage: apalis_sql::postgres::PostgresStorage<CheckJob> =
        apalis_sql::postgres::PostgresStorage::new(jobs_pool);

    seed_node(&pool, "m31-recover.example", "pw").await;
    let node_id: Uuid =
        sqlx::query_scalar("SELECT id FROM nodes WHERE server='m31-recover.example'")
            .fetch_one(&pool)
            .await
            .expect("node id");

    // Simulate the crash: claim the run row, then DON'T push.
    let schedule_id = uuid::Uuid::new_v4();
    let job = CheckJob {
        run_id: Uuid::new_v4(),
        schedule_id,
        checker_tag: "default".into(),
        node_ids: vec![node_id],
        presets: vec!["TEST_PING_RTT".into()],
    };
    let claim = claim_for_push(&pool, &job).await.expect("claim");
    assert_eq!(claim, ClaimOutcome::New);

    // A racing scheduler claiming the same run sees LostBeforePush (it was never
    // pushed and is still 'started') — this is the recovery path.
    let claim2 = claim_for_push(&pool, &job).await.expect("claim 2");
    assert_eq!(
        claim2,
        ClaimOutcome::LostBeforePush,
        "unpushed claim must be recoverable"
    );
    // But once one claim wins, the next sees AlreadyClaimed (exactly-once re-push).
    let claim3 = claim_for_push(&pool, &job).await.expect("claim 3");
    assert_eq!(claim3, ClaimOutcome::AlreadyClaimed);

    // End-to-end through enqueue_scheduled: pass 1 pushes everything; then we reset
    // batch 0's row to unpushed (simulating a scheduler that crashed after row insert
    // but before push); pass 2 must re-push exactly that batch and skip the rest.
    let cfg = CheckScheduleConfig {
        interval_minutes: 30,
        batch_size: 10,
    };
    let stamp = Uuid::new_v4().simple().to_string();
    let period = format!("m31-recover-{stamp}");

    let (p1, s1, r1, n1, _u1) = enqueue_scheduled(&mut storage, &pool, &period, &cfg)
        .await
        .expect("first pass");
    assert!(
        p1 >= 1 && s1 == 0 && r1 == 0,
        "fresh period pushes all: {p1}/{s1}/{r1}"
    );

    // Crash simulation: batch 0 (the deterministic run id) was claimed but not pushed.
    let sid = uuid::Uuid::new_v5(
        &uuid::Uuid::from_bytes(*b"phlogo-check-sch"),
        period.as_bytes(),
    );
    let mut v = sid.as_bytes().to_vec();
    v.extend_from_slice(&0usize.to_le_bytes());
    let run0 = uuid::Uuid::new_v5(&uuid::Uuid::from_bytes(*b"phlogo-check-sch"), &v);
    let reset = sqlx::query("UPDATE check_runs SET pushed=false, status='started' WHERE id=$1")
        .bind(run0)
        .execute(&pool)
        .await
        .expect("reset batch 0")
        .rows_affected();
    assert_eq!(reset, 1, "batch 0 must exist from pass 1");

    let (p2, s2, r2, n2, _u2) = enqueue_scheduled(&mut storage, &pool, &period, &cfg)
        .await
        .expect("recovery pass");
    assert_eq!(r2, 1, "exactly the crashed batch is re-pushed");
    assert_eq!(p2, 0, "nothing else is pushed fresh");
    assert_eq!(s2, n1 - 1, "all healthy batches are skipped");
    assert_eq!(n2, n1, "node set unchanged");

    sqlx::query("DELETE FROM nodes WHERE server='m31-recover.example'")
        .execute(&pool)
        .await
        .ok();
}

/// Fault #4: the Apalis handler surfaces run_check_once errors as `Err` so the retry
/// layer actually retries (previously the error was swallowed into a log line).
#[tokio::test]
async fn handler_returns_err_on_failure() {
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
    // handle_check is the same function Apalis invokes; its Err is what the retry
    // layer sees. Also assert the business row is 'failed'.
    let result = worker::handle_check(job.clone(), apalis::prelude::Data::new(pool.clone())).await;
    assert!(
        result.is_err(),
        "handler must propagate the error for retry"
    );
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
