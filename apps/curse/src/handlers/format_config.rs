//! Per-format subscription templates (`format_configs`, B3 gap #3).
//!
//! Upstream kept a seeded config JSON per registered format and let admins override it;
//! we store the same idea in `format_configs` (migration 0010) and fall back to the
//! embedded defaults from `distribution::output`. Writes are validated by parsing the
//! template with the same parser used at render time, so a broken template is rejected
//! at the door instead of 500-ing every subscription request.
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use crate::state::AppState;

pub const FORMATS: [&str; 3] = ["clash", "singbox", "base64"];

fn err400(msg: impl std::fmt::Display) -> Response {
    (StatusCode::BAD_REQUEST, msg.to_string()).into_response()
}

fn err500(e: impl std::fmt::Display) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response()
}

fn supported(format: &str) -> bool {
    FORMATS.contains(&format)
}

/// Embedded default config for a format (used when the table has no row yet).
pub fn default_config(format: &str) -> Value {
    match format {
        "clash" => json!({ "template": distribution::output::DEFAULT_CLASH_TEMPLATE }),
        "singbox" => json!({ "template": distribution::output::DEFAULT_SINGBOX_TEMPLATE }),
        "base64" => json!({ "separator": "\n" }),
        _ => json!({}),
    }
}

/// Template text for the renderer, preferring the stored override.
pub async fn template_of(pool: &PgPool, format: &str) -> String {
    let stored: Option<Value> =
        sqlx::query_scalar("SELECT config FROM format_configs WHERE format=$1")
            .bind(format)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    let cfg = stored.unwrap_or_else(|| default_config(format));
    if let Some(t) = cfg.get("template").and_then(Value::as_str) {
        return t.to_string();
    }
    default_config(format)
        .get("template")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Reject a template the renderer would choke on (fail at write time, not at serve time).
fn validate(format: &str, cfg: &Value) -> Result<(), String> {
    let obj = cfg.as_object().ok_or("config must be a JSON object")?;
    match format {
        "clash" => {
            let t = obj
                .get("template")
                .and_then(Value::as_str)
                .ok_or("clash config needs a `template` string")?;
            let parsed: Result<serde_yaml::Value, _> = serde_yaml::from_str(t);
            let root = parsed.map_err(|e| format!("template is not valid YAML: {e}"))?;
            if root.as_mapping().is_none() {
                return Err("clash template root must be a mapping".into());
            }
            Ok(())
        }
        "singbox" => {
            let t = obj
                .get("template")
                .and_then(Value::as_str)
                .ok_or("singbox config needs a `template` string")?;
            let parsed: Result<Value, _> = serde_json::from_str(t);
            let root = parsed.map_err(|e| format!("template is not valid JSON: {e}"))?;
            if root.get("outbounds").and_then(Value::as_array).is_none() {
                return Err("singbox template must contain an `outbounds` array".into());
            }
            Ok(())
        }
        "base64" => {
            if let Some(sep) = obj.get("separator") {
                if !sep.is_string() {
                    return Err("base64 separator must be a string".into());
                }
            }
            Ok(())
        }
        other => Err(format!("unknown format: {other}")),
    }
}

type EntryRow = (String, Value, chrono::DateTime<chrono::Utc>);

fn entry(format: &str, config: Value, updated: Option<chrono::DateTime<chrono::Utc>>) -> Value {
    json!({
        "format": format,
        "config": config,
        "updated_at": updated.map(|t| t.to_rfc3339()),
        "source": if updated.is_some() { "stored" } else { "default" },
    })
}

/// GET /api/v1/format-configs — every supported format (stored override or default).
pub async fn list(State(st): State<AppState>) -> Response {
    let rows: Result<Vec<EntryRow>, _> =
        sqlx::query_as("SELECT format, config, updated_at FROM format_configs")
            .fetch_all(st.pool())
            .await;
    let stored = match rows {
        Ok(r) => r,
        Err(e) => return err500(e),
    };
    let items: Vec<Value> = FORMATS
        .iter()
        .map(|f| match stored.iter().find(|r| r.0 == *f) {
            Some((_, cfg, at)) => entry(f, cfg.clone(), Some(*at)),
            None => entry(f, default_config(f), None),
        })
        .collect();
    Json(items).into_response()
}

/// GET /api/v1/format-configs/{format}
pub async fn get(State(st): State<AppState>, Path(format): Path<String>) -> Response {
    if !supported(&format) {
        return err400(format!("unknown format: {format}"));
    }
    let row: Result<Option<EntryRow>, _> = sqlx::query_as(
        "SELECT format, config, updated_at FROM format_configs WHERE format=$1",
    )
    .bind(&format)
    .fetch_optional(st.pool())
    .await;
    match row {
        Ok(Some((_, cfg, at))) => Json(entry(&format, cfg, Some(at))).into_response(),
        Ok(None) => Json(entry(&format, default_config(&format), None)).into_response(),
        Err(e) => err500(e),
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdateReq {
    pub config: Value,
}

/// PUT /api/v1/format-configs/{format} — validate then upsert.
pub async fn update(
    State(st): State<AppState>,
    Path(format): Path<String>,
    Json(req): Json<UpdateReq>,
) -> Response {
    if !supported(&format) {
        return err400(format!("unknown format: {format}"));
    }
    if let Err(msg) = validate(&format, &req.config) {
        return err400(msg);
    }
    let row: Result<EntryRow, _> = sqlx::query_as(
        "INSERT INTO format_configs (format, config, updated_at) VALUES ($1, $2, now()) \
         ON CONFLICT (format) DO UPDATE SET config = EXCLUDED.config, updated_at = now() \
         RETURNING format, config, updated_at",
    )
    .bind(&format)
    .bind(&req.config)
    .fetch_one(st.pool())
    .await;
    match row {
        Ok((_, cfg, at)) => Json(entry(&format, cfg, Some(at))).into_response(),
        Err(e) => err500(e),
    }
}
