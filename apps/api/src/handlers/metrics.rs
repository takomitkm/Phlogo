//! Minimal Prometheus text-format metrics (P7-02 Step 3). Avoids a heavy metrics
//! dependency while exposing the counters the plan names: requests, latency buckets
//! are provided by the log pipeline; here: node/source/check gauges from the DB.
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::state::AppState;

/// Process-level counters (updated by handlers/route layer).
pub static HTTP_REQUESTS_TOTAL: AtomicU64 = AtomicU64::new(0);
pub static SUBSCRIPTION_RENDERS_TOTAL: AtomicU64 = AtomicU64::new(0);

pub async fn metrics(State(st): State<AppState>) -> Response {
    let pool = st.pool();
    let mut body = String::new();

    // Process counters.
    body.push_str("# HELP http_requests_total Total HTTP requests served.\n");
    body.push_str("# TYPE http_requests_total counter\n");
    body.push_str(&format!(
        "http_requests_total {}\n",
        HTTP_REQUESTS_TOTAL.load(Ordering::Relaxed)
    ));
    body.push_str("# HELP subscription_renders_total Subscription renders.\n");
    body.push_str("# TYPE subscription_renders_total counter\n");
    body.push_str(&format!(
        "subscription_renders_total {}\n",
        SUBSCRIPTION_RENDERS_TOTAL.load(Ordering::Relaxed)
    ));

    // DB gauges (best-effort; on error the section is skipped, not a 500).
    async fn scalar(pool: &sqlx::PgPool, sql: &str) -> Option<i64> {
        sqlx::query_scalar(sql).fetch_one(pool).await.ok()
    }
    let nodes = scalar(pool, "SELECT count(*) FROM nodes").await;
    let nodes_linked =
        scalar(pool, "SELECT count(*) FROM nodes WHERE membership='Linked'").await;
    let sources = scalar(pool, "SELECT count(*) FROM sources").await;
    let runs = scalar(pool, "SELECT count(*) FROM check_runs").await;

    body.push_str("# HELP nodes_total Total nodes.\n");
    body.push_str("# TYPE nodes_total gauge\n");
    body.push_str(&format!("nodes_total {}\n", nodes.unwrap_or(0)));
    body.push_str("# HELP nodes_linked_total Linked nodes.\n");
    body.push_str("# TYPE nodes_linked_total gauge\n");
    body.push_str(&format!("nodes_linked_total {}\n", nodes_linked.unwrap_or(0)));
    body.push_str("# HELP sources_total Total sources.\n");
    body.push_str("# TYPE sources_total gauge\n");
    body.push_str(&format!("sources_total {}\n", sources.unwrap_or(0)));
    body.push_str("# HELP check_runs_total Check runs recorded.\n");
    body.push_str("# TYPE check_runs_total counter\n");
    body.push_str(&format!("check_runs_total {}\n", runs.unwrap_or(0)));

    (StatusCode::OK, body).into_response()
}
