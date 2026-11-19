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
    /// Batch size override (clamped to [1, 500]; default from `check_batch_size`).
    #[serde(default)]
    pub batch_size: Option<usize>,
}

/// POST /api/v1/check/trigger — enqueue CheckJobs (AdminJWT only; 202 with stats).
pub async fn trigger(State(st): State<AppState>, req: Option<Json<CheckTriggerReq>>) -> Response {
    let jobs_dsn = std::env::var("PHLOGO_DATABASE_JOBS_DSN").unwrap_or_default();
    if jobs_dsn.is_empty() {
        return (StatusCode::SERVICE_UNAVAILABLE, "jobs dsn not configured").into_response();
    }
    let req = req.map(|Json(r)| r).unwrap_or_default();

    // Hard caps: never let one request enqueue the whole fleet.
    let explicit = !req.node_ids.is_empty();
    let mut node_ids = req.node_ids;
    node_ids.truncate(500);
    let default_batch = worker::load_check_schedule_config(st.pool())
        .await
        .map(|c| c.batch_size)
        .unwrap_or(50);
    let batch_size = req.batch_size.unwrap_or(default_batch).clamp(1, 100).max(1);

    // Distinct period label per request: two manual triggers are two logical runs
    // (unlike the periodic scheduler, which dedups by period).
    let period = format!(
        "manual-{}-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%S"),
        Uuid::new_v4().simple()
    );

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
                })),
            )
                .into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
