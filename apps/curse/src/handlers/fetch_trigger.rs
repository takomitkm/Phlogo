//! Fetch trigger (plan §11 `/api/v1/fetch/*`): manually enqueue fetches for all
//! active sources into the worker's Apalis storage (worker drains them).
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

use crate::state::AppState;

/// POST /api/v1/fetch/trigger — enqueue one FetchJob per active source.
pub async fn trigger(State(st): State<AppState>) -> Response {
    let jobs_dsn = std::env::var("PHLOGO_DATABASE_JOBS_DSN").unwrap_or_default();
    if jobs_dsn.is_empty() {
        return (StatusCode::SERVICE_UNAVAILABLE, "jobs dsn not configured").into_response();
    }
    match worker::trigger_fetch_batch(&jobs_dsn, st.pool()).await {
        Ok(n) => Json(serde_json::json!({ "enqueued": n })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
