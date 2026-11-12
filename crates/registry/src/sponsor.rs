//! Sponsor domain (B3 gap #2, restored per user decision 2026-11-13).
//!
//! Upstream semantics (`internal/sponsor/sponsor.go`): sponsors are evaluated in
//! **ascending priority** order, each match **overrides** the previous one, and an
//! empty match falls back to the `default_sponsor_text` config. The filter is stored
//! as typed AST JSON (our replacement for the upstream expr-lang string) and evaluated
//! in memory against the node view — the same `filter::matches` used by preview.
use serde::Serialize;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct SponsorRow {
    pub id: Uuid,
    pub name: String,
    pub text: String,
    pub filter_ast: Option<Value>,
    pub priority: i32,
}

type Tuple = (Uuid, String, String, Option<Value>, i32);

fn to_row(t: Tuple) -> SponsorRow {
    SponsorRow {
        id: t.0,
        name: t.1,
        text: t.2,
        filter_ast: t.3,
        priority: t.4,
    }
}

pub async fn list(pool: &PgPool, page: i64, limit: i64) -> Result<(Vec<SponsorRow>, i64), sqlx::Error> {
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM sponsors").fetch_one(pool).await?;
    let offset = (page.max(1) - 1) * limit;
    let rows: Vec<Tuple> = sqlx::query_as(
        "SELECT id, name, text, filter_ast, priority FROM sponsors \
         ORDER BY priority ASC, created_at ASC OFFSET $1 LIMIT $2",
    )
    .bind(offset)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok((rows.into_iter().map(to_row).collect(), total))
}

pub async fn get(pool: &PgPool, id: Uuid) -> Result<Option<SponsorRow>, sqlx::Error> {
    let t: Option<Tuple> = sqlx::query_as(
        "SELECT id, name, text, filter_ast, priority FROM sponsors WHERE id=$1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(t.map(to_row))
}

pub struct CreateReq {
    pub name: String,
    pub text: String,
    pub filter_ast: Option<Value>,
    pub priority: i32,
}

pub async fn create(pool: &PgPool, req: &CreateReq) -> Result<SponsorRow, sqlx::Error> {
    let t: Tuple = sqlx::query_as(
        "INSERT INTO sponsors (id, name, text, filter_ast, priority) \
         VALUES ($1, $2, $3, $4, $5) \
         RETURNING id, name, text, filter_ast, priority",
    )
    .bind(Uuid::new_v4())
    .bind(&req.name)
    .bind(&req.text)
    .bind(&req.filter_ast)
    .bind(req.priority)
    .fetch_one(pool)
    .await?;
    Ok(to_row(t))
}

pub struct UpdateReq {
    pub name: Option<String>,
    pub text: Option<String>,
    pub filter_ast: Option<Value>,
    pub priority: Option<i32>,
}

pub async fn update(pool: &PgPool, id: Uuid, req: &UpdateReq) -> Result<Option<SponsorRow>, sqlx::Error> {
    let t: Option<Tuple> = sqlx::query_as(
        "UPDATE sponsors SET \
           name = COALESCE($2, name), \
           text = COALESCE($3, text), \
           filter_ast = COALESCE($4, filter_ast), \
           priority = COALESCE($5, priority), \
           updated_at = now() \
         WHERE id = $1 \
         RETURNING id, name, text, filter_ast, priority",
    )
    .bind(id)
    .bind(&req.name)
    .bind(&req.text)
    .bind(&req.filter_ast)
    .bind(req.priority)
    .fetch_optional(pool)
    .await?;
    Ok(t.map(to_row))
}

pub async fn delete(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let r = sqlx::query("DELETE FROM sponsors WHERE id=$1").bind(id).execute(pool).await?;
    Ok(r.rows_affected() > 0)
}

/// Evaluate one sponsor's AST against a node view. `Err` = filter parse error
/// (reported to the caller; upstream logs and skips, we do the same via `None`).
pub fn sponsor_matches(node: &filter::NodeView, filter_ast: &Option<Value>) -> Option<bool> {
    let v = filter_ast.as_ref()?;
    if v.is_null() {
        return None;
    }
    let ast = filter::parse_with(&v.to_string(), filter::FIELD_WHITELIST).ok()?;
    Some(filter::matches(node, &ast))
}

/// The best sponsor text for a node: ascending priority, later match wins,
/// fallback `default_sponsor_text` (empty when unset). A DB error becomes `Err`.
pub async fn match_text(pool: &PgPool, node: &filter::NodeView) -> anyhow::Result<String> {
    let rows: Vec<Tuple> = sqlx::query_as(
        "SELECT id, name, text, filter_ast, priority FROM sponsors ORDER BY priority ASC",
    )
    .fetch_all(pool)
    .await?;
    let mut matched = String::new();
    for t in rows {
        if sponsor_matches(node, &t.3) == Some(true) {
            matched = t.2;
        }
    }
    if !matched.is_empty() {
        return Ok(matched);
    }
    Ok(default_text(pool).await)
}

async fn default_text(pool: &PgPool) -> String {
    sqlx::query_scalar::<_, String>("SELECT value FROM configs WHERE key='default_sponsor_text'")
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
}
