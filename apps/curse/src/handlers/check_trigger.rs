//! Check trigger (M3 stage 4): `POST /api/v1/check/trigger` — AdminJWT ONLY.
//!
//! The API's job is node selection + batch enqueue into the Apalis storage; the worker
//! performs the actual check via `run_check_once` (the API NEVER talks to MiaoSpeed).
//! Auth: intentionally NOT added to the ServiceBearer whitelist (ruling) — the exact
//! matrix in `auth.rs` already fails closed to AdminOnly for unmatched routes.
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::state::AppState;

#[derive(Debug, Deserialize, Default)]
pub struct CheckTriggerReq {
    /// Explicit node selection. Empty/absent = all schedulable (Linked + payload present).
    #[serde(default)]
    pub node_ids: Vec<Uuid>,
    /// Batch size override (clamped to [1, 100]; default from `check_batch_size`).
    #[serde(default)]
    pub batch_size: Option<usize>,
    /// M3.1-2 idempotency key (ruling): the label is used VERBATIM as the schedule
    /// identity. Re-sending the SAME key recovers lost/unpushed batches instead of
    /// creating a second logical run. Omit it for a fresh one-shot trigger.
    ///
    /// Capped at 128 characters (S76): the key is concatenated into the period label,
    /// which is hashed for the schedule id AND echoed back in the response — an
    /// unbounded key would let a caller put megabytes into a response and a log line.
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

/// Maximum accepted length of the client-supplied idempotency key (S76).
const MAX_IDEMPOTENCY_KEY_LEN: usize = 128;

/// POST /api/v1/check/trigger — enqueue CheckJobs (AdminJWT only; 202 with stats).
pub async fn trigger(State(st): State<AppState>, req: Option<Json<CheckTriggerReq>>) -> Response {
    let jobs_dsn = std::env::var("PHLOGO_DATABASE_JOBS_DSN").unwrap_or_default();
    if jobs_dsn.is_empty() {
        return (StatusCode::SERVICE_UNAVAILABLE, "jobs dsn not configured").into_response();
    }
    let req = req.map(|Json(r)| r).unwrap_or_default();

    // Hard caps (M3.1: explicit rejection, never silent truncation).
    let explicit = !req.node_ids.is_empty();
    if req.node_ids.len() > 500 {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "node_ids exceeds the 500-node cap (got {})",
                req.node_ids.len()
            ),
        )
            .into_response();
    }
    let node_ids = req.node_ids;
    let default_batch = worker::load_check_schedule_config(st.pool())
        .await
        .map(|c| c.batch_size)
        .unwrap_or(50);
    let batch_size = req.batch_size.unwrap_or(default_batch).clamp(1, 100).max(1);

    // Period label = the client's idempotency key when present (verbatim: re-sending
    // the same key recovers instead of duplicating); otherwise a fresh unique label
    // (a new one-shot trigger).
    let key = req.idempotency_key.as_deref().map(str::trim);
    if let Some(k) = key {
        // Count CHARACTERS, not bytes: the cap is documented as 128 characters, and a
        // byte count would silently make the effective limit much smaller for
        // non-ASCII keys (a 128-character CJK key is ~384 bytes).
        let chars = k.chars().count();
        if chars > MAX_IDEMPOTENCY_KEY_LEN {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!(
                    "idempotency_key exceeds the {MAX_IDEMPOTENCY_KEY_LEN}-character cap \
                     (got {chars})"
                ),
            )
                .into_response();
        }
    }
    let period = match key {
        Some(k) if !k.is_empty() => format!("manual-{k}"),
        _ => format!(
            "manual-{}-{}",
            chrono::Utc::now().format("%Y%m%dT%H%M%S"),
            Uuid::new_v4().simple()
        ),
    };

    match worker::trigger_check_batch(&jobs_dsn, st.pool(), Some(node_ids), batch_size, &period)
        .await
    {
        Ok((pushed, skipped, nodes)) => {
            if explicit && nodes == 0 {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "no schedulable nodes in the request (missing payloads or unknown ids)",
                )
                    .into_response();
            }
            (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({
                    "enqueued_batches": pushed,
                    "duplicate_batches": skipped,
                    "nodes": nodes,
                    "batch_size": batch_size,
                    // Echo the label so a client can re-send it verbatim to recover.
                    "period": period,
                })),
            )
                .into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
