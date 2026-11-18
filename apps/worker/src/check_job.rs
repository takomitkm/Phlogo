//! CheckJob definition + run execution (P3-01 envelope, M2 wiring).
//!
//! Apalis is at-least-once: the same `run_id` may be delivered repeatedly, so every
//! side effect keyed by `run_id` must be conflict-safe (`ON CONFLICT DO NOTHING` /
//! guarded UPDATE). A redelivery therefore re-runs the work rather than skipping it —
//! that way a delivery that died mid-way still gets completed.
use std::collections::HashSet;

use apalis::prelude::Data;
use checker_miaospeed::{MiaoSpeedAdapter, SubmitRequest, VendorNode};
use database::check::{persist_check_run, upsert_node_status, CheckResultRow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CheckJob {
    /// == `check_runs.id`; the retry idempotency key (plan §5.1).
    pub run_id: Uuid,
    /// Checker view tag (Phlogo `checker_tag`, e.g. "default" / "miaospeed").
    pub checker_tag: String,
    /// Nodes to check in this batch (internal Uuids).
    pub node_ids: Vec<Uuid>,
    /// Check presets (MiaoSpeed matrix names, e.g. TEST_PING_RTT).
    pub presets: Vec<String>,
}

/// Record a run start; returns true iff this call created the row (first delivery).
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
    let known = existing_ids(pool, &job.node_ids).await.unwrap_or_default();
    let unreported: Vec<Uuid> = job
        .node_ids
        .iter()
        .copied()
        .filter(|id| known.contains(id) && !reported.contains(id))
        .collect();

    if let Err(e) =
        persist_check_run(pool, job.run_id, &job.checker_tag, None, &rows, &unreported).await
    {
        fail_check_run(pool, job.run_id, &format!("persist: {e}")).await;
        return Err(e);
    }

    for row in &rows {
        let health = if row.latency_ms == -1 {
            "dead"
        } else {
            "alive"
        };
        if let Err(e) = upsert_node_status(
            pool,
            row.node_id,
            &job.checker_tag,
            health,
            chrono::Utc::now(),
        )
        .await
        {
            tracing::warn!(node_id = %row.node_id, error = %e, "upsert_node_status failed");
        }
    }

    finish_check_run(pool, job.run_id).await?;
    tracing::info!(
        run_id = %job.run_id,
        checked = rows.len(),
        unreported = unreported.len(),
        "check run completed"
    );
    Ok(())
}
/// Apalis handler: thin wrapper over [`run_check_once`].
pub async fn handle_check(job: CheckJob, pool: Data<PgPool>) {
    let pool: &PgPool = &pool;
    if let Err(e) = run_check_once(pool, &job).await {
        tracing::error!(run_id = %job.run_id, error = %e, "check run errored");
    }
}
