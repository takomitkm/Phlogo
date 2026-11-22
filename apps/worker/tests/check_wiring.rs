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
    mock_server_counting(latency_ms, Duration::ZERO).await.0
}

/// Mock MiaoSpeed that COUNTS submitted requests and can hold each one open for `delay`
/// before answering (M3.1-3: a second executor needs a real window to try claiming while
/// the first is inside the check).
async fn mock_server_counting(
    latency_ms: i64,
    delay: Duration,
) -> (SocketAddr, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let submits = Arc::new(AtomicUsize::new(0));
    let counter = submits.clone();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let counter = counter.clone();
            tokio::spawn(async move {
                let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                while let Some(Ok(msg)) = ws.next().await {
                    let Message::Text(text) = msg else { continue };
                    let Ok(req) = serde_json::from_str::<Value>(&text) else {
                        continue;
                    };
                    // One submit request reached the checker — this is what must never
                    // happen twice for a single run_id.
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
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
    (addr, submits)
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
    let (p1, s1, n1, _u1) = enqueue_scheduled(&mut storage, &pool, &period_a, &cfg)
        .await
        .expect("first pass");
    assert!(n1 >= 2, "our two seeded nodes must be schedulable: n={n1}");
    assert_eq!(p1, n1, "batch_size=1: one batch per node, all fresh");
    assert_eq!(s1, 0, "fresh period skips nothing");

    // Same period again: dedup — everything skipped (push_state='enqueued' is Busy).
    let (p2, s2, n2, _u2) = enqueue_scheduled(&mut storage, &pool, &period_a, &cfg)
        .await
        .expect("second pass");
    assert_eq!((p2, s2, n2), (0, p1, n1), "same period must dedup entirely");

    // Different period: fresh runs again (same shape as the first pass).
    let period_b = format!("test-period-B-{stamp}");
    let (p3, s3, n3, _u3) = enqueue_scheduled(&mut storage, &pool, &period_b, &cfg)
        .await
        .expect("third pass");
    assert_eq!((p3, s3, n3), (p1, 0, n1), "a new period schedules fresh");

    // Nodes without payloads are excluded from scheduling.
    let orphan = seed_node_only(&pool, "m3-orphan.example").await;
    let period_c = format!("test-period-C-{stamp}");
    let (p4, _, n4, _u4) = enqueue_scheduled(&mut storage, &pool, &period_c, &cfg)
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

/// M3 stage 4: the manual trigger service enqueues with explicit labels and honours an
/// explicit node list (API-level auth is covered by the curse tests). Same key =
/// same logical run (recovered, not duplicated) — see the idempotency test below.
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

    // Unique server per run: a leftover row from an earlier panic would otherwise be
    // picked up by a `WHERE server=...` lookup, and its payload was sealed by that
    // process's ephemeral key — i.e. undecryptable here, so the fixture would silently
    // filter itself out and the trigger would push nothing.
    let server = format!("m3-manual-{}.example", Uuid::new_v4().simple());
    let id = seed_node(&pool, &server, "pw").await;

    let key1 = format!("m3-manual-{}", Uuid::new_v4().simple());
    let (p, s, n) = trigger_check_batch(&jobs_dsn, &pool, Some(vec![id]), 10, &key1)
        .await
        .expect("trigger");
    assert_eq!((p, n), (1, 1), "one explicit node, one batch");
    assert_eq!(s, 0);

    // A DIFFERENT key = a different logical run (pushes again).
    let key2 = format!("m3-manual-{}", Uuid::new_v4().simple());
    let (p2, _, _) = trigger_check_batch(&jobs_dsn, &pool, Some(vec![id]), 10, &key2)
        .await
        .expect("trigger 2");
    assert_eq!(p2, 1, "each new key is its own logical run");

    // Unknown ids are FILTERED OUT before batching (no ghost schedules).
    let key3 = format!("m3-manual-{}", Uuid::new_v4().simple());
    let (p3, _, n3) = trigger_check_batch(&jobs_dsn, &pool, Some(vec![Uuid::new_v4()]), 10, &key3)
        .await
        .expect("trigger 3");
    assert_eq!(p3, 0, "unknown node must not schedule");
    assert_eq!(n3, 0, "unknown node is filtered from the list entirely");

    sqlx::query("DELETE FROM nodes WHERE server=$1")
        .bind(&server)
        .execute(&pool)
        .await
        .ok();
}

// ---------------------------------------------------------------------------
// M3.1-2 fault-injection tests (ruling 2026-11-20): the seven behaviors demanded.
// All use the disposable PHLOGO_TEST_JOBS_DSN (skip without it; never DATABASE_URL).
// ---------------------------------------------------------------------------

fn require_jobs_dsn() -> Option<String> {
    match std::env::var("PHLOGO_TEST_JOBS_DSN") {
        Ok(d) => Some(d),
        Err(_) => {
            eprintln!("skip: PHLOGO_TEST_JOBS_DSN not set (refusing to use DATABASE_URL as queue)");
            None
        }
    }
}

/// Read pushed Apalis job payloads and deserialize their CheckJob (ruling: assert on
/// the real payload, never on string guesses). apalis-sql 0.7.4 stores tasks in the
/// `apalis.jobs` table, `job JSONB`, `status` in Pending/Running/Done/... — read all.
async fn pushed_jobs(jobs_dsn: &str) -> Vec<CheckJob> {
    let db = database::connect(jobs_dsn).await.expect("jobs db");
    let rows: Vec<Value> = sqlx::query_scalar("SELECT job FROM apalis.jobs")
        .fetch_all(&db)
        .await
        .expect("jobs");
    rows.into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect()
}

/// Fault #1 (timer identity): workers started at DIFFERENT wall-clock moments share
/// one boundary sequence — next_boundary_after is anchored to UTC slots, and the
/// planned label of a tick equals the slot the tick covers.
#[test]
fn boundary_timer_shares_identity_across_start_times() {
    use worker::{aligned_period_label, next_boundary_after};
    let mk = |s: &str| {
        chrono::DateTime::parse_from_rfc3339(s)
            .unwrap()
            .with_timezone(&chrono::Utc)
    };
    // Worker A starts 12:29, ticks at the first boundary >= 12:59 => 13:00 slot.
    let (a_at, a_label) = next_boundary_after(mk("2026-11-20T12:59:00Z"), 30);
    // Worker B starts 12:31, ticks at the first boundary >= 13:01 => 13:30 slot —
    // DIFFERENT slot, but crucially both land ON a boundary and the LABEL of A's tick
    // is exactly that slot's label (not "now formatted at execution time").
    let (b_at, b_label) = next_boundary_after(mk("2026-11-20T13:01:00Z"), 30);
    assert_eq!(
        a_label, "period-20261120T1300+30m",
        "A's planned slot label"
    );
    assert_eq!(
        b_label, "period-20261120T1330+30m",
        "B's planned slot label"
    );
    // Both timers sit exactly on UTC boundaries (second == 0).
    assert_eq!(a_at.timestamp() % 1800, 0, "A anchored to a boundary");
    assert_eq!(b_at.timestamp() % 1800, 0, "B anchored to a boundary");
    // Restart within the same slot: first boundary >= startup+interval is the SAME
    // slot as an already-running worker's next tick, so no duplicate slot execution.
    let (r_at, r_label) = next_boundary_after(mk("2026-11-20T12:31:00Z"), 30);
    assert_eq!(r_label, "period-20261120T1300+30m");
    assert_eq!(
        r_at.timestamp(),
        a_at.timestamp(),
        "restart aligns to the same tick"
    );
    // A delayed execution still uses the PLANNED label: the label for 13:00 stays
    // 13:00 even when evaluated at 13:04.
    assert_eq!(
        aligned_period_label(mk("2026-11-20T13:04:00Z"), 30),
        "period-20261120T1300+30m",
        "label is slot-derived, not execution-time-derived"
    );
}

/// Fault #2 (concurrent claim): N schedulers race for the same batch through
/// independent connections — exactly one wins the lease, the rest see Busy.
#[tokio::test]
async fn concurrent_claims_yield_exactly_one_winner() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use worker::{claim_for_push, CheckJob};
    // Main-DB setup (migrations) only; racers use their own connections below.
    let _pool = setup().await;
    let Some(_jobs_dsn) = require_jobs_dsn() else {
        return;
    };
    let _ = _jobs_dsn;

    let job = CheckJob {
        run_id: Uuid::new_v4(),
        schedule_id: Uuid::new_v4(),
        checker_tag: "default".into(),
        node_ids: vec![Uuid::new_v4()],
        presets: vec!["TEST_PING_RTT".into()],
    };

    const WORKERS: usize = 8;
    let winners = Arc::new(AtomicUsize::new(0));
    let busy = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(tokio::sync::Barrier::new(WORKERS));
    let mut handles = Vec::with_capacity(WORKERS);
    for _ in 0..WORKERS {
        let job = job.clone();
        let winners = winners.clone();
        let busy = busy.clone();
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            // Each racer gets its OWN connection (real concurrency, not serial calls).
            let racer = database::connect(&std::env::var("DATABASE_URL").unwrap())
                .await
                .expect("racer pool");
            barrier.wait().await;
            let r = claim_for_push(&racer, &job).await.expect("claim");
            match r {
                Some(_) => winners.fetch_add(1, Ordering::SeqCst),
                None => busy.fetch_add(1, Ordering::SeqCst),
            };
        }));
    }
    for h in handles {
        h.await.expect("task");
    }
    assert_eq!(
        winners.load(Ordering::SeqCst),
        1,
        "exactly one lease holder"
    );
    assert_eq!(
        busy.load(Ordering::SeqCst),
        WORKERS - 1,
        "everyone else is Busy"
    );
}

/// Fault #2b (recovery race): two schedulers recover the SAME expired lease through
/// independent connections — exactly one re-claim wins.
#[tokio::test]
async fn concurrent_recovery_of_expired_lease_has_one_winner() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use worker::{claim_for_push, CheckJob};
    let Some(pool) = setup().await else {
        return;
    };
    let Some(_jobs_dsn) = require_jobs_dsn() else {
        return;
    };
    let _ = _jobs_dsn;

    let job = CheckJob {
        run_id: Uuid::new_v4(),
        schedule_id: Uuid::new_v4(),
        checker_tag: "default".into(),
        node_ids: vec![Uuid::new_v4()],
        presets: vec!["TEST_PING_RTT".into()],
    };
    // First claim takes the lease...
    assert!(claim_for_push(&pool, &job).await.expect("claim").is_some());
    // ...then the holder "crashes": backdate the lease so it is expired.
    sqlx::query("UPDATE check_runs SET lease_until = now() - interval '5 seconds' WHERE id=$1")
        .bind(job.run_id)
        .execute(&pool)
        .await
        .expect("expire lease");

    const RECOVERERS: usize = 6;
    let winners = Arc::new(AtomicUsize::new(0));
    let busy = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(tokio::sync::Barrier::new(RECOVERERS));
    let mut handles = Vec::with_capacity(RECOVERERS);
    for _ in 0..RECOVERERS {
        let job = job.clone();
        let winners = winners.clone();
        let busy = busy.clone();
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            let racer = database::connect(&std::env::var("DATABASE_URL").unwrap())
                .await
                .expect("racer pool");
            barrier.wait().await;
            match claim_for_push(&racer, &job).await.expect("claim") {
                Some(_) => winners.fetch_add(1, Ordering::SeqCst),
                None => busy.fetch_add(1, Ordering::SeqCst),
            };
        }));
    }
    for h in handles {
        h.await.expect("task");
    }
    assert_eq!(winners.load(Ordering::SeqCst), 1, "one recovery winner");
    assert_eq!(busy.load(Ordering::SeqCst), RECOVERERS - 1);
}

/// Fault #3: a run that ALREADY SUCCEEDED is never re-executed on redelivery (the
/// at-least-once delivery double-push case) — the execution-side gate skips it.
#[tokio::test]
async fn succeeded_run_is_not_reexecuted_on_redelivery() {
    let Some(pool) = setup().await else {
        return;
    };
    let _guard = ENV_LOCK.lock().await;
    let addr = mock_server(37).await;
    std::env::set_var("PHLOGO_CHECKER_WS_URL", format!("ws://{addr}"));
    std::env::set_var("PHLOGO_CHECKER_SECRET", "test-secret");

    let a = seed_node(&pool, "m31-once.example", "pw").await;
    let job = CheckJob {
        run_id: Uuid::new_v4(),
        schedule_id: Uuid::new_v4(),
        checker_tag: "default".into(),
        node_ids: vec![a],
        presets: vec!["TEST_PING_RTT".into()],
    };
    run_check_once(&pool, &job).await.expect("first run");
    let rows1: i64 =
        sqlx::query_scalar("SELECT count(*) FROM node_check_results WHERE check_run_id=$1")
            .bind(job.run_id)
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(rows1, 1);

    // Simulate the double-push: the SAME run delivered again after success.
    run_check_once(&pool, &job)
        .await
        .expect("redelivery is Ok (skipped)");
    let rows2: i64 =
        sqlx::query_scalar("SELECT count(*) FROM node_check_results WHERE check_run_id=$1")
            .bind(job.run_id)
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(
        rows2, 1,
        "a succeeded run must not execute (or persist) twice"
    );

    sqlx::query("DELETE FROM nodes WHERE server='m31-once.example'")
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM check_runs WHERE id=$1")
        .bind(job.run_id)
        .execute(&pool)
        .await
        .ok();
}

/// Fault #4: state-transition DB errors are NOT swallowed — claim/enqueue surfaces
/// them to the scheduling pass.
#[tokio::test]
async fn state_transition_errors_propagate() {
    use worker::{claim_for_push, mark_enqueued_for_test, CheckJob};
    let Some(pool) = setup().await else {
        return;
    };
    let job = CheckJob {
        run_id: Uuid::new_v4(),
        schedule_id: Uuid::new_v4(),
        checker_tag: "default".into(),
        node_ids: vec![Uuid::new_v4()],
        presets: vec![],
    };
    // Happy path first: claim + mark_enqueued succeed.
    let claim = claim_for_push(&pool, &job)
        .await
        .expect("claim")
        .expect("won");
    mark_enqueued_for_test(&pool, job.run_id, claim.owner)
        .await
        .expect("mark enqueued");
    let state: String = sqlx::query_scalar("SELECT push_state FROM check_runs WHERE id=$1")
        .bind(job.run_id)
        .fetch_one(&pool)
        .await
        .expect("row");
    assert_eq!(state, "enqueued");

    // A STALE owner (previous lease holder) cannot flip the state afterwards.
    let stale_owner = Uuid::new_v4();
    let applied = mark_enqueued_for_test(&pool, job.run_id, stale_owner)
        .await
        .expect("the guarded UPDATE must not error");
    assert!(
        !applied,
        "a stale owner's mark is a reported no-op (0 rows), not an error and not applied"
    );
    let state2: String = sqlx::query_scalar("SELECT push_state FROM check_runs WHERE id=$1")
        .bind(job.run_id)
        .fetch_one(&pool)
        .await
        .expect("row");
    assert_eq!(
        state2, "enqueued",
        "stale owner must not overwrite the state"
    );
}

/// Fault #5: the manual trigger with the SAME idempotency key recovers lost batches
/// instead of creating a second logical run.
#[tokio::test]
async fn manual_trigger_idempotency_key_recovers() {
    use worker::{schedule_id_for_label, scheduled_run_id, trigger_check_batch};
    let Some(pool) = setup().await else {
        return;
    };
    let Some(jobs_dsn) = require_jobs_dsn() else {
        return;
    };
    let jobs_pool = database::connect(&jobs_dsn).await.expect("jobs connect");
    let _ = apalis_sql::postgres::PostgresStorage::setup(&jobs_pool).await;
    let mut storage: apalis_sql::postgres::PostgresStorage<CheckJob> =
        apalis_sql::postgres::PostgresStorage::new(jobs_pool);
    let _ = &mut storage;

    // Unique server per run (see manual_trigger_enqueues_explicit_nodes): a leftover
    // row sealed by a previous process's ephemeral key would be filtered out as
    // undecryptable and this trigger would report (0,0,0).
    let server = format!("m31-idem-{}.example", Uuid::new_v4().simple());
    let id = seed_node(&pool, &server, "pw").await;

    let key = format!("idem-{}", Uuid::new_v4().simple());
    let (p1, s1, n1) = trigger_check_batch(&jobs_dsn, &pool, Some(vec![id]), 10, &key)
        .await
        .expect("trigger 1");
    assert_eq!(
        (p1, s1, n1),
        (1, 0, 1),
        "first trigger with the key pushes one batch"
    );

    // Re-send with the SAME key: the run row is already 'enqueued' → Busy → skip.
    // No second logical run is created.
    let (p2, s2, n2) = trigger_check_batch(&jobs_dsn, &pool, Some(vec![id]), 10, &key)
        .await
        .expect("trigger 2");
    assert_eq!(
        (p2, s2, n2),
        (0, 1, 1),
        "same key is recovered/deduped, not duplicated"
    );

    // Crash recovery: reset the run row to an expired dispatching lease, then re-send
    // the same key — the lost batch is re-pushed by THIS trigger.
    //
    // Derive the id through the PRODUCTION functions: the label is the idempotency key
    // used verbatim, and the run id is v5(NS, schedule_id ‖ batch). Re-implementing the
    // recipe here is what broke this test before (a stray `manual-` prefix meant the
    // UPDATE below hit no row and the recovery assertion silently tested nothing).
    let schedule_id = schedule_id_for_label(&key);
    let run0 = scheduled_run_id(schedule_id, 0);
    sqlx::query(
        "UPDATE check_runs SET push_state='dispatching', lease_until = now() - interval '5 seconds' \
         WHERE id=$1",
    )
    .bind(run0)
    .execute(&pool)
    .await
    .expect("expire lease");
    let (p3, s3, n3) = trigger_check_batch(&jobs_dsn, &pool, Some(vec![id]), 10, &key)
        .await
        .expect("trigger 3");
    assert_eq!(
        (p3, s3, n3),
        (1, 0, 1),
        "the expired lease is recovered by the same key"
    );

    sqlx::query("DELETE FROM nodes WHERE server=$1")
        .bind(&server)
        .execute(&pool)
        .await
        .ok();
}

/// Fault #6 (test-evidence fix): the wrong-key node's UUID must be absent from every
/// pushed CheckJob payload — checked by PARSING the payload, not string guessing. A
/// healthy node IS pushed, so the assertion cannot pass vacuously.
#[tokio::test]
async fn undecryptable_payloads_are_excluded_from_scheduling() {
    use worker::{enqueue_scheduled, CheckScheduleConfig};
    let Some(pool) = setup().await else {
        return;
    };
    let Some(jobs_dsn) = require_jobs_dsn() else {
        return;
    };
    let jobs_pool = database::connect(&jobs_dsn).await.expect("jobs connect");
    let _ = apalis_sql::postgres::PostgresStorage::setup(&jobs_pool).await;
    let mut storage: apalis_sql::postgres::PostgresStorage<CheckJob> =
        apalis_sql::postgres::PostgresStorage::new(jobs_pool);

    // A healthy node (must actually be pushed) and a wrong-key node (must not be).
    // Unique server names per run: a leftover fixture from an earlier panic was sealed
    // by that process's ephemeral key, so it would land in `undecryptable` as well and
    // make the counters unattributable to this run's fixtures.
    let fixture = Uuid::new_v4().simple().to_string();
    let healthy_server = format!("m31-healthy-{fixture}.example");
    let bad_server = format!("m31-wrongkey-{fixture}.example");
    let healthy = seed_node(&pool, &healthy_server, "pw").await;
    let bad = Uuid::new_v4();
    let hash_part = Uuid::new_v4().simple().to_string();
    sqlx::query(
        "INSERT INTO nodes (id,identity_hash,fingerprint_version,protocol,server,port,\
         first_seen_at,last_seen_at,created_at,updated_at,membership,raw_config_reference) \
         VALUES ($1,$2,1,'trojan',$3,443,now(),now(),now(),now(),'Linked','')",
    )
    .bind(bad)
    .bind(format!("{hash_part}{hash_part}"))
    .bind(&bad_server)
    .execute(&pool)
    .await
    .expect("insert node");
    let wrong_key = payload_crypto::PayloadKey::from_bytes([42u8; 32]);
    let sealed = wrong_key
        .seal(
            &payload_crypto::aad_for(&bad.to_string()),
            format!(r#"{{"type":"trojan","server":"{bad_server}"}}"#).as_bytes(),
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
    let (pushed, _skipped, total, undecryptable) =
        enqueue_scheduled(&mut storage, &pool, &format!("m31-wrongkey-{stamp}"), &cfg)
            .await
            .expect("pass");
    assert!(undecryptable >= 1, "the wrong-key node must be counted");
    // `total` counts POST-filter candidates (schedulable only), so it can never include
    // the wrong-key node — the healthy fixture being present is what proves the pass
    // had a real candidate instead of "nothing was schedulable".
    assert!(
        total >= 1,
        "the healthy fixture must be a schedulable candidate"
    );

    // THE REAL ASSERTION: parse every pushed job payload and check node_ids.
    let jobs = pushed_jobs(&jobs_dsn).await;
    let healthy_pushed = jobs.iter().any(|j| j.node_ids.contains(&healthy));
    assert!(
        healthy_pushed,
        "the healthy node MUST be pushed (no vacuous pass)"
    );
    let bad_pushed = jobs.iter().any(|j| j.node_ids.contains(&bad));
    assert!(
        !bad_pushed,
        "the undecryptable node must never appear in any job payload"
    );
    let _ = pushed;

    sqlx::query("DELETE FROM nodes WHERE server IN ($1,$2)")
        .bind(&healthy_server)
        .bind(&bad_server)
        .execute(&pool)
        .await
        .ok();
}

/// Fault #7: the Apalis handler surfaces run_check_once errors as `Err` so the retry
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

/// Fault #7b (ruling item 5.6): the RetryLayer REALLY redelivers a failing job.
///
/// The test above only proves the handler returns `Err`; this one drives the real
/// queue: a job is pushed, a real `Monitor` runs it with `RetryPolicy::retries(3)`, and
/// we assert the actual execution count (1 initial + 3 retries = 4) plus the terminal
/// job status the queue records.
///
/// Its own job type isolates it: `PostgresStorage<T>` namespaces rows by
/// `type_name::<T>()`, so it can neither pick up nor disturb the `CheckJob` rows the
/// other tests push into the same disposable queue.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct RetryProbe {
    id: Uuid,
}

#[derive(Clone, Default)]
struct AttemptCounter(std::sync::Arc<std::sync::atomic::AtomicUsize>);

async fn always_fails(
    _job: RetryProbe,
    attempts: apalis::prelude::Data<AttemptCounter>,
) -> Result<(), anyhow::Error> {
    attempts.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    Err(anyhow::anyhow!(
        "transient failure (retry integration test)"
    ))
}

#[tokio::test]
async fn retry_layer_redelivers_until_exhausted() {
    use std::sync::atomic::Ordering;

    use apalis::layers::WorkerBuilderExt as _;
    use apalis::prelude::{Monitor, Storage as _, WorkerBuilder, WorkerFactoryFn};
    use apalis_sql::postgres::PostgresStorage;

    let Some(jobs_dsn) = require_jobs_dsn() else {
        return;
    };
    let jobs_pool = database::connect(&jobs_dsn).await.expect("jobs connect");
    let job_type = std::any::type_name::<RetryProbe>();
    // `setup` lives on `PostgresStorage<()>` (the schema bootstrap), not on a typed one.
    let _ = apalis_sql::postgres::PostgresStorage::setup(&jobs_pool).await;
    // A leftover probe row from an earlier run would be executed by this monitor too.
    sqlx::query("DELETE FROM apalis.jobs WHERE job_type=$1")
        .bind(job_type)
        .execute(&jobs_pool)
        .await
        .ok();

    let probe = RetryProbe { id: Uuid::new_v4() };
    let mut storage = PostgresStorage::<RetryProbe>::new(jobs_pool.clone());
    storage.push(probe.clone()).await.expect("push probe");

    let attempts = AttemptCounter::default();
    let worker = WorkerBuilder::new("retry-layer-integration-test")
        .retry(apalis::layers::retry::RetryPolicy::retries(3))
        .data(attempts.clone())
        .backend(PostgresStorage::<RetryProbe>::new(jobs_pool.clone()))
        .build_fn(always_fails);
    let monitor = tokio::spawn(Monitor::new().register(worker).run());

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while attempts.0.load(Ordering::SeqCst) < 4 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Let the queue write the terminal state after the last attempt.
    for _ in 0..100 {
        let st: Option<String> =
            sqlx::query_scalar("SELECT status FROM apalis.jobs WHERE job_type=$1 LIMIT 1")
                .bind(job_type)
                .fetch_optional(&jobs_pool)
                .await
                .expect("status query");
        if attempts.0.load(Ordering::SeqCst) >= 4 && st.as_deref() != Some("Running") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let ran = attempts.0.load(Ordering::SeqCst);
    // Queue-side record: `attempts` is incremented by the storage on every delivery, and
    // `status = 'Killed'` is the terminal state apalis writes when the retry policy gives
    // up (`RetryPolicyError::OutOfRetries` → kill, see apalis-sql `kill`); a `Killed` job
    // is never fetched again, so the retries are bounded.
    let (status, delivered): (Option<String>, Option<i32>) =
        sqlx::query_as("SELECT status, attempts FROM apalis.jobs WHERE job_type=$1 LIMIT 1")
            .bind(job_type)
            .fetch_optional(&jobs_pool)
            .await
            .expect("status query")
            .map_or((None, None), |(s, a): (String, i32)| (Some(s), Some(a)));
    monitor.abort();

    assert_eq!(
        ran, 4,
        "retries(3) must re-execute 1 initial + 3 retries; the handler ran {ran} times"
    );
    assert_eq!(
        delivered,
        Some(4),
        "the queue must record the same 4 deliveries as the handler saw"
    );
    assert_eq!(
        status.as_deref(),
        Some("Killed"),
        "exhausting the retry policy must leave a terminal (never re-fetched) job"
    );

    sqlx::query("DELETE FROM apalis.jobs WHERE job_type=$1")
        .bind(job_type)
        .execute(&jobs_pool)
        .await
        .ok();
}

/// Fault #8a: the execution claim is a single atomic statement — N concurrent callers on
/// one run row yield exactly one winner. Deterministic (no mock-timing involved), so it
/// pins the property the previous `SELECT status` gate did not have.
#[tokio::test]
async fn exec_claim_is_exclusive() {
    use worker::{claim_for_exec, record_check_run};

    let Some(pool) = setup().await else {
        return;
    };
    let node = Uuid::new_v4();
    let run_id = Uuid::new_v4();
    let job = CheckJob {
        run_id,
        schedule_id: Uuid::new_v4(),
        checker_tag: "default".into(),
        node_ids: vec![node],
        presets: vec![],
    };
    record_check_run(&pool, &job).await.expect("record run");

    let dsn = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let mut handles = Vec::new();
    for _ in 0..6 {
        let dsn = dsn.clone();
        handles.push(tokio::spawn(async move {
            let pool = database::connect(&dsn).await.expect("pool");
            claim_for_exec(&pool, run_id).await.expect("claim")
        }));
    }
    let mut winners = 0;
    let mut winner: Option<Uuid> = None;
    for h in handles {
        if let Some(owner) = h.await.expect("join") {
            winners += 1;
            winner = Some(owner);
        }
    }
    assert_eq!(
        winners, 1,
        "exactly one concurrent caller may own the execution right"
    );

    let stored: Option<Uuid> = sqlx::query_scalar("SELECT exec_owner FROM check_runs WHERE id=$1")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .expect("owner");
    assert_eq!(
        stored, winner,
        "the row must record the winner's lease owner"
    );

    sqlx::query("DELETE FROM check_runs WHERE id=$1")
        .bind(run_id)
        .execute(&pool)
        .await
        .ok();
}

/// Fault #8 (ruling 2026-11-22): the EXECUTION right is atomic, not a read.
///
/// Two deliveries of the SAME `run_id` are executed simultaneously (independent
/// connection pools + a barrier, so they really overlap). The mock MiaoSpeed holds the
/// first request open, which is exactly the window in which the old `SELECT status` gate
/// let both executors in. Assertion: the checker receives **exactly one** submit, the
/// loser is a silent no-op (`Ok`), the run ends `success` once, and the lease is released.
#[tokio::test]
async fn concurrent_executions_of_one_run_id_hit_miaospeed_once() {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    let Some(pool) = setup().await else {
        return;
    };
    let _guard = ENV_LOCK.lock().await;

    // 400 ms of "inside MiaoSpeed" is a generous window for the second executor.
    let (addr, submits) = mock_server_counting(37, Duration::from_millis(400)).await;
    std::env::set_var("PHLOGO_CHECKER_WS_URL", format!("ws://{addr}"));
    std::env::set_var("PHLOGO_CHECKER_SECRET", "test-secret");
    std::env::remove_var("PHLOGO_CHECKER_VENDOR");

    let server = format!("m31-exec-{}.example", Uuid::new_v4().simple());
    let node = seed_node(&pool, &server, "pw").await;
    let run_id = Uuid::new_v4();
    let job = CheckJob {
        run_id,
        schedule_id: Uuid::new_v4(),
        checker_tag: "default".into(),
        node_ids: vec![node],
        presets: vec!["TEST_PING_RTT".into()],
    };

    // A second, independent pool: two workers never share one connection here either.
    let dsn = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let pool_b = database::connect(&dsn).await.expect("second pool");
    let pool_a = pool.clone();
    let job_a = job.clone();
    let job_b = job.clone();
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let (ba, bb) = (barrier.clone(), barrier.clone());

    let a = tokio::spawn(async move {
        ba.wait().await;
        run_check_once(&pool_a, &job_a).await
    });
    let b = tokio::spawn(async move {
        bb.wait().await;
        run_check_once(&pool_b, &job_b).await
    });
    let (ra, rb) = tokio::join!(a, b);
    let ra = ra.expect("join a");
    let rb = rb.expect("join b");

    assert!(ra.is_ok(), "one executor must complete the run: {ra:?}");
    assert!(
        rb.is_ok(),
        "the delivery without the execution right must be a no-op, not an error: {rb:?}"
    );
    assert_eq!(
        submits.load(Ordering::SeqCst),
        1,
        "one run_id must reach MiaoSpeed exactly once, however many deliveries it gets"
    );

    let (status, results, owner, lease): (String, i64, Option<Uuid>, Option<String>) =
        sqlx::query_as(
            "SELECT r.status, \
                    (SELECT count(*) FROM node_check_results c WHERE c.check_run_id=r.id), \
                    r.exec_owner, \
                    r.exec_lease_until::text \
               FROM check_runs r WHERE r.id=$1",
        )
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .expect("run row");
    assert_eq!(
        status, "success",
        "the winning executor must finish the run"
    );
    assert_eq!(results, 1, "results are persisted exactly once");
    assert!(
        owner.is_none(),
        "the lease owner must be cleared: {owner:?}"
    );
    assert!(
        lease.is_none(),
        "the lease expiry must be cleared: {lease:?}"
    );

    sqlx::query("DELETE FROM node_status WHERE node_id=$1")
        .bind(node)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM nodes WHERE server=$1")
        .bind(&server)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM check_runs WHERE id=$1")
        .bind(run_id)
        .execute(&pool)
        .await
        .ok();
}

/// Fault #9 (M3.1-3 follow-up): a HUNG checker is bounded.
///
/// The adapter has no built-in timeout, so without a cap a stuck check could outlive the
/// execution lease — and then a second delivery could claim the run while the first was
/// still inside MiaoSpeed, i.e. mutual exclusion would only hold inside the lease window.
/// Here the mock accepts the request and never answers; the run must fail on the timeout
/// and the lease must be released so the Apalis retry can take it.
#[tokio::test]
async fn hung_checker_times_out_and_releases_the_lease() {
    use std::sync::atomic::Ordering;

    let Some(pool) = setup().await else {
        return;
    };
    let _guard = ENV_LOCK.lock().await;

    // Answers only after 30 s; the cap below is 1 s.
    let (addr, submits) = mock_server_counting(37, Duration::from_secs(30)).await;
    std::env::set_var("PHLOGO_CHECKER_WS_URL", format!("ws://{addr}"));
    std::env::set_var("PHLOGO_CHECKER_SECRET", "test-secret");
    std::env::set_var("PHLOGO_CHECK_TIMEOUT_SECS", "1");

    let server = format!("m31-hang-{}.example", Uuid::new_v4().simple());
    let node = seed_node(&pool, &server, "pw").await;
    let run_id = Uuid::new_v4();
    let job = CheckJob {
        run_id,
        schedule_id: Uuid::new_v4(),
        checker_tag: "default".into(),
        node_ids: vec![node],
        presets: vec!["TEST_PING_RTT".into()],
    };

    let started = tokio::time::Instant::now();
    let res = run_check_once(&pool, &job).await;
    let elapsed = started.elapsed();
    std::env::remove_var("PHLOGO_CHECK_TIMEOUT_SECS");

    assert!(
        res.is_err(),
        "a hung checker must surface as an error, not a silent success"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "the run must be bounded by the timeout, took {elapsed:?}"
    );
    assert_eq!(
        submits.load(Ordering::SeqCst),
        1,
        "exactly one submit before the timeout fired"
    );

    let (status, owner, lease): (String, Option<Uuid>, Option<String>) = sqlx::query_as(
        "SELECT status, exec_owner, exec_lease_until::text FROM check_runs WHERE id=$1",
    )
    .bind(run_id)
    .fetch_one(&pool)
    .await
    .expect("run row");
    assert_eq!(status, "failed", "a timed-out run is a failed run");
    assert!(
        owner.is_none() && lease.is_none(),
        "the execution lease must be released so a retry can claim it: {owner:?} {lease:?}"
    );

    sqlx::query("DELETE FROM node_status WHERE node_id=$1")
        .bind(node)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM nodes WHERE server=$1")
        .bind(&server)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM check_runs WHERE id=$1")
        .bind(run_id)
        .execute(&pool)
        .await
        .ok();
}
