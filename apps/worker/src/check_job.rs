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

/// Release a delivery claim WITHOUT enqueuing (the push failed).
///
/// The row goes back to `pending`, so the very next attempt can claim it again
/// immediately — instead of waiting for the 60 s lease to expire. This matters because
/// nothing re-pushes an expired `dispatching` row on its own (there is no recovery
/// scanner: §10.4.4), and the periodic scheduler derives a NEW `run_id` per period, so a
/// batch whose push failed would otherwise be stranded in `dispatching` for good.
///
/// Owner-guarded like every other transition: a superseded lease must not resurrect a
/// row the current holder is already pushing.
async fn release_push_claim(pool: &PgPool, run_id: Uuid, owner: Uuid) -> sqlx::Result<()> {
    let applied = sqlx::query(
        "UPDATE check_runs SET push_state='pending', lease_until=NULL, lease_owner=NULL \
         WHERE id=$1 AND push_state='dispatching' AND lease_owner=$2",
    )
    .bind(run_id)
    .bind(owner)
    .execute(pool)
    .await?
    .rows_affected()
        > 0;
    if !applied {
        tracing::warn!(
            run_id = %run_id,
            "push failed and the delivery lease was already superseded; not releasing it"
        );
    }
    Ok(())
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

/// Execution lease length (M3.1-3).
///
/// The delivery lease bounds the push window; this one bounds the execution right. Its
/// length is NOT arbitrary — it sits between two hard bounds (see the module notes on
/// [`CHECK_RUN_TIMEOUT_SECS`]):
///
/// * **> the per-run timeout**: a live executor always finishes (or is failed) before its
///   own lease can expire, so a concurrent claimant that grabs the expired lease can
///   never overlap a running check.
/// * **< apalis's `reenqueue_orphaned_after` (300 s by default in apalis-sql 0.7.4)**:
///   a crashed executor's job is re-enqueued once its worker heartbeat is 300 s stale;
///   by then this lease has already expired, so that re-delivery actually WINS the claim
///   instead of being a no-op that burns the only recovery chance.
pub const EXEC_LEASE_SECS: i64 = 240;

/// Atomically take the EXECUTION right for one run (M3.1-3, ruling 2026-11-22).
///
/// Replaces the previous `SELECT status` gate, which was a read: two deliveries of the
/// same `run_id` both saw `started` and both called MiaoSpeed. Here ONE statement decides
/// — `UPDATE ... WHERE status IN ('started','failed') AND (lease free or expired)` — and
/// PostgreSQL serialises concurrent callers on the row, so exactly one gets `Some(owner)`.
///
/// Claiming also **starts a new attempt**: `status` is reset to `started` and
/// `finished_at` cleared, so a retry of a previously failed run can record its own
/// terminal state (`fail_check_run` still guards on `finished_at IS NULL`).
///
/// `None` means this delivery must not do the work — the run already succeeded
/// (terminal), or another worker holds an unexpired execution lease. Both are no-ops for
/// Apalis: the run has exactly one owner either way.
pub async fn claim_for_exec(pool: &PgPool, run_id: Uuid) -> sqlx::Result<Option<Uuid>> {
    let owner = Uuid::new_v4();
    let row: Option<(Uuid,)> = sqlx::query_as(
        "UPDATE check_runs \
            SET exec_owner = $2, \
                exec_lease_until = now() + make_interval(secs => $3), \
                status = 'started', \
                finished_at = NULL \
          WHERE id = $1 \
            AND status IN ('started','failed') \
            AND (exec_lease_until IS NULL OR exec_lease_until < now()) \
        RETURNING exec_owner",
    )
    .bind(run_id)
    .bind(owner)
    .bind(EXEC_LEASE_SECS)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|_| owner))
}

/// Wall-clock cap for one execution of a check run (M3.1-3).
///
/// The adapter itself has no timeout: a checker that accepts the connection and never
/// sends the terminal frame would hold the worker forever, and would also outlive the
/// execution lease (letting a second delivery run MiaoSpeed concurrently). The cap keeps
/// the execution inside the lease.
///
/// **Invariant chain** (all three numbers are load-bearing):
///
/// ```text
/// CHECK_RUN_TIMEOUT_SECS (180 s)  <  EXEC_LEASE_SECS (240 s)  <  apalis reenqueue_orphaned_after (300 s)
///    a live executor ends           a dead executor's lease       ...so the orphaned job's
///    before its lease can be        has expired by the time       re-delivery actually wins
///    stolen                         the re-delivery arrives        the claim (not a no-op)
/// ```
///
/// 180 s is sized for today's ping-only batches (`TEST_PING_RTT`, seconds of work). The
/// P3 sketch recorded a 30-minute hard timeout mirroring upstream's
/// `context.WithTimeout(30*time.Minute)`; when M4 deep matrices arrive, raise the timeout
/// **and** the lease together (or add lease renewal), otherwise this chain breaks.
/// Override with `PHLOGO_CHECK_TIMEOUT_SECS` (tests use 1 s; clamped below the lease).
pub const CHECK_RUN_TIMEOUT_SECS: i64 = 180;

/// Effective per-run timeout: env override, clamped to remain below the lease.
fn check_run_timeout_secs() -> u64 {
    let requested = env_nonempty("PHLOGO_CHECK_TIMEOUT_SECS")
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|s| *s > 0)
        .unwrap_or(CHECK_RUN_TIMEOUT_SECS as u64);
    let ceiling = (EXEC_LEASE_SECS as u64).saturating_sub(30).max(1);
    if requested > ceiling {
        tracing::warn!(
            requested,
            ceiling,
            "PHLOGO_CHECK_TIMEOUT_SECS exceeds the execution lease; clamping"
        );
        return ceiling;
    }
    requested
}

/// apalis-sql's orphan re-enqueue window that [`EXEC_LEASE_SECS`] is calibrated against.
///
/// apalis-sql 0.7.4 defaults to 300 s, but a default is not a contract: we PIN it here by
/// building the storage with an explicit config, so the invariant chain below cannot be
/// broken by an upstream default change or by someone tuning the storage elsewhere.
pub const APALIS_ORPHAN_REENQUEUE_SECS: u64 = 300;

/// apalis-sql worker heartbeat interval for our storages (kept at the upstream default).
pub const APALIS_KEEP_ALIVE_SECS: u64 = 30;

// The invariant chain is enforced at COMPILE TIME, not just in prose: a timeout at or above
// the lease, or a lease at or above the orphan window, would silently reintroduce either
// "two executors can overlap" or "the only recovery re-delivery no-ops".
const _: () = assert!(CHECK_RUN_TIMEOUT_SECS < EXEC_LEASE_SECS);
const _: () = assert!((EXEC_LEASE_SECS as u64) < APALIS_ORPHAN_REENQUEUE_SECS);

/// Build a `CheckJob` queue storage with OUR pinned timing config (see the consts above).
///
/// The namespace must stay `type_name::<CheckJob>()` (apalis's default): it is the
/// `apalis.jobs.job_type` discriminator, so changing it would strand already-queued jobs.
pub fn jobs_storage(pool: sqlx::PgPool) -> apalis_sql::postgres::PostgresStorage<CheckJob> {
    let cfg = apalis_sql::Config::new(std::any::type_name::<CheckJob>())
        .set_keep_alive(std::time::Duration::from_secs(APALIS_KEEP_ALIVE_SECS))
        .set_reenqueue_orphaned_after(std::time::Duration::from_secs(APALIS_ORPHAN_REENQUEUE_SECS));
    apalis_sql::postgres::PostgresStorage::new_with_config(pool, cfg)
}

/// Mark the run failed with a reason; one-shot.
///
/// (The former `finish_check_run` — the unguarded terminal write — was REMOVED in
/// M3.1-3d: it had no owner guard and no lease release, so it could mark a leased run
/// `success` while leaving a stale `exec_owner` behind, and it was exported from the
/// crate, which made it a foot-gun for future callers. The only terminal write now is
/// `database::check::finish_check_run_owned_in`, reached through `persist_all`.)
///
/// Owner-guarded (M3.1-3): only the worker that still holds the execution lease may
/// finish the run, and the terminal write releases `exec_owner`/`exec_lease_until` in
/// the same statement. If the lease was superseded (we were assumed dead and someone
/// else took the run over), this is a logged no-op — it must NOT overwrite the state the
/// new owner is working on.
async fn fail_check_run(pool: &PgPool, run_id: Uuid, exec_owner: Uuid, reason: &str) {
    match sqlx::query(
        "UPDATE check_runs \
            SET status='failed', finished_at=now(), exec_owner=NULL, exec_lease_until=NULL \
          WHERE id=$1 AND finished_at IS NULL AND exec_owner=$2",
    )
    .bind(run_id)
    .bind(exec_owner)
    .execute(pool)
    .await
    {
        Ok(r) if r.rows_affected() == 0 => {
            tracing::warn!(
                run_id = %run_id, reason,
                "execution lease was superseded; not marking this run failed"
            );
        }
        Ok(_) => tracing::warn!(run_id = %run_id, reason, "check run failed"),
        Err(e) => tracing::error!(run_id = %run_id, error = %e, "fail_check_run failed"),
    }
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
/// Execution-side mutual exclusion (M3.1-3, ruling 2026-11-22): delivery is
/// at-least-once, so the same `run_id` may be delivered more than once (e.g. "push
/// succeeded but the enqueued-mark crashed", lease expired, re-pushed). Each delivery
/// must therefore prove it owns the EXECUTION right before touching MiaoSpeed — see
/// [`claim_for_exec`], which is a single atomic UPDATE, not a read. Consequences:
///
/// * exactly one delivery per `run_id` reaches MiaoSpeed at a time;
/// * a delivery that loses the claim is a no-op (`Ok`), because the run already has an
///   owner: either it is terminal, or a live lease belongs to another worker;
/// * a run that already reached `success` is not claimable at all, so results are
///   persisted once;
/// * `failed` runs ARE claimable (that is the Apalis retry path) — the claim starts a
///   fresh attempt (`status='started'`, `finished_at=NULL`);
/// * execution is bounded by [`CHECK_RUN_TIMEOUT_SECS`], which is kept below the lease, so
///   a live executor never lets its lease be stolen mid-check;
/// * a crashed executor IS recoverable without any scanner of ours: apalis-sql re-enqueues
///   jobs whose worker heartbeat went stale (300 s by default), and because the lease is
///   shorter than that window the re-delivery wins the claim and re-runs the check. The
///   residual gap is a run whose queue row was already consumed as `Done`/`Killed` while
///   the lease was still live — the invariant chain above is what keeps that from
///   happening for the normal paths (see the M3 page §10.4).
pub async fn run_check_once(pool: &PgPool, job: &CheckJob) -> anyhow::Result<()> {
    if let Err(e) = record_check_run(pool, job).await {
        tracing::error!(run_id = %job.run_id, error = %e, "record_check_run failed");
        return Err(e.into());
    }

    // M3.1-3: EXECUTION claim, one atomic statement. The previous version read
    // `status` here — a plain SELECT — so two deliveries of the same run_id could both
    // read 'started' and both call MiaoSpeed. Now exactly one caller gets the lease; the
    // other is a no-op (the run has one owner: either it already succeeded, or a live
    // lease belongs to another worker).
    let exec_owner = match claim_for_exec(pool, job.run_id).await {
        Ok(Some(owner)) => owner,
        Ok(None) => {
            // Diagnostic only — but it must not swallow a DB error either.
            let status: Option<String> =
                sqlx::query_scalar("SELECT status FROM check_runs WHERE id=$1")
                    .bind(job.run_id)
                    .fetch_optional(pool)
                    .await
                    .map_err(|e| anyhow::anyhow!("run status: {e}"))?;
            tracing::info!(
                run_id = %job.run_id, status = ?status,
                "no execution right (terminal run, or another worker holds the lease); \
                 skipping this delivery"
            );
            return Ok(());
        }
        Err(e) => {
            tracing::error!(run_id = %job.run_id, error = %e, "claim_for_exec failed");
            return Err(anyhow::anyhow!("claim_for_exec: {e}"));
        }
    };

    let (url, secret, vendor) = match checker_endpoint() {
        Ok(v) => v,
        Err(reason) => {
            fail_check_run(pool, job.run_id, exec_owner, &reason).await;
            return Err(anyhow::anyhow!(reason));
        }
    };

    let nodes = match load_checkable(pool, &job.node_ids).await {
        Ok(n) => n,
        Err(e) => {
            fail_check_run(pool, job.run_id, exec_owner, &format!("load nodes: {e}")).await;
            return Err(e.into());
        }
    };
    if nodes.is_empty() {
        let reason = "no node has a readable payload (fetch them first; check PHLOGO_PAYLOAD_KEY)";
        fail_check_run(pool, job.run_id, exec_owner, reason).await;
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
    // Bounded execution: a hung checker must not outlive the execution lease (see
    // CHECK_RUN_TIMEOUT_SECS) and must not pin a worker slot forever.
    let timeout_secs = check_run_timeout_secs();
    let results = match tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        adapter.check(&request),
    )
    .await
    {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            fail_check_run(pool, job.run_id, exec_owner, &format!("checker: {e}")).await;
            return Err(anyhow::anyhow!("checker: {e}"));
        }
        Err(_elapsed) => {
            let reason = format!("checker timed out after {timeout_secs}s");
            fail_check_run(pool, job.run_id, exec_owner, &reason).await;
            return Err(anyhow::anyhow!(reason));
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
    //
    // M3.1-3c: and it must also RELEASE THE EXECUTION LEASE. A bare `?` here returned
    // while still holding `exec_owner`, so a transient DB error left the run leased and
    // `started`; the worker is alive (heartbeating), so apalis never treats the job as
    // orphaned, and the instant Apalis retries all lost the claim as no-ops and completed
    // — the run then had no delivery left and never finished. Every error path between
    // the claim and the terminal write goes through `fail_check_run` (owner-guarded,
    // releases the lease, makes the run claimable again by the retry).
    let known = match existing_ids(pool, &job.node_ids).await {
        Ok(k) => k,
        Err(e) => {
            fail_check_run(pool, job.run_id, exec_owner, &format!("existing_ids: {e}")).await;
            return Err(anyhow::anyhow!("existing_ids: {e}"));
        }
    };
    let unreported: Vec<Uuid> = job
        .node_ids
        .iter()
        .copied()
        .filter(|id| known.contains(id) && !reported.contains(id))
        .collect();

    // M2.1: results + node_status + terminal state must succeed TOGETHER. A failure in
    // any of them marks the run failed (never a surface-success run missing status).
    match persist_all(pool, job, exec_owner, &rows, &unreported).await {
        Ok(()) => {}
        Err(e) => {
            fail_check_run(pool, job.run_id, exec_owner, &format!("persist: {e}")).await;
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
    exec_owner: Uuid,
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
    database::check::finish_check_run_owned_in(&mut tx, job.run_id, exec_owner).await?;
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
            // A failed push must NOT leave the row in 'dispatching': nothing re-pushes
            // an expired delivery lease (no recovery scanner), and the periodic
            // scheduler derives a new run_id per period, so a stranded batch would be
            // lost for good. Release the claim so the next attempt can take it at once.
            if let Err(e) = storage.push(job.clone()).await {
                release_push_claim(pool, job.run_id, claim.owner).await?;
                return Err(anyhow::anyhow!("push: {e}"));
            }
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
    let mut storage = jobs_storage(jobs_pool);
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
            // A failed push must NOT leave the row in 'dispatching': nothing re-pushes
            // an expired delivery lease (no recovery scanner), and the periodic
            // scheduler derives a new run_id per period, so a stranded batch would be
            // lost for good. Release the claim so the next attempt can take it at once.
            if let Err(e) = storage.push(job.clone()).await {
                release_push_claim(pool, job.run_id, claim.owner).await?;
                return Err(anyhow::anyhow!("push: {e}"));
            }
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
