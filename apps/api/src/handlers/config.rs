//! Config endpoints (p6-02 Scoring page): read/update the `configs` KV table.
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct ConfigMap(pub HashMap<String, String>);

fn err500(e: impl std::fmt::Display) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response()
}

pub async fn get_config(State(st): State<AppState>) -> Response {
    let pool = st.pool();
    match sqlx::query_as::<_, (String, String)>("SELECT key, value FROM configs ORDER BY key")
        .fetch_all(pool)
        .await
    {
        Ok(rows) => {
            let map: HashMap<String, String> = rows.into_iter().collect();
            Json(ConfigMap(map)).into_response()
        }
        Err(e) => err500(e),
    }
}

/// Grade thresholds from `configs` (`grade_a_min` / `grade_b_min` / `grade_c_min`).
/// These keys were seeded but never read (B3 audit) — the subscription renderer now
/// consumes them instead of the in-code defaults.
pub async fn grade_thresholds(
    pool: &sqlx::PgPool,
) -> Result<distribution::naming::GradeThresholds, sqlx::Error> {
    async fn one(pool: &sqlx::PgPool, key: &str, fallback: f64) -> f64 {
        sqlx::query_scalar::<_, String>("SELECT value FROM configs WHERE key=$1")
            .bind(key)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(fallback)
    }
    Ok(distribution::naming::GradeThresholds {
        a_min: one(pool, "grade_a_min", 0.75).await,
        b_min: one(pool, "grade_b_min", 0.50).await,
        c_min: one(pool, "grade_c_min", 0.25).await,
    })
}

#[derive(Debug, Deserialize)]
pub struct ConfigUpdateReq {
    pub values: HashMap<String, String>,
}

pub async fn put_config(
    State(st): State<AppState>,
    Json(req): Json<ConfigUpdateReq>,
) -> Response {
    let pool = st.pool();
    let mut tx = match pool.begin().await {
        Ok(t) => t,
        Err(e) => return err500(e),
    };
    for (k, v) in &req.values {
        if let Err(e) = sqlx::query(
            "INSERT INTO configs (key,value,updated_at) VALUES ($1,$2,now()) \
             ON CONFLICT (key) DO UPDATE SET value=EXCLUDED.value, updated_at=now()",
        )
        .bind(k)
        .bind(v)
        .execute(&mut *tx)
        .await
        {
            return err500(e);
        }
    }
    if let Err(e) = tx.commit().await {
        return err500(e);
    }
    StatusCode::OK.into_response()
}
