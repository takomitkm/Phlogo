//! Parameter loading from `configs` (scoring.go `loadParams` equivalent; per-call read
//! replaces Go's 1-min TTL cache — parameter changes apply at the next recompute).
use sqlx::PgPool;

use crate::engine::ScoreParams;

async fn cfg_i64(pool: &PgPool, key: &str, fallback: i64) -> anyhow::Result<i64> {
    let v: Option<String> = sqlx::query_scalar("SELECT value FROM configs WHERE key=$1")
        .bind(key)
        .fetch_optional(pool)
        .await?;
    Ok(v.and_then(|s| s.parse().ok()).unwrap_or(fallback))
}

pub async fn load_params(pool: &PgPool) -> anyhow::Result<ScoreParams> {
    let alpha = cfg_i64(pool, "score_decay_alpha", 70).await?;
    let w_latency = cfg_i64(pool, "score_weight_latency", 70).await?;
    let w_stability = cfg_i64(pool, "score_weight_stability", 30).await?;
    let w_speed = cfg_i64(pool, "score_weight_speed", 55).await?;
    let latency_curve_k = cfg_i64(pool, "score_latency_curve_k", 30).await?;
    let jitter = cfg_i64(pool, "score_jitter_linear_threshold_ms", 200).await?;
    Ok(ScoreParams {
        alpha: alpha as f64 / 100.0,
        w_latency: w_latency as f64 / 100.0,
        w_stability: w_stability as f64 / 100.0,
        w_speed: w_speed as f64 / 100.0,
        latency_curve_k: latency_curve_k as f64 / 100.0,
        jitter_linear_threshold_ms: jitter as f64,
    })
}

pub async fn min_samples(pool: &PgPool) -> anyhow::Result<usize> {
    Ok(cfg_i64(pool, "score_min_samples", 1).await? as usize)
}
