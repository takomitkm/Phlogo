//! CheckJob definition + run execution (P3-01 envelope, M2 wiring).
//!
//! Apalis is at-least-once: the same `run_id` may be delivered repeatedly, so every
//! side effect keyed by `run_id` must be conflict-safe (`ON CONFLICT DO NOTHING` /
//! guarded UPDATE). A redelivery therefore re-runs the work rather than skipping it —
//! that way a delivery that died mid-way still gets completed.
use std::collections::HashSet;

use apalis::prelude::Data;
use checker_miaospeed::{MiaoSpeedAdapter, SubmitRequest, VendorNode};
use database::check::CheckResultRow;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CheckJob {
    /// == `check_runs.id`; the retry idempotency key (plan §5.1).
    pub run_id: Uuid,
    /// Logical schedule period that produced this job (deterministic UUID v5 of the
    /// period label). One logical period = one check_runs row = no double work even if
    /// several worker instances race or the job is redelivered.
    #[serde(default)]
    pub schedule_id: Uuid,
    /// Checker view tag (Phlogo `checker_tag`, e.g. "default" / "miaospeed").
    pub checker_tag: String,
    /// Nodes to check in this batch (internal Uuids).
    pub node_ids: Vec<Uuid>,
    /// Check presets (MiaoSpeed matrix names, e.g. TEST_PING_RTT).
    pub presets: Vec<String>,
}

/// Record a run start; returns true iff this call created the row (first delivery).
///
/// M3 dedup: `schedule_id` is deterministic per logical period, and `run_id` for a
/// scheduled batch is derived from it (`Uuid::new_v5(schedule_id, batch_index)`), so
/// re-running the scheduler for the same period — from any worker instance — collides
/// on `check_runs.id` and the duplicate enqueue is dropped (`enqueue_scheduled` counts
/// only first inserts).
pub async fn record_check_run(pool: &PgPool, job: &CheckJob) -> sqlx::Result<bool> {
    let n = sqlx::query(
        "INSERT INTO check_runs (id,checker_tag,node_count,status,started_at) \
         VALUES ($1,$2,$3,'started',now()) ON CONFLICT (id) DO NOTHING",
    )
    .bind(job.run_id)
    .bind(&job.checker_tag)
    .bind(job.node_ids.len() as i32)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n > 0)
}

/// Mark the run finished (idempotent: only the first transition wins).
///
/// `check_runs` uses `success` as its terminal-ok status (`source_fetch_runs` uses
/// `finished`; the two tables are separate vocabularies, see p3-04).
pub async fn finish_check_run(pool: &PgPool, run_id: Uuid) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE check_runs SET status='success', finished_at=now() \
         WHERE id=$1 AND finished_at IS NULL",
    )
    .bind(run_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark the run failed with a reason; one-shot like `finish_check_run`.
async fn fail_check_run(pool: &PgPool, run_id: Uuid, reason: &str) {
    if let Err(e) = sqlx::query(
        "UPDATE check_runs SET status='failed', finished_at=now() \
         WHERE id=$1 AND finished_at IS NULL",
    )
    .bind(run_id)
    .execute(pool)
    .await
    {
        tracing::error!(run_id = %run_id, error = %e, "fail_check_run failed");
    }
    tracing::warn!(run_id = %run_id, reason, "check run failed");
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// MiaoSpeed endpoint (M2). `PHLOGO_CHECKER_VENDOR` defaults to `Clash`, matching the
/// stored payload shape (a Clash proxy object).
fn checker_endpoint() -> Result<(String, String, String), String> {
    let url = env_nonempty("PHLOGO_CHECKER_WS_URL")
        .ok_or_else(|| "PHLOGO_CHECKER_WS_URL is not set".to_string())?;
    let secret = env_nonempty("PHLOGO_CHECKER_SECRET").unwrap_or_default();
    let vendor = env_nonempty("PHLOGO_CHECKER_VENDOR").unwrap_or_else(|| "Clash".to_string());
    Ok((url, secret, vendor))
}

/// A node that can actually be checked: it has a readable sealed payload.
struct CheckableNode {
    id: Uuid,
    payload: Value,
}

async fn load_checkable(pool: &PgPool, ids: &[Uuid]) -> sqlx::Result<Vec<CheckableNode>> {
    let rows: Vec<(Uuid, Vec<u8>, Vec<u8>)> = sqlx::query_as(
        "SELECT n.id, p.nonce, p.ciphertext FROM nodes n \
         JOIN node_payloads p ON p.node_id = n.id WHERE n.id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, nonce, ciphertext)| {
            let plain = payload_crypto::global()
                .open(
                    &payload_crypto::aad_for(&id.to_string()),
                    &nonce,
                    &ciphertext,
                )
                .ok()?;
            let payload = serde_json::from_slice::<Value>(&plain).ok()?;
            Some(CheckableNode { id, payload })
        })
        .collect())
}

/// Node ids from `ids` that actually exist in `nodes` — `node_check_results.node_id`
/// is a foreign key, so "unreported" must never include unknown ids.
async fn existing_ids(pool: &PgPool, ids: &[Uuid]) -> sqlx::Result<HashSet<Uuid>> {
    let rows: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM nodes WHERE id = ANY($1)")
        .bind(ids)
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().collect())
}

/// Execute one check run end to end (testable core; the Apalis handler wraps this).
pub async fn run_check_once(pool: &PgPool, job: &CheckJob) -> anyhow::Result<()> {
    if let Err(e) = record_check_run(pool, job).await {
        tracing::error!(run_id = %job.run_id, error = %e, "record_check_run failed");
        return Err(e.into());
    }

    let (url, secret, vendor) = match checker_endpoint() {
        Ok(v) => v,
        Err(reason) => {
            fail_check_run(pool, job.run_id, &reason).await;
            return Err(anyhow::anyhow!(reason));
        }
    };

    let nodes = match load_checkable(pool, &job.node_ids).await {
        Ok(n) => n,
        Err(e) => {
            fail_check_run(pool, job.run_id, &format!("load nodes: {e}")).await;
            return Err(e.into());
        }
    };
    if nodes.is_empty() {
        let reason = "no node has a readable payload (fetch them first; check PHLOGO_PAYLOAD_KEY)";
        fail_check_run(pool, job.run_id, reason).await;
        return Err(anyhow::anyhow!(reason));
    }

    let vendor_nodes: Vec<VendorNode> = nodes
        .iter()
        .map(|n| VendorNode {
            internal_node_id: n.id,
            // Results map back by `ProxyInfo.Name`, so the vendor id must be unique.
            vendor_id: n.id.to_string(),
            payload: n.payload.clone(),
        })
        .collect();
    let request = SubmitRequest {
        run_id: job.run_id,
        checker_tag: job.checker_tag.clone(),
        vendor,
        presets: job.presets.clone(),
        nodes: vendor_nodes,
    };

    let adapter = MiaoSpeedAdapter::new(url, secret);
    let results = match adapter.check(&request).await {
        Ok(r) => r,
        Err(e) => {
            fail_check_run(pool, job.run_id, &format!("checker: {e}")).await;
            return Err(anyhow::anyhow!("checker: {e}"));
        }
    };

    let rows: Vec<CheckResultRow> = results
        .iter()
        .map(|r| CheckResultRow {
            node_id: r.internal_node_id,
            latency_ms: r.initial.latency_ms,
            deep: r.deep.as_ref().map(|d| {
                (
                    d.avg_latency_ms,
                    d.jitter_ms,
                    d.speed_kbps,
                    d.region.clone(),
                    d.streaming.clone(),
                    d.metadata.clone(),
                )
            }),
        })
        .collect();
    let reported: HashSet<Uuid> = rows.iter().map(|r| r.node_id).collect();
    // M2.1: a DB error here must fail the run — swallowing it would silently drop the
    // `unknown` rows for unreported nodes and leave a run that looks complete.
    let known = existing_ids(pool, &job.node_ids)
        .await
        .map_err(|e| anyhow::anyhow!("existing_ids: {e}"))?;
    let unreported: Vec<Uuid> = job
        .node_ids
        .iter()
        .copied()
        .filter(|id| known.contains(id) && !reported.contains(id))
        .collect();

    // M2.1: results + node_status + terminal state must succeed TOGETHER. A failure in
    // any of them marks the run failed (never a surface-success run missing status).
    match persist_all(pool, job, &rows, &unreported).await {
        Ok(()) => {}
        Err(e) => {
            fail_check_run(pool, job.run_id, &format!("persist: {e}")).await;
            return Err(e);
        }
    }

    tracing::info!(
        run_id = %job.run_id,
        checked = rows.len(),
        unreported = unreported.len(),
        "check run completed"
    );
    Ok(())
}

/// Persist results, per-tag node status and the terminal run state in ONE transaction
/// (M2.1). `check_runs.status` is only flipped to `success` if everything committed;
/// any error rolls the whole thing back and the caller marks the run failed.
///
/// The `persist_check_run` executor form writes `status='success'` unconditionally —
/// a rollback leaves the original `started` row intact, and `fail_check_run` then
/// flips it to `failed`.
async fn persist_all(
    pool: &PgPool,
    job: &CheckJob,
    rows: &[CheckResultRow],
    unreported: &[Uuid],
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    database::check::persist_check_run_in(
        &mut tx,
        job.run_id,
        &job.checker_tag,
        None,
        rows,
        unreported,
    )
    .await?;
    for row in rows {
        let health = if row.latency_ms == -1 {
            "dead"
        } else {
            "alive"
        };
        database::check::upsert_node_status_in(
            &mut tx,
            row.node_id,
            &job.checker_tag,
            health,
            chrono::Utc::now(),
        )
        .await?;
    }
    database::check::finish_check_run_in(&mut tx, job.run_id).await?;
    tx.commit().await?;
    Ok(())
}

/// Apalis handler: thin wrapper over [`run_check_once`].
pub async fn handle_check(job: CheckJob, pool: Data<PgPool>) {
    let pool: &PgPool = &pool;
    if let Err(e) = run_check_once(pool, &job).await {
        tracing::error!(run_id = %job.run_id, error = %e, "check run errored");
    }
}

/// Namespace for deterministic schedule ids (arbitrary but fixed bytes).
const SCHEDULE_NS: [u8; 16] = *b"phlogo-check-sch";

/// Deterministic run id for one (period, batch) pair — identical across workers,
/// so racing schedulers produce the same `check_runs.id` and the duplicate is dropped.
fn scheduled_run_id(schedule_id: Uuid, batch: usize) -> Uuid {
    let mut v5 = schedule_id.as_bytes().to_vec();
    v5.extend_from_slice(&(batch as u64).to_le_bytes());
    Uuid::new_v5(&Uuid::from_bytes(SCHEDULE_NS), &v5)
}

/// Config values for the check scheduler (configs table; sane fallbacks for tests).
pub struct CheckScheduleConfig {
    pub interval_minutes: i32,
    pub batch_size: usize,
}

pub async fn load_check_schedule_config(pool: &PgPool) -> anyhow::Result<CheckScheduleConfig> {
    async fn one(pool: &PgPool, key: &str, fallback: i64) -> anyhow::Result<i64> {
        let v: Option<String> = sqlx::query_scalar("SELECT value FROM configs WHERE key=$1")
            .bind(key)
            .fetch_optional(pool)
            .await?;
        Ok(v.and_then(|s| s.trim().parse::<i64>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(fallback))
    }
    Ok(CheckScheduleConfig {
        interval_minutes: one(pool, "check_schedule_interval_minutes", 30)
            .await?
            .max(1) as i32,
        batch_size: one(pool, "check_batch_size", 50).await?.max(1) as usize,
    })
}

/// Candidate node ids for one scheduling pass: Linked nodes that HAVE a readable sealed
/// payload (nodes without payloads cannot be checked — M2 behavior — so they are
/// excluded at scheduling time, not inside the run).
async fn schedulable_node_ids(pool: &PgPool) -> anyhow::Result<Vec<Uuid>> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT n.id FROM nodes n JOIN node_payloads p ON p.node_id = n.id \
         WHERE n.membership='Linked' ORDER BY n.id",
    )
    .fetch_all(pool)
    .await?;
    Ok(ids)
}

/// One scheduling pass (M3): build batches over schedulable nodes for `period` and
/// enqueue them. Idempotent per period: `run_id = v5(schedule_id, batch)`, and a batch
/// is only pushed when its `check_runs` row does not exist yet (guarded by
/// `record_check_run` BEFORE the push, so a crash between push and run cannot
/// duplicate either side).
///
/// Returns (pushed_batches, skipped_duplicates, total_nodes).
pub async fn enqueue_scheduled(
    storage: &mut apalis_sql::postgres::PostgresStorage<CheckJob>,
    pool: &PgPool,
    period_label: &str,
    cfg: &CheckScheduleConfig,
) -> anyhow::Result<(usize, usize, usize)> {
    use apalis::prelude::Storage as _;
    let schedule_id = Uuid::new_v5(&Uuid::from_bytes(SCHEDULE_NS), period_label.as_bytes());
    let ids = schedulable_node_ids(pool).await?;
    let mut pushed = 0usize;
    let mut skipped = 0usize;
    for (batch, chunk) in ids.chunks(cfg.batch_size).enumerate() {
        let job = CheckJob {
            run_id: scheduled_run_id(schedule_id, batch),
            schedule_id,
            checker_tag: "default".into(),
            node_ids: chunk.to_vec(),
            // Ruling: initial schedule only pings; download matrices come later (M4+).
            presets: vec!["TEST_PING_RTT".into()],
        };
        // Guard FIRST: only the instance that created the run row pushes the job.
        if record_check_run(pool, &job).await? {
            storage.push(job).await?;
            pushed += 1;
        } else {
            skipped += 1;
        }
    }
    Ok((pushed, skipped, ids.len()))
}

/// API-facing scheduled-enqueue trigger (M3 stage 4): same service as the periodic
/// scheduler, with an explicit node list (default: all schedulable). Batches by
/// `batch_size`; `period_label` empty = derive from the current time PLUS a random
/// suffix, so two manual triggers in the same second are still two logical runs.
pub async fn trigger_check_batch(
    jobs_dsn: &str,
    pool: &PgPool,
    node_ids: Option<Vec<Uuid>>,
    batch_size: usize,
    period_label: &str,
) -> anyhow::Result<(usize, usize, usize)> {
    let jobs_pool = database::connect_lazy(jobs_dsn)?;
    let mut storage: apalis_sql::postgres::PostgresStorage<CheckJob> =
        apalis_sql::postgres::PostgresStorage::new(jobs_pool);
    let cfg = CheckScheduleConfig {
        interval_minutes: 0, // not used by this path
        batch_size: batch_size.max(1),
    };
    let label = if period_label.is_empty() {
        format!(
            "manual-{}-{}",
            chrono::Utc::now().format("%Y%m%dT%H%M%S"),
            Uuid::new_v4().simple()
        )
    } else {
        period_label.to_string()
    };
    // Explicit list wins but is FILTERED to schedulable nodes (exists + has payload) —
    // the ruling forbids sending payload-less nodes to MiaoSpeed, and the API surface
    // must not silently schedule ghost ids either. Default: the same schedulable set
    // as the periodic pass.
    let ids = match node_ids {
        Some(ids) if !ids.is_empty() => {
            let all = schedulable_node_ids(pool).await?;
            let set: std::collections::HashSet<Uuid> = all.into_iter().collect();
            ids.into_iter()
                .filter(|id| set.contains(id))
                .collect::<Vec<_>>()
        }
        _ => schedulable_node_ids(pool).await?,
    };
    let schedule_id = Uuid::new_v5(&Uuid::from_bytes(SCHEDULE_NS), label.as_bytes());
    use apalis::prelude::Storage as _;
    let mut pushed = 0usize;
    let mut skipped = 0usize;
    for (batch, chunk) in ids.chunks(cfg.batch_size).enumerate() {
        let job = CheckJob {
            run_id: scheduled_run_id(schedule_id, batch),
            schedule_id,
            checker_tag: "default".into(),
            node_ids: chunk.to_vec(),
            presets: vec!["TEST_PING_RTT".into()],
        };
        if record_check_run(pool, &job).await? {
            storage.push(job).await?;
            pushed += 1;
        } else {
            skipped += 1;
        }
    }
    Ok((pushed, skipped, ids.len()))
}
