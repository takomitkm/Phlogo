//! Read-only view endpoints (p6-02 contract gaps, closed): stats, checkers, logs,
//! per-node check history. All are thin SELECTs over P3/P4 tables.
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::state::AppState;

/// Row tuples returned by `query_as` (clippy type_complexity).
type CheckRow = (Uuid, i32, chrono::DateTime<chrono::Utc>, String);
type FetchRow = (Uuid, String, bool, i64, chrono::DateTime<chrono::Utc>);
type DeepRow = (Value, chrono::DateTime<chrono::Utc>, String);
type InitialRow = (i32, chrono::DateTime<chrono::Utc>, String, Uuid);

fn err500(e: impl std::fmt::Display) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response()
}

/// GET /api/v1/stats — dashboard counters.
pub async fn stats(State(st): State<AppState>) -> Response {
    let pool = st.pool();
    async fn one(pool: &sqlx::PgPool, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(pool).await.unwrap_or(0)
    }
    let total_nodes = one(pool, "SELECT count(*) FROM nodes").await;
    let alive_nodes = one(
        pool,
        "SELECT count(*) FROM node_status WHERE health IN ('alive','degraded')",
    )
    .await;
    let total_sources = one(pool, "SELECT count(*) FROM sources").await;
    let dead_sources = one(pool, "SELECT count(*) FROM sources WHERE status='dead'").await;
    Json(serde_json::json!({
        "total_nodes": total_nodes,
        "alive_nodes": alive_nodes,
        "total_sources": total_sources,
        "dead_sources": dead_sources,
    }))
    .into_response()
}

/// GET /api/v1/checkers — distinct checker tags (from node_status).
pub async fn checkers(State(st): State<AppState>) -> Response {
    let pool = st.pool();
    match sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT checker_tag FROM node_status ORDER BY checker_tag",
    )
    .fetch_all(pool)
    .await
    {
        Ok(tags) => Json(serde_json::json!({ "checkers": tags })).into_response(),
        Err(e) => err500(e),
    }
}

#[derive(Debug, Deserialize)]
pub struct LogQuery {
    pub node_id: Option<Uuid>,
    pub source_id: Option<Uuid>,
    pub limit: Option<i64>,
}

/// GET /api/v1/logs/check — recent initial-check records (latency_ms = -1 means unreachable).
pub async fn logs_check(
    State(st): State<AppState>,
    Query(q): Query<LogQuery>,
) -> Response {
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let rows: Result<Vec<CheckRow>, _> =
        if let Some(node_id) = q.node_id {
            sqlx::query_as(
                "SELECT node_id, (metrics->>'latency_ms')::int, checked_at, checker_tag \
                 FROM node_check_results WHERE node_id=$1 ORDER BY checked_at DESC, id DESC LIMIT $2",
            )
            .bind(node_id)
            .bind(limit)
            .fetch_all(st.pool())
            .await
        } else {
            sqlx::query_as(
                "SELECT node_id, (metrics->>'latency_ms')::int, checked_at, checker_tag \
                 FROM node_check_results ORDER BY checked_at DESC, id DESC LIMIT $1",
            )
            .bind(limit)
            .fetch_all(st.pool())
            .await
        };
    match rows {
        Ok(rows) => {
            let items: Vec<Value> = rows
                .into_iter()
                .map(|(node_id, latency, at, tag)| {
                    serde_json::json!({
                        "node_id": node_id, "latency_ms": latency,
                        "checked_at": at.to_rfc3339(), "checker_tag": tag,
                    })
                })
                .collect();
            Json(items).into_response()
        }
        Err(e) => err500(e),
    }
}

/// GET /api/v1/logs/fetch — fetch-run history (per source when filtered).
pub async fn logs_fetch(
    State(st): State<AppState>,
    Query(q): Query<LogQuery>,
) -> Response {
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let rows: Result<Vec<FetchRow>, _> =
        if let Some(source_id) = q.source_id {
            sqlx::query_as(
                "SELECT source_id, status, (status='finished') AS success, \
                 COALESCE((counts->>'fetched')::bigint, 0) AS node_count, started_at \
                 FROM source_fetch_runs WHERE source_id=$1 ORDER BY started_at DESC LIMIT $2",
            )
            .bind(source_id)
            .bind(limit)
            .fetch_all(st.pool())
            .await
        } else {
            sqlx::query_as(
                "SELECT source_id, status, (status='finished') AS success, \
                 COALESCE((counts->>'fetched')::bigint, 0) AS node_count, started_at \
                 FROM source_fetch_runs ORDER BY started_at DESC LIMIT $1",
            )
            .bind(limit)
            .fetch_all(st.pool())
            .await
        };
    match rows {
        Ok(rows) => {
            let items: Vec<Value> = rows
                .into_iter()
                .map(|(source_id, status, success, count, at)| {
                    serde_json::json!({
                        "source_id": source_id, "status": status,
                        "success": success, "node_count": count, "fetched_at": at.to_rfc3339(),
                    })
                })
                .collect();
            Json(items).into_response()
        }
        Err(e) => err500(e),
    }
}

/// GET /api/v1/nodes/{id}/checks/deep — deep-check timeline for a node.
pub async fn node_deep_checks(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<LogQuery>,
) -> Response {
    let Ok(node_id) = Uuid::parse_str(&id) else {
        return (StatusCode::BAD_REQUEST, "invalid uuid").into_response();
    };
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let rows: Result<Vec<DeepRow>, _> = sqlx::query_as(
        "SELECT metrics, checked_at, checker_tag FROM node_check_results \
         WHERE node_id=$1 AND metrics ? 'avg_latency_ms' \
         ORDER BY checked_at DESC, id DESC LIMIT $2",
    )
    .bind(node_id)
    .bind(limit)
    .fetch_all(st.pool())
    .await;
    match rows {
        Ok(rows) => {
            let items: Vec<Value> = rows
                .into_iter()
                .map(|(mut m, at, tag)| {
                    if let Some(o) = m.as_object_mut() {
                        o.insert("checked_at".into(), serde_json::json!(at.to_rfc3339()));
                        o.insert("checker_tag".into(), serde_json::json!(tag));
                    }
                    m
                })
                .collect();
            Json(items).into_response()
        }
        Err(e) => err500(e),
    }
}

/// GET /api/v1/nodes/{id}/checks/initial — initial-check timeline for a node.
pub async fn node_initial_checks(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<LogQuery>,
) -> Response {
    let Ok(node_id) = Uuid::parse_str(&id) else {
        return (StatusCode::BAD_REQUEST, "invalid uuid").into_response();
    };
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let rows: Result<Vec<InitialRow>, _> = sqlx::query_as(
        "SELECT (metrics->>'latency_ms')::int, checked_at, checker_tag, check_run_id \
         FROM node_check_results WHERE node_id=$1 \
         ORDER BY checked_at DESC, id DESC LIMIT $2",
    )
    .bind(node_id)
    .bind(limit)
    .fetch_all(st.pool())
    .await;
    match rows {
        Ok(rows) => {
            let items: Vec<Value> = rows
                .into_iter()
                .map(|(latency, at, tag, run)| {
                    serde_json::json!({
                        "latency_ms": latency, "checked_at": at.to_rfc3339(),
                        "checker_tag": tag, "check_run_id": run,
                    })
                })
                .collect();
            Json(items).into_response()
        }
        Err(e) => err500(e),
    }
}
