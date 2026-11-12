//! Sponsor admin HTTP handlers (B3 gap #2): CRUD over the `sponsors` table.
//! The matching engine lives in `registry::sponsor` (priority-ascending, later
//! match wins, `default_sponsor_text` fallback).
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::state::AppState;

fn err400(msg: impl std::fmt::Display) -> Response {
    (StatusCode::BAD_REQUEST, msg.to_string()).into_response()
}

fn err500(e: impl std::fmt::Display) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response()
}

fn summary(s: &registry::SponsorRow) -> Value {
    serde_json::json!({
        "id": s.id, "name": s.name, "text": s.text,
        "filter_ast": s.filter_ast, "priority": s.priority,
    })
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub page: Option<i64>,
    pub limit: Option<i64>,
}

/// GET /api/v1/sponsors — paginated, priority ascending.
pub async fn list(State(st): State<AppState>, Query(q): Query<ListQuery>) -> Response {
    let page = q.page.unwrap_or(1).max(1);
    let limit = q.limit.unwrap_or(20).clamp(1, 200);
    match registry::sponsor::list(st.pool(), page, limit).await {
        Ok((rows, total)) => {
            let data: Vec<Value> = rows.iter().map(summary).collect();
            let total_pages = if limit > 0 { (total + limit - 1) / limit } else { 0 };
            Json(serde_json::json!({
                "data": data,
                "pagination": {"page": page, "limit": limit, "total": total, "total_pages": total_pages},
            }))
            .into_response()
        }
        Err(e) => err500(e),
    }
}

#[derive(Debug, Deserialize)]
pub struct SponsorReq {
    pub name: String,
    pub text: String,
    #[serde(default)]
    pub filter_ast: Option<Value>,
    #[serde(default)]
    pub priority: i32,
}

/// POST /api/v1/sponsors — create.
pub async fn create(State(st): State<AppState>, Json(req): Json<SponsorReq>) -> Response {
    if let Err(msg) = validate_filter(&req.filter_ast) {
        return err400(msg);
    }
    let creq = registry::sponsor::CreateReq {
        name: req.name,
        text: req.text,
        filter_ast: req.filter_ast,
        priority: req.priority,
    };
    match registry::sponsor::create(st.pool(), &creq).await {
        Ok(row) => (StatusCode::CREATED, Json(summary(&row))).into_response(),
        Err(e) => err500(e),
    }
}

/// GET /api/v1/sponsors/{id}
pub async fn get(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    let Ok(id) = Uuid::parse_str(&id) else {
        return err400("invalid uuid");
    };
    match registry::sponsor::get(st.pool(), id).await {
        Ok(Some(row)) => Json(summary(&row)).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => err500(e),
    }
}

#[derive(Debug, Deserialize)]
pub struct SponsorUpdateReq {
    pub name: Option<String>,
    pub text: Option<String>,
    pub filter_ast: Option<Value>,
    pub priority: Option<i32>,
}

/// PATCH /api/v1/sponsors/{id} — partial update (non-null fields).
pub async fn update(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SponsorUpdateReq>,
) -> Response {
    let Ok(id) = Uuid::parse_str(&id) else {
        return err400("invalid uuid");
    };
    if let Err(msg) = validate_filter(&req.filter_ast) {
        return err400(msg);
    }
    // Distinguish "absent" from "explicit null" happens inside SQL: COALESCE keeps
    // the stored value when the bind is NULL, so PATCH semantics match upstream.
    let ureq = registry::sponsor::UpdateReq {
        name: req.name,
        text: req.text,
        filter_ast: req.filter_ast,
        priority: req.priority,
    };
    match registry::sponsor::update(st.pool(), id, &ureq).await {
        Ok(Some(row)) => Json(summary(&row)).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => err500(e),
    }
}

/// DELETE /api/v1/sponsors/{id}
pub async fn delete(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    let Ok(id) = Uuid::parse_str(&id) else {
        return err400("invalid uuid");
    };
    match registry::sponsor::delete(st.pool(), id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => err500(e),
    }
}

/// Reject malformed AST at the door (same whitelist as search/preview).
fn validate_filter(ast: &Option<Value>) -> Result<(), String> {
    let Some(v) = ast else { return Ok(()) };
    if v.is_null() {
        return Ok(());
    }
    filter::parse_with(&v.to_string(), filter::FIELD_WHITELIST)
        .map(|_| ())
        .map_err(|e| format!("filter_ast: {e}"))
}

/// Serialize helper for tests.
#[allow(dead_code)]
fn _serialize_marker<T: Serialize>(_: &T) {}
