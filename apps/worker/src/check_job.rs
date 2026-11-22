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
/// on `check_runs.id` and the duplicate enqueue is dropped.
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

/// Delivery lease length for the schedule→push handoff (M3.1-2 state machine).
///
/// A scheduler that claims a batch holds the lease for this long; a row whose lease
/// expired is recoverable by any other pass. The lease does NOT make delivery
/// exactly-once — it bounds the recovery window. Delivery is at-least-once by design;
/// duplicate MiaoSpeed executions of a SUCCEEDED run are prevented on the execution
/// side (`run_check_once` skips runs that already reached `success`).
pub const DELIVERY_LEASE_SECS: i64 = 60;

/// Outcome of the atomic delivery claim (`claim_for_push`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Claim {
    /// True when this call INSERTED the run row (first scheduling of the batch).
    pub created: bool,
    /// Lease holder recorded by the claim statement; `mark_enqueued` only accepts
    /// this owner, so a stale instance cannot overwrite a newer lease.
    pub owner: Uuid,
}

/// Atomically claim the delivery right for one batch (M3.1-2, replaces the racy
/// New/LostBeforePush pair).
///
/// ONE statement handles both first-claim and recovery, so the review's defect A
/// ("insert returns New while another instance claims LostBeforePush") cannot happen:
/// the INSERT ... ON CONFLICT DO UPDATE carries the lease conditions in its WHERE
/// clause, PostgreSQL row-locks the existing row, and exactly one concurrent caller
/// gets `rows_affected = 1`.
///
/// Claimable when: row absent, OR `push_state='pending'`, OR the lease expired
/// (`dispatching` + `lease_until < now()`), AND the run has not reached a terminal
/// state. This is the recovery rule for "claimed but crashed before push" (defect B):
/// the crashed instance's lease expires, the next pass re-claims. The same rule
/// covers "push succeeded but the enqueued-mark crashed" (defect C): delivery is
/// **at-least-once** — the queue may see a second push; duplicate MiaoSpeed execution
/// of a succeeded run is prevented by the execution-side gate in `run_check_once`.
pub async fn claim_for_push(pool: &PgPool, job: &CheckJob) -> sqlx::Result<Option<Claim>> {
    let lease_owner = Uuid::new_v4();
    let row: Option<(bool,)> = sqlx::query_as(
        "INSERT INTO check_runs \
           (id,checker_tag,node_count,status,started_at,push_state,lease_until,lease_owner) \
         VALUES ($1,$2,$3,'started',now(),'dispatching', \
                 now() + make_interval(secs => $4), $5) \
         ON CONFLICT (id) DO UPDATE \
           SET push_state='dispatching', \
               lease_until = now() + make_interval(secs => $4), \
               lease_owner = $5 \
         WHERE (check_runs.push_state = 'pending' \
                OR (check_runs.push_state = 'dispatching' \
                    AND check_runs.lease_until < now())) \
               AND check_runs.status = 'started' \
         RETURNING (xmax = 0) AS inserted",
    )
    .bind(job.run_id)
    .bind(&job.checker_tag)
    .bind(job.node_ids.len() as i32)
    .bind(DELIVERY_LEASE_SECS)
    .bind(lease_owner)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(created,)| Claim {
        created,
        owner: lease_owner,
    }))
}

/// Transition a claimed batch to `enqueued` after a successful `storage.push`.
///
/// Errors are PROPAGATED to the caller (ruling: never silently swallowed). A failure
/// here leaves the row in `dispatching` with a running lease — delivery is
/// at-least-once and the next pass will re-push after the lease expires; the
/// execution-side gate keeps a redelivered job from re-running a succeeded check.
///
/// Returns `true` when the transition applied. `false` means this lease was already
/// superseded (a slower instance lost it), so the row will be — or is being — pushed
/// again by its current holder: the caller logs that as the at-least-once window
/// instead of treating it as success.
async fn mark_enqueued(pool: &PgPool, run_id: Uuid, owner: Uuid) -> sqlx::Result<bool> {
    let applied = sqlx::query(
        "UPDATE check_runs SET push_state='enqueued', lease_until=NULL, lease_owner=NULL \
         WHERE id=$1 AND lease_owner=$2",
    )
    .bind(run_id)
    .bind(owner)
    .execute(pool)
    .await?
    .rows_affected()
        > 0;
    Ok(applied)
}

/// Test-only wrapper for [`mark_enqueued`] (same guard semantics; used by fault-injection
/// tests to verify the owner guard and error propagation).
pub async fn mark_enqueued_for_test(
    pool: &PgPool,
    run_id: Uuid,
    owner: Uuid,
) -> sqlx::Result<bool> {
    mark_enqueued(pool, run_id, owner).await
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
///
/// Execution-side idempotency gate (M3.1-2, ruling defect C): delivery is
/// at-least-once, so the same run may be delivered twice (e.g. "push succeeded but the
/// enqueued-mark crashed"). A run that already reached `success` is NOT re-executed —
/// its results are persisted exactly once by the unique key, and re-running real
/// MiaoSpeed checks would only waste resources. Failed runs MAY be re-executed (that
/// is the retry path).
pub async fn run_check_once(pool: &PgPool, job: &CheckJob) -> anyhow::Result<()> {
    if let Err(e) = record_check_run(pool, job).await {
        tracing::error!(run_id = %job.run_id, error = %e, "record_check_run failed");
        return Err(e.into());
    }

    // Gate: a run that already succeeded must not burn a second real MiaoSpeed pass.
    let prior: Option<String> = sqlx::query_scalar("SELECT status FROM check_runs WHERE id=$1")
        .bind(job.run_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| anyhow::anyhow!("run status: {e}"))?;
    if prior.as_deref() == Some("success") {
        tracing::info!(run_id = %job.run_id, "run already succeeded; skipping redelivery");
        return Ok(());
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

/// Apalis handler (M3.1 fix #3): returns `Err` on failure so the RetryPolicy layer
/// actually retries the task — swallowing the error into a log line would make Apalis
/// treat the job as completed. The business-side run row is still flipped to `failed`
/// inside `run_check_once`; the retry is for transient faults (checker connection,
/// missing terminal frame, DB errors), and idempotent persistence keeps redeliveries
/// from double-counting.
pub async fn handle_check(job: CheckJob, pool: Data<PgPool>) -> Result<(), anyhow::Error> {
    let pool: &PgPool = &pool;
    run_check_once(pool, &job).await
}

/// Namespace for deterministic schedule ids (arbitrary but fixed bytes).
const SCHEDULE_NS: [u8; 16] = *b"phlogo-check-sch";

/// Deterministic schedule id for one logical period label.
///
/// The label is used VERBATIM (no prefixing): the caller's idempotency key *is* the
/// label, so the same key must derive the same schedule id. Exported so tests derive
/// run ids from the production function instead of re-implementing the v5 recipe —
/// a hand-rolled copy silently drifts (it did: a stray `manual-` prefix pointed the
/// test's recovery UPDATE at a nonexistent row).
pub fn schedule_id_for_label(period_label: &str) -> Uuid {
    Uuid::new_v5(&Uuid::from_bytes(SCHEDULE_NS), period_label.as_bytes())
}

/// Deterministic run id for one (period, batch) pair — identical across workers,
/// so racing schedulers produce the same `check_runs.id` and the duplicate is dropped.
///
/// `v5(namespace = SCHEDULE_NS, name = schedule_id ‖ batch_le_u64)`: the namespace is
/// applied exactly once, over the schedule id bytes.
pub fn scheduled_run_id(schedule_id: Uuid, batch: usize) -> Uuid {
    let mut v5 = schedule_id.as_bytes().to_vec();
    v5.extend_from_slice(&(batch as u64).to_le_bytes());
    Uuid::new_v5(&Uuid::from_bytes(SCHEDULE_NS), &v5)
}

/// The shared schedule identity for one logical period (M3.1 fix #1).
///
/// `now` is floored to the interval boundary, so two workers started minutes apart
/// still derive the SAME label for the same wall-clock period — without this, each
/// instance's own `interval_at` start time made the labels drift and the dedup
/// constraint could never link them.
pub fn aligned_period_label(now: chrono::DateTime<chrono::Utc>, interval_minutes: i32) -> String {
    let mins = interval_minutes.max(1) as i64;
    let epoch_min = now.timestamp() / 60;
    let slot = epoch_min.div_euclid(mins);
    let slot_start = slot * mins;
    let start = chrono::DateTime::from_timestamp(slot_start * 60, 0)
        .unwrap_or(now)
        .format("%Y%m%dT%H%M")
        .to_string();
    format!("period-{start}+{mins}m")
}

/// The next global UTC period boundary strictly AFTER `now` (M3.1-2 fix #1).
///
/// Timers are anchored to shared wall-clock boundaries, not to each process's startup
/// instant: all workers tick on the same boundary sequence, so two instances started
/// minutes apart evaluate the SAME slot label at (approximately) the same moment and
/// the dedup constraint actually sees their race. The ruling's "wait at least one full
/// period after startup" is honored by the caller passing `startup + interval` as
/// `not_before`.
///
/// Returns (tick_instant, label_for_that_slot).
pub fn next_boundary_after(
    not_before: chrono::DateTime<chrono::Utc>,
    interval_minutes: i32,
) -> (chrono::DateTime<chrono::Utc>, String) {
    let mins = interval_minutes.max(1) as i64;
    let not_before_min = not_before.timestamp() / 60;
    // First slot START strictly after not_before's minute.
    let slot = not_before_min.div_euclid(mins) + 1;
    let slot_start_min = slot * mins;
    let start = chrono::DateTime::from_timestamp(slot_start_min * 60, 0).unwrap_or(not_before);
    (start, aligned_period_label(start, interval_minutes))
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

/// Candidate node ids for one scheduling pass.
///
/// M3.1 fix #2: "schedulable" means the sealed payload **actually decrypts and parses
/// as JSON** — a row in `node_payloads` is not enough. A wrong/mismatched key (e.g.
/// after a restart with the ephemeral key) or corrupt ciphertext would otherwise be
/// scheduled and only fail inside the run. Undecryptable nodes are excluded here and
/// counted for the log; nothing is logged about the payload contents.
async fn schedulable_node_ids(pool: &PgPool) -> anyhow::Result<(Vec<Uuid>, usize)> {
    let rows: Vec<(Uuid, Vec<u8>, Vec<u8>)> = sqlx::query_as(
        "SELECT n.id, p.nonce, p.ciphertext FROM nodes n \
         JOIN node_payloads p ON p.node_id = n.id \
         WHERE n.membership='Linked' ORDER BY n.id",
    )
    .fetch_all(pool)
    .await?;
    let mut ids = Vec::with_capacity(rows.len());
    let mut undecryptable = 0usize;
    for (id, nonce, ciphertext) in rows {
        let ok = payload_crypto::global()
            .open(
                &payload_crypto::aad_for(&id.to_string()),
                &nonce,
                &ciphertext,
            )
            .ok()
            .and_then(|plain| serde_json::from_slice::<Value>(&plain).ok())
            .is_some();
        if ok {
            ids.push(id);
        } else {
            undecryptable += 1;
        }
    }
    Ok((ids, undecryptable))
}

/// One scheduling pass (M3): build batches over schedulable nodes for `period` and
/// enqueue them via the lease state machine (M3.1-2).
///
/// Delivery is at-least-once; see [`claim_for_push`] and [`run_check_once`] for how
/// duplicates are contained.
///
/// Returns (pushed_batches, skipped_busy, total_nodes, undecryptable_nodes).
pub async fn enqueue_scheduled(
    storage: &mut apalis_sql::postgres::PostgresStorage<CheckJob>,
    pool: &PgPool,
    period_label: &str,
    cfg: &CheckScheduleConfig,
) -> anyhow::Result<(usize, usize, usize, usize)> {
    let schedule_id = schedule_id_for_label(period_label);
    let (ids, undecryptable) = schedulable_node_ids(pool).await?;
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
        // Lease errors propagate (ruling: no `let _ =` on state transitions). A claim
        // that wins here will be marked enqueued below; a mark failure surfaces as an
        // error from this pass — delivery stays at-least-once via the expired lease.
        if let Some(claim) = claim_for_push(pool, &job).await? {
            storage.push(job.clone()).await?;
            // A superseded lease is not an error — the current holder pushes again
            // (that IS the at-least-once window) — but it must be visible in logs.
            if !mark_enqueued(pool, job.run_id, claim.owner).await? {
                tracing::warn!(
                    run_id = %job.run_id,
                    "delivery lease superseded before the enqueued mark; the batch will \
                     be pushed again by its current holder (at-least-once)"
                );
            }
            pushed += 1;
        } else {
            skipped += 1;
        }
    }
    Ok((pushed, skipped, ids.len(), undecryptable))
}

use apalis::prelude::Storage as _;

/// API-facing scheduled-enqueue trigger (M3 stage 4): same service as the periodic
/// scheduler, with an explicit node list (default: all schedulable). Batches by
/// `batch_size`; `period_label` empty = derive from the current time PLUS a random
/// suffix, so two manual triggers in the same second are still two logical runs.
///
/// Uses the same claim/re-push protocol as the periodic pass (M3.1 fix #4), so a
/// manual trigger that dies between claim and push is recovered by the next periodic
/// pass over the same label — or by re-invoking the trigger.
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
    // M3.1-2 (ruling): the period label IS the caller's idempotency key and is used
    // VERBATIM — re-sending the same request recovers lost batches via the claim
    // state machine instead of creating a second logical run.
    anyhow::ensure!(
        !period_label.is_empty(),
        "trigger_check_batch requires an explicit period label (idempotency key)"
    );
    // Explicit list wins but is FILTERED to schedulable nodes (decryptable payload) —
    // the ruling forbids sending payload-less/undecryptable nodes to MiaoSpeed, and the
    // API surface must not silently schedule ghost ids either.
    let (schedulable, _undecryptable) = schedulable_node_ids(pool).await?;
    let ids = match node_ids {
        Some(ids) if !ids.is_empty() => {
            let set: std::collections::HashSet<Uuid> = schedulable.into_iter().collect();
            ids.into_iter()
                .filter(|id| set.contains(id))
                .collect::<Vec<_>>()
        }
        _ => schedulable,
    };
    let schedule_id = schedule_id_for_label(period_label);
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
        if let Some(claim) = claim_for_push(pool, &job).await? {
            storage.push(job.clone()).await?;
            // A superseded lease is not an error — the current holder pushes again
            // (that IS the at-least-once window) — but it must be visible in logs.
            if !mark_enqueued(pool, job.run_id, claim.owner).await? {
                tracing::warn!(
                    run_id = %job.run_id,
                    "delivery lease superseded before the enqueued mark; the batch will \
                     be pushed again by its current holder (at-least-once)"
                );
            }
            pushed += 1;
        } else {
            skipped += 1;
        }
    }
    Ok((pushed, skipped, ids.len()))
}
