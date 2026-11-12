//! Advanced search + filter preview (B3 gap #1; upstream `search.go` / `filter.go`).
//!
//! Upstream evaluated expr-lang strings in memory over all rows. Per decision A3 we take
//! the typed **Filter-AST JSON** instead (`plan §9.2` shape) and keep the same
//! evaluate-in-memory semantics, so behaviour is equivalent while the value surface is a
//! whitelist instead of a scripting language.
//!
//! Known limitation: `unlock` (streaming) conditions never match yet — there is no
//! streaming table (`node_streaming` lands with the deep-check wiring), so the view
//! carries an empty map. Node `region` comes from the latest check's `metrics->>'region'`
//! (the `nodes` table has no region column).
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::state::AppState;

const DEFAULT_LIMIT: i64 = 20;
const MAX_LIMIT: i64 = 200;
const MAX_PREVIEW: usize = 100;

fn err400(msg: impl std::fmt::Display) -> Response {
    (StatusCode::BAD_REQUEST, msg.to_string()).into_response()
}

fn err500(e: impl std::fmt::Display) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response()
}

#[derive(Debug, Deserialize)]
pub struct SearchReq {
    /// Filter-AST JSON. Absent / null / empty string = no filtering.
    #[serde(default)]
    pub filter: Option<Value>,
    pub page: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct Pagination {
    pub page: i64,
    pub limit: i64,
    pub total: i64,
    pub total_pages: i64,
}

fn page_of(req: &SearchReq) -> (i64, i64) {
    let page = req.page.unwrap_or(1).max(1);
    let limit = req.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    (page, limit)
}

fn total_pages(total: i64, limit: i64) -> i64 {
    if limit <= 0 {
        return 0;
    }
    (total + limit - 1) / limit
}

/// Parse the request's filter against a whitelist. `Ok(None)` = no filtering.
#[allow(clippy::result_large_err)]
fn parse_filter(
    value: &Option<Value>,
    allowed: &[&str],
) -> Result<Option<filter::FilterNode>, Response> {
    let Some(v) = value else { return Ok(None) };
    if v.is_null() {
        return Ok(None);
    }
    if let Some(s) = v.as_str() {
        if s.trim().is_empty() {
            return Ok(None);
        }
    }
    filter::parse_with(&v.to_string(), allowed)
        .map(Some)
        .map_err(|e| err400(format!("filter: {e}")))
}

// ── nodes ────────────────────────────────────────────────────────────────────────

type NodeRow = (
    Uuid,
    String,
    String,
    String,
    i32,
    Option<f64>,
    Option<String>,
    Option<String>,
    Option<Vec<String>>,
);

/// The per-node "search view": latest health across tags, max score, latest region.
/// The SQL push-down compiles the AST against this shape (see `filter::to_search_sql`).
const NODE_VIEW_SQL: &str = "(SELECT n.id, n.identity_hash, n.protocol, n.server, n.port, \
        (SELECT max(sc.score) FROM node_scores sc WHERE sc.node_id = n.id)::float8 AS score, \
        (SELECT st.health FROM node_status st WHERE st.node_id = n.id \
           ORDER BY st.last_checked_at DESC LIMIT 1) AS health, \
        (SELECT r.metrics->>'region' FROM node_check_results r \
           WHERE r.node_id = n.id AND r.metrics ? 'region' \
           ORDER BY r.checked_at DESC LIMIT 1) AS region, \
        (SELECT array_agg(ns.source_id::text) FROM node_sources ns \
           WHERE ns.node_id = n.id) AS source_ids \
     FROM nodes n)";

async fn load_nodes(
    pool: &sqlx::PgPool,
    filter_ast: Option<&filter::FilterNode>,
) -> Result<Vec<NodeRow>, sqlx::Error> {
    // Filter push-down: the AST compiles to a parameterized WHERE clause over the
    // per-node aggregates above (latest health across tags, max score, latest region,
    // source membership via EXISTS). No checker_tag is pinned — the search view spans
    // all tags. Memory evaluation still runs afterwards for `unlock` conditions
    // (streaming has no table yet) — see `evaluate_node`.
    let (where_clause, params) = match filter_ast {
        Some(ast) => filter::to_search_sql(ast),
        None => ("TRUE".to_string(), Vec::new()),
    };
    let sql = format!(
        "SELECT * FROM {NODE_VIEW_SQL} v WHERE {where_clause} ORDER BY v.identity_hash"
    );
    let mut q = sqlx::query_as::<_, NodeRow>(&sql);
    for p in &params {
        q = match p {
            filter::SqlParam::Str(s) => q.bind(s.clone()),
            filter::SqlParam::Num(n) => q.bind(*n),
            filter::SqlParam::Bool(b) => q.bind(*b),
        };
    }
    q.fetch_all(pool).await
}

fn node_summary(row: &NodeRow) -> Value {
    serde_json::json!({
        "id": row.0,
        "identity_hash": row.1,
        "protocol": row.2,
        "server": row.3,
        "port": row.4,
        "score": row.5.unwrap_or(0.0),
        "health": row.6.clone().unwrap_or_else(|| "unknown".to_string()),
        "region": row.7,
        "source_ids": row.8.clone().unwrap_or_default(),
    })
}

/// POST /api/v1/nodes/search — AST filter + pagination (upstream `NodeSearch`).
///
/// The filter is authoritative **in SQL** (`filter::to_search_sql` over the per-node
/// view). No memory re-check: the in-memory `matches` does not implement `contains`
/// /`is_null` and would wrongly drop rows SQL already matched. Known limitation:
/// `unlock` leaves compile to SQL `FALSE` (no streaming table yet), so a filter with
/// `unlock` matches nothing.
pub async fn nodes_search(State(st): State<AppState>, Json(req): Json<SearchReq>) -> Response {
    let ast = match parse_filter(&req.filter, filter::FIELD_WHITELIST) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let rows = match load_nodes(st.pool(), ast.as_ref()).await {
        Ok(r) => r,
        Err(e) => return err500(e),
    };
    let matched: Vec<NodeRow> = rows;

    let (page, limit) = page_of(&req);
    let total = matched.len() as i64;
    let start = ((page - 1) * limit).max(0) as usize;
    let data: Vec<Value> = matched
        .iter()
        .skip(start)
        .take(limit as usize)
        .map(node_summary)
        .collect();
    Json(serde_json::json!({
        "data": data,
        "pagination": Pagination { page, limit, total, total_pages: total_pages(total, limit) },
    }))
    .into_response()
}

// ── sources ─────────────────────────────────────────────────────────────────────

type SourceRow = (Uuid, String, String, String, String);

async fn load_sources(pool: &sqlx::PgPool) -> Result<Vec<SourceRow>, sqlx::Error> {
    sqlx::query_as("SELECT id, identifier, type, status, info FROM sources ORDER BY identifier")
        .fetch_all(pool)
        .await
}

fn source_view(row: &SourceRow) -> filter::SourceView {
    filter::SourceView {
        id: row.0.to_string(),
        identifier: row.1.clone(),
        source_type: row.2.clone(),
        status: row.3.clone(),
        info: row.4.clone(),
    }
}

fn source_summary(row: &SourceRow) -> Value {
    serde_json::json!({
        "id": row.0,
        "identifier": row.1,
        "type": row.2,
        "status": row.3,
        "info": row.4,
    })
}

/// POST /api/v1/sources/search — AST filter over `id|identifier|type|status|info`.
pub async fn sources_search(State(st): State<AppState>, Json(req): Json<SearchReq>) -> Response {
    let ast = match parse_filter(&req.filter, filter::SOURCE_FIELD_WHITELIST) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let rows = match load_sources(st.pool()).await {
        Ok(r) => r,
        Err(e) => return err500(e),
    };
    let matched: Vec<SourceRow> = rows
        .into_iter()
        .filter(|row| match &ast {
            None => true,
            Some(ast) => filter::matches_source(&source_view(row), ast),
        })
        .collect();

    let (page, limit) = page_of(&req);
    let total = matched.len() as i64;
    let start = ((page - 1) * limit).max(0) as usize;
    let data: Vec<Value> = matched
        .iter()
        .skip(start)
        .take(limit as usize)
        .map(source_summary)
        .collect();
    Json(serde_json::json!({
        "data": data,
        "pagination": Pagination { page, limit, total, total_pages: total_pages(total, limit) },
    }))
    .into_response()
}

// ── preview ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct PreviewReq {
    /// `"node"` or `"source"` (upstream `FilterPreviewRequest.target`).
    pub target: String,
    #[serde(default)]
    pub filter: Option<Value>,
    pub limit: Option<i64>,
}

/// POST /api/v1/filter/preview — dry-run an AST, capped at 100 rows (upstream parity).
pub async fn preview(State(st): State<AppState>, Json(req): Json<PreviewReq>) -> Response {
    let cap = req.limit.unwrap_or(MAX_PREVIEW as i64).clamp(1, MAX_PREVIEW as i64) as usize;
    match req.target.as_str() {
        "node" => {
            let ast = match parse_filter(&req.filter, filter::FIELD_WHITELIST) {
                Ok(a) => a,
                Err(r) => return r,
            };
            let rows = match load_nodes(st.pool(), ast.as_ref()).await {
                Ok(r) => r,
                Err(e) => return err500(e),
            };
            let hits: Vec<Value> = rows.iter().take(cap).map(node_summary).collect();
            Json(serde_json::json!({ "nodes": hits })).into_response()
        }
        "source" => {
            let ast = match parse_filter(&req.filter, filter::SOURCE_FIELD_WHITELIST) {
                Ok(a) => a,
                Err(r) => return r,
            };
            let rows = match load_sources(st.pool()).await {
                Ok(r) => r,
                Err(e) => return err500(e),
            };
            let hits: Vec<Value> = rows
                .iter()
                .filter(|row| match &ast {
                    None => true,
                    Some(ast) => filter::matches_source(&source_view(row), ast),
                })
                .take(cap)
                .map(source_summary)
                .collect();
            Json(serde_json::json!({ "sources": hits })).into_response()
        }
        other => err400(format!("target must be 'node' or 'source', got '{other}'")),
    }
}
