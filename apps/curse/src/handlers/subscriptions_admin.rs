//! Subscription management (P5-03 admin surface): profile CRUD + token issue/revoke.
//! Tokens: plaintext returned once at creation; only sha256(plain||pepper) is stored.
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::state::AppState;

fn err(status: StatusCode, msg: impl std::fmt::Display) -> Response {
    (status, msg.to_string()).into_response()
}

#[derive(Debug, Deserialize)]
pub struct ProfileUpsert {
    pub name: String,
    pub filter_ast: Option<Value>,
    pub region: Option<String>,
    pub health: Option<String>,
    pub min_score: Option<f64>,
    pub limit_count: Option<i32>,
    pub preset: Option<String>,
    pub naming_rule: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct ProfileView {
    pub id: Uuid,
    pub name: String,
    pub filter_ast: Option<Value>,
    pub region: Option<String>,
    pub health: String,
    pub min_score: f64,
    pub limit_count: i32,
    pub preset: String,
    pub naming_rule: Option<String>,
    pub enabled: bool,
    pub version: i64,
}

fn to_view(r: ProfileRow) -> ProfileView {
    ProfileView {
        id: r.0,
        name: r.1,
        filter_ast: r.2,
        region: r.3,
        health: r.4,
        min_score: r.5,
        limit_count: r.6,
        preset: r.7,
        naming_rule: r.8,
        enabled: r.9,
        version: r.10,
    }
}

/// Profile row tuple (clippy type_complexity).
type ProfileRow = (
    Uuid,
    String,
    Option<Value>,
    Option<String>,
    String,
    f64,
    i32,
    String,
    Option<String>,
    bool,
    i64,
);

/// GET /api/v1/subscriptions — list profiles.
pub async fn list_profiles(State(st): State<AppState>) -> Response {
    let rows: Result<Vec<ProfileRow>, _> = sqlx::query_as(
            "SELECT id,name,filter_ast,region,health,min_score,limit_count,preset,naming_rule,enabled,version \
             FROM subscription_profiles ORDER BY name",
        )
        .fetch_all(st.pool())
        .await;
    match rows {
        Ok(rows) => Json(rows.into_iter().map(to_view).collect::<Vec<_>>()).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// POST /api/v1/subscriptions — create a profile.
pub async fn create_profile(
    State(st): State<AppState>,
    Json(req): Json<ProfileUpsert>,
) -> Response {
    let id = Uuid::new_v4();
    let res = sqlx::query(
        "INSERT INTO subscription_profiles \
           (id,name,filter_ast,region,health,min_score,limit_count,preset,naming_rule,enabled,version,output_format) \
         VALUES ($1,$2,$3,$4,COALESCE($5,'alive'),COALESCE($6,0),COALESCE($7,200),COALESCE($8,'common'),$9,COALESCE($10,true),1,'base64')",
    )
    .bind(id)
    .bind(&req.name)
    .bind(&req.filter_ast)
    .bind(&req.region)
    .bind(&req.health)
    .bind(req.min_score.unwrap_or(0.0))
    .bind(req.limit_count.unwrap_or(200))
    .bind(&req.preset)
    .bind(&req.naming_rule)
    .bind(req.enabled.unwrap_or(true))
    .execute(st.pool())
    .await;
    match res {
        Ok(_) => (StatusCode::CREATED, Json(serde_json::json!({"id": id}))).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// PUT /api/v1/subscriptions/{id} — update a profile (bumps version: cache invalidation).
pub async fn update_profile(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ProfileUpsert>,
) -> Response {
    let Ok(id) = Uuid::parse_str(&id) else {
        return err(StatusCode::BAD_REQUEST, "invalid uuid");
    };
    let res = sqlx::query(
        "UPDATE subscription_profiles SET \
           name=$2, filter_ast=$3, region=$4, health=COALESCE($5,'alive'), \
           min_score=COALESCE($6,0), limit_count=COALESCE($7,200), \
           preset=COALESCE($8,'common'), naming_rule=$9, enabled=COALESCE($10,true), \
           version=version+1, \
           output_format=COALESCE(output_format, 'base64') \
         WHERE id=$1",
    )
    .bind(id)
    .bind(&req.name)
    .bind(&req.filter_ast)
    .bind(&req.region)
    .bind(&req.health)
    .bind(req.min_score.unwrap_or(0.0))
    .bind(req.limit_count.unwrap_or(200))
    .bind(&req.preset)
    .bind(&req.naming_rule)
    .bind(req.enabled.unwrap_or(true))
    .execute(st.pool())
    .await;
    match res {
        Ok(r) if r.rows_affected() > 0 => StatusCode::OK.into_response(),
        Ok(_) => err(StatusCode::NOT_FOUND, "profile not found"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// DELETE /api/v1/subscriptions/{id} — delete a profile (tokens cascade).
pub async fn delete_profile(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    let Ok(id) = Uuid::parse_str(&id) else {
        return err(StatusCode::BAD_REQUEST, "invalid uuid");
    };
    match sqlx::query("DELETE FROM subscription_profiles WHERE id=$1")
        .bind(id)
        .execute(st.pool())
        .await
    {
        Ok(r) if r.rows_affected() > 0 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => err(StatusCode::NOT_FOUND, "profile not found"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[derive(Debug, Serialize)]
pub struct TokenIssued {
    pub id: Uuid,
    pub profile_id: Uuid,
    /// Plaintext: shown exactly once; storage keeps only the hash.
    pub token: String,
}

/// POST /api/v1/subscriptions/{id}/tokens — issue a token for a profile.
pub async fn issue_token(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    let Ok(profile_id) = Uuid::parse_str(&id) else {
        return err(StatusCode::BAD_REQUEST, "invalid uuid");
    };
    let exists: i64 = sqlx::query_scalar("SELECT count(*) FROM subscription_profiles WHERE id=$1")
        .bind(profile_id)
        .fetch_one(st.pool())
        .await
        .unwrap_or(0);
    if exists == 0 {
        return err(StatusCode::NOT_FOUND, "profile not found");
    }
    let pepper = std::env::var("PHLOGO_TOKEN_PEPPER").unwrap_or_default();
    let (plain, hash) = distribution::token::issue(&pepper);
    let token_id = Uuid::new_v4();
    let res = sqlx::query(
        "INSERT INTO subscription_tokens (id,profile_id,token_hash,status) VALUES ($1,$2,$3,'active')",
    )
    .bind(token_id)
    .bind(profile_id)
    .bind(&hash)
    .execute(st.pool())
    .await;
    match res {
        Ok(_) => (
            StatusCode::CREATED,
            Json(TokenIssued { id: token_id, profile_id, token: plain }),
        )
            .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// DELETE /api/v1/subscriptions/tokens/{id} — revoke a token (immediate 401 on /sub).
pub async fn revoke_token(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    let Ok(id) = Uuid::parse_str(&id) else {
        return err(StatusCode::BAD_REQUEST, "invalid uuid");
    };
    match sqlx::query(
        "UPDATE subscription_tokens SET status='revoked', revoked_at=now() \
         WHERE id=$1 AND status='active'",
    )
    .bind(id)
    .execute(st.pool())
    .await
    {
        Ok(r) if r.rows_affected() > 0 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => err(StatusCode::NOT_FOUND, "token not found or already revoked"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// Token list row (clippy type_complexity).
type TokenRow = (Uuid, String, chrono::DateTime<chrono::Utc>, Option<chrono::DateTime<chrono::Utc>>);

/// GET /api/v1/subscriptions/{id}/tokens — list tokens for a profile (hashes only).
pub async fn list_tokens(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    let Ok(profile_id) = Uuid::parse_str(&id) else {
        return err(StatusCode::BAD_REQUEST, "invalid uuid");
    };
    let rows: Result<Vec<TokenRow>, _> = sqlx::query_as(
            "SELECT id, status, created_at, revoked_at FROM subscription_tokens \
             WHERE profile_id=$1 ORDER BY created_at DESC",
        )
        .bind(profile_id)
        .fetch_all(st.pool())
        .await;
    match rows {
        Ok(rows) => {
            let items: Vec<Value> = rows
                .into_iter()
                .map(|(tid, status, created, revoked)| {
                    serde_json::json!({
                        "id": tid, "status": status,
                        "created_at": created.to_rfc3339(),
                        "revoked_at": revoked.map(|r| r.to_rfc3339()),
                    })
                })
                .collect();
            Json(items).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}
